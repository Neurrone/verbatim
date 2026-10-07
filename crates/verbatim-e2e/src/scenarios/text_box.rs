//! The Windows Forms text box the harness creates: a window whose whole
//! client area is one multi-line text box, a standard Win32 edit control,
//! which Verbatim reads through MSAA and the control's window messages.
//! Windows PowerShell shows it, with a script the scenario writes, since
//! every Windows 11 and Windows Server has Windows PowerShell and Windows
//! Forms, so it behaves the same on every machine.
//!
//! The text box does not wrap, and is wide enough to show each paragraph
//! of the scenarios' texts on one line. Its caret starts at the start of
//! the text. The window is announced by its title, then the text box by
//! its name, role, and state, then the caret's line ([`announcement`]).

use std::io;

use crate::scenario::{Scenario, harness_marker};

/// The script showing the text box's window, titled `title`, the box named
/// `box_name`, its text read from `text`.
fn script(title: &str, box_name: &str, text: &str) -> String {
    let title = title.replace('\'', "''");
    let box_name = box_name.replace('\'', "''");
    let text = text.replace('\'', "''");
    format!(
        "Add-Type -AssemblyName System.Windows.Forms\r\n\
         Add-Type -AssemblyName System.Drawing\r\n\
         [System.Windows.Forms.Application]::EnableVisualStyles()\r\n\
         $form = New-Object System.Windows.Forms.Form\r\n\
         $form.Text = '{title}'\r\n\
         $form.ClientSize = New-Object System.Drawing.Size(1100, 400)\r\n\
         $form.StartPosition = 'CenterScreen'\r\n\
         $box = New-Object System.Windows.Forms.TextBox\r\n\
         $box.Multiline = $true\r\n\
         $box.ScrollBars = 'Both'\r\n\
         $box.WordWrap = $false\r\n\
         $box.Dock = 'Fill'\r\n\
         $box.Font = New-Object System.Drawing.Font('Segoe UI', 16)\r\n\
         $box.AccessibleName = '{box_name}'\r\n\
         $box.Text = [IO.File]::ReadAllText('{text}')\r\n\
         $box.Select(0, 0)\r\n\
         $form.Controls.Add($box)\r\n\
         [void]$form.ShowDialog()\r\n"
    )
}

/// Writes `text` and the script into the run folder named for `name`, opens
/// the text box's window, titled with [`harness_marker`] of `name`, its box
/// named `box_name`, and waits for it to take the foreground. The window
/// closes at cleanup, and Windows PowerShell, which owns it, exits then.
/// Returns the process id of Windows PowerShell, the application Verbatim
/// reads.
///
/// # Errors
///
/// Returns an error if a file cannot be written, Windows PowerShell cannot
/// be started, or its window does not take the foreground.
pub(crate) fn open(
    scenario: &mut Scenario,
    name: &str,
    box_name: &str,
    text: &str,
) -> io::Result<u32> {
    let title = harness_marker(name);
    let directory = scenario.harness_folder(name);
    let text_path = format!(r"{directory}\text.txt");
    let script_path = format!(r"{directory}\text-box.ps1");
    scenario.write_agent_file(&text_path, text.as_bytes())?;
    scenario.write_agent_file(
        &script_path,
        script(&title, box_name, &text_path).as_bytes(),
    )?;
    let args: Vec<String> = [
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-WindowStyle",
        "Hidden",
        "-File",
        &script_path,
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    Ok(scenario
        .launch_titled("powershell.exe", &args, &title, true)?
        .pid)
}

/// What Verbatim says as the window named for `name` takes the focus: its
/// title, the box named `box_name`, and the caret's line, `line`.
pub(crate) fn announcement(name: &str, box_name: &str, line: &str) -> Vec<String> {
    vec![
        harness_marker(name),
        format!("{box_name} edit multi line"),
        line.to_owned(),
    ]
}

/// Asserts [`announcement`].
pub(crate) fn expect_announced(scenario: &mut Scenario, name: &str, box_name: &str, line: &str) {
    let expected = announcement(name, box_name, line);
    let expected: Vec<&str> = expected.iter().map(String::as_str).collect();
    scenario.speech().expect(&expected);
}
