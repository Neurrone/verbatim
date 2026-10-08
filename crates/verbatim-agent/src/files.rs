//! [`crate::protocol::Request::ReadFile`] and
//! [`crate::protocol::Request::ListFiles`]: pulling small files (logs, crash
//! dumps) off the guest for a host-side test to inspect. Also the requests
//! that lay out a test's own files and folders and remove them again.

use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use windows::Win32::Foundation::WAIT_OBJECT_0;
use windows::Win32::Storage::FileSystem::{
    FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FindCloseChangeNotification,
    FindFirstChangeNotificationW, FindNextChangeNotification,
};
use windows::Win32::System::Threading::WaitForSingleObject;
use windows::core::HSTRING;

/// Files larger than this are refused outright: this request is for logs
/// and small dumps a test wants to assert on, not bulk transfer.
pub const MAX_READ_FILE_BYTES: u64 = 8 * 1024 * 1024;

/// Waits up to `timeout` for `path` to exist, checking again each time
/// Windows reports a file in its folder created, renamed, or written
/// (`FindFirstChangeNotificationW`): evidence, never a poll. Returns whether
/// it exists when the wait ends.
///
/// # Errors
///
/// Returns an error if `path` has no folder, or the folder cannot be
/// watched.
pub fn wait_for(path: &str, timeout: Duration) -> io::Result<bool> {
    let file = Path::new(path);
    if file.exists() {
        return Ok(true);
    }
    let folder = file
        .parent()
        .ok_or_else(|| io::Error::other(format!("{path} has no folder to watch")))?;
    // SAFETY: a change notification on a folder path; closed below.
    let change = unsafe {
        FindFirstChangeNotificationW(
            &HSTRING::from(folder.as_os_str()),
            false,
            FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_LAST_WRITE,
        )
    }
    .map_err(io::Error::other)?;
    let deadline = Instant::now() + timeout;
    let exists = loop {
        // Checked once the folder is watched, so a file created between the
        // first check and the watch is seen.
        if file.exists() {
            break true;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        let milliseconds = u32::try_from(remaining.as_millis()).unwrap_or(u32::MAX - 1);
        // SAFETY: `change` is an open notification handle.
        let waited = unsafe { WaitForSingleObject(change, milliseconds) };
        if waited != WAIT_OBJECT_0 {
            break file.exists();
        }
        // SAFETY: as above; re-arms the notification.
        if unsafe { FindNextChangeNotification(change) }.is_err() {
            break file.exists();
        }
    };
    // SAFETY: closes the notification handle opened above, once.
    let _ = unsafe { FindCloseChangeNotification(change) };
    Ok(exists)
}

/// Reads `path` and returns its contents, base64 encoded.
///
/// # Errors
///
/// Returns an error if the file cannot be opened or read, or if it is
/// larger than [`MAX_READ_FILE_BYTES`].
pub fn read_base64(path: &str) -> io::Result<String> {
    let metadata = std::fs::metadata(path)?;
    if metadata.len() > MAX_READ_FILE_BYTES {
        return Err(io::Error::other(format!(
            "{path} is {} bytes, over the {MAX_READ_FILE_BYTES}-byte ReadFile limit",
            metadata.len()
        )));
    }
    let bytes = std::fs::read(path)?;
    Ok(STANDARD.encode(bytes))
}

/// Reads at most [`MAX_READ_FILE_BYTES`] of `path` from `offset`, base64
/// encoded; empty at or past the end.
///
/// # Errors
///
/// Returns an error if the file cannot be opened or read.
pub fn read_chunk_base64(path: &str, offset: u64) -> io::Result<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::new();
    file.take(MAX_READ_FILE_BYTES).read_to_end(&mut bytes)?;
    Ok(STANDARD.encode(bytes))
}

/// Writes `data_base64`, decoded, to `path`, creating or replacing it, and
/// creating any missing parent directories, so a test can lay out a folder
/// of files.
///
/// # Errors
///
/// Returns an error if the data is not valid base64, is larger than
/// [`MAX_READ_FILE_BYTES`], or the file cannot be written.
pub fn write_base64(path: &str, data_base64: &str) -> io::Result<()> {
    let bytes = STANDARD
        .decode(data_base64)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_READ_FILE_BYTES {
        return Err(io::Error::other(format!(
            "{} bytes is over the {MAX_READ_FILE_BYTES}-byte WriteFile limit",
            bytes.len()
        )));
    }
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, bytes)
}

/// Deletes the file at `path`; a file that is already gone is not an
/// error.
///
/// # Errors
///
/// Returns an error if the file exists and cannot be deleted.
pub fn delete(path: &str) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

/// Deletes the folder at `path` and everything in it; a folder that is
/// already gone is not an error.
///
/// # Errors
///
/// Returns an error if the folder exists and cannot be deleted, for
/// example because a process still has it open.
pub fn delete_folder(path: &str) -> io::Result<()> {
    match std::fs::remove_dir_all(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

/// The names of the folders directly inside `path`, sorted.
///
/// # Errors
///
/// Returns an error if the directory cannot be read.
pub fn list_folders(path: &str) -> io::Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();
    Ok(names)
}

/// The names of the files directly inside `path`, sorted.
///
/// # Errors
///
/// Returns an error if the directory cannot be read.
pub fn list(path: &str) -> io::Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();
    Ok(names)
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    #[test]
    fn reads_a_small_file_as_base64() {
        let mut file = tempfile();
        file.1.write_all(b"hello, agent").expect("writes");
        drop(file.1);

        let encoded = read_base64(file.0.to_str().expect("utf8 path")).expect("reads");
        let decoded = STANDARD.decode(encoded).expect("valid base64");
        assert_eq!(decoded, b"hello, agent");

        std::fs::remove_file(&file.0).ok();
    }

    #[test]
    fn reads_a_file_over_the_size_limit_in_chunks() {
        let file = tempfile();
        let size = usize::try_from(MAX_READ_FILE_BYTES).unwrap() + 3;
        std::fs::write(&file.0, vec![7u8; size]).expect("writes a large file");
        let path = file.0.to_str().expect("utf8 path");

        let chunk = |offset| {
            STANDARD
                .decode(read_chunk_base64(path, offset).expect("reads"))
                .expect("base64")
        };
        assert_eq!(chunk(0).len(), size - 3);
        assert_eq!(chunk(MAX_READ_FILE_BYTES), vec![7u8; 3]);
        assert!(
            chunk(MAX_READ_FILE_BYTES + 3).is_empty(),
            "empty at the end"
        );

        std::fs::remove_file(&file.0).ok();
    }

    #[test]
    fn refuses_a_file_over_the_size_limit() {
        let file = tempfile();
        let oversized = vec![0u8; usize::try_from(MAX_READ_FILE_BYTES).unwrap() + 1];
        std::fs::write(&file.0, oversized).expect("writes an oversized file");

        let error = read_base64(file.0.to_str().expect("utf8 path"))
            .expect_err("refuses a file over the limit");
        assert!(error.to_string().contains("ReadFile limit"));

        std::fs::remove_file(&file.0).ok();
    }

    #[test]
    fn lists_the_files_in_a_directory_but_not_its_subdirectories() {
        let dir = std::env::temp_dir().join(format!(
            "verbatim-agent-list-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(dir.join("nested")).expect("creates the directories");
        std::fs::write(dir.join("b.log"), b"b").expect("writes");
        std::fs::write(dir.join("a.log"), b"a").expect("writes");

        let names = list(dir.to_str().expect("utf8 path")).expect("lists");
        assert_eq!(names, ["a.log", "b.log"]);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn lists_and_deletes_a_folder_with_its_contents() {
        let dir = std::env::temp_dir().join(format!(
            "verbatim-agent-folder-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let nested = dir.join("nested");
        std::fs::create_dir_all(nested.join("deeper")).expect("creates the directories");
        std::fs::write(dir.join("a.log"), b"a").expect("writes");
        std::fs::write(nested.join("b.log"), b"b").expect("writes");
        let dir_path = dir.to_str().expect("utf8 path");

        assert_eq!(list_folders(dir_path).expect("lists"), ["nested"]);
        let nested_path = nested.to_str().expect("utf8 path");
        delete_folder(nested_path).expect("deletes the folder");
        assert!(!nested.exists(), "the folder and its contents are gone");
        delete_folder(nested_path).expect("a folder already gone is not an error");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A throwaway file path under the OS temp directory plus an open
    /// handle to write through, named uniquely per call so parallel tests
    /// never collide.
    fn tempfile() -> (std::path::PathBuf, std::fs::File) {
        let path = std::env::temp_dir().join(format!(
            "verbatim-agent-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let file = std::fs::File::create(&path).expect("creates a temp file");
        (path, file)
    }
}
