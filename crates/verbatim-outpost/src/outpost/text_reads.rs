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
//!
//! A program's text calls work on a copy of the node's text facts from its
//! own copies of the reducer's facts, with clones of the UIA ranges they
//! name, taken from the store in one hold of its lock. What a call changes
//! is applied to the program's copies at once and carried, with the ranges
//! it made and the patterns and support it learned, to the program's next
//! commit ([`ProgramText`]); the shell writes them to the store only when
//! the reducer accepts that commit.

#![forbid(unsafe_code)]

use std::collections::{BTreeSet, HashMap};

use windows::Win32::UI::Accessibility::{IUIAutomationTextPattern, IUIAutomationTextPattern2};
use windows::Win32::UI::WindowsAndMessaging::OBJID_CLIENT;
use windows::core::AgileReference;

use verbatim_ia2::CHILDID_SELF;
use verbatim_model::{
    Backend, CaretReply, CaretReport, CaretWatch, NodeId, Role, SelectionChange, TextOp, TextReply,
};
use verbatim_uia::map::is_terminal_class;
use verbatim_uia::{Cached, Interning as _, Kept};
use verbatim_uia_rops::Path;

use crate::arbitration::{normalize_class_name, window_class_name};
use crate::text::edit::EditText;
use crate::text::uia::{TextSupport, UiaPos, UiaText};
use crate::text::{
    self, CaretSignal, Checked, LastSelection, NodeTextCopy, Slot, TextError, Watched,
};

use super::commit::Effects;
use super::reducer::{AnswerCut, NodeAnchors, RangeKey, ReportCut, TextChange};
use super::store::TextWrite;
use super::{Context, RequestContext};

/// A node's text patterns, kept so each request does not fetch them again;
/// `None` for a node whose element has no text pattern.
pub(super) type Patterns = Option<(
    AgileReference<IUIAutomationTextPattern>,
    Option<AgileReference<IUIAutomationTextPattern2>>,
)>;

/// What a program holds of the store's text while it runs, and its text
/// writes not yet committed. Its reads see its own writes first: the UIA
/// ranges it made or cloned, by key, and the patterns and support it
/// learned or forgot, by node; anything else is read from the store.
#[derive(Default)]
pub(super) struct ProgramText {
    ranges: HashMap<RangeKey, UiaPos>,
    /// A node's patterns as the program left them: `None` for none kept.
    patterns: HashMap<u64, Option<Patterns>>,
    support: HashMap<u64, TextSupport>,
    /// Its writes since its last commit, in order.
    pending: Vec<TextWrite>,
}

impl ProgramText {
    /// `effects`, a commit's, with the program's text writes since its
    /// last commit ahead of any of their own, which the commit carries.
    pub(super) fn seal(&mut self, effects: Effects) -> Effects {
        let mut effects = effects;
        let mut text = std::mem::take(&mut self.pending);
        text.append(&mut effects.text);
        effects.text = text;
        effects
    }

    /// Carries `write` to the program's next commit.
    pub(super) fn write(&mut self, write: TextWrite) {
        self.pending.push(write);
    }

    /// Whether it has writes no commit carried yet.
    pub(super) fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }
}

/// A text call's copy of one node's text facts, with the part of the
/// program's facts it copied and the anchors it held, to tell what the
/// call changed.
struct TextCall<P, K> {
    copy: NodeTextCopy<P>,
    part: NodeAnchors<K>,
    copied: BTreeSet<u64>,
}

impl<P: Clone, K> TextCall<P, K> {
    /// The copy, as the call reads and changes it.
    fn text(&mut self) -> text::NodeText<'_, P> {
        self.copy.text()
    }
}

/// A copy of `node_id`'s UIA text facts for a text call: the part its
/// operation names (`named`), as the program's copies hold it, its ranges
/// cloned from the program's own or, in one hold, from the store.
fn uia_call(
    context: &RequestContext<'_>,
    node_id: NodeId,
    named: &[u64],
) -> TextCall<UiaPos, RangeKey> {
    let (part, copied) = context
        .facts()
        .text
        .uia(node_id.number())
        .map(|anchors| anchors.part(named))
        .unwrap_or_default();
    let mut program_text = context.program_text.borrow_mut();
    let missing: Vec<RangeKey> = part
        .positions()
        .copied()
        .filter(|key| !program_text.ranges.contains_key(key))
        .collect();
    if !missing.is_empty() {
        program_text
            .ranges
            .extend(context.store.ranges(node_id, &missing));
    }
    let anchors = part.clone().filter_map(|key| {
        Some(Slot {
            at: program_text.ranges.get(&key)?.clone(),
            key: Some(key),
        })
    });
    TextCall {
        copy: NodeTextCopy::new(context.store.anchor_numbers(), anchors),
        part,
        copied,
    }
}

/// A copy of edit control `node_id`'s text facts for a text call, as
/// [`uia_call`] makes one: its positions are offsets.
fn edit_call(context: &RequestContext<'_>, node_id: NodeId, named: &[u64]) -> TextCall<u32, u32> {
    let (part, copied) = context
        .facts()
        .text
        .edit(node_id.number())
        .map(|anchors| anchors.part(named))
        .unwrap_or_default();
    let anchors = part.clone().filter_map(|at| Some(Slot { at, key: None }));
    TextCall {
        copy: NodeTextCopy::new(context.store.anchor_numbers(), anchors),
        part,
        copied,
    }
}

/// Takes in what `call`, a text call on `node_id`'s UIA text, changed:
/// each position it made is given a key, and the change is applied to the
/// program's copies and carried to its next commit, with the ranges made
/// and the anchors it minted or reported. A call that changed nothing
/// writes nothing.
fn put_uia(context: &RequestContext<'_>, node_id: NodeId, call: TextCall<UiaPos, RangeKey>) {
    let TextCall {
        copy,
        part: before,
        copied,
    } = call;
    let mut made = Vec::new();
    let (part, touched) = copy.finish(|slot| {
        slot.key.unwrap_or_else(|| {
            let key = context.store.next_range_key();
            made.push((key, slot.at));
            key
        })
    });
    if part == before && touched.is_empty() {
        return;
    }
    {
        let mut program_text = context.program_text.borrow_mut();
        for (key, range) in &made {
            program_text.ranges.insert(*key, range.clone());
        }
    }
    let change = TextChange::Uia {
        node: node_id.number(),
        part,
        copied,
    };
    record(context, change, (made, touched));
}

/// [`put_uia`] for an edit control's text, whose positions are offsets.
fn put_edit(context: &RequestContext<'_>, node_id: NodeId, call: TextCall<u32, u32>) {
    let TextCall {
        copy,
        part: before,
        copied,
    } = call;
    let (part, touched) = copy.finish(|slot| slot.at);
    if part == before && touched.is_empty() {
        return;
    }
    let change = TextChange::Edit {
        node: node_id.number(),
        part,
        copied,
    };
    record(context, change, (Vec::new(), touched));
}

/// Applies `change` to the program's copies and carries it to the
/// program's next commit, with the UIA ranges it names that it made and
/// the anchors it minted or reported.
fn record(
    context: &RequestContext<'_>,
    change: TextChange,
    (ranges, touched): (Vec<(RangeKey, UiaPos)>, Vec<u64>),
) {
    context.apply_text(&change);
    context.program_text.borrow_mut().write(TextWrite::Facts {
        change,
        ranges,
        touched,
    });
}

/// The text patterns kept for `node_id`, as the program sees them: `None`
/// when none were fetched, `Some(None)` when its element answered it has
/// none.
fn patterns_of(context: &RequestContext<'_>, node_id: NodeId) -> Option<Patterns> {
    if let Some(patterns) = context
        .program_text
        .borrow()
        .patterns
        .get(&node_id.number())
    {
        return patterns.clone();
    }
    context.store.patterns(node_id)
}

/// What `node_id`'s text is known to support, as the program sees it.
fn support_of(context: &RequestContext<'_>, node_id: NodeId) -> TextSupport {
    if let Some(support) = context.program_text.borrow().support.get(&node_id.number()) {
        return *support;
    }
    context.store.support(node_id)
}

/// A check of a caret key's watch, as the worker makes it.
struct Signal<'a> {
    context: &'a RequestContext<'a>,
    node_id: NodeId,
    /// Whether a caret event prompted the check.
    caret_event: bool,
    /// Whether a caret event of the node observed since the key was
    /// pressed waited to be handled when the caret was read: the read
    /// covers it, so the event, once handled, checks nothing
    /// (`Context::caret_read_since`), and the read is evidence as the
    /// event would have been.
    covered: bool,
    /// When the caret read began, once it has, on the observation clock:
    /// committed with what the check sends.
    read_at: Option<u64>,
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
            self.covered = caret_event_waiting(self.context, self.node_id);
        }
        self.read_at = Some(self.context.observed_now());
    }
}

/// Whether a caret event of `node_id` observed at or after the key the
/// watch checked was pressed waits in the intake, as the program's copy
/// says ([`super::reducer::Program::waiting`]): UIA's caret event from the
/// node's element, the console's caret event of its window for the console
/// host's text area, or, for an edit control's client area, the caret's
/// location change or the text selection change of its window.
fn caret_event_waiting(context: &RequestContext<'_>, node_id: NodeId) -> bool {
    use super::reducer::WaitingCaret;
    let runtime_id = context.store.runtime_id_of(node_id);
    let (console, edit) = match &runtime_id {
        Some(_) => (
            console_focus(context, node_id)
                .then(|| context.tracked_window())
                .flatten(),
            None,
        ),
        None => (
            None,
            context
                .store
                .key_of(node_id)
                .filter(|&(_, object, child)| object == OBJID_CLIENT.0 && child == CHILDID_SELF)
                .map(|(hwnd, ..)| hwnd),
        ),
    };
    context.waiting().iter().any(|waiting| match waiting {
        WaitingCaret::Uia(id) => runtime_id.as_ref() == Some(id),
        WaitingCaret::Console(hwnd) => console == Some(*hwnd),
        WaitingCaret::Edit(hwnd) => edit == Some(*hwnd),
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
fn source(context: &RequestContext<'_>, node_id: NodeId) -> Result<Source, TextReply> {
    if context.store.runtime_id_of(node_id).is_some() {
        return uia_source(context, node_id).map(Source::Uia);
    }
    let Some((hwnd, object, child)) = context.store.key_of(node_id) else {
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
fn uia_source(context: &RequestContext<'_>, node_id: NodeId) -> Result<UiaText, TextReply> {
    let element = context
        .store
        .element_of(node_id)
        .and_then(|agile| agile.resolve().ok())
        .ok_or(TextReply::Gone)?;
    let kept = patterns_of(context, node_id);
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
            // Kept for the program's next commit to write: what it replaces
            // is dropped by a worker, outside the store's lock.
            let mut program_text = context.program_text.borrow_mut();
            program_text
                .patterns
                .insert(node_id.number(), Some(fetched.clone()));
            program_text.write(TextWrite::Patterns(node_id, fetched.clone()));
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
    let remote = context.tries_remote(context.tracked_window());
    let support = support_of(context, node_id);
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
pub(super) fn forget_no_text(context: &RequestContext<'_>, node_id: NodeId) -> bool {
    let forgotten = patterns_of(context, node_id).is_some_and(|patterns| patterns.is_none());
    if forgotten {
        let mut program_text = context.program_text.borrow_mut();
        program_text.patterns.insert(node_id.number(), None);
        program_text.write(TextWrite::NoTextForgotten(node_id));
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
    context: &RequestContext<'_>,
    node_id: NodeId,
    element: Option<&Cached<Kept>>,
) {
    if !forget_no_text(context, node_id) {
        return;
    }
    if let (Some(element), Some(runtime_id)) = (element, context.store.runtime_id_of(node_id)) {
        // Which element is replaced, by which: each one's runtime id, which
        // UIA keeps with the element, and its class name, which `Kept`
        // holds, so nothing is read beyond the cache (decision 7 of
        // `docs/design/uia-shapes.md`).
        let describe = |element: &Cached<Kept>| (element.runtime_id(), element.read().class_name);
        let replaced = context
            .store
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
        let _ = context.nodes().id_for_element(&runtime_id, element);
    }
}

/// Keeps what `source`, `node_id`'s text, is now known to support, for the
/// node's next read.
fn keep_support(context: &RequestContext<'_>, node_id: NodeId, source: &UiaText) {
    let support = source.known_support();
    if support == support_of(context, node_id) {
        return;
    }
    let mut program_text = context.program_text.borrow_mut();
    program_text.support.insert(node_id.number(), support);
    program_text.write(TextWrite::Support(node_id, support));
}

/// Forgets `node_id`'s text, for a node whose runtime id names a new
/// element: its patterns, what it supports, and its text facts in both
/// backends, in the program's copies now and in the store and the reducer
/// with its next commit.
pub(super) fn forget_text(context: &RequestContext<'_>, node_id: NodeId) {
    {
        let mut program_text = context.program_text.borrow_mut();
        program_text.patterns.insert(node_id.number(), None);
        program_text
            .support
            .insert(node_id.number(), TextSupport::default());
        program_text.write(TextWrite::Forget(node_id));
    }
    record(
        context,
        TextChange::Forget(node_id.number()),
        (Vec::new(), Vec::new()),
    );
}

/// Forgets the text facts of `gone`, the nodes of a destroyed window the
/// store freed with everything it kept for them.
pub(super) fn forget_nodes(context: &RequestContext<'_>, gone: &[NodeId]) {
    let with_text: Vec<NodeId> = {
        let facts = context.facts();
        gone.iter()
            .copied()
            .filter(|node| {
                facts.text.uia(node.number()).is_some() || facts.text.edit(node.number()).is_some()
            })
            .collect()
    };
    for node in with_text {
        record(
            context,
            TextChange::Forget(node.number()),
            (Vec::new(), Vec::new()),
        );
    }
}

/// Logs a caret read that fell back from its remote operation to the
/// classic reads, and stops trying remote operations in the window when the
/// import failed, as the focus walk and the terminal read do.
fn note_fallback(context: &RequestContext<'_>, source: &mut UiaText) {
    let Some(error) = source.take_fallback() else {
        return;
    };
    let window = context.tracked_window();
    tracing::warn!(?window, %error, "a remote operation failed; read the classic way");
    if let (verbatim_uia_rops::Error::Import(_), Some(window)) = (&error, window) {
        context.read_classically(window);
    }
}

/// Whether `node_id` is the focus and in a console window, the console
/// host's text area.
pub(super) fn console_focus(context: &RequestContext<'_>, node_id: NodeId) -> bool {
    let focus_is_node = context
        .store
        .runtime_id_of(node_id)
        .is_some_and(|runtime_id| {
            context.focused() == Some(super::reducer::Object::Uia(runtime_id))
        });
    focus_is_node
        && context
            .tracked_window()
            .is_some_and(|hwnd| window_class_name(hwnd) == CONSOLE_WINDOW_CLASS)
}

/// The console host's window class.
pub(crate) const CONSOLE_WINDOW_CLASS: &str = "ConsoleWindowClass";

/// Whether a focused node may have text, and so gets a caret report:
/// through UIA, an edit field, a document, or a terminal (whose text
/// pattern is then asked for); through MSAA, an edit control's client area.
pub(super) fn may_have_text(context: &Context, node_id: NodeId, role: Role) -> bool {
    if context.store.runtime_id_of(node_id).is_some() {
        return matches!(role, Role::EditableText | Role::Document | Role::Terminal);
    }
    context
        .store
        .key_of(node_id)
        .is_some_and(|(hwnd, object, child)| edit_version(hwnd, object, child).is_some())
}

/// Answers one of Core's text requests other than a caret key's watch,
/// which [`check_watch`] checks.
pub(super) fn answer(context: &RequestContext<'_>, node_id: NodeId, op: &TextOp) -> TextReply {
    if *op == TextOp::ReadCaret {
        return caret_before_key(context, node_id);
    }
    let source = match source(context, node_id) {
        Ok(source) => source,
        Err(reply) => return reply,
    };
    let named = text::named_anchors(op);
    match source {
        Source::Uia(mut source) => {
            let mut call = uia_call(context, node_id, &named);
            let reply = text::perform(&mut source, &mut call.text(), op);
            put_uia(context, node_id, call);
            note_fallback(context, &mut source);
            keep_support(context, node_id, &source);
            reply
        }
        Source::Edit(mut source) => {
            let mut call = edit_call(context, node_id, &named);
            let reply = text::perform(&mut source, &mut call.text(), op);
            put_edit(context, node_id, call);
            reply
        }
    }
}

/// Reads `node_id`'s caret before a held caret key is passed on
/// ([`TextOp::ReadCaret`], `docs/design/key-holding.md`): the read a caret
/// event's report makes, one round trip through UIA where the window's
/// provider runs remote operations (the caret read program), and the edit
/// control's messages otherwise, with no formatting. It is kept among the
/// node's caret reports, from which the key's watch takes the newest read
/// before the key as its baseline, and leaves the selection kept for the
/// next comparison alone, so the key's answer compares with the same
/// selection it would have without the read. It is not noted as a read of
/// the caret that answers a caret event observed before it: that event's
/// report still compares the selection.
fn caret_before_key(context: &RequestContext<'_>, node_id: NodeId) -> TextReply {
    let source = match source(context, node_id) {
        Ok(source) => source,
        Err(reply) => return reply,
    };
    let read_at_ms = std::cell::Cell::new(0);
    let mut now = || {
        let now = super::now_ms();
        read_at_ms.set(now);
        now
    };
    match source {
        Source::Uia(mut source) => {
            let mut call = uia_call(context, node_id, &[]);
            let read = text::caret_report_since(
                &mut source,
                &mut call.text(),
                &mut now,
                (false, None, LastSelection::Ignore),
            );
            put_uia(context, node_id, call);
            note_fallback(context, &mut source);
            keep_support(context, node_id, &source);
            before_reply(read, read_at_ms.get())
        }
        Source::Edit(mut source) => {
            let mut call = edit_call(context, node_id, &[]);
            let read = text::caret_report_since(
                &mut source,
                &mut call.text(),
                &mut now,
                (false, None, LastSelection::Ignore),
            );
            put_edit(context, node_id, call);
            before_reply(read, read_at_ms.get())
        }
    }
}

/// The answer to [`TextOp::ReadCaret`] from its read, which ended at
/// `read_at_ms`.
fn before_reply<P>(read: Result<text::ReportedCaret<P>, TextError>, read_at_ms: u64) -> TextReply {
    match read {
        Ok(read) => TextReply::Caret(Box::new(CaretReply {
            moved: false,
            caret: read.report,
            read_at_ms,
            unit: None,
            selection_changes: Vec::new(),
            content_changed: false,
            new_selection: None,
            same_line: None,
            removed: None,
        })),
        Err(TextError::Gone) => TextReply::Gone,
        Err(TextError::Failed(reason)) => {
            tracing::debug!(reason, "the caret before a held key could not be read");
            TextReply::Unanswered
        }
    }
}

/// Checks a caret key's watch on `node_id` with one read of its caret
/// ([`text::check_caret`]): the answer once there is evidence, or the reply
/// that ends the request when the node has no text or is gone; otherwise
/// the watch stays open. `caret_event` says whether a caret event prompted
/// the check. With `cuts`, the check of an edit control's client area is
/// cut where its answer would ask whether the control's value change waits
/// ([`value_change_cut`]), and the cut is returned with the control's
/// window instead. Also when the caret read began, if it was read, for the
/// commit of what the check sends.
pub(super) fn check_watch(
    context: &RequestContext<'_>,
    node_id: NodeId,
    watch: &CaretWatch,
    (caret_event, cuts): (bool, bool),
) -> (WatchCheck, Option<u64>) {
    let source = match source(context, node_id) {
        Ok(source) => source,
        // A UIA element that answered no text pattern, or did not answer:
        // an application still starting answers so for an element that has
        // text, and raises a caret event once the key moves its caret, which
        // only an element with text does; the watch stays open for it.
        Err(TextReply::NoText | TextReply::Unanswered)
            if context.store.runtime_id_of(node_id).is_some() =>
        {
            return (Checked::Done(Watched::Watching), None);
        }
        Err(reply) => return (Checked::Done(Watched::Answered(reply)), None),
    };
    let mut signal = Signal {
        context,
        node_id,
        caret_event,
        covered: false,
        read_at: None,
    };
    let named: Vec<u64> = watch.since.iter().map(|since| since.anchor.0).collect();
    let checked = match source {
        Source::Uia(mut source) => {
            let mut call = uia_call(context, node_id, &named);
            let watched = text::check_caret(&mut source, &mut call.text(), watch, &mut signal);
            put_uia(context, node_id, call);
            note_fallback(context, &mut source);
            keep_support(context, node_id, &source);
            Checked::Done(watched)
        }
        Source::Edit(mut source) => {
            let edit = edit_client(context, node_id).filter(|_| cuts);
            let mut call = edit_call(context, node_id, &named);
            let checked = text::check_caret_cut(
                &mut source,
                &mut call.text(),
                watch,
                (&mut signal, edit.is_some()),
            );
            let checked =
                match (checked, edit) {
                    (Checked::Cut(read), Some(edit)) => Checked::Cut(EditCut {
                        edit,
                        length: source.length_read(),
                        read,
                    }),
                    (Checked::Cut(cut), None) => Checked::Done(Watched::Answered(
                        text::finish_answer(&mut source, &mut call.text(), cut, false),
                    )),
                    (Checked::Done(watched), _) => Checked::Done(watched),
                };
            put_edit(context, node_id, call);
            if matches!(checked, Checked::Cut(_)) {
                value_change_cut(context);
            }
            checked
        }
    };
    (checked, signal.read_at)
}

/// [`check_watch`], never cut: the console poll's check, of a UIA text.
pub(super) fn check_watch_uncut(
    context: &RequestContext<'_>,
    node_id: NodeId,
    watch: &CaretWatch,
) -> (Watched, Option<u64>) {
    match check_watch(context, node_id, watch, (false, false)) {
        (Checked::Done(watched), read_at) => (watched, read_at),
        (Checked::Cut(cut), read_at) => (
            Watched::Answered(finish_watch(
                context,
                node_id,
                (cut.length, cut.read),
                false,
            )),
            read_at,
        ),
    }
}

/// A check of a caret key's watch ([`check_watch`]): what it found, or
/// the check cut at its value change check.
pub(super) type WatchCheck = Checked<Watched, EditCut<AnswerCut<u32>>>;

/// A read of an edit control's client area cut at its value change check
/// ([`value_change_cut`]): the control's window, the text's length if the
/// read part read it, and what the read part found.
pub(super) struct EditCut<C> {
    pub(super) edit: isize,
    pub(super) length: Option<u32>,
    pub(super) read: C,
}

/// The edit control `node_id`'s text, for the part that finishes a read
/// cut at its value change check, with the text's length the read part
/// read: the node is the client area of an edit control, as when the read
/// part read it, unless it is gone.
fn edit_source(
    context: &RequestContext<'_>,
    node_id: NodeId,
    length: Option<u32>,
) -> Result<EditText, TextReply> {
    match source(context, node_id)? {
        Source::Edit(source) => Ok(source.knowing_length(length)),
        Source::Uia(_) => Err(TextReply::NoText),
    }
}

/// Finishes the check of a caret key's watch on edit control `node_id` that
/// [`check_watch`] cut: the answer, whose comparison of selections is in
/// general terms when the control's value change waited (`waiting`), or
/// the reply that ends the request when the text cannot be read.
pub(super) fn finish_watch(
    context: &RequestContext<'_>,
    node_id: NodeId,
    (length, cut): (Option<u32>, AnswerCut<u32>),
    waiting: bool,
) -> TextReply {
    let mut source = match edit_source(context, node_id, length) {
        Ok(source) => source,
        Err(reply) => return reply,
    };
    let mut call = edit_call(context, node_id, &[]);
    let watched = text::finish_answer(&mut source, &mut call.text(), cut, waiting);
    put_edit(context, node_id, call);
    watched
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

/// How the selection changed with a caret event's report ([`report_caret`]),
/// as [`NormalizedEvent::CaretMoved`](verbatim_model::NormalizedEvent::CaretMoved)
/// carries it.
#[derive(Default)]
pub(super) struct SelectionReport {
    /// The changes from the selection last reported.
    pub(super) changes: Vec<SelectionChange>,
    /// The node's text changed since that selection was reported, so the
    /// changes carry no text.
    pub(super) content_changed: bool,
    /// With `content_changed`, the whole new selection's text.
    pub(super) new_selection: Option<SelectionChange>,
}

/// What [`report_caret`] found, or the read cut at its value change check,
/// and when its read began, if it was made.
pub(super) type CaretReading = (
    Result<Checked<(CaretReport, bool, SelectionReport), EditCut<ReportCut<u32>>>, NoCaret>,
    Option<u64>,
);

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
/// place, the read says, with the same line. Also when the read began, on
/// the observation clock, when it was made: it covers the caret events
/// observed before then only once its commit notes it.
pub(super) fn report_caret(
    context: &RequestContext<'_>,
    node_id: NodeId,
    purpose: CaretFor,
) -> CaretReading {
    let source = match source(context, node_id) {
        Ok(source) => source,
        Err(reply) => {
            if purpose == CaretFor::Focus {
                // A new focus's selection is not compared with one from
                // before it, though its own could not be read.
                forget_selection(context, node_id, Backend::Uia);
                forget_selection(context, node_id, Backend::Msaa);
            }
            let missed = if reply == TextReply::NoText {
                NoCaret::NoText
            } else {
                NoCaret::NotRead
            };
            return (Err(missed), None);
        }
    };
    let sent = if purpose == CaretFor::Terminal {
        let sent = context
            .store
            .terminal(node_id)
            .and_then(|terminal| terminal.sent_caret);
        sent.and_then(|sent| sent_caret_point(context, node_id, sent))
    } else {
        None
    };
    let last = match purpose {
        CaretFor::Focus => LastSelection::Restart,
        CaretFor::Terminal => LastSelection::Ignore,
        CaretFor::Event => LastSelection::Compare,
    };
    let formats = purpose == CaretFor::Focus;
    let read_at = Some(context.observed_now());
    let report = match source {
        Source::Uia(mut source) => {
            let mut call = uia_call(context, node_id, &[]);
            let report = text::caret_report_since(
                &mut source,
                &mut call.text(),
                &mut super::now_ms,
                (formats, sent.as_ref().map(|(_, point)| point), last),
            )
            .map(|read| {
                let unchanged = !read.moved
                    && sent
                        .as_ref()
                        .is_some_and(|(sent, _)| text::same_caret(sent, &read.report));
                let (report, selection) = selection_report(read);
                Checked::Done((report, unchanged, selection))
            });
            put_uia(context, node_id, call);
            note_fallback(context, &mut source);
            keep_support(context, node_id, &source);
            report
        }
        Source::Edit(mut source) => {
            // An edit control's value change waiting behind this caret
            // event: the text changed since the selection was last
            // reported, so the change is spoken in general terms, as after
            // a value change handled first. NVDA's edit control notes the
            // content changed when a value change for it is pending at its
            // caret event (`Edit.event_caret`,
            // `nvda/source/NVDAObjects/window/edit.py` 1068 to 1084). The
            // check is made once the caret is read, behind the `WinEvent`
            // thread's queue, by the reducer, the read cut there
            // ([`value_change_cut`]): the control may raise the change while
            // it answers the read, as the Windows Forms text box raises a
            // typed character's after its caret event. A UIA text change is
            // never noted: NVDA handles UIA's text changed event as a text
            // change only for Word and the two terminals
            // (`nvda/source/UIAHandler/__init__.py` 145 to 151 and 778 to
            // 799), so a UIA text's comparison always reads the old
            // selection's range as it now is.
            let edit = edit_client(context, node_id).filter(|_| purpose == CaretFor::Event);
            let mut call = edit_call(context, node_id, &[]);
            let report = text::caret_report_checking(
                &mut source,
                &mut call.text(),
                &mut super::now_ms,
                (formats, None, last),
                edit.is_some(),
            )
            .and_then(|checked| match (checked, edit) {
                (Checked::Cut(read), Some(edit)) => Ok(Checked::Cut(EditCut {
                    edit,
                    length: source.length_read(),
                    read,
                })),
                (Checked::Cut(read), None) => {
                    text::finish_report(&mut source, &mut call.text(), read, false)
                        .map(Checked::Done)
                }
                (Checked::Done(read), _) => Ok(Checked::Done(read)),
            })
            .map(|checked| match checked {
                Checked::Done(read) => {
                    let (report, selection) = selection_report(read);
                    Checked::Done((report, false, selection))
                }
                Checked::Cut(cut) => Checked::Cut(cut),
            });
            put_edit(context, node_id, call);
            if matches!(report, Ok(Checked::Cut(_))) {
                value_change_cut(context);
            }
            report
        }
    };
    (report.map_err(not_read), read_at)
}

/// A caret read's report, and its selection changes as a caret event's
/// report carries them.
fn selection_report<P>(read: text::ReportedCaret<P>) -> (CaretReport, SelectionReport) {
    (
        read.report,
        SelectionReport {
            changes: read.selection_changes,
            content_changed: read.content_changed,
            new_selection: read.new_selection,
        },
    )
}

/// Forgets the selection `node_id`, read through `backend`, would have its
/// next caret report compared with ([`text::NodeText::forget_selection`]),
/// at a new focus: the report carries no change.
pub(super) fn forget_selection(context: &RequestContext<'_>, node_id: NodeId, backend: Backend) {
    let node = node_id.number();
    let uia = backend == Backend::Uia;
    let kept = {
        let facts = context.facts();
        if uia {
            facts
                .text
                .uia(node)
                .is_some_and(|anchors| anchors.selection.is_some())
        } else {
            facts
                .text
                .edit(node)
                .is_some_and(|anchors| anchors.selection.is_some())
        }
    };
    if kept {
        record(
            context,
            TextChange::ForgetSelection { node, uia },
            (Vec::new(), Vec::new()),
        );
    }
}

/// Notes that the text of edit control `node_id` changed, its value change
/// handled, so its next comparison of selections is made in general terms
/// ([`TextChange::ContentChanged`]): NVDA's
/// `IAccessible.event_valueChange` for an object that detects its own
/// selection changes (`nvda/source/NVDAObjects/IAccessible/__init__.py`
/// 2019 to 2023).
pub(super) fn note_value_change(context: &RequestContext<'_>, node_id: NodeId) {
    record(
        context,
        TextChange::ContentChanged(node_id.number()),
        (Vec::new(), Vec::new()),
    );
}

/// The window of the edit control whose client area `node_id` is, if it is
/// one: the node whose caret reads are cut at their value change check
/// ([`value_change_cut`]). A UIA text's change is never checked for.
fn edit_client(context: &RequestContext<'_>, node_id: NodeId) -> Option<isize> {
    if context.store.runtime_id_of(node_id).is_some() {
        return None;
    }
    context
        .store
        .key_of(node_id)
        .filter(|&(_, object, child)| object == OBJID_CLIENT.0 && child == CHILDID_SELF)
        .map(|(hwnd, ..)| hwnd)
}

/// Ends the read part of a caret read of an edit control's client area
/// cut at its value change check (`docs/design/ownership.md`): waits for the `WinEvent` thread to take
/// every event raised before now (`Nudger::sync`), up to two wakes of that
/// thread and no call to the application, since the control may have
/// raised its value change while it answered the read just made. Every
/// such value change is then ahead of the read part's end in the mailbox,
/// where the reducer looks for it among its entries not yet dispatched.
fn value_change_cut(context: &RequestContext<'_>) {
    if let Some(nudger) = context.event_thread.get() {
        nudger.sync();
    }
}

/// Finishes the caret event's report of edit control `node_id` that
/// [`report_caret`] cut: the comparison, in general terms when the
/// control's value change waited (`waiting`), and the report.
pub(super) fn finish_caret(
    context: &RequestContext<'_>,
    node_id: NodeId,
    (length, cut): (Option<u32>, ReportCut<u32>),
    waiting: bool,
) -> Result<(CaretReport, SelectionReport), NoCaret> {
    let mut source = match edit_source(context, node_id, length) {
        Ok(source) => source,
        Err(TextReply::NoText) => return Err(NoCaret::NoText),
        Err(_) => return Err(NoCaret::NotRead),
    };
    let mut call = edit_call(context, node_id, &[]);
    let report = text::finish_report(&mut source, &mut call.text(), cut, waiting);
    put_edit(context, node_id, call);
    match report {
        Ok(read) => Ok(selection_report(read)),
        Err(error) => Err(not_read(error)),
    }
}

/// Why a caret read that failed with `error` reports nothing.
fn not_read(error: TextError) -> NoCaret {
    if let TextError::Failed(reason) = error {
        tracing::debug!(reason, "the caret could not be read");
    }
    NoCaret::NotRead
}

/// A terminal's caret as a read found it ([`terminal_read`]).
pub(super) struct TerminalCaret {
    /// The caret.
    pub(super) report: CaretReport,
}

/// `sent`, the caret last sent to Core for terminal `node_id`, with where
/// it was, when the outpost still knows that without a call: what a caret
/// read compares with.
fn sent_caret_point(
    context: &RequestContext<'_>,
    node_id: NodeId,
    sent: CaretReport,
) -> Option<(CaretReport, UiaPos)> {
    let key = context
        .facts()
        .text
        .uia(node_id.number())?
        .reported
        .get(&(sent.line.start.0, sent.line.offset))
        .copied()?;
    let mut program_text = context.program_text.borrow_mut();
    if !program_text.ranges.contains_key(&key) {
        program_text
            .ranges
            .extend(context.store.ranges(node_id, &[key]));
    }
    let point = program_text.ranges.get(&key)?.clone();
    Some((sent, point))
}

/// Reads the focused terminal `node_id` (`crate::terminal::read`), the read
/// of `scope`, with the caret and its line read in the same round trip and
/// returned as a caret report: a terminal raises no caret event for every
/// character typed (the console host's come on a schedule of their own), so
/// Core's copy of the caret would lag behind typing. The same round trip
/// says whether the caret is still where the caret last sent to Core was.
/// `waited` says whether the read waited for Core's speech queue. The read
/// works on `terminal`, a copy of the terminal's entry, which its commit
/// puts in place, with no lock held across it. `None` when the terminal has
/// no text pattern, is gone, or could not be read; otherwise also when the
/// caret was read, for the commit to note.
pub(super) fn terminal_read(
    context: &RequestContext<'_>,
    uia: &verbatim_uia::Uia,
    node_id: NodeId,
    terminal: &mut crate::terminal::Terminal,
    (scope, waited): (Option<verbatim_model::TerminalScope>, bool),
) -> Option<(
    crate::terminal::ReadAnswer,
    Option<TerminalCaret>,
    Option<u64>,
)> {
    let Ok(mut source) = uia_source(context, node_id) else {
        return None;
    };
    let window = context.tracked_window();
    let remote = context.tries_remote(window);
    let sent = terminal
        .sent_caret
        .clone()
        .and_then(|sent| sent_caret_point(context, node_id, sent));
    let since = match &sent {
        Some((_, point)) => point.range_end().ok(),
        None => None,
    };
    let caret = verbatim_uia_rops::CaretLineQuery {
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
    };
    // The caret is stamped as read when the round trip began: a caret event
    // observed while it was in flight may report a move it did not see.
    let started_us = crate::protocol::now_us();
    let read = crate::terminal::read(
        uia,
        (source.element(), source.pattern()),
        caret,
        terminal,
        (scope, remote, waited),
    );
    match read {
        Ok(mut answer) => {
            if let Path::Fallback(error) = &answer.path {
                tracing::warn!(?window, %error, "a remote operation failed; read the classic way");
                if let (verbatim_uia_rops::Error::Import(_), Some(window)) = (error, window) {
                    context.read_classically(window);
                }
            }
            let caret_read = answer.caret.is_some().then_some(started_us);
            let caret = answer.caret.take().and_then(|answer| {
                let read_at_ms = super::now_ms();
                let read = source.caret_read_from(answer).ok()?;
                let mut call = uia_call(context, node_id, &[]);
                let report =
                    crate::text::caret_report_from(&mut source, &mut call.text(), read, read_at_ms);
                put_uia(context, node_id, call);
                Some(TerminalCaret {
                    report: report.ok()?,
                })
            });
            Some((answer, caret, caret_read))
        }
        Err(TextError::Gone) => None,
        Err(TextError::Failed(reason)) => {
            tracing::debug!(reason, "a terminal's text could not be read");
            None
        }
    }
}

/// Reads the history of the focused terminal `node_id` above where its
/// next read starts, for a review command Core answers from its memory
/// ([`verbatim_model::TextOp::TerminalHistory`]): from `above` rows above
/// that start to the document's end, or the whole document when `whole`.
pub(super) fn terminal_history(
    context: &RequestContext<'_>,
    uia: &verbatim_uia::Uia,
    (node_id, terminal): (NodeId, Option<crate::terminal::Terminal>),
    (above, whole): (u32, bool),
) -> TextReply {
    let Ok(source) = uia_source(context, node_id) else {
        return TextReply::Unanswered;
    };
    let remote = context.tries_remote(context.tracked_window());
    // A copy of the terminal's entry, so no lock is held across the read.
    let Some(terminal) = terminal else {
        return TextReply::Unanswered;
    };
    match crate::terminal::history(
        uia,
        (source.element(), source.pattern()),
        &terminal,
        (above, whole, remote),
    ) {
        Ok(history) => TextReply::TerminalHistory {
            text: history.text,
            above: history.above,
            read: history.read,
        },
        Err(TextError::Gone) => TextReply::Gone,
        Err(TextError::Failed(reason)) => {
            tracing::debug!(reason, "a terminal's history could not be read");
            TextReply::Unanswered
        }
    }
}

/// Searches the focused terminal `node_id` for a command mark's line, for
/// Core's command ([`verbatim_model::TextOp::TerminalFind`]).
pub(super) fn terminal_find(
    context: &RequestContext<'_>,
    uia: &verbatim_uia::Uia,
    (node_id, terminal): (NodeId, Option<crate::terminal::Terminal>),
    (needle, from, rows): (&str, i32, u32),
) -> TextReply {
    let Ok(source) = uia_source(context, node_id) else {
        return TextReply::Unanswered;
    };
    let remote = context.tries_remote(context.tracked_window());
    // A copy of the terminal's entry, so no lock is held across the read.
    let Some(terminal) = terminal else {
        return TextReply::Unanswered;
    };
    match crate::terminal::find(
        uia,
        (source.element(), source.pattern()),
        &terminal,
        (needle, from, rows),
        remote,
    ) {
        Ok(found) => TextReply::TerminalFound {
            matches: found.matches,
            read: found.read,
        },
        Err(TextError::Gone) => TextReply::Gone,
        Err(TextError::Failed(reason)) => {
            tracing::debug!(reason, "a terminal could not be searched");
            TextReply::Unanswered
        }
    }
}

/// The position in `node_id`'s text where `range`, the range an active text
/// position change carried, starts: a new anchor there, read from nothing,
/// so it costs no call.
pub(super) fn active_position(
    context: &RequestContext<'_>,
    node_id: NodeId,
    range: &AgileReference<windows::Win32::UI::Accessibility::IUIAutomationTextRange>,
) -> Option<verbatim_model::TextPosition> {
    let range = range.resolve().ok()?;
    let pos = crate::text::uia::UiaPos::start_of(&range).ok()?;
    let mut call = uia_call(context, node_id, &[]);
    let position = call.text().position_at(pos);
    put_uia(context, node_id, call);
    Some(position)
}
