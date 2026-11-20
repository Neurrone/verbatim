//! The count of the messages that carry nodes, and the pure methods of the
//! messages.

use super::*;

/// Every sample a `next` function chains, from the first: each match of
/// `next` names every variant without a wildcard, so a new variant must
/// be given its place in the chain, and so in the list below it.
fn chain<T>(next: impl Fn(Option<&T>) -> Option<T>) -> Vec<T> {
    let mut samples: Vec<T> = Vec::new();
    while let Some(sample) = next(samples.last()) {
        samples.push(sample);
    }
    samples
}

/// The variant's name, from its debug form.
fn name(message: &impl std::fmt::Debug) -> String {
    format!("{message:?}")
        .split(|c: char| !c.is_alphanumeric())
        .next()
        .unwrap_or_default()
        .to_owned()
}

fn timing() -> EventTiming {
    EventTiming::default()
}

/// One sample of each [`ListenerToCore`] variant, after `previous`.
fn next_listener_to_core(previous: Option<&ListenerToCore>) -> Option<ListenerToCore> {
    let Some(previous) = previous else {
        return Some(ListenerToCore::Ready {
            outpost_pid: Pid(1),
        });
    };
    Some(match previous {
        ListenerToCore::Ready { .. } => ListenerToCore::ExitHandled { pid: Pid(1) },
        ListenerToCore::ExitHandled { .. } => ListenerToCore::Progress {
            change: crate::BarChange {
                percent: crate::Percent::parse("41").expect("a number"),
                place: (10, 20),
                in_foreground: true,
            },
            timing: timing(),
        },
        ListenerToCore::Progress { .. } => ListenerToCore::Foreground {
            window: 1,
            root_owner: 1,
            process: 2,
            observed_at_us: 1,
            thread: 1,
            console: false,
            integrity: None,
        },
        ListenerToCore::Foreground { .. } => ListenerToCore::Tell(Box::new(crate::tell::Event {
            trace_id: crate::TraceId::from_raw(5),
            kind: crate::event_kind::EventKind::MsaaFocus,
            process: Pid(2),
            application: "notepad".to_owned(),
            thread: 3,
            at_us: 1,
            object: crate::tell::EventObject::Msaa {
                window: crate::WindowHandle(1),
                object: -4,
                child: 0,
            },
            cached: crate::property::PropertyValues(Vec::new()),
            payload: crate::tell::Payload::None,
            window: crate::tell::TellWindow::NoWindow,
            checked: None,
        })),
        ListenerToCore::Tell(_) => ListenerToCore::Destroyed {
            window: crate::WindowHandle(1),
            process: None,
            at_us: 1,
        },
        ListenerToCore::Destroyed { .. } => ListenerToCore::Answer(crate::request::Answer {
            number: crate::request::RequestNumber(1),
            outcome: crate::request::Outcome::NotThere,
            probes: Vec::new(),
            detail: String::new(),
        }),
        ListenerToCore::Answer(_) => ListenerToCore::Pong { written_at_us: 0 },
        ListenerToCore::Pong { .. } => ListenerToCore::Fault {
            detail: "a fault".to_owned(),
        },
        ListenerToCore::Fault { .. } => ListenerToCore::IdleAnswer { round: 1 },
        ListenerToCore::IdleAnswer { .. } => return None,
    })
}

/// Only an answer with a result carries nodes, as the outpost and
/// verbatim.exe count them.
#[test]
fn only_an_answer_with_a_result_carries_nodes() {
    let carried: Vec<(String, bool)> = super::samples::outpost_to_core()
        .iter()
        .map(|message| (name(message), message.carries_nodes()))
        .collect();
    assert_eq!(
        carried,
        [
            ("Ready".to_owned(), false),
            ("SpareReady".to_owned(), false),
            ("Pong".to_owned(), false),
            ("Fault".to_owned(), false),
            ("TargetExited".to_owned(), false),
            ("IdleAnswer".to_owned(), false),
            ("Tell".to_owned(), false),
            ("Answer".to_owned(), false),
        ]
    );
    let answer = OutpostToCore::Answer(crate::request::Answer {
        number: crate::request::RequestNumber(1),
        outcome: crate::request::Outcome::Result(crate::request::Reply::Followed),
        probes: Vec::new(),
        detail: String::new(),
    });
    assert!(answer.carries_nodes());
}

/// Only a listener tell naming a kept element carries a token, as the
/// listener numbers its messages and verbatim.exe counts them.
#[test]
fn only_a_tell_naming_a_kept_element_carries_a_token() {
    let carried: Vec<(String, bool)> = chain(next_listener_to_core)
        .iter()
        .map(|message| (name(message), message.carries_tokens()))
        .collect();
    assert_eq!(
        carried,
        [
            ("Ready".to_owned(), false),
            ("ExitHandled".to_owned(), false),
            ("Progress".to_owned(), false),
            ("Foreground".to_owned(), false),
            ("Tell".to_owned(), false),
            ("Destroyed".to_owned(), false),
            ("Answer".to_owned(), false),
            ("Pong".to_owned(), false),
            ("Fault".to_owned(), false),
            ("IdleAnswer".to_owned(), false),
        ]
    );
    let told = ListenerToCore::Tell(Box::new(crate::tell::Event {
        trace_id: crate::TraceId::from_raw(5),
        kind: crate::event_kind::EventKind::UiaFocus,
        process: Pid(2),
        application: "notepad".to_owned(),
        thread: 3,
        at_us: 1,
        object: crate::tell::EventObject::Uia {
            token: crate::tell::ElementToken(7),
            runtime_id: crate::property::RuntimeId(vec![42, 1]),
        },
        cached: crate::property::PropertyValues(Vec::new()),
        payload: crate::tell::Payload::None,
        window: crate::tell::TellWindow::NoWindow,
        checked: None,
    }));
    assert_eq!(told.carried_token(), Some(crate::tell::ElementToken(7)));
}

#[test]
fn a_pongs_trip_back_runs_from_its_write_to_verbatim_exes_read() {
    let mut pong = OutpostToCore::Pong {
        parked_count: 0,
        written_at_us: 0,
    };
    pong.written(7_000);
    let OutpostToCore::Pong { written_at_us, .. } = pong else {
        unreachable!("a pong stays a pong");
    };
    assert_eq!(hop_us(written_at_us, 7_250), Some(250));
}

#[test]
fn a_bar_changes_trip_runs_from_the_listeners_write_to_verbatim_exes_read() {
    let mut changed = ListenerToCore::Progress {
        change: crate::BarChange {
            percent: crate::Percent::parse("41").expect("a number"),
            place: (10, 20),
            in_foreground: true,
        },
        timing: EventTiming::default(),
    };
    changed.written(7_000);
    let ListenerToCore::Progress { timing, .. } = changed else {
        unreachable!("a bar's change stays a bar's change");
    };
    assert_eq!(hop_us(timing.listener_written_at_us, 7_250), Some(250));
}

/// A terminal's change tell arriving on an outpost's pipe names its node by
/// that pipe's generation, as every other message's node ids do: Core
/// compares it with the focused terminal's node, and a tell left unstamped
/// named no terminal Core follows.
#[test]
fn a_terminal_change_tell_is_stamped_with_its_outpost() {
    use crate::event_kind::EventKind;
    use crate::property::PropertyValues;
    use crate::tell::{Event, EventObject, Payload, TellWindow};
    let mut message = OutpostToCore::Tell(Box::new(Event {
        trace_id: crate::TraceId::from_raw(1),
        kind: EventKind::TerminalChanged,
        process: crate::Pid(4),
        application: "mockapp".to_owned(),
        thread: 0,
        at_us: 10,
        object: EventObject::Terminal {
            node: NodeId::in_outpost(OutpostId(0), 3),
            window: crate::WindowHandle(9),
        },
        cached: PropertyValues(Vec::new()),
        payload: Payload::None,
        window: TellWindow::NoWindow,
        checked: None,
    }));
    message.assign_outpost(OutpostId(7));
    let OutpostToCore::Tell(tell) = message else {
        panic!("still a tell");
    };
    assert_eq!(
        tell.object,
        EventObject::Terminal {
            node: NodeId::in_outpost(OutpostId(7), 3),
            window: crate::WindowHandle(9),
        }
    );
}
