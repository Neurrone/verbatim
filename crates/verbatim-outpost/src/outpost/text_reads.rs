//! The worker's text (milestone M4): which nodes have text and through
//! which backend, and Core's text requests answered with [`crate::text`].
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
//! completed as the focus, which NVDA reads as a new object, or a caret
//! tell's read adopts the element its caret event came from, which only an
//! element with text raises (`docs/parity/text.md`, "A text pattern
//! missing at the focus").
//!
//! A program's text calls work on a copy of the node's text facts from the
//! program's own copy of the store's, with clones of the UIA ranges they
//! name, taken from the store in one hold of its lock. What a call changes
//! is applied to the program's copy at once and carried, with the ranges
//! it made and the patterns and support it learned, to the program's next
//! commit ([`ProgramText`]); the shell writes them to the store only when
//! the reducer accepts that commit.

#![forbid(unsafe_code)]

use std::collections::{BTreeSet, HashMap};

use windows::Win32::UI::Accessibility::IUIAutomationTextPattern;
use windows::core::AgileReference;

use verbatim_model::request::{FocusCaret, FocusCaretRead};
use verbatim_model::{NodeId, Role, TextDetails, TextFailure};
use verbatim_uia::map::is_terminal_class;
use verbatim_uia::{Cached, Interning as _, Kept};

use crate::text::uia::{TextSupport, UiaPos, UiaText};
use crate::text::{self, NodeTextCopy, Slot, TextError};

use verbatim_outpost_logic::{WatchProgram, WatchWork};

use super::commit::Effects;
use super::reducer::{NodeAnchors, RangeKey, TextChange};
use super::store::TextWrite;
use super::{Context, RequestContext};

/// A node's text pattern, kept so each request does not fetch it again;
/// `None` for a node whose element has no text pattern.
pub(super) type Patterns = Option<AgileReference<IUIAutomationTextPattern>>;

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
/// operation names (`named`), as the program's copy holds it, its ranges
/// cloned from the program's own or, in one hold, from the store.
fn uia_call(
    context: &RequestContext<'_>,
    node_id: NodeId,
    named: &[u64],
) -> TextCall<UiaPos, RangeKey> {
    let (part, copied) = context
        .text_facts()
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
        .text_facts()
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

/// Applies `change` to the program's copy and carries it to the
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

/// A UIA node's text, its patterns fetched once and kept, its reads
/// fetching `details` beyond the text.
fn uia_source(
    context: &RequestContext<'_>,
    node_id: NodeId,
    details: TextDetails,
) -> Result<UiaText, TextFailure> {
    let element = context
        .store
        .element_of(node_id)
        .and_then(|agile| agile.resolve().ok())
        .ok_or(TextFailure::Gone)?;
    let kept = patterns_of(context, node_id);
    let found = if let Some(found) = kept {
        found
    } else {
        {
            let fetched = match verbatim_uia::text::text_pattern(element.live().element()) {
                Ok(text_pattern) => AgileReference::new(&text_pattern).ok(),
                Err(error) if verbatim_uia::element_is_gone(&error) => {
                    return Err(TextFailure::Gone);
                }
                // An answer with no pattern: the element has none.
                Err(error) if error.code().is_ok() => None,
                // The provider did not answer, as one that is not ready yet
                // while its application starts: nothing is kept, so the
                // next read fetches again.
                Err(error) => {
                    tracing::debug!(%error, "the text pattern could not be fetched");
                    return Err(TextFailure::Unanswered);
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
    let Some(text_pattern) = found else {
        return Err(TextFailure::NoText);
    };
    let text_pattern = text_pattern.resolve().map_err(|_| TextFailure::Gone)?;
    let class = element.read().class_name;
    let terminal = is_terminal_class(class.as_deref())
        || super::followed::terminal(context).is_some_and(|(followed, _)| followed == node_id);
    let window = super::asked::known_nearest_of(context, node_id);
    let support = support_of(context, node_id);
    Ok(
        UiaText::new(element.live().element().clone(), text_pattern, terminal)
            .remote(context.tries_remote(window))
            .window(window)
            .details(details)
            .support(support),
    )
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
pub(super) fn adopt_told(
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
/// backends, in the program's copy now and in the store with its next
/// commit.
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

/// Stops trying remote operations in the window of a text read that
/// found its programs cannot run at all, as the focus walk and the
/// terminal read do; the program builder logged the fallback.
fn note_read_classically(context: &RequestContext<'_>, source: &mut UiaText) {
    if source.take_read_classically()
        && let Some(window) = source.window_of()
    {
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
        .is_some_and(|(hwnd, object, child)| {
            verbatim_ia2::text::edit_version(hwnd, object, child).is_some()
        })
}

/// Runs `read` on UI Automation node `node_id`'s text, fetching `details`
/// beyond the text, with a copy of its facts holding the anchors `named`,
/// committing what the read changed: `Err` with the reply a node with no
/// text, or gone, answers.
fn on_uia_text<T>(
    context: &RequestContext<'_>,
    node_id: NodeId,
    (details, named): (TextDetails, &[u64]),
    read: impl FnOnce(&mut UiaText, &mut text::NodeText<'_, UiaPos>) -> T,
) -> Result<T, TextFailure> {
    let mut source = uia_source(context, node_id, details)?;
    let mut call = uia_call(context, node_id, named);
    let read = read(&mut source, &mut call.text());
    put_uia(context, node_id, call);
    note_read_classically(context, &mut source);
    keep_support(context, node_id, &source);
    Ok(read)
}

/// An MSAA node's text facts, as the MSAA executor's text arms work on
/// them ([`verbatim_ia2::text::TextResolve`]): a copy of the node's facts
/// for each read, committed with the program's next commit.
pub(super) struct EditFacts<'a, 'c>(pub(super) &'a RequestContext<'c>);

impl verbatim_ia2::text::TextResolve for EditFacts<'_, '_> {
    fn key_of(&self, node: NodeId) -> Option<(isize, i32, i32)> {
        self.0.store.key_of(node)
    }

    fn edit_anchors(
        &self,
        node: NodeId,
        named: &[u64],
        read: &mut dyn FnMut(&mut text::NodeText<'_, u32>),
    ) {
        let mut call = edit_call(self.0, node, named);
        read(&mut call.text());
        put_edit(self.0, node, call);
    }

    fn now_us(&self) -> u64 {
        self.0.observed_now()
    }
}

/// A run of a batch's fusable text steps on UI Automation node `node_id`
/// through the program builder ([`text::fused::run`]), `first` being the
/// place of its first step and `earlier` the outcomes of the steps before
/// it: each step's outcome, its anchors committed with the program's text
/// writes. A `Caret` adopts the element of the caret tell it names first,
/// as [`caret_step`] does. `Err` with the reply its text's opening
/// answered when the node's text cannot be opened.
pub(super) fn fused_steps(
    context: &RequestContext<'_>,
    uia: &verbatim_uia::Uia,
    node_id: NodeId,
    (steps, first, earlier): (
        &[verbatim_model::batch::Step],
        usize,
        &[verbatim_model::batch::StepOutcome],
    ),
) -> Result<Vec<verbatim_model::batch::StepOutcome>, TextFailure> {
    for step in steps {
        if let verbatim_model::batch::Op::Caret {
            adopt: Some(adopt), ..
        } = step.op
        {
            adopt_told(context, node_id, Some(adopt));
        }
    }
    let mut source = uia_source(context, node_id, TextDetails::NONE)?;
    let named = text::fused::named(steps, first, earlier);
    let mut call = uia_call(context, node_id, &named);
    let window = super::asked::known_nearest_of(context, node_id);
    let read_at_us = context.observed_now();
    let answer = text::fused::run(
        uia,
        (node_id, &mut source, &mut call.text()),
        (steps, first, earlier),
        (window, read_at_us),
    );
    put_uia(context, node_id, call);
    keep_support(context, node_id, &source);
    if answer.read_classically
        && let Some(window) = window
    {
        context.read_classically(window);
    }
    Ok(answer.outcomes)
}

/// How a caret wait's poll ended the wait.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum WaitEnded {
    /// The effect was seen, or the bound passed: with the outcomes of the
    /// batch's later steps when the poll's program read them.
    Waited(
        verbatim_model::batch::Waited,
        Option<Vec<verbatim_model::batch::StepOutcome>>,
    ),
    /// The node, or the starting point, is no longer held.
    NotThere,
}

/// Runs one poll of a caret wait: how it ended the wait, `None` for a
/// check that shows no effect before the bound or whose read failed, which
/// counts as no effect. Through UI Automation one program, the check with
/// the batch's later reads fused after it ([`text::fused::poll`]); with no
/// UI Automation client, the check by parts ([`text::check::check`]); an
/// edit control's, the MSAA executor's check
/// ([`verbatim_ia2::text::check`]).
pub(super) fn watch(
    context: &RequestContext<'_>,
    uia: Option<&verbatim_uia::Uia>,
    program: &WatchProgram,
) -> Option<WaitEnded> {
    let node_id = program.node;
    let wait = &program.wait;
    let WatchWork::Check { word, last } = program.work;
    let is_uia = context.store.runtime_id_of(node_id).is_some();
    if let (Some(uia), true) = (uia, is_uia)
        && let Ok(mut source) = uia_source(context, node_id, TextDetails::NONE)
    {
        let later = program.batch.steps.get(1..).unwrap_or_default();
        let named: Vec<u64> = std::iter::once(wait.from.0)
            .chain(text::fused::named(later, 0, &[]))
            .collect();
        let mut call = uia_call(context, node_id, &named);
        let window = super::asked::known_nearest_of(context, node_id);
        let read_at_us = context.observed_now();
        let polled = text::fused::poll(
            uia,
            (node_id, &mut source, &mut call.text()),
            (wait, word, last),
            later,
            (window, read_at_us),
        );
        put_uia(context, node_id, call);
        keep_support(context, node_id, &source);
        return match polled {
            Ok(answer) => {
                if answer.read_classically
                    && let Some(window) = window
                {
                    context.read_classically(window);
                }
                answer
                    .ended
                    .map(|(waited, later)| WaitEnded::Waited(waited, later))
            }
            Err(error) => watch_outcome(Ok(Err(error))),
        };
    }
    let ask = text::check::CheckAsk { wait, word, last };
    let ran = if is_uia {
        on_uia_text(
            context,
            node_id,
            (TextDetails::NONE, &[wait.from.0]),
            |source, text| match text.held_read(wait.from) {
                Some(start) => text::check::check(source, &start, ask),
                None => Err(TextError::Gone),
            },
        )
    } else {
        verbatim_ia2::text::check(&EditFacts(context), node_id, ask)
    };
    watch_outcome(ran)
}

/// What a wait's check that `ran` gives: not there for a node or starting
/// point gone; no end, as no effect, for a read that failed, the node's
/// text not opened included, as NVDA's wait counts a caret it could not
/// read as one that did not move (`editableText.py` 121 to 127); else the
/// check's end, if it ended the wait.
fn watch_outcome(
    ran: Result<crate::text::TextResult<Option<verbatim_model::batch::Waited>>, TextFailure>,
) -> Option<WaitEnded> {
    match ran {
        Err(TextFailure::Gone) | Ok(Err(TextError::Gone)) => Some(WaitEnded::NotThere),
        Err(reply) => {
            tracing::debug!(?reply, "a wait's text could not be opened");
            None
        }
        Ok(Err(TextError::Failed(reason))) => {
            tracing::debug!(reason, "a wait's read failed");
            None
        }
        Ok(Ok(waited)) => waited.map(|waited| WaitEnded::Waited(waited, None)),
    }
}

/// UI Automation node `node_id`'s text, its reads fetching `details`:
/// `Err` with the reply a node with no text, or gone, answers.
pub(super) fn uia_text(
    context: &RequestContext<'_>,
    node_id: NodeId,
    details: TextDetails,
) -> Result<UiaText, TextFailure> {
    uia_source(context, node_id, details)
}

/// A focus's caret, read after its states (`books/dev/src/reference/crates/verbatim-core.md`, "Caret tells and Core's text facts"): the
/// line at the caret, with the formatting `details` names, and the
/// selection, the next baseline held from the read, compared with nothing
/// (Dickson, 2026-11-17). A node with no text answers
/// [`FocusCaret::NoText`], one whose read failed [`FocusCaret::NotRead`].
/// An edit control's is the MSAA executor's
/// ([`verbatim_ia2::text::focus_caret`]).
pub(super) fn focus_caret(
    context: &RequestContext<'_>,
    uia: Option<&verbatim_uia::Uia>,
    node_id: NodeId,
    details: TextDetails,
) -> FocusCaret {
    if context.store.runtime_id_of(node_id).is_none() {
        return verbatim_ia2::text::focus_caret(&EditFacts(context), node_id);
    }
    let read_at_us = context.observed_now();
    let read =
        on_uia_text(
            context,
            node_id,
            (details, &[]),
            |source, text| match text::InOneGo::in_one_go(source, uia, node_id) {
                Ok(one_go) => text::held::read_focus(source, text, (one_go, read_at_us)),
                Err(error) => Err(error),
            },
        );
    match read {
        Err(TextFailure::NoText) => FocusCaret::NoText,
        Err(_) | Ok(Err(_)) => FocusCaret::NotRead,
        Ok(Ok((caret, selected, baseline))) => FocusCaret::Read(Box::new(FocusCaretRead {
            caret,
            selected_text: selected.map(|selected| selected.text),
            baseline,
        })),
    }
}

#[cfg(test)]
mod watch_tests {
    use super::*;

    /// A wait whose node's text cannot be opened, or whose read fails,
    /// goes on as if the check showed no effect; only a node or starting
    /// point gone ends it as not there.
    #[test]
    fn a_watch_read_that_fails_is_no_effect_and_only_a_gone_node_is_not_there() {
        for failed in [TextFailure::Unanswered, TextFailure::NoText] {
            assert_eq!(watch_outcome(Err(failed)), None, "{failed:?}");
        }
        assert_eq!(
            watch_outcome(Ok(Err(TextError::Failed("busy".to_owned())))),
            None
        );
        assert_eq!(
            watch_outcome(Err(TextFailure::Gone)),
            Some(WaitEnded::NotThere)
        );
        assert_eq!(
            watch_outcome(Ok(Err(TextError::Gone))),
            Some(WaitEnded::NotThere)
        );
    }
}
