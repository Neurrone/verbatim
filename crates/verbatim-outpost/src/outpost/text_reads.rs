//! The worker's text (milestone M4): which nodes have text and through
//! which backend, Core's text requests answered with [`crate::text`], the
//! caret reports sent as `CaretMoved`, and the checks of a caret key's
//! watch for evidence, with the watch the worker keeps open between them.
//!
//! A UIA node has text when its element has a text pattern, fetched once
//! per node and kept; an MSAA node has text when it is the client area of a
//! standard edit or rich edit control's window, read through the window's
//! messages. Any other node answers `NoText`, as MSAA has no text interface:
//! Core then reviews its value or name. A fetch that fails, rather than
//! answering that the element has no text pattern, is not kept: the read
//! answers `Unanswered`, and the next one fetches again, as NVDA keeps no
//! failed fetch. An answer of no text pattern, which is also how UIA reports
//! a provider that fails the request, is kept only until the node is next
//! reported as the focus, which NVDA reads as a new object, or raises a
//! caret or text event, which only an element with text does
//! (`docs/parity/text.md`, "A text pattern missing at the focus").

#![forbid(unsafe_code)]

use std::time::{Duration, Instant};

use windows::Win32::UI::Accessibility::{IUIAutomationTextPattern, IUIAutomationTextPattern2};
use windows::Win32::UI::WindowsAndMessaging::OBJID_CLIENT;
use windows::core::AgileReference;

use verbatim_ia2::CHILDID_SELF;
use verbatim_model::{
    CallCounts, CaretReport, CaretWatch, NodeId, Role, SelectionChange, TextOp, TextReply, TraceId,
};
use verbatim_uia::map::is_terminal_class;
use verbatim_uia::{Cached, Kept};
use verbatim_uia_rops::Path;

use crate::arbitration::{normalize_class_name, window_class_name};
use crate::protocol::EventTiming;
use crate::text::edit::EditText;
use crate::text::uia::UiaText;
use crate::text::{self, CaretSignal, TextError, Watched};

use super::Context;

/// A node's text patterns, kept so each request does not fetch them again;
/// `None` for a node whose element has no text pattern.
pub(super) type Patterns = Option<(
    AgileReference<IUIAutomationTextPattern>,
    Option<AgileReference<IUIAutomationTextPattern2>>,
)>;

/// The longest a caret key's watch stays open without evidence. It only
/// frees the watch: ending it this way says nothing, as ending it any other
/// way does. A key whose application raises its evidence later than this
/// is not spoken. Ten seconds, the time the worker allows an application
/// that is busy, or starting up, to answer one event's reads.
pub(super) const CARET_WATCH_BOUND: Duration = Duration::from_secs(10);

/// A caret key's watch for evidence, open between the worker's entries
/// (`docs/crates/verbatim-outpost.md`, a caret key's watch under "Text").
/// There is at most one: the next key's replaces it.
pub(super) struct OpenWatch {
    /// The request it answers.
    pub(super) request_id: u64,
    pub(super) trace: TraceId,
    /// The node whose caret the key moves.
    pub(super) node_id: NodeId,
    pub(super) watch: CaretWatch,
    /// When it was opened, for [`CARET_WATCH_BOUND`].
    pub(super) opened: Instant,
    /// The request's timing, which its answer carries.
    pub(super) timing: EventTiming,
    /// The calls its checks have made so far, none of which found evidence.
    pub(super) calls: CallCounts,
}

/// A check of a caret key's watch, as the worker makes it.
struct Signal<'a> {
    context: &'a Context,
    node_id: NodeId,
    /// Whether a caret event prompted the check.
    caret_event: bool,
    /// When the key was pressed, as the watch says (0 when unknown).
    pressed_at_ms: u64,
    /// Whether a caret event of the node observed since the key was
    /// pressed waited to be handled when the caret was read: the read
    /// covers it, so the event, once handled, checks nothing
    /// (`Context::caret_read_since`), and the read is evidence as the
    /// event would have been.
    covered: bool,
}

impl CaretSignal for Signal<'_> {
    fn caret_event(&mut self) -> bool {
        self.caret_event || self.covered
    }

    fn now_ms(&mut self) -> u64 {
        super::now_ms()
    }

    fn reading(&mut self) {
        if !self.caret_event {
            self.covered = caret_event_waiting(self.context, self.node_id, self.pressed_at_ms);
        }
        self.context.caret_read(self.node_id);
    }
}

/// Whether a caret event of `node_id` observed at or after `since_ms`
/// waits in the intake: UIA's caret event from the node's element, the
/// console's caret event of its window for the console host's text area,
/// or, for an edit control's client area, the caret's location change or
/// the text selection change of its window.
fn caret_event_waiting(context: &Context, node_id: NodeId, since_ms: u64) -> bool {
    use super::intake::{Item, UiaKind};
    use verbatim_ia2::WinEventKind;
    let runtime_id = context.uia_registry.runtime_id_of(node_id);
    let (console, edit) = match &runtime_id {
        Some(_) => (
            console_focus(context, node_id)
                .then(|| context.tracking().window())
                .flatten(),
            None,
        ),
        None => (
            None,
            context
                .msaa_registry
                .key_of(node_id)
                .filter(|&(_, object, child)| object == OBJID_CLIENT.0 && child == CHILDID_SELF)
                .map(|(hwnd, ..)| hwnd),
        ),
    };
    context.intake.any_waiting(|entry| {
        entry.observed_at_ms >= since_ms
            && match &entry.item {
                Item::Uia(event) => {
                    matches!(event.kind, UiaKind::TextSelection)
                        && runtime_id.as_ref() == Some(&event.parts.runtime_id)
                }
                Item::Msaa {
                    kind: WinEventKind::ConsoleCaret,
                    hwnd,
                    ..
                } => console == Some(*hwnd),
                Item::Msaa {
                    kind: WinEventKind::Caret | WinEventKind::TextSelectionChange,
                    hwnd,
                    ..
                } => edit == Some(*hwnd),
                _ => false,
            }
    })
}

/// A node's text, through its backend.
enum Source {
    Uia(UiaText),
    Edit(EditText),
}

/// The text behind `node_id`, or the reply that ends the request: `NoText`
/// for a node with no text interface, `Gone` for one this outpost no longer
/// knows.
fn source(context: &Context, node_id: NodeId) -> Result<Source, TextReply> {
    if context.uia_registry.runtime_id_of(node_id).is_some() {
        return uia_source(context, node_id).map(Source::Uia);
    }
    let Some((hwnd, object, child)) = context.msaa_registry.key_of(node_id) else {
        return Err(TextReply::Gone);
    };
    match edit_version(hwnd, object, child) {
        Some(version) => Ok(Source::Edit(EditText::new(hwnd, version))),
        None => Err(TextReply::NoText),
    }
}

/// The edit API version of an MSAA object that is an edit control's client
/// area, `None` for any other object.
fn edit_version(hwnd: isize, object: i32, child: i32) -> Option<u8> {
    if object != OBJID_CLIENT.0 || child != CHILDID_SELF {
        return None;
    }
    verbatim_ia2::edit::edit_api_version(&normalize_class_name(&window_class_name(hwnd)))
}

/// A UIA node's text, its patterns fetched once and kept.
fn uia_source(context: &Context, node_id: NodeId) -> Result<UiaText, TextReply> {
    let element = context
        .uia_registry
        .element_of(node_id)
        .and_then(|agile| agile.resolve().ok())
        .ok_or(TextReply::Gone)?;
    let kept = context.patterns().get(&node_id.number()).cloned();
    let found = if let Some(found) = kept {
        found
    } else {
        {
            let fetched = match verbatim_uia::text::text_pattern(element.live().element()) {
                Ok((text_pattern, caret_pattern)) => {
                    AgileReference::new(&text_pattern).ok().map(|text_pattern| {
                        (
                            text_pattern,
                            caret_pattern.and_then(|caret| AgileReference::new(&caret).ok()),
                        )
                    })
                }
                Err(error) if verbatim_uia::element_is_gone(&error) => {
                    return Err(TextReply::Gone);
                }
                // An answer with no pattern: the element has none.
                Err(error) if error.code().is_ok() => None,
                // The provider did not answer, as one that is not ready yet
                // while its application starts: nothing is kept, so the
                // next read fetches again.
                Err(error) => {
                    tracing::debug!(%error, "the text pattern could not be fetched");
                    return Err(TextReply::Unanswered);
                }
            };
            context.patterns().insert(node_id.number(), fetched.clone());
            fetched
        }
    };
    let Some((text_pattern, caret_pattern)) = found else {
        return Err(TextReply::NoText);
    };
    let text_pattern = text_pattern.resolve().map_err(|_| TextReply::Gone)?;
    let caret_pattern = caret_pattern.and_then(|caret| caret.resolve().ok());
    let class = element.read().class_name;
    let terminal = is_terminal_class(class.as_deref()) || console_focus(context, node_id);
    let remote = context.tries_remote(context.tracking().window());
    let support = context
        .text_support()
        .get(&node_id.number())
        .copied()
        .unwrap_or_default();
    Ok(UiaText::new(
        element.live().element().clone(),
        text_pattern,
        caret_pattern,
        terminal,
    )
    .remote(remote)
    .fetches(context.fetches())
    .support(support))
}

/// Forgets that `node_id`'s element answered it has no text pattern, as it
/// is reported as the focus again, or raises a caret or text event, which
/// only an element with text does: NVDA fetches the pattern afresh for every
/// focus, and an application whose provider was not ready when it was
/// fetched (it then answers no pattern) has its text read once it says it
/// has text. A pattern found is kept. Returns whether an answer of no
/// pattern was forgotten.
pub(super) fn forget_no_text(context: &Context, node_id: NodeId) -> bool {
    let mut patterns = context.patterns();
    let forgotten = patterns.get(&node_id.number()).is_some_and(Option::is_none);
    if forgotten {
        patterns.remove(&node_id.number());
    }
    forgotten
}

/// As a caret or text event of `node_id` arrives from `element`, its
/// sender: forgets an answer of no text pattern ([`forget_no_text`]), and
/// then keeps `element` as the node's, so the pattern is fetched from the
/// element that raised the event. The element kept may have been UIA's
/// stand-in for a window whose provider did not answer in time, which has
/// the window's runtime id and no text pattern: a short read of a starting
/// Windows 11 Notepad's focused element has answered with it.
pub(super) fn text_event_from(context: &Context, node_id: NodeId, element: Option<&Cached<Kept>>) {
    if !forget_no_text(context, node_id) {
        return;
    }
    if let (Some(element), Some(runtime_id)) =
        (element, context.uia_registry.runtime_id_of(node_id))
    {
        // Which element is replaced, by which: each one's runtime id, which
        // UIA keeps with the element, and its class name, which `Kept`
        // holds, so nothing is read beyond the cache (decision 7 of
        // `docs/design/uia-shapes.md`).
        let describe = |element: &Cached<Kept>| (element.runtime_id(), element.read().class_name);
        let replaced = context
            .uia_registry
            .element_of(node_id)
            .and_then(|agile| agile.resolve().ok())
            .map(|old| describe(&old));
        tracing::info!(
            ?node_id,
            ?runtime_id,
            ?replaced,
            by = ?describe(element),
            "a caret or text event's element replaced the element of a node that had answered no text pattern"
        );
        // The same node: the registry knows it by this runtime id.
        let _ = context.uia_registry.id_for_element(&runtime_id, element);
    }
}

/// Keeps what `source`, `node_id`'s text, is now known to support, for the
/// node's next read.
fn keep_support(context: &Context, node_id: NodeId, source: &UiaText) {
    context
        .text_support()
        .insert(node_id.number(), source.known_support());
}

/// Logs a caret read that fell back from its remote operation to the
/// classic reads, and stops trying remote operations in the window when the
/// import failed, as the focus walk and the terminal read do.
fn note_fallback(context: &Context, source: &mut UiaText) {
    let Some(error) = source.take_fallback() else {
        return;
    };
    let window = context.tracking().window();
    tracing::warn!(?window, %error, "a remote operation failed; read the classic way");
    if let (verbatim_uia_rops::Error::Import(_), Some(window)) = (&error, window) {
        context.read_classically(window);
    }
}

/// Whether `node_id` is the focus and in a console window, the console
/// host's text area.
pub(super) fn console_focus(context: &Context, node_id: NodeId) -> bool {
    let focus_is_node = context
        .uia_registry
        .runtime_id_of(node_id)
        .is_some_and(|runtime_id| {
            context.intake.focused() == Some(super::intake::Object::Uia(runtime_id))
        });
    focus_is_node
        && context
            .tracking()
            .window()
            .is_some_and(|hwnd| window_class_name(hwnd) == CONSOLE_WINDOW_CLASS)
}

/// The console host's window class.
pub(crate) const CONSOLE_WINDOW_CLASS: &str = "ConsoleWindowClass";

/// Whether a focused node may have text, and so gets a caret report:
/// through UIA, an edit field, a document, or a terminal (whose text
/// pattern is then asked for); through MSAA, an edit control's client area.
pub(super) fn may_have_text(context: &Context, node_id: NodeId, role: Role) -> bool {
    if context.uia_registry.runtime_id_of(node_id).is_some() {
        return matches!(role, Role::EditableText | Role::Document | Role::Terminal);
    }
    context
        .msaa_registry
        .key_of(node_id)
        .is_some_and(|(hwnd, object, child)| edit_version(hwnd, object, child).is_some())
}

/// Answers one of Core's text requests other than a caret key's watch,
/// which [`check_watch`] checks.
pub(super) fn answer(context: &Context, node_id: NodeId, op: &TextOp) -> TextReply {
    let source = match source(context, node_id) {
        Ok(source) => source,
        Err(reply) => return reply,
    };
    match source {
        Source::Uia(mut source) => {
            let mut anchors = context.uia_anchors();
            let reply = text::perform(&mut source, &mut anchors.node(node_id.number()), op);
            drop(anchors);
            note_fallback(context, &mut source);
            keep_support(context, node_id, &source);
            reply
        }
        Source::Edit(mut source) => {
            let mut anchors = context.edit_anchors();
            text::perform(&mut source, &mut anchors.node(node_id.number()), op)
        }
    }
}

/// Checks a caret key's watch on `node_id` with one read of its caret
/// ([`text::check_caret`]): the answer once there is evidence, or the reply
/// that ends the request when the node has no text or is gone; otherwise
/// the watch stays open. `caret_event` says whether a caret event prompted
/// the check.
pub(super) fn check_watch(
    context: &Context,
    node_id: NodeId,
    watch: &CaretWatch,
    caret_event: bool,
) -> Watched {
    let source = match source(context, node_id) {
        Ok(source) => source,
        // A UIA element that answered no text pattern, or did not answer:
        // an application still starting answers so for an element that has
        // text, and raises a caret event once the key moves its caret, which
        // only an element with text does; the watch stays open for it.
        Err(TextReply::NoText | TextReply::Unanswered)
            if context.uia_registry.runtime_id_of(node_id).is_some() =>
        {
            return Watched::Watching;
        }
        Err(reply) => return Watched::Answered(reply),
    };
    let mut signal = Signal {
        context,
        node_id,
        caret_event,
        pressed_at_ms: watch.pressed_at_ms,
        covered: false,
    };
    match source {
        Source::Uia(mut source) => {
            let mut anchors = context.uia_anchors();
            let watched = text::check_caret(
                &mut source,
                &mut anchors.node(node_id.number()),
                watch,
                &mut signal,
            );
            drop(anchors);
            note_fallback(context, &mut source);
            keep_support(context, node_id, &source);
            watched
        }
        Source::Edit(mut source) => {
            let mut anchors = context.edit_anchors();
            text::check_caret(
                &mut source,
                &mut anchors.node(node_id.number()),
                watch,
                &mut signal,
            )
        }
    }
}

/// Why there is no caret to report.
pub(super) enum NoCaret {
    /// The node has no text to read: no text pattern, or an MSAA object
    /// that is not an edit control.
    NoText,
    /// The node has text, or may have, but it could not be read now: its
    /// provider did not answer, or its element is not known yet.
    NotRead,
}

/// What a caret report is for ([`report_caret`]).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CaretFor {
    /// The report after a focus, whose line is spoken with its formatting,
    /// and whose selection the next reports' changes are worked out from.
    Focus,
    /// A caret event of a text focus that is not a terminal, with how the
    /// selection changed since the last report, as NVDA's auto-select
    /// detection reports it (`EditableTextWithAutoSelectDetection.event_caret`,
    /// `nvda/source/NVDAObjects/behaviors.py` 346 to 349;
    /// `detectPossibleSelectionChange`, `nvda/source/editableText.py` 417 to
    /// 471).
    Event,
    /// A caret event of a terminal, compared with the caret last sent to
    /// Core for it. NVDA's console class replaces the selection detection
    /// (`nvda/source/NVDAObjects/UIA/winConsoleUIA.py` 416), not yet traced,
    /// so a terminal's report carries no selection changes.
    Terminal,
}

/// The caret of `node_id`, for a `CaretMoved` event, read as `purpose`
/// says, with how the selection changed for [`CaretFor::Event`];
/// otherwise why there is none. For [`CaretFor::Terminal`], also whether
/// the caret is the one last sent to Core for the terminal: at the same
/// place, the read says, with the same line.
pub(super) fn report_caret(
    context: &Context,
    node_id: NodeId,
    purpose: CaretFor,
) -> Result<(CaretReport, bool, Vec<SelectionChange>), NoCaret> {
    if purpose == CaretFor::Focus {
        forget_selection(context, node_id);
    }
    let source = match source(context, node_id) {
        Ok(source) => source,
        Err(TextReply::NoText) => return Err(NoCaret::NoText),
        Err(_) => return Err(NoCaret::NotRead),
    };
    let sent = if purpose == CaretFor::Terminal {
        sent_caret_point(context, node_id)
    } else {
        None
    };
    let (formats, changes) = (purpose == CaretFor::Focus, purpose == CaretFor::Event);
    context.caret_read(node_id);
    let report = match source {
        Source::Uia(mut source) => {
            let mut anchors = context.uia_anchors();
            let report = text::caret_report_since(
                &mut source,
                &mut anchors.node(node_id.number()),
                &mut super::now_ms,
                (formats, sent.as_ref().map(|(_, point)| point), changes),
            )
            .map(|read| {
                let unchanged = !read.moved
                    && sent
                        .as_ref()
                        .is_some_and(|(sent, _)| text::same_caret(sent, &read.report));
                (read.report, unchanged, read.selection_changes)
            });
            drop(anchors);
            note_fallback(context, &mut source);
            keep_support(context, node_id, &source);
            report
        }
        Source::Edit(mut source) => {
            let mut anchors = context.edit_anchors();
            text::caret_report_since(
                &mut source,
                &mut anchors.node(node_id.number()),
                &mut super::now_ms,
                (formats, None, changes),
            )
            .map(|read| (read.report, false, read.selection_changes))
        }
    };
    match report {
        Ok(report) => Ok(report),
        Err(TextError::Gone) => Err(NoCaret::NotRead),
        Err(TextError::Failed(reason)) => {
            tracing::debug!(reason, "the caret could not be read");
            Err(NoCaret::NotRead)
        }
    }
}

/// Forgets the selection `node_id`'s next caret report would be compared
/// with ([`text::NodeText::forget_selection`]): at a new focus, and when its
/// text changed, after which NVDA speaks a selection change in general terms
/// ("selected instead", "selection removed": `hasContentChangedSinceLastSelection`,
/// `nvda/source/NVDAObjects/behaviors.py` 351 to 358, and
/// `speech.speakSelectionChange`'s `generalize`, `nvda/source/speech/speech.py`
/// 1340 to 1416), which Verbatim does not yet say: the next report carries no
/// change.
pub(super) fn forget_selection(context: &Context, node_id: NodeId) {
    context.uia_anchors().forget_selection(node_id.number());
    context.edit_anchors().forget_selection(node_id.number());
}

/// A terminal's caret as a read found it ([`terminal_output`]).
pub(super) struct TerminalCaret {
    /// The caret.
    pub(super) report: CaretReport,
    /// Whether it is the caret last sent to Core for the terminal
    /// ([`crate::terminal::Terminal::sent_caret`]): at the same place, the
    /// read says, with the same line.
    pub(super) unchanged: bool,
}

/// The caret last sent to Core for terminal `node_id`, with where it was,
/// when the outpost still knows that without a call: what a caret read
/// compares with.
fn sent_caret_point(
    context: &Context,
    node_id: NodeId,
) -> Option<(CaretReport, crate::text::uia::UiaPos)> {
    let sent = context
        .terminals()
        .get(&node_id.number())
        .and_then(|terminal| terminal.sent_caret.clone())?;
    let point = context
        .uia_anchors()
        .node(node_id.number())
        .reported_point(verbatim_model::TextPosition {
            anchor: sent.line.start,
            offset: sent.line.offset,
        })?;
    Some((sent, point))
}

/// Reads what is new in the focused terminal `node_id`'s text
/// (`crate::terminal`), as `mode` says. `None` when it has no text pattern,
/// is gone, or could not be read.
///
/// Except for a baseline, the caret is read with the text, in the same
/// round trip, and returned as a caret report: a terminal raises no caret
/// event for every character typed (the console host's come on a schedule
/// of their own), so Core's copy of the caret would lag behind typing. The
/// same round trip says whether the caret is still where the caret last
/// sent to Core was.
pub(super) fn terminal_output(
    context: &Context,
    uia: &verbatim_uia::Uia,
    node_id: NodeId,
    mode: crate::terminal::ReadMode,
) -> Option<(crate::terminal::Found, Option<TerminalCaret>)> {
    let Ok(mut source) = uia_source(context, node_id) else {
        return None;
    };
    let window = context.tracking().window();
    let remote = context.tries_remote(window);
    let head_wanted = u32::from(context.terminal_lines());
    let sent = sent_caret_point(context, node_id);
    let since = match &sent {
        Some((_, point)) => point.range_end().ok(),
        None => None,
    };
    let caret =
        (mode != crate::terminal::ReadMode::Baseline).then(|| verbatim_uia_rops::CaretLineQuery {
            element: source.element(),
            pattern: source.pattern(),
            pattern2: source.pattern2(),
            max_text: i32::try_from(crate::text::MAX_CHUNK_UNITS + 1).unwrap_or(i32::MAX),
            since: since
                .as_ref()
                .map(|(range, endpoint)| verbatim_uia_rops::RangeEnd {
                    range,
                    endpoint: *endpoint,
                }),
        });
    let mut terminals = context.terminals();
    let terminal = terminals.entry(node_id.number()).or_default();
    // The console host's `FindText` matches a row's padding, so the
    // anchor's row is sought with it there.
    terminal.matches_padding = console_focus(context, node_id);
    // The caret is stamped as read when the round trip began: a caret event
    // observed while it was in flight may report a move it did not see.
    let started_us = crate::protocol::now_us();
    let read = crate::terminal::read(
        uia,
        (source.element(), source.pattern()),
        caret,
        terminal,
        (head_wanted, remote, mode),
    );
    if let Ok(answer) = &read {
        terminal.note_read(started_us / 1_000, &answer.found);
    }
    drop(terminals);
    match read {
        Ok(answer) => {
            if let Some(Path::Fallback(error)) = &answer.path {
                tracing::warn!(?window, %error, "a remote operation failed; read the classic way");
                if let (verbatim_uia_rops::Error::Import(_), Some(window)) = (error, window) {
                    context.read_classically(window);
                }
            }
            let compared = since.is_some();
            let caret = answer.caret.and_then(|answer| {
                context.caret_read_from(node_id, started_us);
                let read_at_ms = super::now_ms();
                let read = source.caret_read_from(answer).ok()?;
                let moved = read.moved;
                let mut anchors = context.uia_anchors();
                let report = crate::text::caret_report_from(
                    &mut source,
                    &mut anchors.node(node_id.number()),
                    read,
                    read_at_ms,
                )
                .ok()?;
                let unchanged = compared
                    && !moved
                    && sent
                        .as_ref()
                        .is_some_and(|(sent, _)| crate::text::same_caret(sent, &report));
                Some(TerminalCaret { report, unchanged })
            });
            Some((answer.found, caret))
        }
        Err(TextError::Gone) => None,
        Err(TextError::Failed(reason)) => {
            tracing::debug!(reason, "a terminal's text could not be read");
            None
        }
    }
}

/// The position in `node_id`'s text where `range`, the range an active text
/// position change carried, starts: a new anchor there, read from nothing,
/// so it costs no call.
pub(super) fn active_position(
    context: &Context,
    node_id: NodeId,
    range: &AgileReference<windows::Win32::UI::Accessibility::IUIAutomationTextRange>,
) -> Option<verbatim_model::TextPosition> {
    let range = range.resolve().ok()?;
    let pos = crate::text::uia::UiaPos::start_of(&range).ok()?;
    Some(
        context
            .uia_anchors()
            .node(node_id.number())
            .position_at(pos),
    )
}

/// Forgets what was kept for released nodes.
pub(super) fn forget(context: &Context, released: impl IntoIterator<Item = u64>) {
    let released: Vec<u64> = released.into_iter().collect();
    let mut patterns = context.patterns();
    let mut support = context.text_support();
    let mut uia = context.uia_anchors();
    let mut edit = context.edit_anchors();
    let mut terminals = context.terminals();
    let mut owed = context.owed_reads();
    for node in released {
        patterns.remove(&node);
        support.remove(&node);
        uia.forget_node(node);
        edit.forget_node(node);
        terminals.remove(&node);
        owed.remove(&node);
    }
}
