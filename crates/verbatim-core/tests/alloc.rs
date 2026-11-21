//! Core's allocation invariant: a reduce step allocates the same number of
//! bytes whatever the size of the state it changes, but a focus step, whose
//! request carries the ancestors Core already knows and whose allocation
//! grows no faster than the focus's depth.
//!
//! This test binary installs a global allocator that counts the bytes each
//! thread asks for, so the test runner's parallel threads do not disturb
//! one another's counts. Today the parts of the state that grow with the
//! application are the focus ancestor chain (and the navigator that follows
//! focus) and the text Core keeps for the caret and the review cursor (a
//! line, up to 64 KB), and a terminal's output waiting to be spoken is
//! bounded by the flood policy; the large state here has a previous focus
//! with 10,000 ancestors and the small one has 10. Each step is one input
//! to Core's event queue, the same in both cases, so only the state
//! varies.

mod desk;
#[path = "support/readers.rs"]
mod readers;

use readers::Found;
mod terminal_support;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use desk::{Desk, LISTENER, ONE, focus_read_of, focus_tell, listener_read};
use verbatim_model::request::{
    Answer, FocusCaret, FocusCaretRead, Instance, Outcome, Press, PressAction, QueueEffect,
    QueueInput, Reply, RequestNumber,
};
use verbatim_model::stroke::{HeldStroke, Stroke};
use verbatim_model::{
    Backend, CaretKey, CaretMotion, CaretReport, Effect, KeySeq, NodeDetails, NodeSnapshot,
    ReviewCommand, Role, SegmentContent, SpeechPriority, StateSet, TextAnchor, TextChunk, TextUnit,
    TraceId, Utterance, UtteranceSegment,
};

#[path = "support/caret_batches.rs"]
mod caret_batches;

/// The one caret key's batch `effects` asked: its number and the batch.
fn text_batch(effects: &[Effect]) -> (u64, verbatim_model::batch::Batch) {
    let batches: Vec<_> = effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Queue(QueueEffect::Send { ask, .. }) => match &ask.request {
                verbatim_model::request::Question::Batch(batch) => {
                    Some((ask.number.0, batch.clone()))
                }
                verbatim_model::request::Question::Legacy(_) => None,
            },
            _ => None,
        })
        .collect();
    let [one] = &batches[..] else {
        panic!("one batch asked: {effects:?}");
    };
    one.clone()
}

/// The system allocator, counting the bytes requested on each thread.
struct Counting;

thread_local! {
    static ALLOCATED: Cell<usize> = const { Cell::new(0) };
}

fn count(bytes: usize) {
    // `try_with` because the allocator also runs while a thread's locals are
    // being torn down; those allocations belong to no measurement.
    let _ = ALLOCATED.try_with(|allocated| allocated.set(allocated.get() + bytes));
}

// SAFETY: every call is forwarded unchanged to the system allocator; the
// counting touches only a thread-local integer and never allocates.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count(layout.size());
        // SAFETY: the caller upholds `alloc`'s contract, passed on as is.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count(layout.size());
        // SAFETY: as for `alloc`.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        count(new_size);
        // SAFETY: the caller upholds `realloc`'s contract, passed on as is.
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller upholds `dealloc`'s contract, passed on as is.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// Runs `work` and returns its result with the bytes it allocated on this
/// thread.
fn allocated_by<R>(work: impl FnOnce() -> R) -> (R, usize) {
    let before = ALLOCATED.with(Cell::get);
    let result = work();
    let after = ALLOCATED.with(Cell::get);
    (result, after - before)
}

const SMALL_CHAIN: u64 = 10;
const LARGE_CHAIN: u64 = 10_000;

fn node(number: u64, role: Role, name: &str) -> NodeSnapshot {
    NodeSnapshot {
        id: ONE.node(number),
        backend: Backend::Uia,
        role,
        name: Some(name.to_owned()),
        value: None,
        states: StateSet::new(),
        details: NodeDetails::default(),
    }
}

/// A chain of `depth` named groups, outermost first. Its node numbers
/// (from 100,000) never collide with the nodes the measured steps use.
fn chain(depth: u64) -> Vec<NodeSnapshot> {
    (0..depth)
        .map(|index| node(100_000 + index, Role::Group, &format!("Group {index}")))
        .collect()
}

/// A desk whose focus, and the navigator following it, sit under a chain
/// of `depth` named groups.
fn desk_with_chain(depth: u64) -> Desk {
    let mut desk = Desk::new();
    let _ = desk.focus(ONE, &node(1, Role::Button, "Previous"), &chain(depth));
    desk
}

/// Feeds `input` to `small` and `large` at their next moment, the input
/// built before either is fed: the effects they had, which must be the
/// same, as must the bytes each step allocated.
fn same(small: &mut Desk, large: &mut Desk, input: &QueueInput) -> Vec<Effect> {
    small.at_us += 1_000;
    large.at_us += 1_000;
    let small_input = input.clone();
    let large_input = input.clone();
    let (small_effects, small_bytes) = allocated_by(|| small.feed(small_input));
    let (large_effects, large_bytes) = allocated_by(|| large.feed(large_input));
    assert_eq!(small_effects, large_effects);
    assert!(small_bytes > 0, "the step allocates its effects");
    assert_eq!(small_bytes, large_bytes, "{input:?}");
    small_effects
}

/// The answer to request `number` from `from`, `reply`.
fn answer(from: Instance, number: u64, reply: Reply) -> QueueInput {
    QueueInput::Answer {
        from,
        answer: Answer {
            number: RequestNumber(number),
            outcome: Outcome::Result(reply),
            probes: Vec::new(),
            detail: String::new(),
        },
    }
}

/// The one batch of `kind` ([`desk::kind`]) among `effects`.
fn sent_batch(effects: &[Effect], kind: &str) -> (u64, verbatim_model::batch::Batch) {
    let mut asked: Vec<(u64, verbatim_model::batch::Batch)> = effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Queue(QueueEffect::Send { ask, .. }) if desk::kind(&ask.request) == kind => {
                match &ask.request {
                    verbatim_model::request::Question::Batch(batch) => {
                        Some((ask.number.0, batch.clone()))
                    }
                    verbatim_model::request::Question::Legacy(_) => None,
                }
            }
            _ => None,
        })
        .collect();
    let (Some(one), None) = (asked.pop(), asked.pop()) else {
        panic!("one batch asked: {effects:?}");
    };
    one
}

/// The answer to request `number` from `from`, `reply`.
fn answer_found(
    from: Instance,
    (number, batch): &(u64, verbatim_model::batch::Batch),
    found: &Found,
) -> QueueInput {
    QueueInput::Answer {
        from,
        answer: Answer {
            number: RequestNumber(*number),
            outcome: readers::outcome(batch, found),
            probes: Vec::new(),
            detail: String::new(),
        },
    }
}

/// The chain depths a focus step is measured at, a tenfold step apart.
const SHORTER_CHAIN: u64 = 1_000;

/// Feeds `input` to `shorter` and `longer`, as [`same`] does, where the
/// step may grow with the chain: what each said must be the same, and the
/// bytes the longer step allocated no more than twice the depths' ratio
/// times the shorter's, so that growth is at most proportional to the
/// depth. The factor of two is a vector's growth by doubling, which this
/// allocator counts at each reallocation; growth with the square of the
/// depth would be a hundred times.
fn proportional(shorter: &mut Desk, longer: &mut Desk, input: &QueueInput) -> Vec<Effect> {
    shorter.at_us += 1_000;
    longer.at_us += 1_000;
    let shorter_input = input.clone();
    let longer_input = input.clone();
    let (shorter_effects, shorter_bytes) = allocated_by(|| shorter.feed(shorter_input));
    let (longer_effects, longer_bytes) = allocated_by(|| longer.feed(longer_input));
    let said = |effects: &[Effect]| -> Vec<Effect> {
        effects
            .iter()
            .filter(|effect| matches!(effect, Effect::Speak(..)))
            .cloned()
            .collect()
    };
    assert_eq!(said(&shorter_effects), said(&longer_effects));
    assert!(shorter_bytes > 0, "the step allocates its effects");
    let ratio = usize::try_from(LARGE_CHAIN / SHORTER_CHAIN).expect("a small ratio");
    assert!(
        longer_bytes <= 2 * shorter_bytes * ratio,
        "{longer_bytes} bytes at {LARGE_CHAIN} ancestors, {shorter_bytes} at {SHORTER_CHAIN}: {input:?}"
    );
    shorter_effects
}

#[test]
fn a_focus_step_allocates_no_more_than_in_proportion_to_the_ancestor_chain() {
    // ADR 0003's focus request carries the ancestors Core already knows,
    // and its answer is joined to the committed chain, so a focus step's
    // allocation grows with the depth of the focus: inherent, as NVDA walks
    // the same chain, and an allocation is no cost to design around
    // without a measurement (CLAUDE.md). What is pinned is that it grows
    // no faster than the depth: a chain ten times deeper allocates at most
    // ten times as much, with a vector's doubling allowed for.
    //
    // A focus moving to a new button under three new containers: told,
    // read by the listener, and completed by the outpost.
    let mut shorter = desk_with_chain(SHORTER_CHAIN);
    let mut longer = desk_with_chain(LARGE_CHAIN);
    let tell = QueueInput::Tell {
        from: LISTENER,
        tell: Box::new(focus_tell(ONE, 2, shorter.at_us + 1_000)),
    };
    let effects = proportional(&mut shorter, &mut longer, &tell);
    let read = sent_batch(&effects, "ReadObject");
    let effects = proportional(
        &mut shorter,
        &mut longer,
        &answer_found(LISTENER, &read, &listener_read(ONE, 2)),
    );
    let completion = sent_batch(&effects, "Completion");
    let completed = focus_read_of(
        ONE,
        &node(2, Role::Button, "Next"),
        &[
            node(3, Role::Dialog, "Options"),
            node(4, Role::Group, "General"),
            node(5, Role::Group, "Display"),
        ],
    );
    let effects = proportional(
        &mut shorter,
        &mut longer,
        &answer_found(ONE.instance(), &completion, &Found::Read(completed)),
    );
    assert!(
        effects
            .iter()
            .any(|effect| matches!(effect, Effect::Speak(..))),
        "the focus is spoken"
    );
}

/// A long line of text, as the outpost sends the caret's line: just under
/// the 64 KB a chunk may carry.
fn long_line(anchor: u64, offset: u32) -> TextChunk {
    TextChunk {
        unit: TextUnit::Line,
        text: "word ".repeat(13_000),
        start: TextAnchor(anchor),
        offset,
        languages: Vec::new(),
        first: false,
        last: false,
        truncated: false,
        formats: Vec::new(),
    }
}

/// The character `text` at anchor `anchor`.
fn character(text: &str, anchor: u64) -> TextChunk {
    TextChunk {
        unit: TextUnit::Character,
        text: text.to_owned(),
        ..long_line(anchor, 0)
    }
}

fn caret_on(line: TextChunk) -> CaretReport {
    CaretReport {
        line,
        selection: None,
    }
}

/// A desk whose focus is an edit field under a chain of `depth` groups,
/// its caret on a long line.
fn editing_with_chain(depth: u64) -> Desk {
    let mut desk = Desk::new();
    let mut read = focus_read_of(ONE, &node(1, Role::EditableText, "Body"), &chain(depth));
    read.caret = FocusCaret::Read(Box::new(FocusCaretRead {
        caret: caret_on(long_line(10, 0)),
        selected_text: None,
        baseline: TextAnchor(11),
    }));
    let _ = desk.focus_read(ONE, read);
    desk
}

#[test]
fn text_steps_allocate_the_same_whatever_the_ancestor_chain() {
    let mut small = editing_with_chain(SMALL_CHAIN);
    let mut large = editing_with_chain(LARGE_CHAIN);
    // Right arrow, held by the hook and bound to the field: its starting
    // read, its pass and watch, and its watch's answer, which speaks the
    // character at the caret.
    let binding = small.state.caret_binding().expect("the field has text");
    assert_eq!(large.state.caret_binding(), Some(binding.clone()));
    let press = QueueInput::Press(Box::new(Press {
        key_seq: KeySeq::new(1),
        pressed_at_us: small.at_us + 1_000,
        action: PressAction::CaretKey {
            key: CaretKey {
                motion: CaretMotion::NextCharacter,
                select: false,
            },
            held: Some(HeldStroke {
                stroke: Stroke {
                    vk: 0x27,
                    scan_code: 0x4D,
                    extended: true,
                    sequence: 1,
                },
                modifiers: Vec::new(),
            }),
            bound: Some(Box::new(binding)),
        },
    }));
    let effects = same(&mut small, &mut large, &press);
    let (starting, batch) = text_batch(&effects);
    let facts = caret_batches::read(caret_on(long_line(10, 0)), character("w", 12));
    let effects = same(
        &mut small,
        &mut large,
        &answer(
            ONE.instance(),
            starting,
            Reply::Steps(caret_batches::answer(&batch, &facts)),
        ),
    );
    let (watch, batch) = text_batch(&effects);
    let facts = caret_batches::Facts {
        waited: Some(verbatim_model::batch::Waited::Effect),
        ..caret_batches::read(caret_on(long_line(13, 1)), character("o", 14))
    };
    let effects = same(
        &mut small,
        &mut large,
        &answer(
            ONE.instance(),
            watch,
            Reply::Steps(caret_batches::answer(&batch, &facts)),
        ),
    );
    assert_eq!(
        effects,
        [
            Effect::Speak(
                Utterance {
                    trace_id: TraceId::from_raw(9),
                    priority: SpeechPriority::Queued,
                    segments: vec![UtteranceSegment::new(SegmentContent::Character(
                        "o".to_owned()
                    ))],
                    source: None,
                    validity: None,
                    say_all: false,
                },
                Some(KeySeq::new(1)),
            ),
            Effect::Queue(QueueEffect::WakeAt(Some(505_000))),
        ],
        "the key's character spoken"
    );
    // The review cursor's next word, on the field's text.
    let next_word = QueueInput::Press(Box::new(Press {
        key_seq: KeySeq::new(2),
        pressed_at_us: small.at_us + 1_000,
        action: PressAction::Command {
            command: ReviewCommand::ReviewNextWord,
            repeat: 0,
        },
    }));
    let _ = same(&mut small, &mut large, &next_word);
}

#[test]
fn a_terminal_line_allocates_the_same_whatever_the_ancestor_chain() {
    // The simulated terminal read as its outpost reads it, focused inside
    // a chain of groups: a first read, then a line of output, told and
    // read.
    let session = |depth: u64| {
        let mut outpost =
            terminal_support::Outpost::new(verbatim_term_sim::Flavor::ConsoleHost, 40, 5, 20);
        outpost.sim.write(b"PS> ");
        let mut session = terminal_support::Session::inside(outpost, depth);
        // Playback's marks are entries of their own, outside the line's
        // step.
        session.manual_play = true;
        session
    };
    let mut small = session(SMALL_CHAIN);
    let mut large = session(LARGE_CHAIN);
    let ((), small_bytes) = allocated_by(|| small.write("\r\ntotal 42\r\nPS> ", 200));
    let ((), large_bytes) = allocated_by(|| large.write("\r\ntotal 42\r\nPS> ", 200));
    small.play();
    large.play();
    let heard = small.heard();
    assert_eq!(
        heard,
        [
            terminal_support::said(None, "total 42"),
            terminal_support::said(None, "PS>")
        ]
    );
    assert_eq!(large.heard(), heard);
    assert_eq!(
        small.log.last().map(|(_, effects)| effects.clone()),
        large.log.last().map(|(_, effects)| effects.clone())
    );
    assert_eq!(small_bytes, large_bytes);
}
