//! Stdin command parsing and the reader thread.
//!
//! Commands are one per line: `focus <id>`, `set-focus <id>`,
//! `set-name <id> <text>`, `set-value <id> <text>`,
//! `set-description <id> <text>`, `set-controller-for <id> [<controlled>]`,
//! `set-states <id> <state>...`, `select <id>`,
//! `caret <id> <start> [<end> [end]]`, `caret-event <id>`,
//! `console-caret <column> <row>`, `system-caret [show]`,
//! `set-text <id> <text>`, `write-on-read <id> <text>`, `text-changed <id>`,
//! `screen <id> <rows>`, `active-text-position <id> <start> <end>`,
//! `notify <text>`, `stall <ms> [<at> <command>]`, `on-activate <command>`,
//! `slow <ms>`, `slow-patterns <ms>`, `refuse-text <on|off>`,
//! `answer-property <property id> <fail|empty|usual>`,
//! `answer-parent <id> <fail|none|usual> [<after>]`,
//! `slow-parent <id> <ms> [<after>]`, `slow-acc-parent <id> <ms>`,
//! `fail-acc-parent <id>`, `move <id> <left> <top> <width> <height>`,
//! `fail-acc-location <id>`, `recreate <id>`, `report-window <id>`,
//! `take-runtime-id <id> <from>`, `replace-items <id>`, `disconnect <id>`,
//! `disconnect-at-first-child <id>`,
//! `focus-child <container> <child>`, `client-name <text>`,
//! `client-identity`, `disable-client`, `acc-focus-window <hwnd>`,
//! `child-window`, `child-client-identity`, `window-focus`, `hold`,
//! `release`, and `quit`.
//! Parsing runs on a dedicated thread (reading stdin blocks, and the window
//! thread must keep pumping its message loop); parsed commands are handed
//! to the window thread over a channel, woken by a lightweight posted
//! message. The window thread acknowledges each command on stdout once it
//! has taken effect: `applied`, or `rejected: <reason>`; `stall` with its
//! own two lines instead, and `quit` not at all. `release` is applied and
//! acknowledged by the reading thread itself, since the window thread is
//! the one a `hold` holds.

use std::io::BufRead;
use std::sync::mpsc::Sender;

/// How every UIA provider answers `GetPropertyValue` for the property
/// `answer-property` names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PropertyAnswer {
    /// The call fails with `E_FAIL`.
    Fail,
    /// The call answers `VT_EMPTY`.
    Empty,
    /// The call answers as usual.
    Usual,
}

/// How a node's UIA provider answers `Navigate(Parent)`, as
/// `answer-parent` sets it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ParentAnswer {
    /// The call fails with `E_FAIL`.
    Fail,
    /// The call answers no parent, a null element.
    Missing,
    /// The call answers the node's parent.
    Usual,
}

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
    /// `SetName`; raises `EVENT_OBJECT_DESCRIPTIONCHANGE`, or in the UIA
    /// backend a help text change, the description being served as both
    /// the full description and the help text.
    SetDescription(String, String),
    /// `set-controller-for <id> [<controlled>]`: makes the node's UIA
    /// `ControllerFor` relation name node `controlled`, or nothing when it
    /// is left out, and raises its property change. UIA-only.
    SetControllerFor(String, Option<String>),
    /// `set-states <id> <state>...`: replaces the node's states with those
    /// named, in the fixture's snake case, and raises
    /// `EVENT_OBJECT_STATECHANGE`, or in the UIA backend the change of the
    /// `ExpandCollapseState` property, the one state-bearing property
    /// change it raises.
    SetStates(String, verbatim_model::StateSet),
    /// `select <id>`: marks the node selected (moving the state off any
    /// previously selected node) and raises the backend-appropriate
    /// selection notification — `SelectionItem_ElementSelected` for UIA,
    /// `EVENT_OBJECT_SELECTION` for MSAA.
    Select(String),
    /// `caret <id> <start> [<end> [end]]`: selects a text node's text from
    /// `start` to `end` (UTF-16 offsets; the caret alone at `start` when
    /// `end` is left out), raising no event, as an application's caret
    /// moves before the client asks where it is. `TextPattern2`'s caret is
    /// the selection's start, or its end with the word `end` after the
    /// offsets, as Windows 11 Notepad's is after Shift+Right Arrow (UIA
    /// only).
    Caret(String, usize, usize, bool),
    /// `caret-event <id>`: raises the backend's caret event for a text
    /// node, as an application reports its caret after moving it: UIA's
    /// text selection changed event from the node, or, in the MSAA backend,
    /// `EVENT_OBJECT_TEXTSELECTIONCHANGED` from the edit control's client
    /// object, which a Common Controls version 6 edit control raises
    /// whenever its caret moves.
    CaretEvent(String),
    /// `console-caret <column> <row>`: raises `EVENT_CONSOLE_CARET` from the
    /// window, its cursor visible at that cell, as the console host raises
    /// it for its console window.
    ConsoleCaret(u16, u16),
    /// `system-caret` or `system-caret show`: raises
    /// `EVENT_OBJECT_LOCATIONCHANGE`, or `EVENT_OBJECT_SHOW`, on the system
    /// caret (`OBJID_CARET`) from the window that owns the caret (the
    /// window on the UIA backend, the edit control on the MSAA backend),
    /// as Windows raises it when an application moves its caret, or shows
    /// it again after a move, as a classic edit control does, and nothing
    /// else: an application whose text raises no caret event of its own.
    /// `true` for the show.
    SystemCaret(bool),
    /// `set-text <id> <text>`: replaces a text node's text, raising no
    /// event of mockapp's own, as a terminal's text changes before a client
    /// reads it; `\n` in `text` is a line feed, `\r` a carriage return,
    /// and `\\` a backslash, so one stdin line can carry many lines. In the MSAA backend, the edit
    /// control's text is replaced (`WM_SETTEXT`).
    SetText(String, String),
    /// `write-on-read <id> <text>`: the next time any range of a UIA text
    /// node's text is read, `text` is first written over the text at the
    /// caret and the caret moved past it, raising no event, as a terminal
    /// writes a typed character between two calls of a client's read. In
    /// the MSAA backend, the next time another thread reads the edit
    /// control's text or selection, `text` is first typed over the
    /// selection (`EM_REPLACESEL`) and the control's value change raised,
    /// as an application handles a typed character during a client's read
    /// of the caret event before it.
    WriteOnRead(String, String),
    /// `text-changed <id>`: raises UIA's text changed event from a text
    /// node, as a terminal reports that its text changed, or, in the MSAA
    /// backend, `EVENT_OBJECT_VALUECHANGE` from the edit control's client
    /// object, as an edit control reports that its text changed.
    TextChanged(String),
    /// `screen <id> <rows>`: only the last `rows` lines of a UIA text
    /// node's text are on screen (its visible range), as a terminal shows
    /// the end of its buffer; 0 shows the whole text again. Raises no
    /// event. UIA-only.
    Screen(String, usize),
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
    /// `stall <ms> [<at> <command>]`: blocks the window thread for that many
    /// milliseconds, so every cross-process call into the window waits, as
    /// with an application that is starting up or busy. The window thread
    /// prints `stall started` on stdout as it begins, and `stall ended <us>`,
    /// with the time in microseconds since the Unix epoch on the run's clock
    /// (`verbatim_process::clock`), as it ends. With
    /// `<at> <command>`, the window thread applies the command `at`
    /// milliseconds into the stall, still taking no message, as a busy
    /// application raises an event from the middle of its work, and
    /// acknowledges it as usual between the two lines.
    Stall(u64, Option<(u64, Box<Command>)>),
    /// `on-activate <command>`: the command is applied once the window is
    /// next activated, after the window messages of the activation have
    /// been handled, as an application starts work the moment it is
    /// activated; several are applied in the order given. Acknowledged
    /// `applied` when armed, and again by the command itself when it runs.
    OnActivate(Box<Command>),
    /// `slow <ms>`: every provider call from now on is answered that many
    /// milliseconds late, as by an application busy building a window; 0
    /// answers at once again. Applied on the window thread, so the calls
    /// before it are answered at the old pace.
    Slow(u64),
    /// `slow-patterns <ms>`: as `slow`, for pattern requests alone
    /// (`GetPatternProvider`), which filling an element's cache makes and a
    /// walk to its nearest window does not; 0 answers them at once again.
    SlowPatterns(u64),
    /// `refuse-text on` or `refuse-text off`: while on, every request for a
    /// node's text pattern fails (`E_FAIL`) rather than answering, as a
    /// provider that is not ready yet does while its application starts.
    /// UIA-only.
    RefuseText(bool),
    /// `answer-property <property id> fail`, `... empty` or `... usual`:
    /// from now on every provider's `GetPropertyValue` for that UIA
    /// property, by its numeric id, fails (`E_FAIL`), answers an empty
    /// value (`VT_EMPTY`), as a provider that does not support it does, or
    /// answers as usual again; one property at a time, the last named.
    /// UIA-only.
    AnswerProperty(i32, PropertyAnswer),
    /// `answer-parent <id> fail`, `... none` or `... usual`, optionally
    /// followed by a count, `after`: node `id`'s provider answers its next
    /// `after` `Navigate(Parent)` calls as usual, and every one after them
    /// failing (`E_FAIL`), with no parent (a null element), or as usual,
    /// while every other call on it answers as usual, as an element File
    /// Explorer has taken out of its tree can. The count lets a client's
    /// walk to the nearest window, which steps to the node's parent once,
    /// pass before the answer takes effect. UIA-only.
    AnswerParent(String, ParentAnswer, u32),
    /// `slow-parent <id> <ms>`, optionally followed by a count, `after`:
    /// node `id`'s provider answers its next `after` `Navigate(Parent)`
    /// calls at once, counted as `answer-parent` counts them, and every one
    /// after them `ms` milliseconds late, blocking the window thread
    /// meanwhile, as an application stuck in one call does; 0 answers at
    /// once again. Each late call prints `slow-parent started <us>` and
    /// `slow-parent ended <us>` on stdout, on the run's clock, around its
    /// wait. UIA-only.
    SlowParent(String, u64, u32),
    /// `slow-acc-parent <id> <ms>`: node `id`'s `accParent` is answered
    /// `ms` milliseconds late from now on, as `slow-parent` answers UIA's
    /// parent step, printing `slow-acc-parent started <us>` and
    /// `slow-acc-parent ended <us>` around each wait; 0 answers at once
    /// again. MSAA-only.
    SlowAccParent(String, u64),
    /// `fail-acc-parent <id>`: node `id`'s `accParent` fails with `E_FAIL`
    /// from now on, while every other call on it answers as usual, printing
    /// `fail-acc-parent started <us>` and `fail-acc-parent ended <us>` as
    /// each failing call begins and ends. MSAA-only.
    FailAccParent(String),
    /// `move <id> <left> <top> <width> <height>`: node `id`'s MSAA
    /// location is that rectangle from now on, raising no event, as a
    /// scrolled container's moves. MSAA-only.
    Move(String, [i32; 4]),
    /// `fail-acc-location <id>`: node `id`'s `accLocation` fails with
    /// `E_FAIL` from now on. MSAA-only.
    FailAccLocation(String),
    /// `recreate <id>`: node `id`'s MSAA object is made again, as an
    /// application that destroys a container and makes another in its
    /// place: the object handed out before answers every call as one whose
    /// server has disconnected it. MSAA-only.
    Recreate(String),
    /// `report-window <id>`: makes a child window of the host window and
    /// destroys it, and node `id`'s provider answers `NativeWindowHandle`
    /// with its handle, a window it does not host that no longer exists,
    /// as a fragment element of some providers can. UIA then reads the
    /// node as that window's element, runtime id `[42, handle]`, with no
    /// parent. UIA-only.
    ReportWindow(String),
    /// `take-runtime-id <id> <from>`: node `from` dies, leaving the tree
    /// (its parent no longer lists it, and every call on its elements fails
    /// as on an element that is gone, `UIA_E_ELEMENTNOTAVAILABLE`), and node
    /// `id` takes its runtime id, as File Explorer gives a new item the
    /// runtime id of one it destroyed. Raises no event. UIA-only.
    TakeRuntimeId(String, String),
    /// `replace-items <id>`: every child of node `id`, with everything
    /// under it, dies, as `take-runtime-id`'s node does, and a copy of
    /// each, with a runtime id of its own, takes its place under the same
    /// fixture id, keeping its states, so the copy of the selected item is
    /// selected, as the Settings app replaces its search suggestions'
    /// items under new runtime ids after raising a selection for one of
    /// the old ones. Raises no event. UIA-only.
    ReplaceItems(String),
    /// `disconnect <id>`: node `id` dies where it stands: its parent
    /// still lists it, but every call on it fails as on an object that is
    /// gone, `UIA_E_ELEMENTNOTAVAILABLE` on UIA and `RPC_E_DISCONNECTED`
    /// on MSAA, as an element of a menu that closed after its parent named
    /// it does. Raises no event.
    Disconnect(String),
    /// `disconnect-at-first-child <id>`: node `id` dies where it stands
    /// the first time its provider is asked for its first child, and that
    /// call fails with `UIA_E_ELEMENTNOTAVAILABLE`, as an element of a menu
    /// that closed while a reader went down into it. Its parent still lists
    /// it, and a read of the parent's children made before then has it.
    /// Raises no event. UIA-only.
    DisconnectAtFirstChild(String),
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
    /// `acc-focus-window <hwnd>`: every node's `accFocus` answers the
    /// client object of window `hwnd`, a decimal handle, from then on,
    /// acquired with `AccessibleObjectFromWindow` as it is asked, as an
    /// application whose own `accFocus` names an object another process
    /// serves would. MSAA-only.
    AccFocusWindow(isize),
    /// `child-window`: makes a child window of the host window, of the
    /// host's class, which answers `WM_GETOBJECT` as the host does, so an
    /// event can name a node at its object id in a window of its own
    /// inside the host's, as a control's window is inside a dialog's. Its
    /// objects are the node's own in that window, and `accParent` names
    /// the node's parent in it.
    ChildWindow,
    /// `child-client-identity`: the client area of the child window
    /// `child-window` made answers as `client-identity` makes the host's
    /// answer, `IAccIdentity` with its address and `accParent` with the
    /// child window's own window object, whose parent Windows answers with
    /// the host's client area, as a control's window is inside a Windows
    /// Forms panel whose client area offers no identity string.
    /// MSAA-only.
    ChildClientIdentity,
    /// `window-focus`: the host window takes the keyboard focus within its
    /// thread (`SetFocus`), so it is its thread's focus window, as a
    /// window whose application has activated it is.
    WindowFocus,
    /// `hold`: the next provider call, whichever client makes it, prints
    /// `held <method>` on stdout as it begins and then waits until
    /// `release`, so a test knows a call is in progress and decides when it
    /// finishes. Applied on the window thread, so the calls before it are
    /// not held.
    Hold,
    /// `release`: lets the call `hold` held go on. Applied and acknowledged
    /// on the stdin thread, since the window thread is the one held.
    Release,
    /// `quit`.
    Quit,
    /// A line that is no command, rejected on the window thread, so its
    /// acknowledgement keeps its place among the others'.
    Unrecognized(String),
}

/// `console-caret`'s arguments, a column and a row.
fn console_caret(rest: &str) -> Option<Command> {
    let (column, row) = rest.split_once(' ')?;
    Some(Command::ConsoleCaret(
        column.parse().ok()?,
        row.trim().parse().ok()?,
    ))
}

/// `caret`'s arguments, a node's id, a start, and an optional end.
fn caret(rest: &str) -> Option<Command> {
    let mut parts = rest.split_whitespace();
    let id = parts.next()?;
    let start: usize = parts.next()?.parse().ok()?;
    let end = match parts.next() {
        Some(end) => end.parse().ok()?,
        None => start,
    };
    let at_end = match parts.next() {
        Some("end") => true,
        Some(_) => return None,
        None => false,
    };
    Some(Command::Caret(id.to_owned(), start, end, at_end))
}

/// `stall`'s arguments: its length, and optionally when into it to apply
/// which command.
fn stall(rest: &str) -> Option<Command> {
    let mut parts = rest.splitn(3, ' ');
    let ms = parts.next()?.parse().ok()?;
    let Some(at) = parts.next() else {
        return Some(Command::Stall(ms, None));
    };
    let at = at.parse().ok()?;
    let command = parse_command(parts.next()?)?;
    Some(Command::Stall(ms, Some((at, Box::new(command)))))
}

/// `answer-property`'s arguments, a property's numeric id and how it is
/// answered.
fn answer_property(rest: &str) -> Option<Command> {
    let (property, answer) = rest.split_once(' ')?;
    let answer = match answer.trim() {
        "fail" => PropertyAnswer::Fail,
        "empty" => PropertyAnswer::Empty,
        "usual" => PropertyAnswer::Usual,
        _ => return None,
    };
    Some(Command::AnswerProperty(property.parse().ok()?, answer))
}

/// `answer-parent`'s arguments, a node's id, how its parent is answered,
/// and after how many calls answered as usual.
fn answer_parent(rest: &str) -> Option<Command> {
    let mut parts = rest.split_whitespace();
    let id = parts.next()?;
    let answer = match parts.next()? {
        "fail" => ParentAnswer::Fail,
        "none" => ParentAnswer::Missing,
        "usual" => ParentAnswer::Usual,
        _ => return None,
    };
    let after = match parts.next() {
        Some(after) => after.parse().ok()?,
        None => 0,
    };
    parts
        .next()
        .is_none()
        .then(|| Command::AnswerParent(id.to_owned(), answer, after))
}

/// `slow-parent`'s and `slow-acc-parent`'s arguments, a node's id, how
/// many milliseconds late its parent step is answered, and, where
/// `with_after` allows one, after how many steps answered at once.
fn slow_parent(rest: &str, with_after: bool) -> Option<(String, u64, u32)> {
    let mut parts = rest.split_whitespace();
    let id = parts.next()?;
    let ms = parts.next()?.parse().ok()?;
    let after = match parts.next() {
        Some(after) if with_after => after.parse().ok()?,
        Some(_) => return None,
        None => 0,
    };
    parts.next().is_none().then(|| (id.to_owned(), ms, after))
}

/// `slow-parent`, `slow-acc-parent` or `fail-acc-parent`, as `verb` names,
/// with its arguments.
fn parent_step(verb: &str, rest: &str) -> Option<Command> {
    match verb {
        "slow-parent" => {
            slow_parent(rest, true).map(|(id, ms, after)| Command::SlowParent(id, ms, after))
        }
        "slow-acc-parent" => {
            slow_parent(rest, false).map(|(id, ms, _)| Command::SlowAccParent(id, ms))
        }
        _ => (!rest.is_empty() && !rest.contains(' '))
            .then(|| Command::FailAccParent(rest.to_owned())),
    }
}

/// `take-runtime-id`'s arguments, the node that takes the runtime id and
/// the node it is taken from.
fn take_runtime_id(rest: &str) -> Option<Command> {
    let mut parts = rest.split_whitespace();
    let id = parts.next()?;
    let from = parts.next()?;
    parts
        .next()
        .is_none()
        .then(|| Command::TakeRuntimeId(id.to_owned(), from.to_owned()))
}

/// `write-on-read`'s arguments, a node's id and the text to write.
fn write_on_read(rest: &str) -> Option<Command> {
    let (id, text) = rest.split_once(' ')?;
    (!id.is_empty() && !text.is_empty())
        .then(|| Command::WriteOnRead(id.to_owned(), text.to_owned()))
}

/// Parses one stdin line into a [`Command`]. Blank lines and unrecognized
/// verbs yield `None` (a stray blank line is ignored; the caller hands an
/// unknown verb on as [`Command::Unrecognized`], to be rejected).
#[expect(clippy::too_many_lines, reason = "one arm per stdin command")]
pub(crate) fn parse_command(line: &str) -> Option<Command> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let (verb, rest) = line.split_once(' ').unwrap_or((line, ""));
    let rest = rest.trim();
    match verb {
        "quit" => Some(Command::Quit),
        "hold" => Some(Command::Hold),
        "release" => Some(Command::Release),
        "focus" if !rest.is_empty() => Some(Command::Focus(rest.to_owned())),
        "set-focus" if !rest.is_empty() => Some(Command::SetFocus(rest.to_owned())),
        "select" if !rest.is_empty() => Some(Command::Select(rest.to_owned())),
        "notify" if !rest.is_empty() => Some(Command::Notify(rest.to_owned())),
        "caret-event" if !rest.is_empty() => Some(Command::CaretEvent(rest.to_owned())),
        "text-changed" if !rest.is_empty() => Some(Command::TextChanged(rest.to_owned())),
        "console-caret" => console_caret(rest),
        "system-caret" => match rest {
            "" => Some(Command::SystemCaret(false)),
            "show" => Some(Command::SystemCaret(true)),
            _ => None,
        },
        "stall" => stall(rest),
        "on-activate" => parse_command(rest).map(|command| Command::OnActivate(Box::new(command))),
        "slow" => rest.parse().ok().map(Command::Slow),
        "slow-patterns" => rest.parse().ok().map(Command::SlowPatterns),
        "refuse-text" => match rest {
            "on" => Some(Command::RefuseText(true)),
            "off" => Some(Command::RefuseText(false)),
            _ => None,
        },
        "answer-property" => answer_property(rest),
        "answer-parent" => answer_parent(rest),
        "slow-parent" | "slow-acc-parent" | "fail-acc-parent" => parent_step(verb, rest),
        "move" => {
            let mut parts = rest.split_whitespace();
            let id = parts.next()?.to_owned();
            let mut bounds = [0i32; 4];
            for value in &mut bounds {
                *value = parts.next()?.parse().ok()?;
            }
            parts.next().is_none().then_some(Command::Move(id, bounds))
        }
        "fail-acc-location" if !rest.is_empty() && !rest.contains(' ') => {
            Some(Command::FailAccLocation(rest.to_owned()))
        }
        "recreate" if !rest.is_empty() && !rest.contains(' ') => {
            Some(Command::Recreate(rest.to_owned()))
        }
        "report-window" if !rest.is_empty() => Some(Command::ReportWindow(rest.to_owned())),
        "take-runtime-id" => take_runtime_id(rest),
        "replace-items" if !rest.is_empty() => Some(Command::ReplaceItems(rest.to_owned())),
        "disconnect" if !rest.is_empty() => Some(Command::Disconnect(rest.to_owned())),
        "disconnect-at-first-child" if !rest.is_empty() => {
            Some(Command::DisconnectAtFirstChild(rest.to_owned()))
        }
        "active-text-position" => {
            let mut parts = rest.split_whitespace();
            let id = parts.next()?;
            let start: usize = parts.next()?.parse().ok()?;
            let end: usize = parts.next()?.parse().ok()?;
            Some(Command::ActiveTextPosition(id.to_owned(), start, end))
        }
        "caret" => caret(rest),
        "set-name" => {
            let (id, text) = rest.split_once(' ').unwrap_or((rest, ""));
            (!id.is_empty()).then(|| Command::SetName(id.to_owned(), text.trim().to_owned()))
        }
        "screen" => {
            let (id, rows) = rest.split_once(' ')?;
            Some(Command::Screen(id.to_owned(), rows.trim().parse().ok()?))
        }
        "write-on-read" => write_on_read(rest),
        "set-text" => {
            let (id, text) = rest.split_once(' ').unwrap_or((rest, ""));
            (!id.is_empty()).then(|| Command::SetText(id.to_owned(), unescape(text)))
        }
        "client-name" => Some(Command::ClientName(rest.to_owned())),
        "client-identity" => Some(Command::ClientIdentity),
        "disable-client" => Some(Command::DisableClient),
        "acc-focus-window" => Some(Command::AccFocusWindow(rest.trim().parse().ok()?)),
        "child-window" => Some(Command::ChildWindow),
        "child-client-identity" => Some(Command::ChildClientIdentity),
        "window-focus" => Some(Command::WindowFocus),
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
        "set-controller-for" => {
            let mut parts = rest.split_whitespace();
            let id = parts.next()?;
            Some(Command::SetControllerFor(
                id.to_owned(),
                parts.next().map(str::to_owned),
            ))
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
            Some('r') => out.push('\r'),
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
            Some(Command::Release) => {
                crate::hits::release();
                crate::window::acknowledge("applied");
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
    fn parses_text_changed() {
        match parse_command("text-changed term") {
            Some(Command::TextChanged(id)) => assert_eq!(id, "term"),
            _ => panic!("expected TextChanged"),
        }
        assert!(parse_command("text-changed").is_none());
    }

    #[test]
    fn parses_a_screen_of_rows() {
        match parse_command("screen term 30") {
            Some(Command::Screen(id, rows)) => assert_eq!((id.as_str(), rows), ("term", 30)),
            _ => panic!("expected Screen"),
        }
        assert!(parse_command("screen term many").is_none());
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
            Some(Command::Caret(id, start, end, at_end)) => {
                assert_eq!((id.as_str(), start, end, at_end), ("doc", 3, 3, false));
            }
            _ => panic!("expected Caret"),
        }
        match parse_command("caret doc 3 7") {
            Some(Command::Caret(_, start, end, at_end)) => {
                assert_eq!((start, end, at_end), (3, 7, false));
            }
            _ => panic!("expected Caret"),
        }
        match parse_command("caret doc 3 7 end") {
            Some(Command::Caret(_, start, end, at_end)) => {
                assert_eq!((start, end, at_end), (3, 7, true));
            }
            _ => panic!("expected Caret"),
        }
        assert!(parse_command("caret doc 3 7 middle").is_none());
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
        assert!(matches!(
            parse_command("slow-patterns 20"),
            Some(Command::SlowPatterns(20))
        ));
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
    fn parses_a_stall_with_a_command_inside_it() {
        assert!(matches!(
            parse_command("stall 8000"),
            Some(Command::Stall(8000, None))
        ));
        match parse_command("stall 20000 8000 focus btn2") {
            Some(Command::Stall(20000, Some((8000, command)))) => {
                assert!(matches!(*command, Command::Focus(ref id) if id == "btn2"));
            }
            _ => panic!("expected Stall with a command"),
        }
        assert!(parse_command("stall 20000 8000").is_none());
        assert!(parse_command("stall 20000 8000 frobnicate").is_none());
    }

    #[test]
    fn parses_the_slow_and_failing_parent_steps() {
        assert!(matches!(
            parse_command("slow-parent item 4000 1"),
            Some(Command::SlowParent(ref id, 4000, 1)) if id == "item"
        ));
        assert!(matches!(
            parse_command("slow-parent item 0"),
            Some(Command::SlowParent(ref id, 0, 0)) if id == "item"
        ));
        assert!(parse_command("slow-parent item").is_none());
        assert!(parse_command("slow-parent item 4000 1 2").is_none());
        assert!(matches!(
            parse_command("slow-acc-parent item 15000"),
            Some(Command::SlowAccParent(ref id, 15000)) if id == "item"
        ));
        assert!(parse_command("slow-acc-parent item 15000 1").is_none());
        assert!(matches!(
            parse_command("fail-acc-parent item"),
            Some(Command::FailAccParent(ref id)) if id == "item"
        ));
        assert!(parse_command("fail-acc-parent").is_none());
        assert!(parse_command("fail-acc-parent item other").is_none());
    }

    #[test]
    fn parses_on_activate() {
        match parse_command("on-activate stall 6000") {
            Some(Command::OnActivate(command)) => {
                assert!(matches!(*command, Command::Stall(6000, None)));
            }
            _ => panic!("expected OnActivate"),
        }
        assert!(parse_command("on-activate").is_none());
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
