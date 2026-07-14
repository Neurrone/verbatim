//! [`crate::protocol::Request::ReadFile`]: pulling small files (logs,
//! crash dumps) off the guest for a host-side test to inspect.

use std::io;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

/// Files larger than this are refused outright: this request is for logs
/// and small dumps a test wants to assert on, not bulk transfer.
pub const MAX_READ_FILE_BYTES: u64 = 8 * 1024 * 1024;

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
    fn refuses_a_file_over_the_size_limit() {
        let file = tempfile();
        let oversized = vec![0u8; usize::try_from(MAX_READ_FILE_BYTES).unwrap() + 1];
        std::fs::write(&file.0, oversized).expect("writes an oversized file");

        let error = read_base64(file.0.to_str().expect("utf8 path"))
            .expect_err("refuses a file over the limit");
        assert!(error.to_string().contains("ReadFile limit"));

        std::fs::remove_file(&file.0).ok();
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
