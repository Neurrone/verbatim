//! `cargo xtask park`: moves this Remote Desktop session onto the machine's
//! console, so it stays signed in, unlocked, and able to take injected input
//! with no RDP client connected, and the end-to-end suite can run on a
//! development VM nobody is looking at.
//!
//! The move is `tscon <session> /dest:console`, which needs administrator
//! rights. `vm/scripts/Register-VerbatimParkTask.ps1`, run once elevated,
//! registers a task that runs it as SYSTEM and that this user may start;
//! this verb starts the task and then checks the outcome: the session is on
//! the console, no lock screen is showing, and a real, uncloaked window
//! holds the foreground. Parking disconnects any connected RDP client;
//! reconnecting takes the session back.

use std::process::{Command, ExitCode};
use std::thread;
use std::time::{Duration, Instant};

/// Must match `$taskName` in `vm/scripts/Register-VerbatimParkTask.ps1`.
const TASK_NAME: &str = "Verbatim park session";

/// How long the session is given to arrive on the console.
const MOVE_TIMEOUT: Duration = Duration::from_secs(15);

/// How long the console desktop is given to show an unlocked foreground.
const DESKTOP_TIMEOUT: Duration = Duration::from_secs(10);

const POLL: Duration = Duration::from_millis(250);

/// Prints one line describing the desktop of the session it runs in:
/// `locked` while the lock screen's process runs in this session, otherwise
/// the foreground window's handle, cloaked state (`DWMWA_CLOAKED`, 14), and
/// title.
const DESKTOP_PROBE: &str = r#"
Add-Type @'
using System; using System.Runtime.InteropServices; using System.Text;
public static class ParkProbe {
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int a, out int v, int s);
}
'@
$session = (Get-Process -Id $PID).SessionId
if (Get-Process LogonUI -ErrorAction SilentlyContinue | Where-Object SessionId -eq $session) {
    'locked'
    exit
}
$window = [ParkProbe]::GetForegroundWindow()
$title = New-Object Text.StringBuilder 256
[void][ParkProbe]::GetWindowText($window, $title, 256)
$cloaked = 0
[void][ParkProbe]::DwmGetWindowAttribute($window, 14, [ref]$cloaked, 4)
"foreground $window cloaked $cloaked title $title"
"#;

/// Entry point for `cargo xtask park`.
pub(crate) fn run() -> ExitCode {
    match park() {
        Ok(desktop) => {
            println!("xtask park: on the console, {desktop}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("xtask park: {message}");
            ExitCode::FAILURE
        }
    }
}

fn park() -> Result<String, String> {
    if current_session_name()? == "console" {
        println!("xtask park: this session is already on the console");
    } else {
        start_task()?;
        let deadline = Instant::now() + MOVE_TIMEOUT;
        loop {
            let name = current_session_name()?;
            if name == "console" {
                break;
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "the session did not move to the console within {} seconds; it is still {name}",
                    MOVE_TIMEOUT.as_secs()
                ));
            }
            thread::sleep(POLL);
        }
    }
    wait_for_desktop()
}

/// Starts the registered task, explaining how to register it when that
/// fails.
fn start_task() -> Result<(), String> {
    let output = Command::new("schtasks")
        .args(["/Run", "/TN", TASK_NAME])
        .output()
        .map_err(|error| format!("could not run schtasks: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "could not start the task '{TASK_NAME}': {}\n\
         Register it once from an elevated PowerShell with \
         vm\\scripts\\Register-VerbatimParkTask.ps1",
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

/// The name of the session this process runs in, from `query session`,
/// which marks the caller's own row with `>`: `console`, or an RDP
/// connection such as `rdp-tcp#0`.
fn current_session_name() -> Result<String, String> {
    let output = Command::new("query")
        .arg("session")
        .output()
        .map_err(|error| format!("could not run query session: {error}"))?;
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .find_map(|line| line.strip_prefix('>'))
        .and_then(|row| row.split_whitespace().next())
        .map(str::to_owned)
        .ok_or_else(|| format!("query session did not mark this session:\n{text}"))
}

/// Waits for the console desktop to be unlocked with a real, uncloaked
/// foreground window, returning the probe's description of it.
fn wait_for_desktop() -> Result<String, String> {
    let deadline = Instant::now() + DESKTOP_TIMEOUT;
    loop {
        let desktop = probe_desktop()?;
        let usable = desktop.starts_with("foreground ")
            && !desktop.starts_with("foreground 0 ")
            && desktop.contains(" cloaked 0 ");
        if usable {
            return Ok(desktop);
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "the session is on the console, but its desktop is not usable for a run: {desktop}"
            ));
        }
        thread::sleep(POLL);
    }
}

fn probe_desktop() -> Result<String, String> {
    let output = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", DESKTOP_PROBE])
        .output()
        .map_err(|error| format!("could not run the desktop probe: {error}"))?;
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| {
            format!(
                "the desktop probe printed nothing: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )
        })
}
