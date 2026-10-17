//! PDBs for system images from Microsoft's public symbol server, fetched
//! only when `--symbol-server` asks for them.
//!
//! An image names its PDB in its `CodeView` debug record (the PDB's file
//! name, a GUID and an age), read here from the image's file on disk. The
//! server holds that PDB at `<pdb>/<GUID><age>/<pdb>`, the GUID as 32
//! upper-case hexadecimal digits and the age in hexadecimal; it is fetched
//! with Windows' own `WinHTTP` into a cache folder laid out the same way, and
//! read from there on later runs. When the server has only the compressed
//! form (the name's last character replaced by `_`), Windows' own
//! `expand.exe` expands it into the cache. An image that cannot be read or
//! fetched is reported and left to its exports; it never ends the run.

use std::collections::HashMap;
use std::ffi::c_void;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use windows::Win32::Networking::WinHttp::{
    INTERNET_DEFAULT_HTTPS_PORT, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE,
    WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE, WinHttpCloseHandle, WinHttpConnect,
    WinHttpOpen, WinHttpOpenRequest, WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse,
    WinHttpSendRequest,
};
use windows::core::{GUID, PCWSTR};

/// The symbol server's host and the path its PDBs are under.
const HOST: &str = "msdl.microsoft.com";
const ROOT: &str = "/download/symbols";

/// An image's `CodeView` record: the file name of its PDB, the PDB's GUID
/// and its age.
#[derive(Debug, PartialEq, Eq)]
pub struct CodeView {
    pub pdb: String,
    pub guid: GUID,
    pub age: u32,
}

impl CodeView {
    /// The PDB's place on the server and in the cache:
    /// `<pdb>/<GUID><age>/<pdb>`.
    pub fn key(&self) -> String {
        let GUID {
            data1,
            data2,
            data3,
            data4,
        } = self.guid;
        let tail = data4.iter().fold(String::new(), |mut tail, byte| {
            let _ = write!(tail, "{byte:02X}");
            tail
        });
        format!(
            "{pdb}/{data1:08X}{data2:04X}{data3:04X}{tail}{age:X}/{pdb}",
            pdb = self.pdb,
            age = self.age
        )
    }

    /// The PDB's address on the server.
    pub fn url(&self) -> String {
        format!("https://{HOST}{ROOT}/{}", self.key())
    }
}

/// The `CodeView` record of the PE image in `file`, or why there is none.
pub fn read_codeview(file: &Path) -> Result<CodeView, String> {
    let bytes = std::fs::read(file).map_err(|error| error.to_string())?;
    codeview(&bytes).ok_or_else(|| "it has no CodeView record".to_owned())
}

/// The `CodeView` (`RSDS`) record of the PE image in `bytes`: the debug
/// directory (data directory 6) lists debug records, each with its offset
/// in the file; the `CodeView` one (type 2) holds `RSDS`, the GUID, the age
/// and the PDB's path.
fn codeview(bytes: &[u8]) -> Option<CodeView> {
    let u16_at = |offset: usize| {
        Some(u16::from_le_bytes(
            bytes.get(offset..offset + 2)?.try_into().ok()?,
        ))
    };
    let u32_at = |offset: usize| {
        Some(u32::from_le_bytes(
            bytes.get(offset..offset + 4)?.try_into().ok()?,
        ))
    };
    let at = |offset: u32| usize::try_from(offset).ok();
    if bytes.get(..2)? != b"MZ" {
        return None;
    }
    let pe = at(u32_at(0x3c)?)?;
    if bytes.get(pe..pe + 4)? != b"PE\0\0" {
        return None;
    }
    let sections = usize::from(u16_at(pe + 6)?);
    let optional = pe + 24;
    let section_table = optional + usize::from(u16_at(pe + 20)?);
    // The data directories follow the optional header's fixed fields,
    // which a 64-bit image's (magic 0x20b) has 16 bytes more of.
    let directories = match u16_at(optional)? {
        0x10b => optional + 96,
        0x20b => optional + 112,
        _ => return None,
    };
    let debug_rva = u32_at(directories + 6 * 8)?;
    let debug_size = u32_at(directories + 6 * 8 + 4)?;
    // The debug directory's offset in the file, from the section holding
    // its address.
    let debug = (0..sections).find_map(|index| {
        let section = section_table + index * 40;
        let virtual_size = u32_at(section + 8)?;
        let address = u32_at(section + 12)?;
        let raw_size = u32_at(section + 16)?;
        let raw = u32_at(section + 20)?;
        let inside = debug_rva >= address && debug_rva - address < virtual_size.max(raw_size);
        inside.then(|| at(raw + (debug_rva - address)))?
    })?;
    (0..at(debug_size)? / 28).find_map(|index| {
        let entry = debug + index * 28;
        if u32_at(entry + 12)? != 2 {
            return None;
        }
        let size = at(u32_at(entry + 16)?)?;
        let record = at(u32_at(entry + 24)?)?;
        let record = bytes.get(record..record + size)?;
        if record.get(..4)? != b"RSDS" {
            return None;
        }
        let guid = GUID::from_values(
            u32::from_le_bytes(record.get(4..8)?.try_into().ok()?),
            u16::from_le_bytes(record.get(8..10)?.try_into().ok()?),
            u16::from_le_bytes(record.get(10..12)?.try_into().ok()?),
            record.get(12..20)?.try_into().ok()?,
        );
        let age = u32::from_le_bytes(record.get(20..24)?.try_into().ok()?);
        let path = record.get(24..)?;
        let path = &path[..path
            .iter()
            .position(|&byte| byte == 0)
            .unwrap_or(path.len())];
        let path = String::from_utf8_lossy(path);
        let pdb = path.rsplit(['\\', '/']).next().unwrap_or(&path).to_owned();
        Some(CodeView { pdb, guid, age })
    })
}

/// Fetches PDBs into a cache, each image's once.
pub struct Server {
    cache: PathBuf,
    /// Each image's PDB folder in the cache, by the image's path, or
    /// `None` when it could not be had.
    folders: HashMap<String, Option<PathBuf>>,
}

impl Server {
    /// Fetches into `cache`, whose PDBs earlier runs fetched are reused.
    pub fn new(cache: PathBuf) -> Self {
        Self {
            cache,
            folders: HashMap::new(),
        }
    }

    /// The folder in the cache holding the PDB of the image at `image`,
    /// fetched from the server if it is not there yet, or `None` when it
    /// cannot be had; what happened is printed to standard error.
    pub fn folder(&mut self, image: &str) -> Option<PathBuf> {
        if let Some(folder) = self.folders.get(image) {
            return folder.clone();
        }
        let folder = self.fetch(image);
        self.folders.insert(image.to_owned(), folder.clone());
        folder
    }

    fn fetch(&self, image: &str) -> Option<PathBuf> {
        let record = match read_codeview(Path::new(image)) {
            Ok(record) => record,
            Err(error) => {
                eprintln!("symbol server: no PDB for {image}: {error}");
                return None;
            }
        };
        let key = record.key();
        let pdb = self.cache.join(key.replace('/', "\\"));
        let folder = pdb.parent()?.to_path_buf();
        if pdb.is_file() {
            eprintln!(
                "symbol server: {} for {image} found in the cache",
                record.pdb
            );
            return Some(folder);
        }
        let fetched = std::fs::create_dir_all(&folder)
            .map_err(|error| error.to_string())
            .and_then(|()| fetch_pdb(&record, &pdb));
        match fetched {
            Ok(how) => {
                eprintln!("symbol server: fetched {} for {image}{how}", record.url());
                Some(folder)
            }
            Err(error) => {
                eprintln!(
                    "symbol server: could not fetch {} for {image}: {error}",
                    record.url()
                );
                None
            }
        }
    }
}

/// Fetches `record`'s PDB to `pdb`, or its compressed form expanded there,
/// saying which.
fn fetch_pdb(record: &CodeView, pdb: &Path) -> Result<&'static str, String> {
    let path = format!("{ROOT}/{}", record.key());
    let partial = pdb.with_extension("partial");
    if download(&path, &partial)? {
        std::fs::rename(&partial, pdb).map_err(|error| error.to_string())?;
        return Ok("");
    }
    // Only the compressed form: the name's last character as `_`.
    let mut compressed_path = path;
    compressed_path.pop();
    compressed_path.push('_');
    let compressed = pdb.with_extension("pd_");
    if !download(&compressed_path, &compressed)? {
        return Err("the server has no such PDB".to_owned());
    }
    let expanded = std::process::Command::new("expand.exe")
        .arg(&compressed)
        .arg(pdb)
        .output()
        .map_err(|error| format!("could not run expand.exe: {error}"));
    let _ = std::fs::remove_file(&compressed);
    let expanded = expanded?;
    if !expanded.status.success() || !pdb.is_file() {
        return Err(format!(
            "expand.exe could not expand the compressed PDB: {}",
            String::from_utf8_lossy(&expanded.stdout).trim()
        ));
    }
    Ok(", expanded from its compressed form")
}

/// A `WinHTTP` handle, closed when dropped.
struct Handle(*mut c_void);

impl Handle {
    fn new(handle: *mut c_void, what: &str) -> Result<Self, String> {
        if handle.is_null() {
            Err(format!("{what}: {}", std::io::Error::last_os_error()))
        } else {
            Ok(Self(handle))
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: a handle WinHTTP returned, closed once.
        let _ = unsafe { WinHttpCloseHandle(self.0) };
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Downloads `path` from the symbol server to `file`: `true` when it was
/// there, `false` when the server has no such file (404). `WinHTTP` follows
/// the server's redirects to its content store itself.
fn download(path: &str, file: &Path) -> Result<bool, String> {
    let agent = wide("verbatim-trace-waits");
    let host = wide(HOST);
    let verb = wide("GET");
    let object = wide(path);
    // SAFETY: NUL-terminated strings alive across the call.
    let session = Handle::new(
        unsafe {
            WinHttpOpen(
                PCWSTR(agent.as_ptr()),
                WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                PCWSTR::null(),
                PCWSTR::null(),
                0,
            )
        },
        "WinHttpOpen",
    )?;
    // SAFETY: a session opened above and a NUL-terminated host name.
    let connection = Handle::new(
        unsafe {
            WinHttpConnect(
                session.0,
                PCWSTR(host.as_ptr()),
                INTERNET_DEFAULT_HTTPS_PORT,
                0,
            )
        },
        "WinHttpConnect",
    )?;
    // SAFETY: a connection opened above and NUL-terminated strings; no
    // accept types.
    let request = Handle::new(
        unsafe {
            WinHttpOpenRequest(
                connection.0,
                PCWSTR(verb.as_ptr()),
                PCWSTR(object.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                std::ptr::null(),
                WINHTTP_FLAG_SECURE,
            )
        },
        "WinHttpOpenRequest",
    )?;
    // SAFETY: a request opened above, with no headers or body.
    unsafe { WinHttpSendRequest(request.0, None, None, 0, 0, 0) }
        .map_err(|error| format!("could not send the request: {error}"))?;
    // SAFETY: as above.
    unsafe { WinHttpReceiveResponse(request.0, std::ptr::null_mut()) }
        .map_err(|error| format!("no response: {error}"))?;
    let mut status = 0u32;
    let mut length = 4u32;
    // SAFETY: the status as a number into a u32, its size given.
    unsafe {
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some((&raw mut status).cast()),
            &raw mut length,
            std::ptr::null_mut(),
        )
    }
    .map_err(|error| format!("no status: {error}"))?;
    match status {
        200 => {}
        404 => return Ok(false),
        other => return Err(format!("the server answered {other}")),
    }
    let mut body = Vec::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let mut read = 0u32;
        // SAFETY: a buffer of the length given, which the call fills.
        unsafe {
            WinHttpReadData(
                request.0,
                buffer.as_mut_ptr().cast(),
                u32::try_from(buffer.len()).unwrap_or(u32::MAX),
                &raw mut read,
            )
        }
        .map_err(|error| format!("could not read the response: {error}"))?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&buffer[..usize::try_from(read).unwrap_or(0)]);
    }
    std::fs::write(file, body).map_err(|error| error.to_string())?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Diagnostics::Debug::{
        IMAGEHLP_MODULEW64, SymCleanup, SymGetModuleInfoW64, SymInitializeW, SymLoadModuleExW,
    };

    /// The `CodeView` record read from this test program's image on disk is
    /// the one `DbgHelp` reports for the same image. The image is this
    /// program's own, a plain image with its PDB beside it, because a system
    /// DLL on Windows on Arm is an ARM64X image, which `DbgHelp` does not
    /// load without its PDB.
    #[test]
    fn the_codeview_record_read_is_the_one_dbghelp_reports() {
        let path = std::env::current_exe().unwrap().display().to_string();
        let read = read_codeview(Path::new(&path)).unwrap();

        let session = HANDLE(0x7_0000 as *mut c_void);
        let empty = wide("");
        let file = wide(&path);
        let base = 0x1_0000_0000;
        // SAFETY: a made-up handle, not read as a process, with an empty
        // search path alive across the call.
        unsafe { SymInitializeW(session, PCWSTR(empty.as_ptr()), false) }.unwrap();
        // SAFETY: a session initialized above and a NUL-terminated path
        // alive across the call.
        let loaded = unsafe {
            SymLoadModuleExW(
                session,
                None,
                PCWSTR(file.as_ptr()),
                PCWSTR::null(),
                base,
                0,
                None,
                None,
            )
        };
        assert_eq!(loaded, base);
        let mut reported = IMAGEHLP_MODULEW64 {
            SizeOfStruct: u32::try_from(size_of::<IMAGEHLP_MODULEW64>()).unwrap(),
            ..IMAGEHLP_MODULEW64::default()
        };
        // SAFETY: the structure, its size given, for the module loaded above.
        unsafe { SymGetModuleInfoW64(session, base, &raw mut reported) }.unwrap();
        // SAFETY: the session initialized above, cleaned up once.
        unsafe { SymCleanup(session) }.unwrap();
        let end = reported.CVData.iter().position(|&unit| unit == 0).unwrap();
        let pdb = String::from_utf16_lossy(&reported.CVData[..end]);
        let reported = CodeView {
            pdb: pdb.rsplit('\\').next().unwrap().to_owned(),
            guid: reported.PdbSig70,
            age: reported.PdbAge,
        };
        assert_eq!(read, reported);
    }

    /// The server address is `<pdb>/<GUID><age>/<pdb>`, the GUID as 32
    /// upper-case hexadecimal digits and the age in hexadecimal.
    #[test]
    fn the_server_address_is_built_from_the_record() {
        let record = CodeView {
            pdb: "ntkrnlmp.pdb".to_owned(),
            guid: GUID::from_u128(0x0a1b2c3d_4e5f_6a7b_8c9d_0e1f2a3b4c5d),
            age: 0x1a,
        };
        assert_eq!(
            record.url(),
            "https://msdl.microsoft.com/download/symbols/ntkrnlmp.pdb/0A1B2C3D4E5F6A7B8C9D0E1F2A3B4C5D1A/ntkrnlmp.pdb"
        );
    }
}
