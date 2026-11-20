//! A window's destruction as the listener sends it
//! (`ListenerToCore::Destroyed`, which the shell queues as
//! `QueueInput::WindowDestroyed`): routed by its listener's generation,
//! passed to the outpost of the raising thread's process when one is
//! ready, and Core's own records of the window cleared whatever the
//! process; the followed node's window destroyed sends its generation a
//! `Follow` naming nothing (Table 10, column E,
//! 2026-11-19).

use verbatim_model::request::{
    Follow, Outcome as WireOutcome, QueueEffect, QueueInput, Reply, Request as Wire,
};
use verbatim_model::{
    Backend, Effect as Said, NodeDetails, NodeId, NodeSnapshot, OutpostId, Pid, Role, StateSet,
    TraceId, WindowHandle,
};

use crate::entries::tests::{
    APP, GENERATION, Held, LISTENER, NOTHING, OUTPOST, answer_outcome, feed_at, ready,
};
use crate::event_queue::{Input, Instance};
use crate::focus::versions::Focus;

/// The window the followed node is in.
const NODE_WINDOW: WindowHandle = WindowHandle(11);

/// The listener's destroy of `window`, raised by a thread of `process`,
/// from the listener instance `from`.
fn destroyed(from: Instance, window: WindowHandle, process: Option<Pid>) -> QueueInput {
    QueueInput::WindowDestroyed {
        from,
        window,
        process,
    }
}

/// The reducer fed `input` from the shell at `at_us`: its effects, but
/// the wake.
fn shell_feeds(held: &mut Held, at_us: u64, input: QueueInput) -> Vec<Said> {
    let mut effects = crate::reduce(
        &mut held.state,
        &mut held.runtime,
        &verbatim_model::Input::Queue {
            trace_id: TraceId::from_raw(5),
            at_us,
            input,
        },
    );
    effects.retain(|effect| !matches!(effect, Said::Queue(QueueEffect::WakeAt(_))));
    effects
}

/// What a test reads of `effects`: the window's destruction sent, each
/// request by its reader, number and request, and anything else whole.
fn read(effects: &[Said]) -> Vec<String> {
    effects
        .iter()
        .map(|effect| match effect {
            Said::Queue(QueueEffect::WindowDestroyed { to, window }) => {
                format!("destroyed {window:?} to {:?} {}", to.reader, to.generation)
            }
            Said::Queue(QueueEffect::Send { to, ask }) => {
                format!("send {:?} {} {:?}", to.reader, ask.number.0, ask.request)
            }
            other => format!("{other:?}"),
        })
        .collect()
}

/// The destruction of `window` sent to APP's ready outpost, as a test
/// reads it.
fn sent_destroy(window: WindowHandle) -> String {
    format!("destroyed {window:?} to Outpost({APP:?}) {GENERATION}")
}

/// The `Follow` naming nothing, request `number`, as a test reads it.
fn follows_nothing(number: u64) -> String {
    format!(
        "send Outpost({APP:?}) {number} {:?}",
        Wire::Follow(Follow { node: None })
    )
}

/// A UI Automation node of `role` of APP's ready generation, in
/// [`NODE_WINDOW`], named to its outpost as the committed focus's node:
/// the follows of `held` name it.
fn followed(held: &mut Held, role: Role) -> Focus {
    let mut focus = crate::focus::versions::tests::focus(10);
    focus.context.snapshot.id = NodeId::new(1).with_outpost(OutpostId(GENERATION));
    assert_eq!(focus.kept.place.window, NODE_WINDOW);
    let node = NodeSnapshot {
        id: focus.context.snapshot.id,
        backend: Backend::Uia,
        role,
        name: None,
        value: None,
        states: StateSet::default(),
        details: NodeDetails::default(),
    };
    let _ = held
        .state
        .follows
        .committed(Some((&focus.kept, &node, None)), false);
    assert!(held.state.follows.named(APP).is_some());
    focus
}

/// A ready Core whose focused terminal, APP's node in [`NODE_WINDOW`], is
/// followed and baselining, its baseline taken from what Core wants to
/// send.
fn terminal_followed() -> Held {
    let mut state = ready();
    let focus = followed(&mut state, Role::Terminal);
    crate::terminal::reading::committed(
        &mut state.state,
        Some(crate::terminal::reading::Terminal {
            node: focus.context.snapshot.id,
            window: focus.kept.asked,
            console: false,
        }),
        Vec::new(),
        500,
    );
    let _ = crate::terminal::reading::wanted_for_tests(&mut state.state);
    state
}

/// With the raising process's outpost ready, its destruction is sent to
/// that outpost.
#[test]
fn a_destroy_with_a_ready_outpost_is_sent_to_it() {
    let mut state = ready();
    assert_eq!(
        read(&shell_feeds(
            &mut state,
            1_000,
            destroyed(LISTENER, WindowHandle(7), Some(APP))
        )),
        [sent_destroy(WindowHandle(7))]
    );
}

/// With no outpost of the raising process, nothing is sent, and no outpost
/// is started.
#[test]
fn a_destroy_of_a_process_with_no_outpost_sends_nothing() {
    let mut state = ready();
    assert_eq!(
        read(&shell_feeds(
            &mut state,
            1_000,
            destroyed(LISTENER, WindowHandle(7), Some(Pid(99)))
        )),
        Vec::<String>::new()
    );
}

/// With no process, as the listener sends a destroy whose thread it could
/// not open, Core's records of the window are cleared, the followed node's
/// among them, and no destroy is sent to an outpost: the `Follow` naming
/// nothing goes to the generation named, which Core knows itself.
#[test]
fn a_destroy_with_no_process_clears_cores_records_and_sends_no_destroy() {
    let mut state = ready();
    let _ = followed(&mut state, Role::Button);
    assert_eq!(
        read(&shell_feeds(
            &mut state,
            1_000,
            destroyed(LISTENER, NODE_WINDOW, None)
        )),
        [follows_nothing(1)]
    );
    assert_eq!(state.state.follows.named(APP), None);
}

/// The listener replaced, its replacement started, a late destroy from
/// the replaced one is dropped as a stale frame, and clears nothing.
#[test]
fn a_destroy_from_a_replaced_listener_is_dropped() {
    let mut state = ready();
    let _ = followed(&mut state, Role::Button);
    let _ = shell_feeds(
        &mut state,
        500,
        QueueInput::Process(verbatim_model::request::ProcessFact::ListenerStarted(2)),
    );
    assert_eq!(
        shell_feeds(
            &mut state,
            1_000,
            destroyed(LISTENER, NODE_WINDOW, Some(APP))
        ),
        [Said::Queue(QueueEffect::StaleFrame(LISTENER))]
    );
    assert!(state.state.follows.named(APP).is_some());
}

/// The followed node's window destroyed: the destruction goes to the
/// outpost first, then one `Follow` naming nothing, as Core's own request,
/// so it runs after the outpost's forget; the node is named no more, and
/// a second destroy of the window sends no second `Follow`.
#[test]
fn the_followed_nodes_window_destroyed_sends_one_follow_naming_nothing_after_the_destroy() {
    let mut state = ready();
    let _ = followed(&mut state, Role::Button);
    assert_eq!(
        read(&shell_feeds(
            &mut state,
            1_000,
            destroyed(LISTENER, NODE_WINDOW, Some(APP))
        )),
        [sent_destroy(NODE_WINDOW), follows_nothing(1)]
    );
    assert_eq!(state.state.follows.named(APP), None);
    assert_eq!(
        read(&shell_feeds(
            &mut state,
            2_000,
            destroyed(LISTENER, NODE_WINDOW, Some(APP))
        )),
        [sent_destroy(NODE_WINDOW)]
    );
}

/// The named generation ended before its node's window is destroyed: its
/// end named nothing, so no `Follow` is sent and no outpost is started.
#[test]
fn the_followed_nodes_window_destroyed_after_its_generation_ended_sends_nothing() {
    let mut state = ready();
    let _ = followed(&mut state, Role::Button);
    assert_eq!(
        feed_at(&mut state, 500, Input::ReaderEnded(OUTPOST)),
        NOTHING
    );
    assert_eq!(
        read(&shell_feeds(
            &mut state,
            1_000,
            destroyed(LISTENER, NODE_WINDOW, Some(APP))
        )),
        Vec::<String>::new()
    );
}

/// The `Follow` naming nothing, sent when the focused terminal's window is
/// destroyed, is Core's own: its answer, "not there" or applied, says
/// nothing and changes nothing of the focused terminal, which build 3's
/// Table 1 reads only for the `Follow` a commit sent.
#[test]
fn the_follow_naming_nothing_answered_changes_no_terminal_state() {
    for outcome in [WireOutcome::NotThere, WireOutcome::Result(Reply::Followed)] {
        let mut state = terminal_followed();
        let before = state.state.terminal_focus.focused;
        assert_eq!(
            read(&shell_feeds(
                &mut state,
                1_000,
                destroyed(LISTENER, NODE_WINDOW, Some(APP))
            )),
            [sent_destroy(NODE_WINDOW), follows_nothing(1)]
        );
        assert_eq!(
            feed_at(&mut state, 2_000, answer_outcome(OUTPOST, 1, outcome)),
            NOTHING
        );
        assert_eq!(state.state.terminal_focus.focused, before);
        assert_eq!(
            crate::terminal::reading::wanted_for_tests(&mut state.state),
            []
        );
    }
}
