//! Stdin command parsing and the reader thread.
//!
//! Commands are one per line: `focus <id>`, `set-focus <id>`,
//! `set-name <id> <text>`, `set-value <id> <text>`, `select <id>`,
//! `caret <id> <start> [<end>]`, `set-text <id> <text>`, `notify <text>`,
//! `stall <ms>`, and `quit`.
//! Parsing runs on a dedicated thread (reading stdin blocks, and the window
//! thread must keep pumping its message loop); parsed commands are handed
//! to the window thread over a channel, woken by a lightweight posted
//! message.

use std::io::BufRead;
use std::sync::mpsc::Sender;

/// One parsed stdin command.
pub(crate) enum Command {
    /// `focus <id>`.
    Focus(String),
    /// `set-focus <id>`: moves the focused state as `focus` does, raising no
    /// event, so no client anywhere reacts to it: for the tests that count
    /// an operation's provider calls exactly, which then hand the focus to
    /// the outpost themselves.
    SetFocus(String),
    /// `set-name <id> <text>`. `text` is empty when the command clears the
    /// name (`set-name <id>` with nothing after the id).
    SetName(String, String),
    /// `set-value <id> <text>`, same empty-text convention as `SetName`.
    SetValue(String, String),
    /// `select <id>`: marks the node selected (moving the state off any
    /// previously selected node) and raises the backend-appropriate
    /// selection notification — `SelectionItem_ElementSelected` for UIA,
    /// `EVENT_OBJECT_SELECTION` for MSAA.
    Select(String),
    /// `caret <id> <start> [<end>]`: selects a text node's text from `start`
    /// to `end` (UTF-16 offsets; the caret alone at `start` when `end` is
    /// left out), raising no event, as an application's caret moves before
    /// the client asks where it is.
    Caret(String, usize, usize),
    /// `set-text <id> <text>`: replaces a UIA text node's text, raising no
    /// event, as a terminal's text changes before a client reads it; `\n`
    /// in `text` is a line feed and `\\` a backslash, so one stdin line can
    /// carry many lines. UIA-only.
    SetText(String, String),
    /// `notify <text>`: raises a UIA `AutomationNotification` carrying
    /// `text` as its display string, from the root provider. UIA-only; the
    /// MSAA backend reports it as unsupported, since MSAA has no
    /// notification event.
    Notify(String),
    /// `stall <ms>`: blocks the window thread for that many milliseconds, so
    /// every cross-process call into the window waits, as with an
    /// application that is starting up or busy. The window thread prints
    /// `stall started` on stdout as it begins, and `stall ended <us>`, with
    /// the time in microseconds since the Unix epoch, as it ends.
    Stall(u64),
    /// `quit`.
    Quit,
}

/// Parses one stdin line into a [`Command`]. Blank lines and unrecognized
/// verbs yield `None` (a stray blank line is silently ignored; an unknown
/// verb is reported to stderr by the caller).
pub(crate) fn parse_command(line: &str) -> Option<Command> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let (verb, rest) = line.split_once(' ').unwrap_or((line, ""));
    let rest = rest.trim();
    match verb {
        "quit" => Some(Command::Quit),
        "focus" if !rest.is_empty() => Some(Command::Focus(rest.to_owned())),
        "set-focus" if !rest.is_empty() => Some(Command::SetFocus(rest.to_owned())),
        "select" if !rest.is_empty() => Some(Command::Select(rest.to_owned())),
        "notify" if !rest.is_empty() => Some(Command::Notify(rest.to_owned())),
        "stall" => rest.parse().ok().map(Command::Stall),
        "caret" => {
            let mut parts = rest.split_whitespace();
            let id = parts.next()?;
            let start: usize = parts.next()?.parse().ok()?;
            let end = match parts.next() {
                Some(end) => end.parse().ok()?,
                None => start,
            };
            Some(Command::Caret(id.to_owned(), start, end))
        }
        "set-name" => {
            let (id, text) = rest.split_once(' ').unwrap_or((rest, ""));
            (!id.is_empty()).then(|| Command::SetName(id.to_owned(), text.trim().to_owned()))
        }
        "set-text" => {
            let (id, text) = rest.split_once(' ').unwrap_or((rest, ""));
            (!id.is_empty()).then(|| Command::SetText(id.to_owned(), unescape(text)))
        }
        "set-value" => {
            let (id, text) = rest.split_once(' ').unwrap_or((rest, ""));
            (!id.is_empty()).then(|| Command::SetValue(id.to_owned(), text.trim().to_owned()))
        }
        _ => None,
    }
}

/// `text` with `\n` turned into a line feed and `\\` into a backslash.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// Reads stdin line by line, sending each parsed command to `sink` and
/// calling `wake` so the window thread's message loop notices. Returns when
/// stdin closes or a `quit` command was sent.
pub(crate) fn run(sink: &Sender<Command>, wake: impl Fn()) {
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        match parse_command(&line) {
            Some(Command::Quit) => {
                let _ = sink.send(Command::Quit);
                wake();
                break;
            }
            Some(command) => {
                if sink.send(command).is_err() {
                    break;
                }
                wake();
            }
            None => {
                if !line.trim().is_empty() {
                    eprintln!("mockapp: unrecognized command: {line}");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_focus() {
        match parse_command("focus btn1") {
            Some(Command::Focus(id)) => assert_eq!(id, "btn1"),
            _ => panic!("expected Focus"),
        }
        match parse_command("set-focus btn1") {
            Some(Command::SetFocus(id)) => assert_eq!(id, "btn1"),
            _ => panic!("expected SetFocus"),
        }
    }

    #[test]
    fn parses_set_text_with_line_feeds() {
        match parse_command(r"set-text term a  \nb\\c") {
            Some(Command::SetText(id, text)) => {
                assert_eq!(id, "term");
                assert_eq!(text, "a  \nb\\c");
            }
            _ => panic!("expected SetText"),
        }
    }

    #[test]
    fn parses_set_name_and_set_value() {
        match parse_command("set-name btn1 New Label") {
            Some(Command::SetName(id, text)) => {
                assert_eq!(id, "btn1");
                assert_eq!(text, "New Label");
            }
            _ => panic!("expected SetName"),
        }
        match parse_command("set-value slider1 42") {
            Some(Command::SetValue(id, text)) => {
                assert_eq!(id, "slider1");
                assert_eq!(text, "42");
            }
            _ => panic!("expected SetValue"),
        }
    }

    #[test]
    fn set_name_with_no_text_clears_it() {
        match parse_command("set-name btn1") {
            Some(Command::SetName(id, text)) => {
                assert_eq!(id, "btn1");
                assert_eq!(text, "");
            }
            _ => panic!("expected SetName"),
        }
    }

    #[test]
    fn parses_caret() {
        match parse_command("caret doc 3") {
            Some(Command::Caret(id, start, end)) => {
                assert_eq!((id.as_str(), start, end), ("doc", 3, 3));
            }
            _ => panic!("expected Caret"),
        }
        match parse_command("caret doc 3 7") {
            Some(Command::Caret(_, start, end)) => assert_eq!((start, end), (3, 7)),
            _ => panic!("expected Caret"),
        }
        assert!(parse_command("caret doc").is_none());
    }

    #[test]
    fn parses_quit() {
        assert!(matches!(parse_command("quit"), Some(Command::Quit)));
    }

    #[test]
    fn parses_select_and_notify() {
        match parse_command("select item1") {
            Some(Command::Select(id)) => assert_eq!(id, "item1"),
            _ => panic!("expected Select"),
        }
        match parse_command("notify Window snapped to the left") {
            Some(Command::Notify(text)) => assert_eq!(text, "Window snapped to the left"),
            _ => panic!("expected Notify"),
        }
    }

    #[test]
    fn blank_and_unknown_lines_are_none() {
        assert!(parse_command("").is_none());
        assert!(parse_command("   ").is_none());
        assert!(parse_command("frobnicate").is_none());
        assert!(parse_command("focus").is_none());
        assert!(parse_command("select").is_none());
        assert!(parse_command("notify").is_none());
    }
}
