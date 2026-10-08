//! Core's allocation invariant: a reduce step allocates the same number of
//! bytes whatever the size of the state it changes, and a flight-recorder
//! checkpoint of the state allocates a small, fixed number of bytes
//! whatever the state's size.
//!
//! This test binary installs a global allocator that counts the bytes each
//! thread asks for, so the test runner's parallel threads do not disturb
//! one another's counts. Today the parts of the state that grow with the
//! application are the focus ancestor chain (and the navigator that follows
//! focus) and the text Core keeps for the caret and the review cursor (a
//! line, up to 64 KB, and the lines of the caret's last 8 timed reports,
//! shared rather than copied by a checkpoint), and a terminal's output waiting to be spoken is
//! bounded by the flood policy; the large state here has a previous focus with
//! 10,000 ancestors and the small one has 10. Each step's input is the same
//! in both cases, so only the state varies.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use verbatim_core::{SrState, reduce};
use verbatim_model::{
    Backend, CaretKey, CaretMotion, CaretReply, CaretReport, Effect, FetchResult, Input,
    NodeDetails, NodeId, NodeSnapshot, NormalizedEvent, Pid, ReviewCommand, Role, StateSet,
    TextAnchor, TextChunk, TextReply, TextUnit, TraceId,
};

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

fn node(id: u64, role: Role, name: &str) -> NodeSnapshot {
    NodeSnapshot {
        id: NodeId::new(id),
        backend: Backend::Uia,
        role,
        name: Some(name.to_owned()),
        value: None,
        states: StateSet::new(),
        details: NodeDetails::default(),
    }
}

fn focus_event(node: NodeSnapshot, ancestors: Vec<NodeSnapshot>) -> Input {
    Input::Event {
        trace_id: TraceId::mint(),
        observed_at_ms: 0,
        source: Pid(1),
        backend: Backend::Uia,
        window: None,
        event: NormalizedEvent::FocusChanged {
            node,
            foreground: false,
            ancestors,
            ancestors_unknown: false,
            selected_child: None,
        },
    }
}

/// A state whose focus, and the navigator following it, sit under a chain
/// of `depth` named groups. The chain's node ids (from 100,000) never
/// collide with the nodes the measured steps use.
fn state_with_chain(depth: u64) -> SrState {
    let ancestors = (0..depth)
        .map(|index| node(100_000 + index, Role::Group, &format!("Group {index}")))
        .collect();
    let mut state = SrState::new();
    let _ = reduce(
        &mut state,
        &focus_event(node(1, Role::Button, "Previous"), ancestors),
    );
    state
}

/// A focus moving to a new button under three new containers.
fn next_focus() -> Input {
    focus_event(
        node(2, Role::Button, "Next"),
        vec![
            node(3, Role::Dialog, "Options"),
            node(4, Role::Group, "General"),
            node(5, Role::Group, "Display"),
        ],
    )
}

#[test]
fn a_focus_step_allocates_the_same_whatever_the_ancestor_chain() {
    let input = next_focus();
    let mut small = state_with_chain(SMALL_CHAIN);
    let mut large = state_with_chain(LARGE_CHAIN);

    let (small_effects, small_bytes) = allocated_by(|| reduce(&mut small, &input));
    let (large_effects, large_bytes) = allocated_by(|| reduce(&mut large, &input));

    assert_eq!(small_effects, large_effects);
    assert!(small_bytes > 0, "the step allocates its effects");
    assert_eq!(small_bytes, large_bytes);
}

#[test]
fn a_navigation_step_allocates_the_same_whatever_the_ancestor_chain() {
    let command = Input::Command {
        trace_id: TraceId::mint(),
        command: ReviewCommand::Parent,
        repeat: 0,
    };
    let mut small = state_with_chain(SMALL_CHAIN);
    let mut large = state_with_chain(LARGE_CHAIN);

    let (small_effects, small_bytes) = allocated_by(|| reduce(&mut small, &command));
    let (large_effects, large_bytes) = allocated_by(|| reduce(&mut large, &command));
    assert_eq!(small_effects, large_effects);
    assert_eq!(small_bytes, large_bytes);

    let Some(Effect::Fetch(query)) = small_effects.first() else {
        panic!("expected a navigation fetch, got {small_effects:?}");
    };
    let completion = Input::FetchCompleted {
        trace_id: TraceId::mint(),
        query_id: query.query_id,
        kind: query.kind,
        result: FetchResult::Node(node(6, Role::Pane, "Toolbar")),
    };
    let (small_effects, small_bytes) = allocated_by(|| reduce(&mut small, &completion));
    let (large_effects, large_bytes) = allocated_by(|| reduce(&mut large, &completion));
    assert_eq!(small_effects, large_effects);
    assert!(!small_effects.is_empty(), "the navigator moves and speaks");
    assert_eq!(small_bytes, large_bytes);
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

/// A state whose focus is an edit field under a chain of `depth` groups,
/// its caret on a long line, reported often enough to fill Core's history
/// of timed caret reports.
fn editing_with_chain(depth: u64) -> SrState {
    let ancestors = (0..depth)
        .map(|index| node(100_000 + index, Role::Group, &format!("Group {index}")))
        .collect();
    let mut state = SrState::new();
    let _ = reduce(
        &mut state,
        &focus_event(node(1, Role::EditableText, "Body"), ancestors),
    );
    for observed_at_ms in 1..=10 {
        let _ = reduce(&mut state, &caret_moved(long_line(10, 0), observed_at_ms));
    }
    state
}

fn caret_moved(line: TextChunk, observed_at_ms: u64) -> Input {
    Input::Event {
        trace_id: TraceId::mint(),
        observed_at_ms,
        source: Pid(1),
        backend: Backend::Uia,
        window: None,
        event: NormalizedEvent::CaretMoved {
            node_id: NodeId::new(1),
            caret: CaretReport {
                line,
                selection: None,
            },
        },
    }
}

#[test]
fn text_steps_allocate_the_same_whatever_the_ancestor_chain() {
    let mut small = editing_with_chain(SMALL_CHAIN);
    let mut large = editing_with_chain(LARGE_CHAIN);
    let mut same = |input: &Input| {
        let (small_effects, small_bytes) = allocated_by(|| reduce(&mut small, input));
        let (large_effects, large_bytes) = allocated_by(|| reduce(&mut large, input));
        assert_eq!(small_effects, large_effects);
        assert_eq!(small_bytes, large_bytes, "{input:?}");
        small_effects
    };

    let effects = same(&Input::CaretKey {
        trace_id: TraceId::mint(),
        key: CaretKey {
            motion: CaretMotion::NextCharacter,
            select: false,
        },
        pressed_at_ms: 100,
    });
    let Some(Effect::Text(request)) = effects.first() else {
        panic!("expected a caret wait, got {effects:?}");
    };
    let _ = same(&Input::TextCompleted {
        trace_id: TraceId::mint(),
        query_id: request.query_id,
        reply: TextReply::Caret(Box::new(CaretReply {
            same_line: None,
            moved: true,
            read_at_ms: 101,
            caret: CaretReport {
                line: long_line(10, 1),
                selection: None,
            },
            unit: None,
            selection_changes: Vec::new(),
        })),
    });
    let _ = same(&Input::Command {
        trace_id: TraceId::mint(),
        command: ReviewCommand::ReviewNextWord,
        repeat: 0,
    });
}

#[test]
fn a_terminal_line_allocates_the_same_whatever_the_ancestor_chain() {
    let terminal_with_chain = |depth: u64| {
        let ancestors = (0..depth)
            .map(|index| node(100_000 + index, Role::Group, &format!("Group {index}")))
            .collect();
        let mut state = SrState::new();
        let _ = reduce(
            &mut state,
            &focus_event(node(1, Role::Terminal, "Terminal"), ancestors),
        );
        state
    };
    let mut small = terminal_with_chain(SMALL_CHAIN);
    let mut large = terminal_with_chain(LARGE_CHAIN);
    let output = Input::Event {
        trace_id: TraceId::mint(),
        observed_at_ms: 0,
        source: Pid(1),
        backend: Backend::Uia,
        window: None,
        event: NormalizedEvent::TerminalOutput {
            node_id: NodeId::new(1),
            output: verbatim_model::TerminalOutput {
                above: Vec::new(),
                changed: None,
                head: Vec::new(),
                skipped: None,
                lines: vec!["total 42".to_owned()],
            },
        },
    };
    let (small_effects, small_bytes) = allocated_by(|| reduce(&mut small, &output));
    let (large_effects, large_bytes) = allocated_by(|| reduce(&mut large, &output));
    assert_eq!(small_effects, large_effects);
    assert!(!small_effects.is_empty(), "the line is spoken");
    assert_eq!(small_bytes, large_bytes);
}

#[test]
fn the_themes_fetches_change_the_state_without_allocating() {
    let mut state = state_with_chain(LARGE_CHAIN);
    let fetches = verbatim_model::Fetches {
        description: false,
        ..verbatim_model::Fetches::default()
    };
    let (effects, bytes) = allocated_by(|| reduce(&mut state, &Input::Fetches(fetches)));
    assert_eq!(effects, Vec::<Effect>::new());
    assert_eq!(bytes, 0, "the fetches are held inline");
    assert_eq!(state.fetches(), fetches);
}

#[test]
fn a_checkpoint_with_text_allocates_a_small_fixed_amount() {
    let small = editing_with_chain(SMALL_CHAIN);
    let large = editing_with_chain(LARGE_CHAIN);
    let (_, small_bytes) = allocated_by(|| small.clone());
    let (_, large_bytes) = allocated_by(|| large.clone());
    assert_eq!(small_bytes, large_bytes);
    // The caret's line and the review cursor's are shared, not copied.
    assert!(
        large_bytes < 1024,
        "a checkpoint allocated {large_bytes} bytes"
    );
}

#[test]
fn a_checkpoint_allocates_a_small_fixed_amount_whatever_the_ancestor_chain() {
    let small = state_with_chain(SMALL_CHAIN);
    let large = state_with_chain(LARGE_CHAIN);

    let (small_copy, small_bytes) = allocated_by(|| small.clone());
    let (large_copy, large_bytes) = allocated_by(|| large.clone());

    assert_eq!(small_copy, small);
    assert_eq!(large_copy, large);
    assert_eq!(small_bytes, large_bytes);
    // The focus and navigator snapshots are copied; the chain is shared.
    assert!(
        large_bytes < 1024,
        "a checkpoint allocated {large_bytes} bytes"
    );
}
