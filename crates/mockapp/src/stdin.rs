//! Stdin command parsing and the reader thread.
//!
//! Commands are one per line: `focus <id>`, `set-focus <id>`,
//! `set-name <id> <text>`, `set-value <id> <text>`,
//! `set-description <id> <text>`, `set-states <id> <state>...`, `select <id>`,
//! `caret <id> <start> [<end>]`, `caret-event <id>`, `set-text <id> <text>`,
//! `notify <text>`,
//! `active-text-position <id> <start> <end>`, `take-runtime-id <id> <from>`,
//! `stall <ms>`, `slow <ms>`, and `quit`.
//! Parsing runs on a dedicated thread (reading stdin blocks, and the window
//! thread must keep pumping its message loop); parsed commands are handed
//! to the window thread over a channel, woken by a lightweight posted
//! message. The window thread acknowledges each command on stdout once it
//! has taken effect: `applied`, or `rejected: <reason>`; `stall` with its
//! own two lines instead, and `quit` not at all.

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
    /// `set-description <id> <text>`, same empty-text convention as
    /// `SetName`; raises `EVENT_OBJECT_DESCRIPTIONCHANGE`. MSAA-only.
    SetDescription(String, String),
    /// `set-states <id> <state>...`: replaces the node's states with those
    /// named, in the fixture's snake case, and raises
    /// `EVENT_OBJECT_STATECHANGE`. MSAA-only.
    SetStates(String, verbatim_model::StateSet),
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
    /// `caret-event <id>`: raises the backend's caret event for a text
    /// node, as an application reports its caret after moving it: UIA's
    /// text selection changed event from the node, or, in the MSAA backend,
    /// `EVENT_OBJECT_TEXTSELECTIONCHANGED` from the edit control's client
    /// object, which a Common Controls version 6 edit control raises
    /// whenever its caret moves.
    CaretEvent(String),
    /// `set-text <id> <text>`: replaces a UIA text node's text, raising no
    /// event, as a terminal's text changes before a client reads it; `\n`
    /// in `text` is a line feed and `\\` a backslash, so one stdin line can
    /// carry many lines. UIA-only.
    SetText(String, String),
    /// `active-text-position <id> <start> <end>`: raises UIA's active text
    /// position changed event from a text node, with the range of its text
    /// from `start` to `end` (UTF-16 offsets), as an application raises it
    /// when it scrolls to a place in a document without moving the caret.
    /// UIA-only.
    ActiveTextPosition(String, usize, usize),
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
    /// `slow <ms>`: every provider call from now on is answered that many
    /// milliseconds late, as by an application busy building a window; 0
    /// answers at once again. Applied on the window thread, so the calls
    /// before it are answered at the old pace.
    Slow(u64),
    /// `take-runtime-id <id> <from>`: node `from` dies, leaving the tree
    /// (its parent no longer lists it, and every call on its elements fails
    /// as on an element that is gone, `UIA_E_ELEMENTNOTAVAILABLE`), and node
    /// `id` takes its runtime id, as File Explorer gives a new item the
    /// runtime id of one it destroyed. Raises no event. UIA-only.
    TakeRuntimeId(String, String),
    /// `focus-child <container> <child>`: addresses `container`'s children
    /// as numbered simple children from then on, moves the focused state to
    /// `child`, and raises `EVENT_OBJECT_FOCUS` on `container` itself and
    /// then on `child` by its child id, in the one turn of the window
    /// thread, as a Win32 tree view taking the focus does within its one
    /// `SetFocus` call. MSAA-only.
    FocusChild(String, String),
    /// `client-name <text>`: names the window's client area, the root
    /// node, `text`, leaving the window's text as it is, and raises
    /// `EVENT_OBJECT_NAMECHANGE` on the client area, as Windows 11 Notepad
    /// renames its window as it is first activated. MSAA-only.
    ClientName(String),
    /// `client-identity`: the window's client area, the root node, answers
    /// as the object Windows provides for a window's client area does from
    /// then on: `IAccIdentity` with its address, the window's
    /// `OBJID_CLIENT`, so a client finds it again at that address however
    /// it reached it, and `accParent` with the window's own window object.
    /// MSAA-only.
    ClientIdentity,
    /// `disable-client`: the window is disabled, as a modal dialog's owner
    /// is when the dialog opens: the root node gains the unavailable state,
    /// and `EVENT_OBJECT_STATECHANGE` is raised on the client area.
    /// MSAA-only.
    DisableClient,
    /// `quit`.
    Quit,
    /// A line that is no command, rejected on the window thread, so its
    /// acknowledgement keeps its place among the others'.
    Unrecognized(String),
}

/// Parses one stdin line into a [`Command`]. Blank lines and unrecognized
/// verbs yield `None` (a stray blank line is ignored; the caller hands an
/// unknown verb on as [`Command::Unrecognized`], to be rejected).
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
        "caret-event" if !rest.is_empty() => Some(Command::CaretEvent(rest.to_owned())),
        "stall" => rest.parse().ok().map(Command::Stall),
        "slow" => rest.parse().ok().map(Command::Slow),
        "take-runtime-id" => {
            let mut parts = rest.split_whitespace();
            let id = parts.next()?;
            let from = parts.next()?;
            parts
                .next()
                .is_none()
                .then(|| Command::TakeRuntimeId(id.to_owned(), from.to_owned()))
        }
        "active-text-position" => {
            let mut parts = rest.split_whitespace();
            let id = parts.next()?;
            let start: usize = parts.next()?.parse().ok()?;
            let end: usize = parts.next()?.parse().ok()?;
            Some(Command::ActiveTextPosition(id.to_owned(), start, end))
        }
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
        "client-name" => Some(Command::ClientName(rest.to_owned())),
        "client-identity" => Some(Command::ClientIdentity),
        "disable-client" => Some(Command::DisableClient),
        "focus-child" => {
            let (container, child) = rest.split_once(' ')?;
            Some(Command::FocusChild(
                container.to_owned(),
                child.trim().to_owned(),
            ))
        }
        "set-description" => {
            let (id, text) = rest.split_once(' ').unwrap_or((rest, ""));
            (!id.is_empty()).then(|| Command::SetDescription(id.to_owned(), text.trim().to_owned()))
        }
        "set-states" => {
            let mut parts = rest.split_whitespace();
            let id = parts.next()?;
            let states = parts
                .map(crate::fixture::state_from_fixture_str)
                .collect::<Option<verbatim_model::StateSet>>()?;
            Some(Command::SetStates(id.to_owned(), states))
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
                    if sink
                        .send(Command::Unrecognized(line.trim().to_owned()))
                        .is_err()
                    {
                        break;
                    }
                    wake();
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
    fn parses_active_text_position() {
        match parse_command("active-text-position doc 6 10") {
            Some(Command::ActiveTextPosition(id, start, end)) => {
                assert_eq!((id.as_str(), start, end), ("doc", 6, 10));
            }
            _ => panic!("expected ActiveTextPosition"),
        }
        assert!(parse_command("active-text-position doc 6").is_none());
    }

    #[test]
    fn parses_slow_and_take_runtime_id() {
        assert!(matches!(parse_command("slow 20"), Some(Command::Slow(20))));
        assert!(parse_command("slow soon").is_none());
        match parse_command("take-runtime-id inner delta") {
            Some(Command::TakeRuntimeId(id, from)) => {
                assert_eq!((id.as_str(), from.as_str()), ("inner", "delta"));
            }
            _ => panic!("expected TakeRuntimeId"),
        }
        assert!(parse_command("take-runtime-id inner").is_none());
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
