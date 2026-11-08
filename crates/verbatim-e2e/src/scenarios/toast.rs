//! A Windows toast raised from PowerShell by the toast notification APIs is
//! spoken, as NVDA reads it from the window-opened event of its
//! `ToastView` window (`docs/parity/alerts.md`, "Toasts"). Do Not Disturb
//! would swallow it, so the body turns Do Not Disturb off for the run and
//! teardown restores what setup found (roadmap, M4.2, "Toasts").
//!
//! The expected speech is the design's (`docs/design/m42-parity.md`,
//! "Toasts"), with this scenario's own title and body, to be confirmed
//! against NVDA in compare mode.

use std::io;
use std::time::Duration;

use crate::registry::ScenarioState;
use crate::scenario::Scenario;
use crate::speech::heard;

/// How long the found Do Not Disturb state's file waits to be written.
const FOUND_TIMEOUT: Duration = Duration::from_secs(10);

const TITLE: &str = "Verbatim toast title";
const BODY: &str = "Verbatim toast body";

/// The speech of a toast: the shell's name for it, with the toast's own
/// title and body, its position among the toasts on screen and its role.
fn toast_speech() -> String {
    format!("New notification from Windows PowerShell, {TITLE}, {BODY}.. 1 of 1 window")
}

/// Reads and sets Windows' Do Not Disturb state, the WNF state
/// `WNF_SHEL_QUIETHOURS_ACTIVE_PROFILE_CHANGED`, whose value is 0 for off:
/// `get <file>` writes the state found into `file`; `set <value>` sets it.
const DO_NOT_DISTURB: &str = r#"
param([string]$Verb, [string]$Value)
$ErrorActionPreference = 'Stop'
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class Wnf {
    [DllImport("ntdll.dll")] public static extern int NtQueryWnfStateData(ref ulong StateName, IntPtr TypeId, IntPtr Scope, out uint Stamp, out uint Buffer, ref uint Size);
    [DllImport("ntdll.dll")] public static extern int NtUpdateWnfStateData(ref ulong StateName, ref uint Buffer, uint Size, IntPtr TypeId, IntPtr Scope, uint MatchingStamp, uint CheckStamp);
}
"@
$name = [uint64]'0x0D83063EA3BF1C75'
if ($Verb -eq 'get') {
    $stamp = 0; $state = 0; $size = 4
    if ([Wnf]::NtQueryWnfStateData([ref]$name, [IntPtr]::Zero, [IntPtr]::Zero, [ref]$stamp, [ref]$state, [ref]$size) -ne 0) { throw 'the state could not be read' }
    Set-Content -LiteralPath $Value -Value $state -NoNewline
} else {
    $state = [uint32]$Value
    if ([Wnf]::NtUpdateWnfStateData([ref]$name, [ref]$state, 4, [IntPtr]::Zero, [IntPtr]::Zero, 0, 0) -ne 0) { throw 'the state could not be set' }
}
"#;

/// Raises a toast from PowerShell's own application id.
const RAISE_TOAST: &str = r"
param([string]$Title, [string]$Body)
$ErrorActionPreference = 'Stop'
[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null
$xml = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02)
$texts = $xml.GetElementsByTagName('text')
[void]$texts.Item(0).AppendChild($xml.CreateTextNode($Title))
[void]$texts.Item(1).AppendChild($xml.CreateTextNode($Body))
$toast = [Windows.UI.Notifications.ToastNotification]::new($xml)
$appId = '{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe'
[Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier($appId).Show($toast)
";

fn run(scenario: &mut Scenario, script: &str, args: &[&str]) -> io::Result<()> {
    let mut command: Vec<String> = ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    command.push(script.to_owned());
    command.extend(args.iter().map(|arg| (*arg).to_owned()));
    scenario.run_to_success("powershell.exe", &command)
}

/// Records the Do Not Disturb state it finds, for teardown to restore, which
/// a failure anywhere after this still runs.
pub(crate) fn setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    let script = scenario.harness_file("do-not-disturb", "ps1");
    scenario.write_agent_file(&script, DO_NOT_DISTURB.as_bytes())?;
    let found = scenario.harness_file("do-not-disturb-found", "txt");
    run(scenario, &script, &["get", &found])?;
    let state = String::from_utf8_lossy(&scenario.wait_for_agent_file(&found, FOUND_TIMEOUT)?)
        .trim()
        .to_owned();
    Ok(ScenarioState::Names(vec![script, state]))
}

pub(crate) fn body(scenario: &mut Scenario, state: &mut ScenarioState) {
    let ScenarioState::Names(names) = state else {
        unreachable!("setup records its script and the state it found");
    };
    let do_not_disturb = names.first().expect("setup records its script").clone();
    run(scenario, &do_not_disturb, &["set", "0"]).expect("turns Do Not Disturb off");
    let script = scenario.harness_file("raise-toast", "ps1");
    scenario
        .write_agent_file(&script, RAISE_TOAST.as_bytes())
        .expect("writes the script that raises the toast");
    scenario.begin_action("raise a toast");
    run(scenario, &script, &[TITLE, BODY]).expect("raises the toast");
    scenario.speech().expect_sequence(&[heard(&toast_speech())]);
}

/// Restores the Do Not Disturb state setup found.
pub(crate) fn teardown(scenario: &mut Scenario, state: ScenarioState) {
    let ScenarioState::Names(names) = state else {
        unreachable!("setup records its script and the state it found");
    };
    let [script, found] = names.as_slice() else {
        unreachable!("setup records its script and the state it found");
    };
    run(scenario, script, &["set", found]).expect("restores Do Not Disturb");
}
