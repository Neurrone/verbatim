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
//! (`docs/parity.md`, "A text pattern missing at the focus").

#![forbid(unsafe_code)]

use std::time::{Duration, Instant};

use windows::Win32::UI::Accessibility::{IUIAutomationTextPattern, IUIAutomationTextPattern2};
use windows::Win32::UI::WindowsAndMessaging::OBJID_CLIENT;
use windows::core::AgileReference;

use verbatim_ia2::CHILDID_SELF;
use verbatim_model::{
    CallCounts, CaretReport, CaretWatch, NodeId, Role, TextOp, TextReply, TraceId,
};
use verbatim_uia::ElementExt;
use verbatim_uia::map::is_terminal_class;
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
}

impl CaretSignal for Signal<'_> {
    fn caret_event(&mut self) -> bool {
        self.caret_event
    }

    fn now_ms(&mut self) -> u64 {
        super::now_ms()
    }

    fn reading(&mut self) {
        self.context.caret_read(self.node_id);
    }
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
            let fetched = match verbatim_uia::text::text_pattern(&element) {
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
    // Built with the base cache request, which caches the class name.
    let class = element.cached_string(windows::Win32::UI::Accessibility::UIA_ClassNamePropertyId);
    let terminal = is_terminal_class(class.as_deref()) || console_focus(context, node_id);
    let remote = context.tries_remote(context.tracking().window());
    let support = context
        .text_support()
        .get(&node_id.number())
        .copied()
        .unwrap_or_default();
    Ok(UiaText::new(element, text_pattern, caret_pattern, terminal)
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
pub(super) fn text_event_from(
    context: &Context,
    node_id: NodeId,
    element: Option<&windows::Win32::UI::Accessibility::IUIAutomationElement>,
) {
    if !forget_no_text(context, node_id) {
        return;
    }
    if let (Some(element), Some(runtime_id)) =
        (element, context.uia_registry.runtime_id_of(node_id))
    {
        // Cached reads only: which element is replaced, by which.
        let describe = |element: &windows::Win32::UI::Accessibility::IUIAutomationElement| {
            use windows::Win32::UI::Accessibility::{
                UIA_ClassNamePropertyId, UIA_FrameworkIdPropertyId, UIA_NamePropertyId,
            };
            (
                element.cached_string(UIA_NamePropertyId),
                element.cached_string(UIA_ClassNamePropertyId),
                element.cached_string(UIA_FrameworkIdPropertyId),
            )
        };
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
pub(super) const CONSOLE_WINDOW_CLASS: &str = "ConsoleWindowClass";

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

/// The caret of `node_id`, for a `CaretMoved` event, with the line's
/// formatting when `formats` (the report after a focus, whose line is
/// spoken); otherwise why there is none.
pub(super) fn report_caret(
    context: &Context,
    node_id: NodeId,
    formats: bool,
) -> Result<CaretReport, NoCaret> {
    let source = match source(context, node_id) {
        Ok(source) => source,
        Err(TextReply::NoText) => return Err(NoCaret::NoText),
        Err(_) => return Err(NoCaret::NotRead),
    };
    context.caret_read(node_id);
    let report = match source {
        Source::Uia(mut source) => {
            let mut anchors = context.uia_anchors();
            let report = text::caret_report(
                &mut source,
                &mut anchors.node(node_id.number()),
                &mut super::now_ms,
                formats,
            )
            .map(|(report, _)| report);
            drop(anchors);
            note_fallback(context, &mut source);
            keep_support(context, node_id, &source);
            report
        }
        Source::Edit(mut source) => {
            let mut anchors = context.edit_anchors();
            text::caret_report(
                &mut source,
                &mut anchors.node(node_id.number()),
                &mut super::now_ms,
                formats,
            )
            .map(|(report, _)| report)
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

/// Reads what is new in the focused terminal `node_id`'s text
/// (`crate::terminal`), as `mode` says. `None` when it has no text pattern,
/// is gone, or could not be read.
///
/// Except for a baseline, the caret is read with the text, in the same
/// round trip, and returned as a caret report: a terminal raises no caret
/// event for every character typed (the console host's come on a schedule
/// of their own), so Core's copy of the caret would lag behind typing.
pub(super) fn terminal_output(
    context: &Context,
    uia: &verbatim_uia::Uia,
    node_id: NodeId,
    mode: crate::terminal::ReadMode,
) -> Option<(crate::terminal::Found, Option<CaretReport>)> {
    let Ok(mut source) = uia_source(context, node_id) else {
        return None;
    };
    let window = context.tracking().window();
    let remote = context.tries_remote(window);
    let head_wanted = u32::from(context.terminal_lines());
    let caret =
        (mode != crate::terminal::ReadMode::Baseline).then(|| verbatim_uia_rops::CaretLineQuery {
            element: source.element(),
            pattern: source.pattern(),
            pattern2: source.pattern2(),
            max_text: i32::try_from(crate::text::MAX_CHUNK_UNITS + 1).unwrap_or(i32::MAX),
        });
    let mut terminals = context.terminals();
    let terminal = terminals.entry(node_id.number()).or_default();
    // The console host's `FindText` matches a row's padding, so the
    // anchor's row is sought with it there.
    terminal.matches_padding = console_focus(context, node_id);
    // The caret is stamped as read when the round trip began: a caret event
    // observed while it was in flight may report a move it did not see.
    let started_us = crate::protocol::now_us();
    terminal.read_started_ms = started_us / 1_000;
    let read = crate::terminal::read(
        uia,
        (source.element(), source.pattern()),
        caret,
        terminal,
        (head_wanted, remote, mode),
    );
    drop(terminals);
    match read {
        Ok(answer) => {
            if let Some(Path::Fallback(error)) = &answer.path {
                tracing::warn!(?window, %error, "a remote operation failed; read the classic way");
                if let (verbatim_uia_rops::Error::Import(_), Some(window)) = (error, window) {
                    context.read_classically(window);
                }
            }
            let caret = answer.caret.and_then(|answer| {
                context.caret_read_from(node_id, started_us);
                let read_at_ms = super::now_ms();
                let read = source.caret_read_from(answer).ok()?;
                let mut anchors = context.uia_anchors();
                crate::text::caret_report_from(
                    &mut source,
                    &mut anchors.node(node_id.number()),
                    read,
                    read_at_ms,
                )
                .ok()
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
    for node in released {
        patterns.remove(&node);
        support.remove(&node);
        uia.forget_node(node);
        edit.forget_node(node);
        terminals.remove(&node);
    }
}
