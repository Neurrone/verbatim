//! Preconditions a fact can need. A failed one makes the claims that need
//! it inconclusive, with the reason; it never passes and never takes
//! another path.

use std::path::PathBuf;

use windows::Win32::System::StationsAndDesktops::{
    CloseDesktop, DESKTOP_ACCESS_FLAGS, DESKTOP_CONTROL_FLAGS, GetUserObjectInformationW,
    OpenInputDesktop, UOI_NAME,
};

/// Whether the input desktop is the user's unlocked desktop, `Default`.
///
/// # Errors
///
/// The reason, when it is locked or another desktop is in front.
///
/// # Panics
///
/// Never: the buffer's size fits a `u32`.
pub fn unlocked_desktop() -> Result<(), String> {
    // SAFETY: no precondition; the handle is closed below.
    let desktop =
        unsafe { OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_ACCESS_FLAGS(0)) }
            .map_err(|e| format!("the input desktop cannot be opened, so it is locked ({e})"))?;
    let mut name = [0u16; 64];
    let mut needed = 0u32;
    // SAFETY: the desktop opened above and a live buffer of the size given.
    let read = unsafe {
        GetUserObjectInformationW(
            windows::Win32::Foundation::HANDLE(desktop.0),
            UOI_NAME,
            Some(name.as_mut_ptr().cast()),
            u32::try_from(std::mem::size_of_val(&name)).expect("a small buffer"),
            Some(&raw mut needed),
        )
    };
    // SAFETY: the desktop opened above, closed once.
    let _ = unsafe { CloseDesktop(desktop) };
    read.map_err(|e| format!("the input desktop's name cannot be read ({e})"))?;
    let length = name.iter().position(|c| *c == 0).unwrap_or(name.len());
    let name = String::from_utf16_lossy(&name[..length]);
    if name == "Default" {
        Ok(())
    } else {
        Err(format!(
            "the input desktop is {name}, not the unlocked Default desktop"
        ))
    }
}

/// cdb's path: `CDB` when set, otherwise the app execution alias of `WinDbg` for
/// this machine's architecture in `%LOCALAPPDATA%\Microsoft\WindowsApps`
/// (`cdbX64.exe` or `cdbARM64.exe`), where winget's `Microsoft.WinDbg`
/// puts it (`books/dev/src/how-to/debugging.md`, "Where cdb is").
///
/// # Errors
///
/// The reason, naming `CDB`, when no cdb is there.
pub fn cdb() -> Result<PathBuf, String> {
    let alias = if cfg!(target_arch = "aarch64") {
        "cdbARM64.exe"
    } else {
        "cdbX64.exe"
    };
    let path = if let Some(path) = std::env::var_os("CDB") {
        PathBuf::from(path)
    } else {
        let local = std::env::var_os("LOCALAPPDATA")
            .ok_or("LOCALAPPDATA is not set; set CDB to cdb's path")?;
        PathBuf::from(local)
            .join(r"Microsoft\WindowsApps")
            .join(alias)
    };
    // An app execution alias is a reparse point that ordinary metadata
    // cannot follow, so its presence is read without following it.
    if std::fs::symlink_metadata(&path).is_ok() {
        Ok(path)
    } else {
        Err(format!(
            "cdb is not at {}; install WinDbg (winget's Microsoft.WinDbg) or set CDB to cdb's path",
            path.display()
        ))
    }
}
