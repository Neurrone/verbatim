//! Protected (password) fields, as
//! `text_box_password` in a Windows Forms password text box, a standard
//! Win32 edit control read through MSAA and its window messages, and as
//! `wpf_password` in a WPF `PasswordBox`, read through UI Automation (each
//! its own code, as `books/dev/src/reference/testing.md` requires): a protected field's text
//! is read as any field's, masked as the control gives it, on focus, by
//! character, and for Backspace, Delete, Control+Backspace and
//! Control+Delete, while its typed characters stay echoed as
//! the protected character (`docs/parity/focus-announcement.md`, "Focus
//! announcement content and property order", the text on focus).
//!
//! Every expectation comes from a capture of NVDA pressing the same keys
//! in the same form: `winforms-password-focus.txt` and
//! `wpf-uia-password-focus.txt` in
//! `target/e2e-evidence/m42-selection-keys-2026-10-22/`, and, from the
//! second Tab on, `winforms-password-deleting-keys.txt` and
//! `wpf-password-deleting-keys.txt` in
//! `target/e2e-evidence/m42-protected-fixes-2026-10-22/`. The differences
//! are declared where they occur: Verbatim has no symbol processing yet
//! (M8.2), so it says a run of masked characters as the characters
//! themselves, where NVDA collapses six stars to "6 star" and drops a run
//! of circles, whose symbol level is above its default; and it has no
//! delayed character description, the empty utterance NVDA says after a
//! character it moves to. Control+Backspace says the whole word around the
//! character before the caret, as the field's provider defines the word,
//! as NVDA's Backspace scripts do: a run of masked characters is one word
//! in both fields.
//!
//! Every key is a real key press the keyboard hook sees and passes to the
//! form; each step waits for its speech to be heard in full before the
//! next, and there is no other wait but the WPF window's, before it comes
//! forward, for its thread to answer UI Automation.

use std::io;

use crate::registry::ScenarioState;
use crate::scenario::{Scenario, harness_marker};

pub(crate) use super::no_teardown as teardown;

/// The Windows Forms form's harness name.
const FORMS_NAME: &str = "password";

/// The WPF window's harness name.
const WPF_NAME: &str = "wpf-password";

/// The password both forms hold, six characters, which neither scenario
/// ever hears: each field shows it masked.
const PASSWORD: &str = "secret";

/// Presses `keys` and asserts exactly `heard`.
fn press(scenario: &mut Scenario, keys: &str, heard: &[&str]) {
    scenario.send_keys(&[keys]).expect("sends the key");
    scenario.speech().expect(heard);
}

/// Presses `keys`, declared a deliberate difference from NVDA's symbol
/// processing, which Verbatim does not have yet, and asserts exactly
/// `heard`.
fn press_without_symbols(scenario: &mut Scenario, keys: &str, heard: &[&str]) {
    scenario.send_keys(&[keys]).expect("sends the key");
    scenario.deviates_from_nvda(
        "docs/parity/speech-audio.md",
        "Symbol/punctuation processing, speech dictionaries, character descriptions",
    );
    scenario.speech().expect(heard);
}

/// Presses `keys`, declared a deliberate difference from NVDA's delayed
/// character description, which Verbatim does not have yet, and asserts
/// exactly `heard`.
fn press_without_description(scenario: &mut Scenario, keys: &str, heard: &[&str]) {
    scenario.send_keys(&[keys]).expect("sends the key");
    scenario.deviates_from_nvda("docs/parity/text.md", "TextInfo-equivalent layer");
    scenario.speech().expect(heard);
}

/// The Windows Forms script: a "User name" text box holding "dickson" and
/// a "Password" text box with the system's password character holding
/// [`PASSWORD`], in a form titled `title`, which writes the file `ready`
/// from its Shown event, once Windows Forms has finished showing it and
/// put the focus on the first box.
fn forms_script(title: &str, ready: &str) -> String {
    let title = title.replace('\'', "''");
    let ready = ready.replace('\'', "''");
    format!(
        "Add-Type -AssemblyName System.Windows.Forms\r\n\
         Add-Type -AssemblyName System.Drawing\r\n\
         [System.Windows.Forms.Application]::EnableVisualStyles()\r\n\
         $form = New-Object System.Windows.Forms.Form\r\n\
         $form.Text = '{title}'\r\n\
         $form.ClientSize = New-Object System.Drawing.Size(600, 200)\r\n\
         $form.StartPosition = 'CenterScreen'\r\n\
         $user = New-Object System.Windows.Forms.TextBox\r\n\
         $user.AccessibleName = 'User name'\r\n\
         $user.Text = 'dickson'\r\n\
         $user.Location = New-Object System.Drawing.Point(20, 20)\r\n\
         $user.Width = 400\r\n\
         $pass = New-Object System.Windows.Forms.TextBox\r\n\
         $pass.AccessibleName = 'Password'\r\n\
         $pass.UseSystemPasswordChar = $true\r\n\
         $pass.Text = '{PASSWORD}'\r\n\
         $pass.Location = New-Object System.Drawing.Point(20, 80)\r\n\
         $pass.Width = 400\r\n\
         $form.Controls.Add($user)\r\n\
         $form.Controls.Add($pass)\r\n\
         $form.Add_Shown({{ [IO.File]::WriteAllText('{ready}', '') }})\r\n\
         [void]$form.ShowDialog()\r\n"
    )
}

/// The WPF script: a "User name" text box holding "dickson" and a
/// "Password" `PasswordBox` holding [`PASSWORD`], in a window titled
/// `title`, which puts the focus on the text box and writes the file
/// `ready` once its content has been rendered.
fn wpf_script(title: &str, ready: &str) -> String {
    let title = title.replace('\'', "''");
    let ready = ready.replace('\'', "''");
    format!(
        "Add-Type -AssemblyName PresentationFramework\r\n\
         $window = New-Object System.Windows.Window\r\n\
         $window.Title = '{title}'\r\n\
         $window.Width = 600\r\n\
         $window.Height = 250\r\n\
         $window.WindowStartupLocation = 'CenterScreen'\r\n\
         $panel = New-Object System.Windows.Controls.StackPanel\r\n\
         $user = New-Object System.Windows.Controls.TextBox\r\n\
         [System.Windows.Automation.AutomationProperties]::SetName($user, 'User name')\r\n\
         $user.Text = 'dickson'\r\n\
         $user.Margin = '20'\r\n\
         $pass = New-Object System.Windows.Controls.PasswordBox\r\n\
         [System.Windows.Automation.AutomationProperties]::SetName($pass, 'Password')\r\n\
         $pass.Password = '{PASSWORD}'\r\n\
         $pass.Margin = '20'\r\n\
         [void]$panel.Children.Add($user)\r\n\
         [void]$panel.Children.Add($pass)\r\n\
         $window.Content = $panel\r\n\
         $window.Add_ContentRendered({{ [void]$user.Focus(); [IO.File]::WriteAllText('{ready}', '') }})\r\n\
         [void]$window.ShowDialog()\r\n"
    )
}

/// A form's script written into the run folder named for `name`: the
/// window's title, [`harness_marker`] of `name`; the arguments that run
/// the script in Windows PowerShell, with `sta` for WPF; and the file the
/// script writes once the window is shown.
struct Launch {
    title: String,
    args: Vec<String>,
    ready: String,
}

fn write_script(
    scenario: &mut Scenario,
    name: &str,
    script: fn(&str, &str) -> String,
    sta: bool,
) -> io::Result<Launch> {
    let title = harness_marker(name);
    let directory = scenario.harness_folder(name);
    let script_path = format!(r"{directory}\form.ps1");
    let ready = format!(r"{directory}\shown");
    scenario.write_agent_file(&script_path, script(&title, &ready).as_bytes())?;
    let mut args = vec!["-NoProfile"];
    if sta {
        args.push("-STA");
    }
    args.extend([
        "-ExecutionPolicy",
        "Bypass",
        "-WindowStyle",
        "Hidden",
        "-File",
        &script_path,
    ]);
    let args: Vec<String> = args.into_iter().map(str::to_owned).collect();
    Ok(Launch { title, args, ready })
}

/// Opens the Windows Forms form, brought forward once the script says it
/// is shown. The window closes at cleanup, and Windows PowerShell, which
/// owns it, exits then.
pub(crate) fn text_box_setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    let launch = write_script(scenario, FORMS_NAME, forms_script, false)?;
    scenario.launch_titled_once(
        "powershell.exe",
        &launch.args,
        &launch.title,
        true,
        Some(&launch.ready),
    )?;
    Ok(ScenarioState::None)
}

/// Opens the WPF window, brought forward once the script says it is shown
/// and the window has answered UI Automation: WPF's thread can stay busy
/// for seconds after the window has rendered, and a window that does not
/// answer UI Automation in time is read through MSAA, as NVDA reads it
/// then. The window closes at cleanup, and Windows PowerShell, which owns
/// it, exits then.
pub(crate) fn wpf_setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    let launch = write_script(scenario, WPF_NAME, wpf_script, true)?;
    scenario.launch_answering_uia("powershell.exe", &launch.args, &launch.title, &launch.ready)?;
    Ok(ScenarioState::None)
}

/// The scenario in the Windows Forms password text box, whose text the
/// control masks with stars and which selects all of its text when Tab
/// first reaches it.
pub(crate) fn text_box_body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    let title = harness_marker(FORMS_NAME);
    scenario
        .speech()
        .expect(&[&title, "User name edit", "selected dickson"]);

    // Tab into the password: NVDA says "selected 6 star".
    press_without_symbols(
        scenario,
        "tab",
        &["Password edit protected", "selected ******"],
    );
    // End: nothing at the caret, then the six stars unselected, which
    // NVDA says "6 star unselected".
    press_without_symbols(scenario, "end", &["blank", "****** unselected"]);
    // Left Arrow: the star, with no delayed description after it.
    press_without_description(scenario, "leftarrow", &["star"]);
    // Backspace: the star it deleted.
    press(scenario, "backspace", &["star"]);
    press(
        scenario,
        "shift+tab",
        &["User name edit", "selected dickson"],
    );
    // Tab back: the caret's line, five stars now, with nothing selected,
    // since the box selects all of its text only when Tab first reaches
    // it; NVDA says "5 star".
    press_without_symbols(scenario, "tab", &["Password edit protected", "*****"]);
    // Home, then Delete: the star now at the caret, each with no delayed
    // description after it (`winforms-password-deleting-keys.txt`).
    press_without_description(scenario, "home", &["star"]);
    press_without_description(scenario, "delete", &["star"]);
    press(scenario, "end", &["blank"]);
    press_without_description(scenario, "leftarrow", &["star"]);
    press_without_description(scenario, "leftarrow", &["star"]);
    // Control+Backspace: the word around the star before the caret, the
    // four stars, which Uniscribe makes one word of in a plain edit
    // control, said once the caret moves; NVDA says "4 star"
    // (`winforms-password-deleting-keys.txt`), though the edit control
    // deletes nothing and inserts a DEL character, masked as one more
    // star.
    press_without_symbols(scenario, "control+backspace", &["****"]);
    press_without_description(scenario, "home", &["star"]);
    // Control+Delete: the box deletes all of its text, leaving nothing at
    // the caret.
    press(scenario, "control+delete", &["blank"]);
    // A typed character: the protected character
    // (`m42-text-captures-2026-10-23/20a-forms-password-type-a-run1.txt`).
    type_character(scenario);
}

/// Types "a" into the password field, which echoes only the protected
/// character, "star", as NVDA's captures of both fields show
/// (`target/e2e-evidence/m42-text-captures-2026-10-23/`, items 20a and
/// 20b).
fn type_character(scenario: &mut Scenario) {
    scenario.type_text("a").expect("types a");
    scenario.speech().expect(&["star"]);
}

/// The scenario in the WPF `PasswordBox`, whose text pattern gives its
/// text masked with circles (U+25CF) and which keeps its caret where it
/// was, selecting nothing, when Tab reaches it.
pub(crate) fn wpf_body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    let title = harness_marker(WPF_NAME);
    scenario
        .speech()
        .expect(&[&format!("{title} dialog"), "User name edit", "dickson"]);

    // Tab into the password: NVDA's symbol level drops the circles, so it
    // says no text.
    press_without_symbols(
        scenario,
        "tab",
        &[
            "Password edit protected",
            "\u{25cf}\u{25cf}\u{25cf}\u{25cf}\u{25cf}\u{25cf}",
        ],
    );
    press(scenario, "end", &["blank"]);
    // Left Arrow: the circle, with no delayed description after it.
    press_without_description(scenario, "leftarrow", &["circle"]);
    // Backspace: the circle it deleted.
    press(scenario, "backspace", &["circle"]);
    press(scenario, "shift+tab", &["User name edit", "dickson"]);
    // Tab back: five circles now, which NVDA drops.
    press_without_symbols(
        scenario,
        "tab",
        &[
            "Password edit protected",
            "\u{25cf}\u{25cf}\u{25cf}\u{25cf}\u{25cf}",
        ],
    );
    // Home, then Delete: the circle now at the caret, each with no delayed
    // description after it (`wpf-password-deleting-keys.txt`).
    press_without_description(scenario, "home", &["circle"]);
    press_without_description(scenario, "delete", &["circle"]);
    press(scenario, "end", &["blank"]);
    press_without_description(scenario, "leftarrow", &["circle"]);
    press_without_description(scenario, "leftarrow", &["circle"]);
    // Control+Backspace: the word around the circle before the caret, which
    // NVDA's symbol level drops, saying an utterance with no words
    // (`wpf-password-deleting-keys.txt`). The PasswordBox's text provider
    // expands any position in a run of circles to the whole run as its
    // word (its `ExpandToEnclosingUnit(Word)`, queried on a PasswordBox
    // holding four circles, 2026-11-18), so the four circles.
    press_without_symbols(
        scenario,
        "control+backspace",
        &["\u{25cf}\u{25cf}\u{25cf}\u{25cf}"],
    );
    // Control+Delete at the text's start: the two circles left go, leaving
    // nothing at the caret (`wpf-password-deleting-keys-without-home.txt`).
    press(scenario, "control+delete", &["blank"]);
    // A typed character: the protected character
    // (`m42-text-captures-2026-10-23/20b-wpf-password-type-a-run1.txt`).
    type_character(scenario);
}
