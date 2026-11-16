//! The worker's text (milestone M4): which nodes have text and through
//! which backend, Core's text requests answered with [`crate::text`], the
//! caret reports sent as `CaretMoved`, and the reads made under a key's note
//! with the facts Core judges evidence from.
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
use verbatim_model::request::{FocusCaret, FocusCaretRead, Outcome, Reply};
use verbatim_model::{
    CaretRead, CaretReply, CaretReport, NodeId, ReadFacts, Role, SelectionChange,
    SelectionReadReply, TextDetails, TextOp, TextReply, WatchFinalRead, Watched,
};
use verbatim_uia::map::is_terminal_class;
use verbatim_uia::{Cached, Interning as _, Kept};
use verbatim_uia_rops::Path;

use crate::text::edit::EditText;
use crate::text::uia::{TextSupport, UiaPos, UiaText};
use crate::text::{
    self, CheckKey, Checked, FetchAsk, KeyChecked, NodeTextCopy, ReportAsk, Slot, TextError,
    TextSource as _,
};
use verbatim_reader_rules::class::normalize_class_name;
use verbatim_win32::window::window_class_name;

use verbatim_outpost_logic::{WatchProgram, WatchWork};

use super::commit::Effects;
use super::reducer::{AnswerCut, NodeAnchors, NoteFetch, RangeKey, ReportCut, TextChange};
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

/// A node's text, through its backend.
enum Source {
    Uia(UiaText),
    Edit(EditText),
}

/// The text behind `node_id`, or the reply that ends the request: `NoText`
/// for a node with no text interface, `Gone` for one this outpost no longer
/// knows.
fn source(
    context: &RequestContext<'_>,
    node_id: NodeId,
    details: TextDetails,
) -> Result<Source, TextReply> {
    if context.store.runtime_id_of(node_id).is_some() {
        return uia_source(context, node_id, details).map(Source::Uia);
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

/// A UIA node's text, its patterns fetched once and kept, its reads
/// fetching `details` beyond the text.
fn uia_source(
    context: &RequestContext<'_>,
    node_id: NodeId,
    details: TextDetails,
) -> Result<UiaText, TextReply> {
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
    let terminal = is_terminal_class(class.as_deref())
        || super::followed::terminal(context).is_some_and(|(followed, _)| followed == node_id);
    let remote = context.tries_remote(context.tracked_window());
    let support = support_of(context, node_id);
    Ok(UiaText::new(
        element.live().element().clone(),
        text_pattern,
        caret_pattern,
        terminal,
    )
    .remote(remote)
    .details(details)
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
        // `books/dev/src/explanation/design/uia-shapes.md`).
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

/// A caret tell's read of `node_id` adopts the element its request names,
/// `adopt`, the one the tell's event came from, when the node's own
/// answered it has no text pattern: that answer is forgotten and the event's
/// element kept as the node's ([`text_event_from`]). The element is let go
/// of either way, its tell answered.
fn adopt_told(
    context: &RequestContext<'_>,
    node_id: NodeId,
    adopt: Option<verbatim_model::tell::ElementToken>,
) {
    let Some(element) = adopt
        .and_then(|token| context.store.take_parked(token.0))
        .and_then(super::store::Parked::element)
    else {
        return;
    };
    text_event_from(context, node_id, Some(&element));
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

/// What `op` fetches beyond the text, as Core named it in the request: a
/// read that names none fetches nothing more, since it reads no
/// formatting.
fn details_of(op: &TextOp) -> TextDetails {
    match op {
        TextOp::ReadCaret(read) => read.fetch.details,
        TextOp::StartingRead(read) => read.details,
        TextOp::MoveBySentence(moved) => moved.details,
        TextOp::Watch(watch) => match watch.final_read {
            WatchFinalRead::Unit { details, .. } | WatchFinalRead::UnitAtEffect { details, .. } => {
                details
            }
            WatchFinalRead::Terminal => TextDetails::NONE,
        },
        _ => TextDetails::NONE,
    }
}

/// Answers one of Core's text requests other than a caret key's, which
/// [`before_read`] and [`note_read`] answer.
pub(super) fn answer(context: &RequestContext<'_>, node_id: NodeId, op: &TextOp) -> TextReply {
    let source = match source(context, node_id, details_of(op)) {
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

/// Runs `$read` on `$node_id`'s text, through whichever source it has,
/// fetching `$details` beyond the text,
/// with a copy of its facts holding the anchors `$named`, committing what
/// the read changed: `Err` with the reply a node with no text, or gone,
/// answers.
macro_rules! on_text {
    ($context:expr, $node_id:expr, $details:expr, $named:expr, |$source:ident, $text:ident| $read:expr) => {{
        let context = $context;
        let node_id = $node_id;
        match source(context, node_id, $details) {
            Err(reply) => Err(reply),
            Ok(Source::Uia(mut source)) => {
                let mut call = uia_call(context, node_id, $named);
                let read = {
                    let $source = &mut source;
                    let $text = &mut call.text();
                    $read
                };
                put_uia(context, node_id, call);
                note_fallback(context, &mut source);
                keep_support(context, node_id, &source);
                Ok(read)
            }
            Ok(Source::Edit(mut source)) => {
                let mut call = edit_call(context, node_id, $named);
                let read = {
                    let $source = &mut source;
                    let $text = &mut call.text();
                    $read
                };
                put_edit(context, node_id, call);
                Ok(read)
            }
        }
    }};
}

/// The reply a text read that failed answers.
fn failed_reply(error: TextError) -> TextReply {
    match error {
        TextError::Gone => TextReply::Gone,
        TextError::Failed(reason) => {
            tracing::debug!(reason, "a text request was not answered");
            TextReply::Unanswered
        }
    }
}

/// Answers one of Core's text requests from the event queue or outside it
/// (`docs/design/readers-tell-core-asks-build1.md`, "Keys and commands in
/// this build"): a caret key's starting read, a caret tell's read, and
/// the shared list's reads and moves, a move answered with the next
/// selection baseline.
pub(super) fn answer_asked(
    context: &RequestContext<'_>,
    node_id: NodeId,
    op: &TextOp,
) -> TextReply {
    let named = text::named_anchors(op);
    let details = details_of(op);
    let read_at_us = context.observed_now();
    let replied = match op {
        TextOp::StartingRead(read) => {
            on_text!(context, node_id, details, &named, |source, text| {
                text::held::starting_read(source, text, read_at_us, (read.unit, read.word))
                    .map_or_else(failed_reply, |started| {
                        TextReply::Started(Box::new(started))
                    })
            })
        }
        TextOp::ReadSelection(read) => {
            adopt_told(context, node_id, read.adopt);
            on_text!(context, node_id, details, &named, |source, text| {
                text::held::read_selection(source, text, read_at_us, read.baseline).map_or_else(
                    failed_reply,
                    |found| {
                        TextReply::SelectionRead(Box::new(SelectionReadReply {
                            edit_selection: source.edit_selection(&found.state),
                            caret: found.report,
                            baseline: found.baseline,
                            difference: found.difference,
                        }))
                    },
                )
            })
        }
        TextOp::MoveBySentence(moved) => {
            on_text!(context, node_id, details, &named, |source, text| {
                text::sentence::move_by_sentence(source, text, (moved.point, moved.forward))
                    .unwrap_or_else(failed_reply)
            })
        }
        // A watch waits in the reducer's table, which runs its checks and
        // final read as programs of their own ([`watch`]).
        TextOp::Watch(_) => Ok(TextReply::Unsupported),
        other => on_text!(context, node_id, details, &named, |source, text| {
            text::held::perform_placed(source, text, other)
        }),
    };
    replied.unwrap_or_else(|reply| reply)
}

/// Runs a watch's check or final read
/// (`docs/design/readers-tell-core-asks-build1.md`, "The watch", Table 15):
/// the watch's answer when the program ends it, `None` for a check that
/// shows no effect before the bound or whose read failed, which counts as
/// no effect. A final read that fails answers nothing, and the reducer
/// answers the watch failed.
pub(super) fn watch(context: &RequestContext<'_>, program: &WatchProgram) -> Option<Outcome> {
    let node_id = program.node;
    let named = [program.watch.point.0];
    let read_at_us = context.observed_now();
    // A terminal's watch reads no unit at its end: Core's full read of the
    // terminal follows (build 3).
    let (unit, details) = match program.watch.final_read {
        WatchFinalRead::Unit { unit, details } | WatchFinalRead::UnitAtEffect { unit, details } => {
            (unit, details)
        }
        WatchFinalRead::Terminal => (verbatim_model::TextUnit::Character, TextDetails::NONE),
    };
    let ran = on_text!(
        context,
        node_id,
        details,
        &named,
        |source, text| match program.work {
            WatchWork::Check { word, last } => text::check::check(
                source,
                text,
                text::check::CheckAsk {
                    point: program.watch.point,
                    motion: program.watch.motion,
                    word,
                    last,
                    final_read: program.watch.final_read,
                },
                read_at_us,
            ),
            WatchWork::Final if program.watch.final_read == WatchFinalRead::Terminal => {
                Ok(Some(Watched::EffectUnread))
            }
            WatchWork::Final => {
                text::check::final_read(source, text, unit, read_at_us)
                    .map(|read| Some(Watched::Effect(read)))
            }
        }
    );
    match ran {
        Err(_) | Ok(Err(TextError::Gone)) => Some(Outcome::NotThere),
        Ok(Err(TextError::Failed(reason))) => {
            tracing::debug!(reason, "a watch's read failed");
            None
        }
        Ok(Ok(watched)) => watched
            .map(|watched| Outcome::Result(Reply::Text(TextReply::Watched(Box::new(watched))))),
    }
}

/// A focus's caret, read after its states (Table 17, columns A to C): the
/// line at the caret and the selection, compared with the baseline Core
/// named when this node holds it, the next baseline held from the read.
/// A node with no text answers [`FocusCaret::NoText`], one whose read
/// failed [`FocusCaret::NotRead`].
pub(super) fn focus_caret(
    context: &RequestContext<'_>,
    node_id: NodeId,
    baseline: Option<verbatim_model::TextAnchor>,
) -> FocusCaret {
    let named: Vec<u64> = baseline.map(|baseline| baseline.0).into_iter().collect();
    let read_at_us = context.observed_now();
    // The read takes no formatting: nothing beyond the text is fetched.
    let read = on_text!(
        context,
        node_id,
        TextDetails::NONE,
        &named,
        |source, text| {
            text::held::read_selection(source, text, read_at_us, baseline).map(|found| {
                (
                    found.report,
                    found.selected,
                    found.baseline,
                    found.difference,
                )
            })
        }
    );
    match read {
        Err(TextReply::NoText) => FocusCaret::NoText,
        Err(_) | Ok(Err(_)) => FocusCaret::NotRead,
        Ok(Ok((caret, selected, baseline, difference))) => {
            FocusCaret::Read(Box::new(FocusCaretRead {
                caret,
                selected_text: selected.map(|selected| selected.text),
                baseline,
                difference,
            }))
        }
    }
}

/// The ask of a read made for `note`'s key, its end read off the shared
/// clock.
fn fetch_ask(note: NoteFetch) -> FetchAsk {
    FetchAsk {
        note,
        ended: verbatim_process::clock::now_us,
    }
}

/// Reads `node_id`'s caret before a held caret key is passed on, the read
/// a caret event's report makes with the key's unit at the caret: it
/// compares the selection with the node's last read and replaces it, and
/// its answer carries what the key's words need. With `cuts`, the read of
/// an edit control's client area is cut where its comparison would ask
/// whether the control's value change waits, and the cut is returned
/// instead. Also when the read began, if it was made.
pub(super) fn before_read(
    context: &RequestContext<'_>,
    node_id: NodeId,
    read: CaretRead,
    cuts: bool,
) -> BeforeReading {
    let note = NoteFetch {
        unit: read.fetch.unit,
        motion: read.fetch.motion,
        details: read.fetch.details,
    };
    let Ok(source) = source(context, node_id, note.details) else {
        return (Err(NoCaret::NotRead), None);
    };
    let read_at_us = context.observed_now();
    let report = match source {
        Source::Uia(mut source) => {
            let mut call = uia_call(context, node_id, &[]);
            let read = text::caret_report_since(
                &mut source,
                &mut call.text(),
                read_at_us,
                &ReportAsk {
                    formats: false,
                    since: None,
                    compare: true,
                    fetch: Some(fetch_ask(note)),
                },
            );
            put_uia(context, node_id, call);
            note_fallback(context, &mut source);
            keep_support(context, node_id, &source);
            read.and_then(before_reply)
                .map(Checked::Done)
                .map_err(not_read)
        }
        Source::Edit(mut source) => {
            let edit = edit_client(context, node_id).filter(|_| cuts);
            let mut call = edit_call(context, node_id, &[]);
            let report = text::caret_report_checking(
                &mut source,
                &mut call.text(),
                read_at_us,
                &ReportAsk {
                    formats: false,
                    since: None,
                    compare: true,
                    fetch: Some(fetch_ask(note)),
                },
                edit.is_some(),
            )
            .and_then(|checked| match (checked, edit) {
                (Checked::Cut(read), Some(edit)) => Ok(Checked::Cut(EditCut {
                    edit,
                    length: source.length_read(),
                    read,
                })),
                (Checked::Cut(read), None) => text::finish_report(
                    &mut source,
                    &mut call.text(),
                    read,
                    false,
                    verbatim_process::clock::now_us,
                )
                .and_then(before_reply)
                .map(Checked::Done),
                (Checked::Done(read), _) => before_reply(read).map(Checked::Done),
            })
            .map_err(not_read);
            put_edit(context, node_id, call);
            if matches!(report, Ok(Checked::Cut(_))) {
                value_change_cut(context);
            }
            report
        }
    };
    (report, Some(read_at_us))
}

/// A before read's answer, or the read cut at its value change check, and
/// when it began.
pub(super) type BeforeReading = (
    Result<Checked<CaretReply, EditCut<ReportCut<u32>>>, NoCaret>,
    Option<u64>,
);

/// The answer to a before read, from what it found: it was made with a
/// key's fetch, so it has the key's facts.
fn before_reply<P>(read: text::ReportedCaret<P>) -> Result<CaretReply, TextError> {
    let facts = read
        .facts
        .ok_or_else(|| TextError::Failed("a key's read carries no facts".to_owned()))?;
    Ok(CaretReply {
        caret: read.report,
        selection_changes: read.selection_changes,
        content_changed: read.content_changed,
        new_selection: read.new_selection,
        facts,
    })
}

/// Finishes the before read of edit control `node_id` that [`before_read`]
/// cut: the comparison, in general terms when the control's value change
/// waited (`waiting`).
pub(super) fn finish_before(
    context: &RequestContext<'_>,
    node_id: NodeId,
    (length, cut): (Option<u32>, ReportCut<u32>),
    waiting: bool,
) -> Result<CaretReply, NoCaret> {
    let mut source = match edit_source(context, node_id, length) {
        Ok(source) => source,
        Err(TextReply::NoText) => return Err(NoCaret::NoText),
        Err(_) => return Err(NoCaret::NotRead),
    };
    let mut call = edit_call(context, node_id, &[]);
    let report = text::finish_report(
        &mut source,
        &mut call.text(),
        cut,
        waiting,
        verbatim_process::clock::now_us,
    )
    .and_then(before_reply);
    put_edit(context, node_id, call);
    report.map_err(not_read)
}

/// Reads `node_id`'s caret for a key's note or final read, one read
/// ([`text::check_key`]) compared with the node's last read, the node's text
/// unreadable answering the read failed. `final_read` says it is the key's
/// final read. With `cuts`, the read of an edit control's client area is
/// cut where its comparison would ask whether the control's value change
/// waits ([`value_change_cut`]), and the cut is returned with the control's
/// window instead. Also when the caret read began, if it was read, for the
/// commit of what the read sends.
pub(super) fn note_read(
    context: &RequestContext<'_>,
    node_id: NodeId,
    note: NoteFetch,
    (final_read, cuts): (bool, bool),
) -> (NoteCheck, Option<u64>) {
    let Ok(source) = source(context, node_id, note.details) else {
        return (Checked::Done(KeyChecked::Failed), None);
    };
    let read_at_us = context.observed_now();
    let check = CheckKey {
        note,
        read_at_us,
        final_read,
        ended: verbatim_process::clock::now_us,
    };
    match source {
        Source::Uia(mut source) => {
            let mut call = uia_call(context, node_id, &[]);
            let checked = text::check_key(&mut source, &mut call.text(), &check);
            put_uia(context, node_id, call);
            note_fallback(context, &mut source);
            keep_support(context, node_id, &source);
            (Checked::Done(checked), Some(read_at_us))
        }
        Source::Edit(mut source) => {
            let edit = edit_client(context, node_id).filter(|_| cuts);
            let mut call = edit_call(context, node_id, &[]);
            let checked =
                text::check_key_cut(&mut source, &mut call.text(), &check, edit.is_some());
            let checked = match (checked, edit) {
                (Checked::Cut(read), Some(edit)) => Checked::Cut(EditCut {
                    edit,
                    length: source.length_read(),
                    read,
                }),
                (Checked::Cut(cut), None) => Checked::Done(text::finish_key(
                    &mut source,
                    &mut call.text(),
                    &check,
                    cut,
                    false,
                )),
                (Checked::Done(found), _) => Checked::Done(found),
            };
            put_edit(context, node_id, call);
            if matches!(checked, Checked::Cut(_)) {
                value_change_cut(context);
            }
            (checked, Some(read_at_us))
        }
    }
}

/// A read for a key's note ([`note_read`]): what it found, or the read cut
/// at its value change check.
pub(super) type NoteCheck = Checked<KeyChecked, EditCut<AnswerCut<u32>>>;

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
    // An edit control's messages read no detail beyond the text.
    match source(context, node_id, TextDetails::NONE)? {
        Source::Edit(source) => Ok(source.knowing_length(length)),
        Source::Uia(_) => Err(TextReply::NoText),
    }
}

/// Finishes the read for a key's note of edit control `node_id` that
/// [`note_read`] cut, its comparison of selections in general terms when
/// the control's value change waited (`waiting`).
pub(super) fn finish_note(
    context: &RequestContext<'_>,
    node_id: NodeId,
    (length, cut): (Option<u32>, AnswerCut<u32>),
    waiting: bool,
) -> KeyChecked {
    let Ok(mut source) = edit_source(context, node_id, length) else {
        return KeyChecked::Failed;
    };
    let mut call = edit_call(context, node_id, &[]);
    let check = CheckKey {
        note: cut.note,
        read_at_us: cut.read_at_us,
        final_read: false,
        ended: verbatim_process::clock::now_us,
    };
    let found = text::finish_key(&mut source, &mut call.text(), &check, cut, waiting);
    put_edit(context, node_id, call);
    found
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
    /// What the read found for a key, when it was made for one.
    pub(super) facts: Option<Box<ReadFacts>>,
}

/// What a [`report_caret`] read found, or the read cut at its value change
/// check.
pub(super) type ReportOutcome =
    Result<Checked<(CaretReport, SelectionReport), EditCut<ReportCut<u32>>>, NoCaret>;

/// What [`report_caret`] found, and when its read began, if it was made.
pub(super) type CaretReading = (ReportOutcome, Option<u64>);

/// What a caret report is for ([`report_caret`]).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CaretFor {
    /// The report after a focus, whose line is spoken with its formatting,
    /// and whose comparison with the node's last read Core speaks for a
    /// repeated focus.
    Focus,
    /// A caret event of a text focus, with how the selection changed since the last report, as NVDA's auto-select
    /// detection reports it (`EditableTextWithAutoSelectDetection.event_caret`,
    /// `nvda/source/NVDAObjects/behaviors.py` 346 to 349;
    /// `detectPossibleSelectionChange`, `nvda/source/editableText.py` 417 to
    /// 471).
    Event,
}

/// The caret of `node_id`, for a `CaretMoved` event, read as `purpose`
/// says, with how the selection changed for [`CaretFor::Event`], and with
/// the key's facts when `fetch` is given: a read made under a key's note;
/// otherwise why there is none. Also when the read began, on
/// the observation clock, when it was made: it covers the caret events
/// observed before then only once its commit notes it.
pub(super) fn report_caret(
    context: &RequestContext<'_>,
    node_id: NodeId,
    purpose: CaretFor,
    (fetch, details): (Option<NoteFetch>, TextDetails),
) -> CaretReading {
    let source = match source(context, node_id, details) {
        Ok(source) => source,
        Err(reply) => {
            if purpose == CaretFor::Focus {
                // A focus read that fails drops the node's last read, as
                // NVDA's `initAutoSelectDetection` starts again.
                forget_last_read(context, node_id);
            }
            let missed = if reply == TextReply::NoText {
                NoCaret::NoText
            } else {
                NoCaret::NotRead
            };
            return (Err(missed), None);
        }
    };
    let formats = purpose == CaretFor::Focus;
    let read_at_us = context.observed_now();
    let read_at = Some(read_at_us);
    let report = match source {
        Source::Uia(mut source) => {
            let mut call = uia_call(context, node_id, &[]);
            let report = text::caret_report_since(
                &mut source,
                &mut call.text(),
                read_at_us,
                &ReportAsk {
                    formats,
                    since: None,
                    compare: true,
                    fetch: fetch.map(fetch_ask),
                },
            )
            .map(|read| Checked::Done(selection_report(read)));
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
                read_at_us,
                &ReportAsk {
                    formats,
                    since: None,
                    compare: true,
                    fetch: fetch.map(fetch_ask),
                },
                edit.is_some(),
            )
            .and_then(|checked| match (checked, edit) {
                (Checked::Cut(read), Some(edit)) => Ok(Checked::Cut(EditCut {
                    edit,
                    length: source.length_read(),
                    read,
                })),
                (Checked::Cut(read), None) => text::finish_report(
                    &mut source,
                    &mut call.text(),
                    read,
                    false,
                    verbatim_process::clock::now_us,
                )
                .map(Checked::Done),
                (Checked::Done(read), _) => Ok(Checked::Done(read)),
            })
            .map(|checked| match checked {
                Checked::Done(read) => Checked::Done(selection_report(read)),
                Checked::Cut(cut) => Checked::Cut(cut),
            });
            put_edit(context, node_id, call);
            if matches!(report, Ok(Checked::Cut(_))) {
                value_change_cut(context);
            }
            report
        }
    };
    if purpose == CaretFor::Focus && report.is_err() {
        forget_last_read(context, node_id);
    }
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
            facts: read.facts.map(Box::new),
        },
    )
}

/// Records that a read of `node_id` under the note ending at `until_us`
/// showed the key's effect: its note reads no more text changes.
pub(super) fn note_effect(context: &RequestContext<'_>, node_id: NodeId, until_us: u64) {
    record(
        context,
        TextChange::NoteEffect {
            node: node_id.number(),
            until_us,
        },
        (Vec::new(), Vec::new()),
    );
}

/// Clears the note of `node_id` ending at `until_us`, its key's final read
/// having committed.
pub(super) fn clear_note(context: &RequestContext<'_>, node_id: NodeId, until_us: u64) {
    record(
        context,
        TextChange::NoteCleared {
            node: node_id.number(),
            until_us,
        },
        (Vec::new(), Vec::new()),
    );
}

/// Drops `node_id`'s last read, its focus read having failed.
fn forget_last_read(context: &RequestContext<'_>, node_id: NodeId) {
    let node = node_id.number();
    let kept = {
        let facts = context.facts();
        facts
            .text
            .uia(node)
            .is_some_and(|anchors| anchors.last.is_some())
            || facts
                .text
                .edit(node)
                .is_some_and(|anchors| anchors.last.is_some())
    };
    if kept {
        record(
            context,
            TextChange::LastReadFailed(node),
            (Vec::new(), Vec::new()),
        );
    }
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
/// cut at its value change check (`books/dev/src/explanation/design/ownership.md`): waits for the `WinEvent` thread to take
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
    let report = text::finish_report(
        &mut source,
        &mut call.text(),
        cut,
        waiting,
        verbatim_process::clock::now_us,
    );
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

/// The range a start anchor Core named holds in `call`, `Err` with the reply
/// a start no longer held answers: the anchor was forgotten with its
/// window (Table 16, column E), so the terminal is not there.
fn named_start(
    call: &mut TextCall<UiaPos, RangeKey>,
    start: verbatim_model::TextAnchor,
) -> Result<windows::Win32::UI::Accessibility::IUIAutomationTextRange, TextReply> {
    let pos = call.text().anchor_point(start).ok_or(TextReply::Gone)?;
    pos.range_end()
        .map(|(range, _)| range)
        .map_err(failed_reply)
}

/// Reads terminal `node_id` as Core asks (`read`,
/// `docs/design/readers-tell-core-asks-build3.md`, "The full read request
/// and its starts"): from a screen above the start anchor Core names, with
/// the caret and its line read in the same round trip and given as a caret
/// report, the start anchor this read mints at the top of its screen, and,
/// for a caret key's starting read, the key's caret starting point. Every
/// fact the read needs that Core owns comes in `read`; the outpost keeps
/// nothing of the terminal but the anchors it holds for Core.
pub(super) fn terminal_read(
    context: &RequestContext<'_>,
    uia: &verbatim_uia::Uia,
    node_id: NodeId,
    read: &verbatim_model::TerminalRead,
) -> TextReply {
    let mut source = match uia_source(context, node_id, TextDetails::NONE) {
        Ok(source) => source,
        Err(reply) => return reply,
    };
    let named: Vec<u64> = read.start.map(|start| start.0).into_iter().collect();
    let mut call = uia_call(context, node_id, &named);
    let start = match read.start.map(|start| named_start(&mut call, start)) {
        None => None,
        Some(Ok(range)) => Some(range),
        Some(Err(reply)) => return reply,
    };
    let remote = context.tries_remote(context.tracked_window());
    let caret = verbatim_uia_rops::CaretLineQuery {
        element: source.element(),
        pattern: source.pattern(),
        pattern2: source.pattern2(),
        max_text: i32::try_from(crate::text::MAX_CHUNK_UNITS + 1).unwrap_or(i32::MAX),
        since: None,
    };
    let ask = crate::terminal::ReadAsk {
        start: start.as_ref(),
        scope: read.scope,
        height: read.height,
        width: read.width,
        colours: read.colours,
    };
    let answer = crate::terminal::read(
        uia,
        (source.element(), source.pattern()),
        caret,
        &ask,
        remote,
    );
    let mut answer = match answer {
        Ok(answer) => answer,
        Err(TextError::Gone) => return TextReply::Gone,
        Err(TextError::Failed(reason)) => {
            tracing::debug!(reason, "a terminal's text could not be read");
            return TextReply::Unanswered;
        }
    };
    if let Path::Fallback(error) = &answer.path {
        let window = context.tracked_window();
        tracing::warn!(?window, %error, "a remote operation failed; read the classic way");
        if let (verbatim_uia_rops::Error::Import(_), Some(window)) = (error, window) {
            context.read_classically(window);
        }
    }
    let Some(next_start) = answer
        .next_start
        .take()
        .and_then(|range| UiaPos::start_of(&range).ok())
    else {
        tracing::debug!("a terminal's read found no start");
        return TextReply::Unanswered;
    };
    let minted = call.text().hold_point(next_start);
    let point = match terminal_caret(&mut source, &mut call, &mut answer, read.starting) {
        Ok(point) => point,
        Err(reply) => return reply,
    };
    put_uia(context, node_id, call);
    keep_support(context, node_id, &source);
    TextReply::TerminalRead(Box::new(verbatim_model::TerminalReadReply {
        snapshot: answer.snapshot,
        start: minted,
        started_us: answer.started_us,
        point,
    }))
}

/// The caret a terminal's full read found, given as its snapshot's caret
/// report: a terminal raises no caret event for every character typed, so
/// the read keeps Core's caret following typing. For a caret key's
/// starting read, `starting` names the key's unit, and the key's caret
/// starting point, which its watch's checks compare with (build 1, "A
/// terminal's caret key"), is minted from that same caret, so the read
/// stays one round trip; it is returned. A node gone answers its reply.
fn terminal_caret(
    source: &mut UiaText,
    call: &mut TextCall<UiaPos, RangeKey>,
    answer: &mut crate::terminal::ReadAnswer,
    starting: Option<verbatim_model::TextUnit>,
) -> Result<Option<verbatim_model::TextAnchor>, TextReply> {
    let Some(found) = answer
        .caret
        .take()
        .and_then(|found| source.caret_read_from(found).ok())
    else {
        return Ok(None);
    };
    let Some(unit) = starting else {
        answer.snapshot.caret = crate::text::caret_report_from(
            source,
            &mut call.text(),
            found,
            (answer.started_us, false),
        )
        .ok();
        return Ok(None);
    };
    match text::held::starting_from(
        source,
        &mut call.text(),
        found,
        answer.started_us,
        (unit, false),
    ) {
        Ok(started) => {
            answer.snapshot.caret = Some(started.caret);
            Ok(Some(started.point))
        }
        Err(TextError::Gone) => Err(TextReply::Gone),
        Err(TextError::Failed(reason)) => {
            tracing::debug!(reason, "a terminal key's starting point was not read");
            Ok(None)
        }
    }
}

/// Reads the history of terminal `node_id` above the start anchor Core
/// names, for a review command Core answers from its memory
/// ([`verbatim_model::TextOp::TerminalHistory`]): from `above` rows above
/// that start to the document's end, or the whole document when `whole`.
pub(super) fn terminal_history(
    context: &RequestContext<'_>,
    uia: &verbatim_uia::Uia,
    node_id: NodeId,
    (start, above, whole, height): (verbatim_model::TextAnchor, u32, bool, u32),
) -> TextReply {
    let source = match uia_source(context, node_id, TextDetails::NONE) {
        Ok(source) => source,
        Err(reply) => return reply,
    };
    let mut call = uia_call(context, node_id, &[start.0]);
    let start = match named_start(&mut call, start) {
        Ok(range) => range,
        Err(reply) => return reply,
    };
    let remote = context.tries_remote(context.tracked_window());
    match crate::terminal::history(
        uia,
        (source.element(), source.pattern()),
        &start,
        (above, whole, height, remote),
    ) {
        Ok(history) => TextReply::TerminalHistory {
            text: history.text,
            above: history.above,
            read: 0,
        },
        Err(TextError::Gone) => TextReply::Gone,
        Err(TextError::Failed(reason)) => {
            tracing::debug!(reason, "a terminal's history could not be read");
            TextReply::Unanswered
        }
    }
}

/// Searches terminal `node_id` for a command mark's line from the start
/// anchor Core names, for Core's command
/// ([`verbatim_model::TextOp::TerminalFind`]).
pub(super) fn terminal_find(
    context: &RequestContext<'_>,
    uia: &verbatim_uia::Uia,
    node_id: NodeId,
    start: verbatim_model::TextAnchor,
    (needle, from, rows): (&str, i32, u32),
) -> TextReply {
    let source = match uia_source(context, node_id, TextDetails::NONE) {
        Ok(source) => source,
        Err(reply) => return reply,
    };
    let mut call = uia_call(context, node_id, &[start.0]);
    let start = match named_start(&mut call, start) {
        Ok(range) => range,
        Err(reply) => return reply,
    };
    let remote = context.tries_remote(context.tracked_window());
    match crate::terminal::find(
        uia,
        (source.element(), source.pattern()),
        &start,
        (needle, from, rows),
        remote,
    ) {
        Ok(found) => TextReply::TerminalFound {
            matches: found.matches,
            read: 0,
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
