//! The worker's text (milestone M4): which nodes have text and through
//! which backend, Core's text requests answered with [`crate::text`], the
//! caret reports sent as `CaretMoved`, and the caret events a caret key's
//! wait for evidence listens for.
//!
//! A UIA node has text when its element has a text pattern, fetched once
//! per node and kept; an MSAA node has text when it is the client area of a
//! standard edit or rich edit control's window, read through the window's
//! messages. Any other node answers `NoText`, as MSAA has no text interface:
//! Core then reviews its value or name.

#![forbid(unsafe_code)]

use std::sync::{Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use windows::Win32::UI::Accessibility::{IUIAutomationTextPattern, IUIAutomationTextPattern2};
use windows::Win32::UI::WindowsAndMessaging::OBJID_CLIENT;
use windows::core::AgileReference;

use verbatim_ia2::CHILDID_SELF;
use verbatim_model::{CaretReport, NodeId, NodeSnapshot, Role, TextOp, TextReply};
use verbatim_uia::ElementExt;
use verbatim_uia::map::is_terminal_class;

use crate::arbitration::{normalize_class_name, window_class_name};
use crate::text::edit::EditText;
use crate::text::uia::UiaText;
use crate::text::{self, CaretSignal, TextError};

use super::Context;

/// A node's text patterns, kept so each request does not fetch them again;
/// `None` for a node whose element has no text pattern.
pub(super) type Patterns = Option<(
    AgileReference<IUIAutomationTextPattern>,
    Option<AgileReference<IUIAutomationTextPattern2>>,
)>;

/// Caret events, counted as they arrive on the event and UIA callback
/// threads, for a caret key's wait to listen for.
#[derive(Default)]
pub(crate) struct CaretEvents {
    count: Mutex<u64>,
    arrived: Condvar,
}

impl CaretEvents {
    /// Records a caret event. Called on event threads; it never waits.
    pub(crate) fn arrived(&self) {
        *self.count.lock().unwrap_or_else(PoisonError::into_inner) += 1;
        self.arrived.notify_all();
    }

    fn count(&self) -> u64 {
        *self.count.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A caret key's wait, listening for caret events since it began.
struct Signal<'a> {
    context: &'a Context,
    node_id: NodeId,
    since: u64,
}

impl CaretSignal for Signal<'_> {
    fn caret_event(&mut self) -> bool {
        self.context.caret_events.count() > self.since
    }

    fn wait(&mut self, timeout: Duration) {
        let events = &self.context.caret_events;
        let count = events.count.lock().unwrap_or_else(PoisonError::into_inner);
        let since = self.since;
        let _ = events
            .arrived
            .wait_timeout_while(count, timeout, |count| *count <= since);
    }

    fn now(&mut self) -> Instant {
        Instant::now()
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
                Err(_) => None,
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
    Ok(UiaText::new(text_pattern, caret_pattern, terminal))
}

/// Whether `node_id` is the focus and in a console window, the console
/// host's text area.
fn console_focus(context: &Context, node_id: NodeId) -> bool {
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
pub(super) fn may_have_text(context: &Context, node: &NodeSnapshot) -> bool {
    if context.uia_registry.runtime_id_of(node.id).is_some() {
        return matches!(
            node.role,
            Role::EditableText | Role::Document | Role::Terminal
        );
    }
    context
        .msaa_registry
        .key_of(node.id)
        .is_some_and(|(hwnd, object, child)| edit_version(hwnd, object, child).is_some())
}

/// Answers one of Core's text requests.
pub(super) fn answer(context: &Context, node_id: NodeId, op: &TextOp) -> TextReply {
    let source = match source(context, node_id) {
        Ok(source) => source,
        Err(reply) => return reply,
    };
    let mut signal = Signal {
        context,
        node_id,
        since: context.caret_events.count(),
    };
    match source {
        Source::Uia(mut source) => {
            let mut anchors = context.uia_anchors();
            text::perform(
                &mut source,
                &mut anchors.node(node_id.number()),
                op,
                &mut signal,
            )
        }
        Source::Edit(mut source) => {
            let mut anchors = context.edit_anchors();
            text::perform(
                &mut source,
                &mut anchors.node(node_id.number()),
                op,
                &mut signal,
            )
        }
    }
}

/// The caret of `node_id`, for a `CaretMoved` event; `None` when the node
/// has no text or the read failed.
pub(super) fn report_caret(context: &Context, node_id: NodeId) -> Option<CaretReport> {
    let source = source(context, node_id).ok()?;
    context.caret_read(node_id);
    let report = match source {
        Source::Uia(mut source) => {
            let mut anchors = context.uia_anchors();
            text::caret_report(&mut source, &mut anchors.node(node_id.number()))
                .map(|(report, _)| report)
        }
        Source::Edit(mut source) => {
            let mut anchors = context.edit_anchors();
            text::caret_report(&mut source, &mut anchors.node(node_id.number()))
                .map(|(report, _)| report)
        }
    };
    match report {
        Ok(report) => Some(report),
        Err(TextError::Gone) => None,
        Err(TextError::Failed(reason)) => {
            tracing::debug!(reason, "the caret could not be read");
            None
        }
    }
}

/// Forgets what was kept for released nodes.
pub(super) fn forget(context: &Context, released: impl IntoIterator<Item = u64>) {
    let released: Vec<u64> = released.into_iter().collect();
    let mut patterns = context.patterns();
    let mut uia = context.uia_anchors();
    let mut edit = context.edit_anchors();
    for node in released {
        patterns.remove(&node);
        uia.forget_node(node);
        edit.forget_node(node);
    }
}
