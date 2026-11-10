//! What is recorded: a process by name or id, or a top-level window.

use std::os::windows::io::{FromRawHandle as _, OwnedHandle, RawHandle};

use serde_json::{Value, json};
use verbatim_uia::{is_top_level_window, provider_process};
use verbatim_win32::window::{
    related, top_level_of, top_level_windows, window_class_name, window_is_visible, window_text,
};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE};
use windows::Win32::UI::WindowsAndMessaging::GW_OWNER;

use crate::line::window_text as window_field;

/// The class of a Win32 popup menu's top-level window.
const MENU_CLASS: &str = "#32768";

/// The class of a common control tool tip's top-level window.
const TOOL_TIP_CLASS: &str = "tooltips_class32";

/// The target as the command line names it.
pub(crate) enum Selector {
    /// `--process <name or id>`.
    Process(String),
    /// `--window <handle>`.
    Window(isize),
}

/// The resolved target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    /// Every window of this process.
    Process(u32),
    /// This top-level window, served by process `pid`.
    Window {
        /// The window.
        hwnd: isize,
        /// `provider_process` of the window.
        pid: u32,
    },
}

impl Target {
    /// The process its hook is scoped to.
    pub(crate) fn pid(self) -> u32 {
        match self {
            Self::Process(pid) | Self::Window { pid, .. } => pid,
        }
    }

    /// The header line's `target` field.
    pub(crate) fn header_field(self) -> Value {
        match self {
            Self::Process(pid) => json!({ "pid": pid, "window": Value::Null }),
            Self::Window { hwnd, pid } => json!({ "pid": pid, "window": window_field(hwnd) }),
        }
    }
}

/// `--window`'s handle, in decimal or `0x` hexadecimal.
///
/// # Errors
///
/// Returns a message for anything else.
pub(crate) fn parse_window(text: &str) -> Result<isize, String> {
    let parsed = match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => isize::from_str_radix(hex, 16),
        None => text.parse(),
    };
    parsed.map_err(|_| format!("not a window handle: {text:?}"))
}

/// Resolves `selector` into the [`Target`].
///
/// # Errors
///
/// Returns a message naming what is wrong: no such process, several by
/// that name, or a handle that is not a top-level window with a process.
pub(crate) fn resolve(selector: &Selector) -> Result<Target, String> {
    match selector {
        Selector::Window(hwnd) => {
            if !is_top_level_window(*hwnd) {
                return Err(format!(
                    "window {} is not a top-level window",
                    window_field(*hwnd)
                ));
            }
            let pid = provider_process(*hwnd).ok_or_else(|| {
                format!(
                    "window {} has no process that serves it",
                    window_field(*hwnd)
                )
            })?;
            Ok(Target::Window { hwnd: *hwnd, pid })
        }
        Selector::Process(text) => resolve_process(text),
    }
}

/// `text` as a process id, or a unique process name.
fn resolve_process(text: &str) -> Result<Target, String> {
    if let Ok(pid) = text.parse::<u32>() {
        open_process(pid).map_err(|error| format!("--process {pid}: {error}"))?;
        return Ok(Target::Process(pid));
    }
    let pids = processes_named(text)?;
    match pids.as_slice() {
        [] => Err(format!("no process is named {text}")),
        [pid] => Ok(Target::Process(*pid)),
        several => {
            let listed: Vec<String> = several
                .iter()
                .map(|&pid| format!("{pid}, {}", window_title(pid)))
                .collect();
            Err(format!(
                "several processes are named {text}; name one by id:\n{}",
                listed.join("\n")
            ))
        }
    }
}

/// The title of the first visible top-level window of `pid` that has one.
fn window_title(pid: u32) -> String {
    top_level_windows(pid)
        .into_iter()
        .filter(|&window| window_is_visible(window))
        .find_map(window_text)
        .unwrap_or_else(|| "no window title".to_owned())
}

/// The ids of the processes named `name`, with or without `.exe`.
fn processes_named(name: &str) -> Result<Vec<u32>, String> {
    let name = process_stem(name);
    // SAFETY: a snapshot of the system's processes; closed below.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }
        .map_err(|error| format!("could not list the processes: {error}"))?;
    let mut entry = PROCESSENTRY32W {
        dwSize: u32::try_from(size_of::<PROCESSENTRY32W>()).unwrap_or(0),
        ..Default::default()
    };
    let mut pids = Vec::new();
    // SAFETY: `snapshot` is the live snapshot and `entry` has its size set.
    let mut more = unsafe { Process32FirstW(snapshot, &raw mut entry) }.is_ok();
    while more {
        let length = entry
            .szExeFile
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(entry.szExeFile.len());
        let exe = String::from_utf16_lossy(&entry.szExeFile[..length]);
        if process_stem(&exe) == name {
            pids.push(entry.th32ProcessID);
        }
        // SAFETY: as above.
        more = unsafe { Process32NextW(snapshot, &raw mut entry) }.is_ok();
    }
    // SAFETY: the handle the snapshot call returned, closed once.
    let _ = unsafe { CloseHandle(snapshot) };
    Ok(pids)
}

/// `name` as process names are compared: lower case, without `.exe`.
fn process_stem(name: &str) -> String {
    let lower = name.to_lowercase();
    match lower.strip_suffix(".exe") {
        Some(stem) => stem.to_owned(),
        None => lower,
    }
}

/// What the `--window` rule reads of a window's top-level window.
#[derive(Debug)]
struct RootFacts {
    /// The top-level window (`GetAncestor` with `GA_ROOT`).
    root: isize,
    /// The root's owners, nearest first (`GetWindow` with `GW_OWNER`).
    owners: Vec<isize>,
    /// The root's window class.
    class: String,
    /// The process serving the root.
    process: Option<u32>,
}

/// `window`'s [`RootFacts`], read locally.
fn root_facts(window: isize) -> RootFacts {
    let root = top_level_of(window);
    let mut owners = Vec::new();
    let mut last = root;
    loop {
        let owner = related(last, GW_OWNER);
        if owner == 0 {
            break;
        }
        owners.push(owner);
        last = owner;
    }
    RootFacts {
        root,
        owners,
        class: window_class_name(root),
        process: provider_process(root),
    }
}

/// The `--window` rule: an element is the target's when its root window
/// is the target, the target owns that root through its owner chain, or
/// that root is a menu or tool tip of the target's process, each of which
/// Windows makes a top-level window of its own.
fn counts_as_target(hwnd: isize, pid: u32, facts: &RootFacts) -> bool {
    facts.root == hwnd
        || facts.owners.contains(&hwnd)
        || (facts.process == Some(pid)
            && (facts.class == MENU_CLASS || facts.class == TOOL_TIP_CLASS))
}

/// Whether `window` counts as part of the `--window` target `hwnd`, served
/// by `pid`. Local calls only.
pub(crate) fn window_in_target(hwnd: isize, pid: u32, window: isize) -> bool {
    counts_as_target(hwnd, pid, &root_facts(window))
}

/// Opens `pid` for waiting on its end.
///
/// # Errors
///
/// Returns a message when the process cannot be opened, as when it has
/// gone.
pub(crate) fn open_process(pid: u32) -> Result<OwnedHandle, String> {
    // SAFETY: opening a process by id; the handle is owned below.
    let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) }
        .map_err(|error| format!("process {pid} could not be opened: {error}"))?;
    // SAFETY: a valid handle just returned, owned by nothing else.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle.0 as RawHandle) })
}

/// The handle's raw value, for a wait.
pub(crate) fn raw(owned: &OwnedHandle) -> HANDLE {
    use std::os::windows::io::AsRawHandle as _;
    HANDLE(owned.as_raw_handle())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TARGET: isize = 100;
    const PROCESS: u32 = 7;

    fn root(root: isize, owners: &[isize], class: &str, process: u32) -> RootFacts {
        RootFacts {
            root,
            owners: owners.to_vec(),
            class: class.to_owned(),
            process: Some(process),
        }
    }

    #[test]
    fn a_process_name_matches_in_any_case_with_or_without_exe() {
        assert_eq!(process_stem("NOTEPAD.EXE"), "notepad");
        assert_eq!(process_stem("Notepad"), "notepad");
    }

    #[test]
    fn an_element_whose_root_is_the_target_counts() {
        assert!(counts_as_target(
            TARGET,
            PROCESS,
            &root(TARGET, &[], "Frame", PROCESS)
        ));
    }

    #[test]
    fn a_window_the_target_owns_through_its_chain_counts() {
        assert!(counts_as_target(
            TARGET,
            PROCESS,
            &root(300, &[200, TARGET], "Dialog", 99)
        ));
    }

    #[test]
    fn a_menu_of_the_targets_process_counts() {
        assert!(counts_as_target(
            TARGET,
            PROCESS,
            &root(300, &[], MENU_CLASS, PROCESS)
        ));
    }

    #[test]
    fn a_tool_tip_of_the_targets_process_counts() {
        assert!(counts_as_target(
            TARGET,
            PROCESS,
            &root(300, &[], TOOL_TIP_CLASS, PROCESS)
        ));
    }

    #[test]
    fn a_menu_of_another_process_does_not_count() {
        assert!(!counts_as_target(
            TARGET,
            PROCESS,
            &root(300, &[], MENU_CLASS, 99)
        ));
    }

    #[test]
    fn another_top_level_window_of_the_targets_process_does_not_count() {
        assert!(!counts_as_target(
            TARGET,
            PROCESS,
            &root(300, &[400], "Frame", PROCESS)
        ));
    }
}
