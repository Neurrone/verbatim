//! The Windows Forms text box the harness creates: a window whose whole
//! client area is one multi-line text box, a standard Win32 edit control,
//! which Verbatim reads through MSAA and the control's window messages.
//! `text_box_form.exe` shows it, a program beside Verbatim in the run's
//! directory that mockapp's build script compiles from
//! `crates/mockapp/text_box_form.cs` with the .NET Framework every Windows
//! 11 and Windows Server has, so it behaves the same on every machine.
//!
//! The text box does not wrap, and is wide enough to show each paragraph
//! of the scenarios' texts on one line. Its caret starts at the start of
//! the text. The window is announced by its title, then the text box by
//! its name, role, and state, then the caret's line ([`announcement`]).

use std::io;
use std::time::Instant;

use verbatim_control::protocol::Frame;
use verbatim_model::NormalizedEvent;

use crate::scenario::Launch;
use crate::scenario::{Scenario, WINDOW_TIMEOUT, harness_marker};

/// The text box's accessible name in the editing scenarios.
pub(crate) const BOX_NAME: &str = "Text";

/// Writes `text` into the run folder named for `name` and opens the text
/// box's window, titled with [`harness_marker`] of `name`, its box named
/// `box_name`. The program writes the file `ready` from the form's Shown
/// event, once Windows Forms has finished showing the window, so the window
/// is brought forward only then, and waits for it to take the foreground:
/// activated earlier, while the form was still being shown, it put no focus
/// on its text box. The window closes at cleanup, and the program exits
/// then. Returns the process id of the program, the application Verbatim
/// reads.
///
/// # Errors
///
/// Returns an error if a file cannot be written, the program cannot be
/// started, or its window does not take the foreground.
pub(crate) fn open(
    scenario: &mut Scenario,
    name: &str,
    box_name: &str,
    text: &str,
) -> io::Result<u32> {
    let title = harness_marker(name);
    let directory = scenario.harness_folder(name);
    let text_path = format!(r"{directory}\text.txt");
    let ready = format!(r"{directory}\shown");
    scenario.write_agent_file(&text_path, text.as_bytes())?;
    let program = format!(r"{}\text_box_form.exe", scenario.run_directory());
    let args = [title.clone(), ready.clone(), box_name.to_owned(), text_path];
    Ok(scenario
        .launch(
            &program,
            &args,
            Launch {
                ready: Some(&ready),
                ..Launch::titled(&title)
            },
        )?
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

/// Pastes the clipboard at the caret with Control+V, which says nothing,
/// and waits for the evidence that the text changed: the text box's change
/// of value as Core receives it. In compare mode NVDA offers no events, and
/// its quiet period, waited for before the next input, is the only
/// evidence there is.
pub(crate) fn paste(scenario: &mut Scenario) {
    if scenario.is_comparing() {
        scenario.send_keys(&["control+v"]).expect("sends control+v");
        return;
    }
    let mut events = scenario
        .subscribe_events()
        .expect("subscribes to Verbatim's events");
    scenario.send_keys(&["control+v"]).expect("sends control+v");
    let deadline = Instant::now() + WINDOW_TIMEOUT;
    loop {
        match events.next_frame() {
            Ok(Frame::Event {
                event: NormalizedEvent::TextChanged { .. },
                ..
            }) => return,
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Err(error) => panic!("the event subscription failed: {error}"),
        }
        assert!(
            Instant::now() < deadline,
            "the text box's text did not change within {WINDOW_TIMEOUT:?} of the paste"
        );
    }
}

/// Asserts [`announcement`].
pub(crate) fn expect_announced(scenario: &mut Scenario, name: &str, box_name: &str, line: &str) {
    let expected = announcement(name, box_name, line);
    let expected: Vec<&str> = expected.iter().map(String::as_str).collect();
    scenario.speech().expect(&expected);
}
