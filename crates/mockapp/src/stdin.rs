//! Stdin command parsing and the reader thread.
//!
//! Commands are one per line: `focus <id>`, `set-name <id> <text>`,
//! `set-value <id> <text>`, and `quit`. Parsing runs on a dedicated thread
//! (reading stdin blocks, and the window thread must keep pumping its
//! message loop); parsed commands are handed to the window thread over a
//! channel, woken by a lightweight posted message.

use std::io::BufRead;
use std::sync::mpsc::Sender;

/// One parsed stdin command.
pub(crate) enum Command {
    /// `focus <id>`.
    Focus(String),
    /// `set-name <id> <text>`. `text` is empty when the command clears the
    /// name (`set-name <id>` with nothing after the id).
    SetName(String, String),
    /// `set-value <id> <text>`, same empty-text convention as `SetName`.
    SetValue(String, String),
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
        "set-name" => {
            let (id, text) = rest.split_once(' ').unwrap_or((rest, ""));
            (!id.is_empty()).then(|| Command::SetName(id.to_owned(), text.trim().to_owned()))
        }
        "set-value" => {
            let (id, text) = rest.split_once(' ').unwrap_or((rest, ""));
            (!id.is_empty()).then(|| Command::SetValue(id.to_owned(), text.trim().to_owned()))
        }
        _ => None,
    }
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
    fn parses_quit() {
        assert!(matches!(parse_command("quit"), Some(Command::Quit)));
    }

    #[test]
    fn blank_and_unknown_lines_are_none() {
        assert!(parse_command("").is_none());
        assert!(parse_command("   ").is_none());
        assert!(parse_command("frobnicate").is_none());
        assert!(parse_command("focus").is_none());
    }
}
