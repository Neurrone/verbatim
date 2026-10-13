//! Function names for addresses a trace recorded, through Windows' own
//! `DbgHelp`, which needs nothing installed: an image whose PDB is where its
//! build left it (Verbatim's own) is named from the PDB, and any other from
//! its exported functions, the nearest export below the address, which for
//! a system library is a hint at best. Kernel addresses are left as module
//! and offset.
//!
//! The trace names each image by its path on a device
//! (`\Device\HarddiskVolume3\...`), turned into a drive path through the
//! system's DOS device names. Each traced process gets its own `DbgHelp`
//! session, keyed by a made-up handle as `DbgHelp` allows when it is not
//! asked to read a live process, since processes load images at the same
//! addresses.

use std::collections::{HashMap, HashSet};
use std::ffi::c_void;

use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::QueryDosDeviceW;
use windows::Win32::System::Diagnostics::Debug::{
    SYMBOL_INFOW, SYMOPT_DEFERRED_LOADS, SYMOPT_FAIL_CRITICAL_ERRORS, SYMOPT_NO_PROMPTS,
    SYMOPT_UNDNAME, SymFromAddrW, SymInitializeW, SymLoadModuleExW, SymSetOptions,
    SymSetSearchPathW,
};
use windows::core::PCWSTR;

/// The longest name read, in UTF-16 units.
const MAX_NAME: usize = 1024;

/// One image loaded in a process: its base, size and path on a device.
pub struct Image<'a> {
    pub base: u64,
    pub size: u64,
    pub path: &'a str,
}

/// Names addresses, loading each image's symbols the first time one of its
/// addresses is asked for.
pub struct Symbols {
    /// Each drive's device name and its drive, such as
    /// `\Device\HarddiskVolume3` and `C:`.
    drives: Vec<(String, String)>,
    /// The processes with a `DbgHelp` session.
    sessions: HashSet<u32>,
    /// The images loaded into each process's session.
    loaded: HashSet<(u32, u64)>,
    /// The folders each process's session searches for PDBs: those of the
    /// images loaded into it, since a Rust binary names its PDB without a
    /// folder, after the folders given for every image.
    folders: HashMap<u32, Vec<String>>,
    /// Folders searched for every image's PDB, for images run from a copy
    /// without its PDB (the end-to-end harness's stage).
    given: Vec<String>,
    names: HashMap<(u32, u64), Option<String>>,
}

impl Symbols {
    /// Names addresses, searching `given` for every image's PDB as well as
    /// the image's own folder.
    pub fn new(given: Vec<String>) -> Self {
        // SAFETY: sets process-wide `DbgHelp` options; no pointer is passed.
        unsafe {
            SymSetOptions(
                SYMOPT_UNDNAME
                    | SYMOPT_DEFERRED_LOADS
                    | SYMOPT_FAIL_CRITICAL_ERRORS
                    | SYMOPT_NO_PROMPTS,
            );
        }
        Self {
            drives: drives(),
            sessions: HashSet::new(),
            loaded: HashSet::new(),
            folders: HashMap::new(),
            given,
            names: HashMap::new(),
        }
    }

    /// The name of the function holding `address` in `image` of `process`,
    /// or `None` when `DbgHelp` has none (a kernel image, or one that cannot
    /// be read).
    pub fn name(&mut self, process: u32, address: u64, image: &Image<'_>) -> Option<String> {
        if let Some(name) = self.names.get(&(process, address)) {
            return name.clone();
        }
        let name = self.look_up(process, address, image);
        self.names.insert((process, address), name.clone());
        name
    }

    fn look_up(&mut self, process: u32, address: u64, image: &Image<'_>) -> Option<String> {
        let path = self.drive_path(image.path)?;
        let session = session(process);
        if self.sessions.insert(process) {
            // SAFETY: a made-up handle, as `DbgHelp` allows when it is not
            // asked to read the process; no search path.
            unsafe { SymInitializeW(session, PCWSTR::null(), false) }.ok()?;
        }
        if self.loaded.insert((process, image.base)) {
            let folder = path.rsplit_once('\\').map_or("", |(folder, _)| folder);
            let folders = self
                .folders
                .entry(process)
                .or_insert_with(|| self.given.clone());
            if !folders.iter().any(|known| known == folder) {
                folders.push(folder.to_owned());
                let joined: Vec<u16> = folders
                    .join(";")
                    .encode_utf16()
                    .chain(std::iter::once(0))
                    .collect();
                // SAFETY: a NUL-terminated search path alive across the
                // call, for a session initialized above.
                let _ = unsafe { SymSetSearchPathW(session, PCWSTR(joined.as_ptr())) };
            }
            let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
            // SAFETY: a NUL-terminated path alive across the call; the
            // session was initialized above. A failed load leaves the image
            // unnamed.
            unsafe {
                SymLoadModuleExW(
                    session,
                    None,
                    PCWSTR(wide.as_ptr()),
                    PCWSTR::null(),
                    image.base,
                    u32::try_from(image.size).unwrap_or(u32::MAX),
                    None,
                    None,
                )
            };
        }
        // SYMBOL_INFOW is followed by its name: room for MAX_NAME units,
        // in a buffer of u64s so the structure is aligned.
        let words = (size_of::<SYMBOL_INFOW>() + MAX_NAME * 2).div_ceil(8);
        let mut buffer = vec![0u64; words];
        let info = buffer.as_mut_ptr().cast::<SYMBOL_INFOW>();
        let header = SYMBOL_INFOW {
            SizeOfStruct: u32::try_from(size_of::<SYMBOL_INFOW>()).unwrap_or(0),
            MaxNameLen: u32::try_from(MAX_NAME).unwrap_or(0),
            ..SYMBOL_INFOW::default()
        };
        // SAFETY: `info` points at an aligned buffer at least as large as
        // the structure.
        unsafe { info.write(header) };
        let mut displacement = 0u64;
        // SAFETY: the buffer holds the structure and MaxNameLen units after
        // it; the call writes no more.
        unsafe { SymFromAddrW(session, address, Some(&raw mut displacement), info) }.ok()?;
        // SAFETY: the call wrote the structure.
        let written = unsafe { info.read() };
        let length = usize::try_from(written.NameLen).unwrap_or(0).min(MAX_NAME);
        // SAFETY: the name starts at the structure's `Name` field, inside
        // the buffer.
        let name = unsafe { (&raw const (*info).Name).cast::<u16>() };
        // SAFETY: the call wrote `NameLen` units from there, within
        // `MaxNameLen`, which the buffer has room for.
        let units = unsafe { std::slice::from_raw_parts(name, length) };
        Some(String::from_utf16_lossy(units))
    }

    /// `path` on a device as a path on a drive, or `None` for one on no
    /// drive (the kernel's `\SystemRoot\...` images).
    fn drive_path(&self, path: &str) -> Option<String> {
        self.drives.iter().find_map(|(device, drive)| {
            let rest = path.strip_prefix(device.as_str())?;
            rest.starts_with('\\').then(|| format!("{drive}{rest}"))
        })
    }
}

/// The made-up handle naming `process`'s `DbgHelp` session.
fn session(process: u32) -> HANDLE {
    HANDLE((process as usize + 0x1_0000) as *mut c_void)
}

/// Each drive letter's device name.
fn drives() -> Vec<(String, String)> {
    let mut drives = Vec::new();
    for letter in b'A'..=b'Z' {
        let drive = format!("{}:", char::from(letter));
        let wide: Vec<u16> = drive.encode_utf16().chain(std::iter::once(0)).collect();
        let mut target = [0u16; 512];
        // SAFETY: a NUL-terminated drive name and a buffer the call fills,
        // returning how many units it wrote.
        let written = unsafe { QueryDosDeviceW(PCWSTR(wide.as_ptr()), Some(&mut target)) };
        if written == 0 {
            continue;
        }
        let end = target.iter().position(|&unit| unit == 0).unwrap_or(0);
        drives.push((String::from_utf16_lossy(&target[..end]), drive));
    }
    drives
}
