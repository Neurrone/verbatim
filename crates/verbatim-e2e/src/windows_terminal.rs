//! The copy of Windows Terminal the terminal scenarios drive: the official
//! portable release, at a pinned version, kept apart from any Windows
//! Terminal the machine's user runs.
//!
//! Windows Terminal runs every window of one installation in one process.
//! A scenario that opened its window through `wt.exe` joined the user's own
//! terminal process, so anything that went wrong in that process ended the
//! user's terminals with it. The scenarios instead run the release's zip,
//! unpacked into a folder of its own under the stage, in portable mode (a
//! `.portable` file next to the executable): its settings live in that
//! folder, and since an unpackaged Windows Terminal names its
//! single-instance window class and mutex with a hash of its own
//! executable's path, it never hands its command line to the installed
//! one, nor the installed one to it. The release is downloaded from GitHub
//! with Windows' own `curl.exe`, checked against its pinned SHA-256, and
//! unpacked with Windows' own `tar.exe`, for the host's architecture, x64
//! or ARM64.

use std::fmt::Write as _;
use std::fs;
use std::io::{self, Read as _};
use std::os::windows::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest as _, Sha256};

/// The pinned release: the version the development machine's own Windows
/// Terminal runs, so a scenario tests what its user has.
pub const VERSION: &str = "1.24.12741.0";

/// The folder, under the stage, that holds the unpacked release.
pub const FOLDER: &str = "windows-terminal-1.24.12741.0";

/// The executable a scenario starts, inside [`FOLDER`].
pub const EXECUTABLE: &str = "WindowsTerminal.exe";

/// The file whose presence next to [`EXECUTABLE`] puts Windows Terminal in
/// portable mode.
const PORTABLE_MARKER: &str = ".portable";

/// The folder, inside [`FOLDER`], where portable mode keeps its settings
/// and state.
pub const SETTINGS_FOLDER: &str = "settings";

/// The file written last when the release has been unpacked, holding the
/// archive's SHA-256: a folder without it, or with another hash, is
/// unpacked again.
const STAMP: &str = "verbatim-release.sha256";

/// One architecture's release archive.
struct Release {
    /// Where GitHub serves it.
    url: &'static str,
    /// Its SHA-256, as GitHub's release lists it, in lowercase hex.
    sha256: &'static str,
}

/// The x64 release archive.
const X64: Release = Release {
    url: "https://github.com/microsoft/terminal/releases/download/v1.24.12741.0/Microsoft.WindowsTerminal_1.24.12741.0_x64.zip",
    sha256: "7aaa8321064c45d5af79205db13deb87f146f644bd8abdfae29f0155d7ce744e",
};

/// The ARM64 release archive.
const ARM64: Release = Release {
    url: "https://github.com/microsoft/terminal/releases/download/v1.24.12741.0/Microsoft.WindowsTerminal_1.24.12741.0_arm64.zip",
    sha256: "50af3d2c5ca3c278a601b4f7f12c533c1443a66e84359c3200cef2642d689100",
};

/// The release for the architecture this was built for, which is the
/// machine's own: builds never name a target.
fn release() -> &'static Release {
    if cfg!(target_arch = "aarch64") {
        &ARM64
    } else {
        &X64
    }
}

/// Makes sure the pinned release is unpacked, in portable mode, in
/// [`FOLDER`] under `stage_dir`, downloading and checking it when it is
/// missing or was unpacked from another archive, and returns that folder.
/// An unpacked release is left as it is, settings included; a scenario
/// clears the settings before it starts the terminal.
///
/// # Errors
///
/// Returns an error if the download fails, the archive's SHA-256 is not the
/// pinned one, unpacking fails, or the archive holds no
/// `WindowsTerminal.exe`.
pub fn prepare(stage_dir: &Path) -> io::Result<PathBuf> {
    let release = release();
    let folder = stage_dir.join(FOLDER);
    let stamp = folder.join(STAMP);
    if fs::read_to_string(&stamp).is_ok_and(|hash| hash.trim() == release.sha256)
        && folder.join(EXECUTABLE).is_file()
        && folder.join(PORTABLE_MARKER).is_file()
    {
        return Ok(folder);
    }
    fs::create_dir_all(stage_dir)?;
    let archive = stage_dir.join(format!("{FOLDER}.zip"));
    let unpacked = stage_dir.join(format!("{FOLDER}.unpacking"));
    remove_dir_if_present(&unpacked)?;
    remove_dir_if_present(&folder)?;

    println!(
        "downloading Windows Terminal {VERSION} from {}",
        release.url
    );
    run(Command::new(system_tool("curl.exe"))
        .args([
            "--location",
            "--fail",
            "--silent",
            "--show-error",
            "--output",
        ])
        .arg(&archive)
        .arg(release.url))?;
    let actual = sha256(&archive)?;
    if actual != release.sha256 {
        fs::remove_file(&archive)?;
        return Err(io::Error::other(format!(
            "{} has SHA-256 {actual}, not the pinned {}",
            release.url, release.sha256
        )));
    }

    fs::create_dir_all(&unpacked)?;
    run(Command::new(system_tool("tar.exe"))
        .arg("-xf")
        .arg(&archive)
        .arg("-C")
        .arg(&unpacked))?;
    // The archive holds one folder, `terminal-<version>`, with the
    // executable in it.
    let mut roots = Vec::new();
    for entry in fs::read_dir(&unpacked)? {
        let path = entry?.path();
        if path.join(EXECUTABLE).is_file() {
            roots.push(path);
        }
    }
    let [root] = roots.as_slice() else {
        return Err(io::Error::other(format!(
            "{} holds {} folders with {EXECUTABLE}, not one",
            release.url,
            roots.len()
        )));
    };
    fs::rename(root, &folder)?;
    fs::remove_dir_all(&unpacked)?;
    fs::remove_file(&archive)?;
    fs::write(folder.join(PORTABLE_MARKER), b"")?;
    fs::write(&stamp, release.sha256)?;
    Ok(folder)
}

/// Deletes `dir` and everything in it, when it exists.
fn remove_dir_if_present(dir: &Path) -> io::Result<()> {
    match fs::remove_dir_all(dir) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

/// The SHA-256 of the file at `path`, in lowercase hex.
fn sha256(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 1 << 16];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let mut hex = String::with_capacity(64);
    for byte in hasher.finalize() {
        // Writing to a String cannot fail.
        let _ = write!(hex, "{byte:02x}");
    }
    Ok(hex)
}

/// A program Windows ships in `System32`, by its full path: under Git Bash
/// a bare `tar` is GNU tar, which reads `C:\...` as a remote host.
fn system_tool(name: &str) -> PathBuf {
    let system_root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    Path::new(&system_root).join("System32").join(name)
}

/// Runs `command`, failing unless it succeeds.
fn run(command: &mut Command) -> io::Result<()> {
    let status = command
        .creation_flags(crate::CREATE_NO_WINDOW)
        .status()
        .map_err(|error| io::Error::other(format!("could not run {command:?}: {error}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("{command:?} failed: {status}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_release_is_the_pinned_version_for_this_architecture() {
        let release = release();
        assert!(release.url.contains(&format!("v{VERSION}/")));
        let architecture = if cfg!(target_arch = "aarch64") {
            "arm64"
        } else {
            "x64"
        };
        assert!(
            release
                .url
                .ends_with(&format!("_{VERSION}_{architecture}.zip"))
        );
        assert_eq!(FOLDER, format!("windows-terminal-{VERSION}"));
    }

    #[test]
    fn a_file_s_sha256_is_lowercase_hex() {
        let dir = std::env::temp_dir().join(format!(
            "verbatim-e2e-windows-terminal-hash-{}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("creates the folder");
        let file = dir.join("abc");
        fs::write(&file, b"abc").expect("writes the file");
        assert_eq!(
            sha256(&file).expect("hashes the file"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        fs::remove_dir_all(&dir).expect("removes the folder");
    }
}
