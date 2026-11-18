//! The listener's told elements Core holds (`SrState::held_tokens`, the
//! listener group's design, "Told elements released by Core's held view"),
//! driven through Core's event queue in the listener's order: a token is
//! held from its tell while its entry waits or runs, and afterwards only
//! while it is the readers' last focus; an entry refused, a duplicate, a
//! read that fails and a kind Core never reads leave it out; and the set
//! never takes back a token it dropped.

mod desk;

use std::collections::BTreeSet;

use desk::{App, Desk, LISTENER, ONE, focus_read_of, focus_tell, listener_read};
use verbatim_core::HeldTokens;
use verbatim_model::event_kind::EventKind;
use verbatim_model::request::{
    Instance, Outcome, ProcessFact, QueueInput, Reader, Reply, Request as Wire,
};
use verbatim_model::tell::{ElementToken, Payload, Read};
use verbatim_model::{Backend, NodeDetails, NodeSnapshot, Role, StateSet, TraceId};

/// A button of `app`, node `number`, named `name`.
fn button(app: App, number: u64, name: &str) -> NodeSnapshot {
    NodeSnapshot {
        id: app.node(number),
        backend: Backend::Uia,
        role: Role::Button,
        name: Some(name.to_owned()),
        value: None,
        states: StateSet::new(),
        details: NodeDetails::default(),
    }
}

/// The tokens Core holds of the listener generation 1.
fn held(tokens: &[u64]) -> HeldTokens {
    HeldTokens {
        generation: LISTENER.generation,
        tokens: tokens.iter().copied().map(ElementToken).collect(),
    }
}

/// The listener's UI Automation focus on node `number` of `app` is told:
/// its entry waits for its read, the read Core asks of the listener.
fn told(desk: &mut Desk, app: App, number: u64) {
    desk.app(app);
    let tell = focus_tell(app, number, desk.at_us + 1_000);
    let _ = desk.tell(LISTENER, tell);
}

/// A focus's token is held while its read is out and then as the readers'
/// last focus; a newer focus executed takes its place, and the older
/// token leaves the set.
#[test]
fn a_focus_is_held_while_it_runs_and_as_the_readers_last_focus() {
    let mut desk = Desk::new();
    assert_eq!(desk.state.held_tokens(), Some(held(&[])));
    let _ = desk.focus(ONE, &button(ONE, 5, "OK"), &[]);
    assert_eq!(desk.state.held_tokens(), Some(held(&[5])));
    told(&mut desk, ONE, 6);
    assert_eq!(desk.state.held_tokens(), Some(held(&[5, 6])));
    let [(read, _, Wire::ReadObject(_))] = desk.sent[..] else {
        panic!("one listener read asked: {:?}", desk.sent);
    };
    let _ = desk.answer(
        LISTENER,
        read,
        Outcome::Result(Reply::Object(Box::new(listener_read(ONE, 6)))),
    );
    assert_eq!(desk.state.held_tokens(), Some(held(&[5, 6])));
    let [(completion, to, Wire::CompleteUia(_))] = desk.sent[..] else {
        panic!("one completion asked: {:?}", desk.sent);
    };
    let _ = desk.answer(
        to,
        completion,
        Outcome::Result(Reply::Focus(Box::new(focus_read_of(
            ONE,
            &button(ONE, 6, "Cancel"),
            &[],
        )))),
    );
    assert_eq!(desk.state.held_tokens(), Some(held(&[6])));
}

/// A focus refused at its turn, here one whose element no longer has the
/// keyboard focus, and a duplicate of the readers' last focus, are ended
/// at the front: neither token is held, and the last focus's stays.
#[test]
fn a_refused_focus_and_a_duplicate_are_not_held() {
    let mut desk = Desk::new();
    let _ = desk.focus(ONE, &button(ONE, 5, "OK"), &[]);
    let mut refused = focus_tell(ONE, 7, desk.at_us + 1_000);
    if let Some(checked) = refused.checked.as_mut() {
        checked.keyboard_focus = Some(Read::Answered(false));
    }
    assert_eq!(desk.tell(LISTENER, refused), []);
    assert_eq!(desk.state.held_tokens(), Some(held(&[5])));
    let _ = desk.duplicate(ONE, 5);
    assert_eq!(desk.state.held_tokens(), Some(held(&[5])));
}

/// A tell of a listener generation that is not the one running is
/// dropped at its arrival: never held.
#[test]
fn a_tell_dropped_at_its_arrival_is_never_held() {
    let mut desk = Desk::new();
    let ended = Instance {
        reader: Reader::Listener,
        generation: 0,
    };
    let tell = focus_tell(ONE, 5, desk.at_us + 1_000);
    assert_eq!(desk.tell(ended, tell), []);
    assert_eq!(desk.state.held_tokens(), Some(held(&[])));
}

/// A notification is told with a token, and Core never reads it: it is
/// never held.
#[test]
fn a_notification_is_never_held() {
    let mut desk = Desk::new();
    let mut tell = focus_tell(ONE, 9, desk.at_us + 1_000);
    tell.kind = EventKind::UiaNotification;
    tell.trace_id = TraceId::from_raw(21);
    tell.checked = None;
    tell.payload = Payload::Notification {
        text: "Saved".to_owned(),
        kind: verbatim_model::NotificationKind::ActionCompleted,
        processing: verbatim_model::NotificationProcessing::All,
        activity: String::new(),
    };
    let _ = desk.tell(LISTENER, tell);
    assert_eq!(desk.state.held_tokens(), Some(held(&[])));
}

/// The set never takes back a token it dropped: a focus whose read fails
/// leaves the readers' last focus as it was; after the listener ends, a
/// re-take by the application's outpost holds no token, and the next
/// listener's set starts empty.
#[test]
fn the_set_never_regains_a_dropped_token() {
    let mut desk = Desk::new();
    let _ = desk.focus(ONE, &button(ONE, 5, "OK"), &[]);
    let _ = desk.focus(ONE, &button(ONE, 6, "Cancel"), &[]);
    told(&mut desk, ONE, 7);
    let [(read, _, Wire::ReadObject(_))] = desk.sent[..] else {
        panic!("one listener read asked: {:?}", desk.sent);
    };
    let _ = desk.answer(LISTENER, read, Outcome::NotThere);
    assert_eq!(desk.state.held_tokens(), Some(held(&[6])));
    let _ = desk.step(QueueInput::ReaderEnded(LISTENER));
    let next = Instance {
        reader: Reader::Listener,
        generation: 2,
    };
    let _ = desk.step(QueueInput::Process(ProcessFact::ListenerStarted(2)));
    assert_eq!(
        desk.state.held_tokens(),
        Some(HeldTokens {
            generation: 2,
            tokens: BTreeSet::new(),
        })
    );
    let _ = desk.step(QueueInput::ReaderReady(next));
    assert_eq!(
        desk.state.held_tokens(),
        Some(HeldTokens {
            generation: 2,
            tokens: BTreeSet::new(),
        })
    );
}
