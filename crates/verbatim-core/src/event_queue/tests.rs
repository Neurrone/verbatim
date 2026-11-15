//! The driver's five rules and every cell of Table 2 (a request in Core)
//! and Table 18 (the recovery set), from
//! `docs/design/readers-tell-core-asks-build1.md`, against a small logic
//! whose entries each ask a chain of property reads and say each outcome.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Waker};

use verbatim_model::property::{PropertyId, PropertyValues};
use verbatim_model::request::{
    Answer, Ask, Cancel, CancelReason, Outcome as WireOutcome, Refusal, Reply, Request as Wire,
    RequestNumber, WindowVerdict,
};
use verbatim_model::{NodeId, Pid, WindowHandle};

use super::driver::{Core, Cx};
use super::inbox::{Effect, Input, Timed};
use super::limiter::{MsaaKey, Place};
use super::request::{Outcome, Request};
use super::table::Asker;
use super::{
    FOCUS_DUE_US, Foreground, Function, HANG_ROUND_US, Instance, Logic, OTHER_DUE_US, Reader,
    RequestWindow,
};
use crate::focus::backend::Backend;

/// The application of the tests' window.
const APP: Pid = Pid(1);
/// The window the tests' requests name, inside the top-level window 10.
const WINDOW: RequestWindow = RequestWindow {
    handle: WindowHandle(11),
    top_level: WindowHandle(10),
    root_owner: WindowHandle(10),
    process: APP,
    thread: 3,
    by_class: Some(Backend::Msaa),
};
/// The application's outpost.
const OUTPOST: Instance = Instance {
    reader: Reader::Outpost(APP),
    generation: 1,
};
/// Its replacement.
const NEXT_OUTPOST: Instance = Instance {
    reader: Reader::Outpost(APP),
    generation: 2,
};
/// The application's top-level window in front.
const IN_FRONT: Foreground = Foreground {
    window: WindowHandle(10),
    root_owner: WindowHandle(10),
    process: APP,
};
/// Another window of the same application in front, outside the request
/// window's tree.
const SAME_APP_ELSEWHERE: Foreground = Foreground {
    window: WindowHandle(30),
    root_owner: WindowHandle(30),
    process: APP,
};
/// Another application in front.
const ELSEWHERE: Foreground = Foreground {
    window: WindowHandle(20),
    root_owner: WindowHandle(20),
    process: Pid(2),
};
/// An asker outside the queue.
const ASKER: Asker = Asker(5);

/// The tests' logic.
struct Test;

/// The tests' entries.
#[derive(Clone, Debug, PartialEq)]
enum Entry {
    /// Asks `reads` property reads in a chain under one due time, saying
    /// each outcome, and stops at the first that is not a result.
    Read { reads: u8, exempt: bool },
    /// A key, which withdraws a read at the front, and says how many results came before it and how many entries wait behind it.
    Key,
    /// The entry the recovery set appends when its window answers again.
    Recovered(WindowHandle),
    /// Waits on something other than an ask.
    Stray,
}

/// The tests' read.
#[derive(Debug)]
struct ReadName;

impl Request for ReadName {
    type Reply = PropertyValues;

    fn reader(&self) -> Reader {
        OUTPOST.reader
    }

    fn window(&self) -> RequestWindow {
        WINDOW
    }

    fn wire(&self) -> Wire {
        wire()
    }

    fn decode(reply: Reply) -> Option<PropertyValues> {
        match reply {
            Reply::Properties(values) => Some(values),
            _ => None,
        }
    }
}

/// The read as it is sent.
fn wire() -> Wire {
    Wire::Properties {
        node: NodeId::new(1),
        set: vec![PropertyId::Name],
    }
}

impl Logic for Test {
    type Entry = Entry;
    type Facts = u32;
    type Said = String;

    fn run(mut cx: Cx<Self>, entry: Entry) -> Function {
        Box::pin(async move {
            match entry {
                Entry::Read { reads, .. } => {
                    let due_us = cx.due_in(OTHER_DUE_US);
                    for _ in 0..reads {
                        let outcome = cx.ask(ReadName, due_us).await;
                        let result = matches!(outcome, Outcome::Result(_));
                        if result {
                            *cx.facts_mut() += 1;
                        }
                        cx.emit(format!("{outcome:?}"));
                        if !result {
                            break;
                        }
                    }
                }
                Entry::Key => {
                    let words = format!(
                        "key after {} results, {} waiting",
                        *cx.facts(),
                        cx.waiting().len()
                    );
                    cx.emit(words);
                }
                Entry::Recovered(window) => cx.emit(format!("recovered {}", window.0)),
                Entry::Stray => std::future::pending::<()>().await,
            }
        })
    }

    fn exempt_from_foreground(entry: &Entry, _: &RequestWindow) -> bool {
        matches!(entry, Entry::Read { exempt: true, .. })
    }

    fn withdraws(_: &u32, front: &Entry, arriving: &Entry) -> bool {
        matches!(front, Entry::Read { .. }) && *arriving == Entry::Key
    }

    fn recovered(_: &u32, window: &RequestWindow) -> Option<Entry> {
        Some(Entry::Recovered(window.handle))
    }

    fn place(_: &u32, _: &Entry) -> Place {
        Place::Other {
            merges: false,
            focus_kind: false,
            ends_run: true,
        }
    }

    fn merges(_: &u32, _: &Entry, _: &Entry) -> bool {
        false
    }

    fn exempt_from_cap(_: &u32, _: &MsaaKey) -> bool {
        false
    }

    fn run_entry(_: Vec<Entry>) -> Entry {
        unreachable!("the tests' entries make no run")
    }

    fn ends_at_foreground(_: &u32, _: &Entry, _: &Foreground) -> bool {
        false
    }

    fn foreground_marker(_: &Foreground) -> Option<Entry> {
        None
    }
}

/// A read entry of `reads` requests.
fn read(reads: u8) -> Input<Entry> {
    Input::Entry(Entry::Read {
        reads,
        exempt: false,
    })
}

/// The send of request `number`.
fn send(number: u64) -> Effect<String> {
    Effect::Send {
        to: OUTPOST,
        ask: Ask {
            number: RequestNumber(number),
            window: WINDOW.handle,
            verdict: WindowVerdict::Msaa,
            request: wire(),
        },
    }
}

/// The cancel of request `number` for `reason`.
fn cancel(number: u64, reason: CancelReason) -> Effect<String> {
    Effect::Cancel {
        to: OUTPOST,
        cancel: Cancel {
            number: RequestNumber(number),
            reason,
        },
    }
}

/// An answer from `from` to request `number`.
fn answer(from: Instance, number: u64, outcome: WireOutcome) -> Input<Entry> {
    Input::Answer {
        from,
        answer: Answer {
            number: RequestNumber(number),
            outcome,
            probes: Vec::new(),
            detail: String::new(),
        },
    }
}

/// A result.
fn result() -> WireOutcome {
    WireOutcome::Result(Reply::Properties(PropertyValues::default()))
}

/// What the tests' entry says of a result.
fn said_result() -> Effect<String> {
    said("Result(PropertyValues([]))")
}

/// What an entry said.
fn said(text: &str) -> Effect<String> {
    Effect::Said(text.to_owned())
}

/// A request from outside the queue.
fn ask_outside() -> Input<Entry> {
    Input::AskOutside {
        asker: ASKER,
        reader: OUTPOST.reader,
        window: WINDOW,
        request: wire(),
        due_after_us: OTHER_DUE_US,
    }
}

/// Feeds `input` at `at_us`.
fn feed(core: &mut Core<Test>, at_us: u64, input: Input<Entry>) -> Vec<Effect<String>> {
    core.feed(Timed { at_us, input })
}

/// A core whose outpost is ready, with `foreground` in front.
fn core_with(foreground: Foreground) -> Core<Test> {
    let mut core = Core::default();
    assert_eq!(feed(&mut core, 0, Input::ReaderReady(OUTPOST)), []);
    assert_eq!(feed(&mut core, 0, Input::Foreground(foreground)), []);
    core
}

/// A core with the tests' application in front and one read out, as
/// request 1.
fn core_with_read_out() -> Core<Test> {
    let mut core = core_with(IN_FRONT);
    assert_eq!(feed(&mut core, 1_000, read(1)), [send(1)]);
    core
}

/// A core with the tests' application in front and one request outside
/// the queue out, as request 1.
fn core_with_outside_out() -> Core<Test> {
    let mut core = core_with(IN_FRONT);
    assert_eq!(feed(&mut core, 1_000, ask_outside()), [send(1)]);
    core
}

/// The entries `core` has ended, as the audit writes them.
fn ended(core: &Core<Test>) -> Vec<String> {
    core.ended.clone()
}

/// The audit's line for a read entry with these answers.
fn read_ended(reads: u8, answers: &str) -> String {
    format!("Read {{ reads: {reads}, exempt: false }} [{answers}]")
}

// Table 2, in the queue, waiting for its outpost.

#[test]
fn a_waiting_request_is_sent_when_its_reader_can_take_it() {
    let mut core = Core::<Test>::default();
    assert_eq!(feed(&mut core, 0, Input::Foreground(IN_FRONT)), []);
    assert_eq!(feed(&mut core, 1_000, read(1)), []);
    assert_eq!(
        feed(&mut core, 2_000, Input::ReaderReady(OUTPOST)),
        [send(1)]
    );
    assert_eq!(
        feed(&mut core, 3_000, answer(OUTPOST, 1, result())),
        [said_result()]
    );
}

#[test]
fn a_request_the_shell_finds_hung_is_answered_not_responding_with_the_sound_when_attended() {
    let mut core = core_with_read_out();
    assert_eq!(
        feed(&mut core, 1_500, Input::RefusedHung(RequestNumber(1))),
        [Effect::NotRespondingSound, said("NotResponding")]
    );
    // The window joined the recovery set: the half-second round armed at
    // the send goes on checking it.
    assert_eq!(core.next_due(), Some(1_000 + HANG_ROUND_US));
}

#[test]
fn a_request_the_shell_finds_hung_while_unattended_plays_no_sound_and_is_not_watched() {
    let mut core = core_with(ELSEWHERE);
    let exempt = Input::Entry(Entry::Read {
        reads: 1,
        exempt: true,
    });
    assert_eq!(feed(&mut core, 1_000, exempt), [send(1)]);
    assert_eq!(
        feed(&mut core, 1_500, Input::RefusedHung(RequestNumber(1))),
        [said("NotResponding")]
    );
    assert_eq!(core.next_due(), None);
}

#[test]
fn a_waiting_request_is_not_checked_for_a_hang() {
    let mut core = Core::<Test>::default();
    assert_eq!(feed(&mut core, 0, Input::Foreground(IN_FRONT)), []);
    assert_eq!(feed(&mut core, 1_000, read(1)), []);
    assert_eq!(
        feed(
            &mut core,
            2_000,
            Input::HangCheck {
                window: WINDOW.handle,
                hung: true
            }
        ),
        []
    );
    assert_eq!(core.next_due(), Some(1_000 + OTHER_DUE_US));
}

#[test]
fn a_waiting_request_past_its_due_time_ends_silently_and_is_never_sent() {
    let mut core = Core::<Test>::default();
    assert_eq!(feed(&mut core, 0, Input::Foreground(IN_FRONT)), []);
    assert_eq!(feed(&mut core, 1_000, read(1)), []);
    assert_eq!(
        feed(&mut core, 1_000 + OTHER_DUE_US, Input::Tick),
        [said("NoLongerAwaited(DueTime)")]
    );
    assert_eq!(
        feed(&mut core, OTHER_DUE_US + 2_000, Input::ReaderReady(OUTPOST)),
        []
    );
}

#[test]
fn a_waiting_request_waits_on_when_a_starting_reader_fails() {
    let mut core = Core::<Test>::default();
    assert_eq!(feed(&mut core, 0, Input::Foreground(IN_FRONT)), []);
    assert_eq!(feed(&mut core, 1_000, read(1)), []);
    assert_eq!(feed(&mut core, 2_000, Input::ReaderEnded(OUTPOST)), []);
    assert_eq!(
        feed(&mut core, 3_000, Input::ReaderReady(NEXT_OUTPOST)),
        [Effect::Send {
            to: NEXT_OUTPOST,
            ask: Ask {
                number: RequestNumber(1),
                window: WINDOW.handle,
                verdict: WindowVerdict::Msaa,
                request: wire(),
            },
        }]
    );
}

#[test]
fn a_waiting_request_ends_unsent_when_the_foreground_leaves_its_window() {
    let mut core = Core::<Test>::default();
    assert_eq!(feed(&mut core, 0, Input::Foreground(IN_FRONT)), []);
    assert_eq!(feed(&mut core, 1_000, read(1)), []);
    assert_eq!(
        feed(&mut core, 2_000, Input::Foreground(SAME_APP_ELSEWHERE)),
        [said("NoLongerAwaited(Foreground)")]
    );
    assert_eq!(feed(&mut core, 3_000, Input::ReaderReady(OUTPOST)), []);
}

#[test]
fn a_waiting_request_stays_when_the_foreground_holds_its_window() {
    let mut core = Core::<Test>::default();
    assert_eq!(feed(&mut core, 0, Input::Foreground(ELSEWHERE)), []);
    assert_eq!(feed(&mut core, 1_000, read(1)), []);
    assert_eq!(feed(&mut core, 2_000, Input::Foreground(IN_FRONT)), []);
    assert_eq!(
        feed(&mut core, 3_000, Input::ReaderReady(OUTPOST)),
        [send(1)]
    );
}

#[test]
fn a_waiting_request_withdrawn_by_its_entry_is_dropped_unsent() {
    let mut core = Core::<Test>::default();
    assert_eq!(feed(&mut core, 0, Input::Foreground(IN_FRONT)), []);
    assert_eq!(feed(&mut core, 1_000, read(1)), []);
    assert_eq!(
        feed(&mut core, 2_000, Input::Entry(Entry::Key)),
        [
            said("NoLongerAwaited(Withdrawn)"),
            said("key after 0 results, 0 waiting")
        ]
    );
    assert_eq!(feed(&mut core, 3_000, Input::ReaderReady(OUTPOST)), []);
}

// Table 2, in the queue, out.

#[test]
fn a_request_out_whose_window_is_found_hung_stops_with_the_sound_when_attended() {
    let mut core = core_with_read_out();
    assert_eq!(
        feed(
            &mut core,
            1_200,
            Input::HangCheck {
                window: WINDOW.handle,
                hung: true
            }
        ),
        [
            cancel(1, CancelReason::Hung),
            Effect::NotRespondingSound,
            said("NoLongerAwaited(Hung)")
        ]
    );
}

#[test]
fn a_request_out_found_hung_while_unattended_stops_with_no_sound() {
    let mut core = core_with(ELSEWHERE);
    let exempt = Input::Entry(Entry::Read {
        reads: 1,
        exempt: true,
    });
    assert_eq!(feed(&mut core, 1_000, exempt), [send(1)]);
    assert_eq!(
        feed(
            &mut core,
            1_200,
            Input::HangCheck {
                window: WINDOW.handle,
                hung: true
            }
        ),
        [cancel(1, CancelReason::Hung), said("NoLongerAwaited(Hung)")]
    );
    assert_eq!(core.next_due(), None);
}

#[test]
fn a_request_out_past_its_due_time_stops_with_no_sound() {
    let mut core = core_with_read_out();
    assert_eq!(
        feed(&mut core, 1_000 + OTHER_DUE_US, Input::Tick),
        [
            cancel(1, CancelReason::DueTime),
            said("NoLongerAwaited(DueTime)")
        ]
    );
}

#[test]
fn a_chain_of_requests_shares_one_due_time_from_the_first_ask() {
    let mut core = core_with(IN_FRONT);
    assert_eq!(feed(&mut core, 1_000, read(2)), [send(1)]);
    assert_eq!(
        feed(&mut core, 5_000_000, answer(OUTPOST, 1, result())),
        [said_result(), send(2)]
    );
    assert_eq!(core.next_due(), Some(1_000 + HANG_ROUND_US));
    assert_eq!(
        feed(&mut core, 1_000 + OTHER_DUE_US, Input::Tick),
        [
            cancel(2, CancelReason::DueTime),
            said("NoLongerAwaited(DueTime)")
        ]
    );
}

#[test]
fn a_request_out_is_judged_by_its_entrys_function_on_each_outcome() {
    for (outcome, words) in [
        (result(), "Result(PropertyValues([]))"),
        (
            WireOutcome::Refused(Refusal::Unsupported),
            "Refused(Reader(Unsupported))",
        ),
        (WireOutcome::NotThere, "NotThere"),
        (WireOutcome::NotResponding, "NotResponding"),
        (WireOutcome::Ended, "ReaderEnded"),
    ] {
        let mut core = core_with_read_out();
        assert_eq!(
            feed(&mut core, 2_000, answer(OUTPOST, 1, outcome)),
            [said(words)]
        );
        assert_eq!(ended(&core), [read_ended(1, words)]);
    }
}

#[test]
fn a_request_out_ends_reader_ended_when_its_reader_ends() {
    let mut core = core_with_read_out();
    assert_eq!(
        feed(&mut core, 2_000, Input::ReaderEnded(OUTPOST)),
        [said("ReaderEnded")]
    );
}

#[test]
fn a_request_out_stops_when_the_foreground_leaves_its_window_even_in_its_own_application() {
    let mut core = core_with_read_out();
    assert_eq!(
        feed(&mut core, 2_000, Input::Foreground(SAME_APP_ELSEWHERE)),
        [
            cancel(1, CancelReason::Foreground),
            said("NoLongerAwaited(Foreground)")
        ]
    );
}

#[test]
fn a_request_out_stays_when_the_foreground_holds_its_window() {
    let mut core = core_with_read_out();
    assert_eq!(feed(&mut core, 2_000, Input::Foreground(IN_FRONT)), []);
    assert_eq!(
        feed(&mut core, 3_000, answer(OUTPOST, 1, result())),
        [said_result()]
    );
}

#[test]
fn an_exempt_entrys_request_out_stays_when_the_foreground_changes() {
    let mut core = core_with(IN_FRONT);
    let exempt = Input::Entry(Entry::Read {
        reads: 1,
        exempt: true,
    });
    assert_eq!(feed(&mut core, 1_000, exempt), [send(1)]);
    assert_eq!(feed(&mut core, 2_000, Input::Foreground(ELSEWHERE)), []);
    assert_eq!(
        feed(&mut core, 3_000, answer(OUTPOST, 1, result())),
        [said_result()]
    );
}

#[test]
fn a_request_out_withdrawn_by_its_entry_is_cancelled_with_no_sound() {
    let mut core = core_with_read_out();
    assert_eq!(
        feed(&mut core, 2_000, Input::Entry(Entry::Key)),
        [
            cancel(1, CancelReason::Withdrawn),
            said("NoLongerAwaited(Withdrawn)"),
            said("key after 0 results, 0 waiting")
        ]
    );
}

// Table 2, outside the queue.

#[test]
fn a_request_outside_the_queue_waits_for_its_reader_and_is_sent_when_ready() {
    let mut core = Core::<Test>::default();
    assert_eq!(feed(&mut core, 1_000, ask_outside()), []);
    assert_eq!(
        feed(&mut core, 2_000, Input::ReaderReady(OUTPOST)),
        [send(1)]
    );
}

#[test]
fn a_request_outside_the_queue_found_hung_by_the_shell_is_answered_not_responding() {
    let mut core = core_with_outside_out();
    assert_eq!(
        feed(&mut core, 1_500, Input::RefusedHung(RequestNumber(1))),
        [
            Effect::NotRespondingSound,
            Effect::Answered {
                asker: ASKER,
                outcome: Outcome::NotResponding
            }
        ]
    );
}

#[test]
fn a_waiting_request_outside_the_queue_past_its_due_time_answers_its_asker_unsent() {
    let mut core = Core::<Test>::default();
    assert_eq!(feed(&mut core, 1_000, ask_outside()), []);
    assert_eq!(
        feed(&mut core, 1_000 + OTHER_DUE_US, Input::Tick),
        [Effect::Answered {
            asker: ASKER,
            outcome: Outcome::NoLongerAwaited(CancelReason::DueTime)
        }]
    );
    assert_eq!(
        feed(&mut core, OTHER_DUE_US + 2_000, Input::ReaderReady(OUTPOST)),
        []
    );
}

#[test]
fn a_waiting_request_outside_the_queue_waits_for_the_next_reader_when_one_fails() {
    let mut core = Core::<Test>::default();
    assert_eq!(feed(&mut core, 1_000, ask_outside()), []);
    assert_eq!(feed(&mut core, 2_000, Input::ReaderEnded(OUTPOST)), []);
    assert_eq!(
        feed(&mut core, 3_000, Input::ReaderReady(OUTPOST)),
        [send(1)]
    );
}

#[test]
fn a_waiting_request_outside_the_queue_is_untouched_by_the_foreground() {
    let mut core = Core::<Test>::default();
    assert_eq!(feed(&mut core, 1_000, ask_outside()), []);
    assert_eq!(feed(&mut core, 2_000, Input::Foreground(ELSEWHERE)), []);
    assert_eq!(
        feed(&mut core, 3_000, Input::ReaderReady(OUTPOST)),
        [send(1)]
    );
}

#[test]
fn a_waiting_request_outside_the_queue_withdrawn_is_dropped_unsent() {
    let mut core = Core::<Test>::default();
    assert_eq!(feed(&mut core, 1_000, ask_outside()), []);
    assert_eq!(
        feed(&mut core, 2_000, Input::WithdrawOutside(ASKER)),
        [Effect::Answered {
            asker: ASKER,
            outcome: Outcome::NoLongerAwaited(CancelReason::Withdrawn)
        }]
    );
    assert_eq!(feed(&mut core, 3_000, Input::ReaderReady(OUTPOST)), []);
}

#[test]
fn a_request_outside_the_queue_found_hung_is_cancelled_with_the_sound_and_answered() {
    let mut core = core_with_outside_out();
    assert_eq!(
        feed(
            &mut core,
            1_200,
            Input::HangCheck {
                window: WINDOW.handle,
                hung: true
            }
        ),
        [
            cancel(1, CancelReason::Hung),
            Effect::NotRespondingSound,
            Effect::Answered {
                asker: ASKER,
                outcome: Outcome::NoLongerAwaited(CancelReason::Hung)
            }
        ]
    );
}

#[test]
fn a_request_outside_the_queue_past_its_due_time_is_cancelled_and_answered() {
    let mut core = core_with_outside_out();
    assert_eq!(
        feed(&mut core, 1_000 + OTHER_DUE_US, Input::Tick),
        [
            cancel(1, CancelReason::DueTime),
            Effect::Answered {
                asker: ASKER,
                outcome: Outcome::NoLongerAwaited(CancelReason::DueTime)
            }
        ]
    );
}

#[test]
fn a_request_outside_the_queue_passes_its_answer_to_its_asker() {
    let mut core = core_with_outside_out();
    assert_eq!(
        feed(&mut core, 2_000, answer(OUTPOST, 1, result())),
        [Effect::Answered {
            asker: ASKER,
            outcome: Outcome::Result(Reply::Properties(PropertyValues::default()))
        }]
    );
}

#[test]
fn a_request_outside_the_queue_answers_reader_ended_to_its_asker() {
    let mut core = core_with_outside_out();
    assert_eq!(
        feed(&mut core, 2_000, Input::ReaderEnded(OUTPOST)),
        [Effect::Answered {
            asker: ASKER,
            outcome: Outcome::ReaderEnded
        }]
    );
}

#[test]
fn a_request_outside_the_queue_out_is_untouched_by_the_foreground() {
    let mut core = core_with_outside_out();
    assert_eq!(feed(&mut core, 2_000, Input::Foreground(ELSEWHERE)), []);
    assert_eq!(
        feed(&mut core, 3_000, answer(OUTPOST, 1, result())),
        [Effect::Answered {
            asker: ASKER,
            outcome: Outcome::Result(Reply::Properties(PropertyValues::default()))
        }]
    );
}

#[test]
fn a_request_outside_the_queue_out_withdrawn_is_cancelled() {
    let mut core = core_with_outside_out();
    assert_eq!(
        feed(&mut core, 2_000, Input::WithdrawOutside(ASKER)),
        [
            cancel(1, CancelReason::Withdrawn),
            Effect::Answered {
                asker: ASKER,
                outcome: Outcome::NoLongerAwaited(CancelReason::Withdrawn)
            }
        ]
    );
}

// Table 2, no longer awaited and answered.

/// A core whose request 1 Core stopped waiting for at its due time.
fn core_with_late_request() -> Core<Test> {
    let mut core = core_with_read_out();
    assert_eq!(
        feed(&mut core, 1_000 + OTHER_DUE_US, Input::Tick),
        [
            cancel(1, CancelReason::DueTime),
            said("NoLongerAwaited(DueTime)")
        ]
    );
    core
}

#[test]
fn a_request_no_longer_awaited_is_not_checked_for_a_hang_again() {
    let mut core = core_with_late_request();
    assert_eq!(core.next_due(), None);
    assert_eq!(
        feed(
            &mut core,
            OTHER_DUE_US + 2_000,
            Input::HangCheck {
                window: WINDOW.handle,
                hung: true
            }
        ),
        []
    );
}

#[test]
fn a_late_answer_is_dropped_by_number() {
    let mut core = core_with_late_request();
    assert_eq!(
        feed(
            &mut core,
            OTHER_DUE_US + 2_000,
            answer(OUTPOST, 1, result())
        ),
        [Effect::AnswerDropped {
            number: RequestNumber(1),
            late: true
        }]
    );
}

#[test]
fn a_request_no_longer_awaited_is_forgotten_when_its_reader_ends() {
    let mut core = core_with_late_request();
    assert_eq!(
        feed(&mut core, OTHER_DUE_US + 2_000, Input::ReaderEnded(OUTPOST)),
        []
    );
    assert_eq!(
        feed(
            &mut core,
            OTHER_DUE_US + 3_000,
            answer(OUTPOST, 1, result())
        ),
        [Effect::AnswerDropped {
            number: RequestNumber(1),
            late: false
        }]
    );
}

#[test]
fn a_request_no_longer_awaited_is_untouched_by_the_foreground() {
    let mut core = core_with_late_request();
    assert_eq!(
        feed(
            &mut core,
            OTHER_DUE_US + 2_000,
            Input::Foreground(ELSEWHERE)
        ),
        []
    );
}

#[test]
fn an_answered_request_takes_nothing_more() {
    let mut core = core_with_read_out();
    assert_eq!(
        feed(&mut core, 2_000, answer(OUTPOST, 1, result())),
        [said_result()]
    );
    assert_eq!(
        feed(&mut core, 3_000, answer(OUTPOST, 1, result())),
        [Effect::AnswerDropped {
            number: RequestNumber(1),
            late: false
        }]
    );
    for input in [
        Input::HangCheck {
            window: WINDOW.handle,
            hung: true,
        },
        Input::Tick,
        Input::ReaderEnded(OUTPOST),
        Input::Foreground(ELSEWHERE),
        Input::ReaderReady(OUTPOST),
    ] {
        assert_eq!(feed(&mut core, 4_000 + OTHER_DUE_US, input), []);
    }
}

#[test]
fn an_answer_from_an_instance_the_request_did_not_go_to_is_dropped() {
    let mut core = core_with_read_out();
    assert_eq!(
        feed(&mut core, 2_000, answer(NEXT_OUTPOST, 1, result())),
        [Effect::AnswerDropped {
            number: RequestNumber(1),
            late: false
        }]
    );
    assert_eq!(
        feed(&mut core, 3_000, answer(OUTPOST, 1, result())),
        [said_result()]
    );
}

// The half-second round and Table 18, the recovery set.

#[test]
fn the_round_checks_every_request_outs_window_each_half_second_while_one_is_out() {
    let mut core = core_with_read_out();
    assert_eq!(core.next_due(), Some(1_000 + HANG_ROUND_US));
    assert_eq!(
        feed(&mut core, 1_000 + HANG_ROUND_US, Input::Tick),
        [Effect::CheckHung(vec![WINDOW.handle])]
    );
    assert_eq!(
        feed(
            &mut core,
            1_100 + HANG_ROUND_US,
            Input::HangCheck {
                window: WINDOW.handle,
                hung: false
            }
        ),
        []
    );
    assert_eq!(core.next_due(), Some(1_000 + 2 * HANG_ROUND_US));
    assert_eq!(
        feed(
            &mut core,
            1_200 + HANG_ROUND_US,
            answer(OUTPOST, 1, result())
        ),
        [said_result()]
    );
    assert_eq!(core.next_due(), None);
}

/// A core whose request 1 was stopped as hung while attended, its window
/// now watched.
fn core_watching() -> Core<Test> {
    let mut core = core_with_read_out();
    assert_eq!(
        feed(
            &mut core,
            1_200,
            Input::HangCheck {
                window: WINDOW.handle,
                hung: true
            }
        ),
        [
            cancel(1, CancelReason::Hung),
            Effect::NotRespondingSound,
            said("NoLongerAwaited(Hung)")
        ]
    );
    core
}

#[test]
fn a_watched_window_that_answers_again_leaves_the_set_and_its_entry_joins_the_queue() {
    let mut core = core_watching();
    assert_eq!(core.next_due(), Some(1_000 + HANG_ROUND_US));
    assert_eq!(
        feed(&mut core, 1_000 + HANG_ROUND_US, Input::Tick),
        [Effect::CheckHung(vec![WINDOW.handle])]
    );
    assert_eq!(
        feed(
            &mut core,
            1_100 + HANG_ROUND_US,
            Input::HangCheck {
                window: WINDOW.handle,
                hung: true
            }
        ),
        []
    );
    assert_eq!(
        feed(
            &mut core,
            1_100 + 2 * HANG_ROUND_US,
            Input::HangCheck {
                window: WINDOW.handle,
                hung: false
            }
        ),
        [said("recovered 11")]
    );
    assert_eq!(core.next_due(), None);
}

#[test]
fn a_watched_window_stays_when_another_request_to_it_is_found_hung() {
    let mut core = core_watching();
    assert_eq!(feed(&mut core, 2_000, read(1)), [send(2)]);
    assert_eq!(
        feed(&mut core, 2_100, Input::RefusedHung(RequestNumber(2))),
        [Effect::NotRespondingSound, said("NotResponding")]
    );
    assert_eq!(
        feed(
            &mut core,
            2_200,
            Input::HangCheck {
                window: WINDOW.handle,
                hung: false
            }
        ),
        [said("recovered 11")]
    );
}

#[test]
fn a_watched_window_leaves_the_set_when_it_is_destroyed() {
    let mut core = core_watching();
    assert_eq!(
        feed(&mut core, 2_000, Input::WindowDestroyed(WINDOW.handle)),
        []
    );
    assert_eq!(core.next_due(), None);
}

#[test]
fn a_watched_window_leaves_the_set_when_its_application_loses_attention() {
    let mut core = core_watching();
    assert_eq!(feed(&mut core, 2_000, Input::Foreground(ELSEWHERE)), []);
    assert_eq!(core.next_due(), None);
}

#[test]
fn a_watched_window_stays_when_another_window_of_its_application_comes_in_front() {
    let mut core = core_watching();
    assert_eq!(
        feed(&mut core, 2_000, Input::Foreground(SAME_APP_ELSEWHERE)),
        []
    );
    assert_eq!(core.next_due(), Some(1_000 + HANG_ROUND_US));
}

#[test]
fn a_watched_window_leaves_the_set_when_its_outpost_ends() {
    let mut core = core_watching();
    assert_eq!(feed(&mut core, 2_000, Input::ReaderEnded(OUTPOST)), []);
    assert_eq!(core.next_due(), None);
}

// The five rules.

#[test]
#[should_panic(expected = "an entry's function waited on something other than an ask")]
fn a_function_waiting_on_anything_but_an_ask_is_caught() {
    let mut core = Core::<Test>::default();
    let _ = feed(&mut core, 1_000, Input::Entry(Entry::Stray));
}

/// The driver's sources, as the rule that only an ask is awaited reads
/// them.
const SOURCES: [(&str, &str); 4] = [
    ("mod.rs", include_str!("mod.rs")),
    ("driver.rs", include_str!("driver.rs")),
    ("inbox.rs", include_str!("inbox.rs")),
    ("table.rs", include_str!("table.rs")),
];

/// Only an ask is awaited: the one future the event queue implements is
/// `Asking`. A later author adding another awaitable changes this list.
#[test]
fn the_only_future_the_queue_implements_is_the_ask() {
    let futures: Vec<(&str, &str)> = SOURCES
        .iter()
        .flat_map(|(file, source)| {
            source
                .lines()
                .filter(|line| line.contains("Future for"))
                .map(move |line| (*file, line.trim()))
        })
        .collect();
    assert_eq!(
        futures,
        [(
            "driver.rs",
            "impl<L: Logic, R: Request> Future for Asking<'_, L, R> {"
        )]
    );
}

#[test]
fn a_result_that_does_not_decode_is_refused_undecodable_and_keeps_the_reader() {
    let mut core = core_with(IN_FRONT);
    assert_eq!(feed(&mut core, 1_000, read(1)), [send(1)]);
    assert_eq!(
        feed(
            &mut core,
            2_000,
            answer(OUTPOST, 1, WireOutcome::Result(Reply::Children(Vec::new())))
        ),
        [
            Effect::Undecodable(RequestNumber(1)),
            said("Refused(Undecodable)")
        ]
    );
    assert_eq!(feed(&mut core, 3_000, read(1)), [send(2)]);
}

#[test]
fn an_ask_let_go_after_it_was_made_is_counted() {
    let mut core = core_with(IN_FRONT);
    // A front with no function yet, whose function asks by hand.
    assert_eq!(
        feed(&mut core, 1_000, Input::Entry(Entry::Key)),
        [said("key after 0 results, 0 waiting")]
    );
    core.shared.borrow_mut().state.queue.front = Some(super::driver::Front {
        entry: Entry::Key,
        answers: Vec::new(),
        asked: None,
    });
    let mut cx = core.cx_for_tests();
    let mut asking = cx.ask(ReadName, FOCUS_DUE_US);
    let polled = Pin::new(&mut asking).poll(&mut Context::from_waker(Waker::noop()));
    assert!(polled.is_pending());
    drop(asking);
    assert_eq!(core.unanswered_for_tests(), 1);
}

#[test]
fn every_entry_ends_with_each_of_its_requests_answered_once() {
    let mut core = core_with(IN_FRONT);
    assert_eq!(feed(&mut core, 1_000, read(2)), [send(1)]);
    assert_eq!(feed(&mut core, 1_100, read(1)), []);
    assert_eq!(
        feed(&mut core, 2_000, answer(OUTPOST, 1, result())),
        [said_result(), send(2)]
    );
    assert_eq!(
        feed(&mut core, 3_000, answer(OUTPOST, 2, WireOutcome::NotThere)),
        [said("NotThere"), send(3)]
    );
    assert_eq!(
        feed(&mut core, 4_000, Input::Foreground(ELSEWHERE)),
        [
            cancel(3, CancelReason::Foreground),
            said("NoLongerAwaited(Foreground)")
        ]
    );
    assert_eq!(
        ended(&core),
        [
            read_ended(2, "Result(PropertyValues([])), NotThere"),
            read_ended(1, "NoLongerAwaited(Foreground)"),
        ]
    );
}

/// A scenario touching every path: a chain, an outside request, a key
/// withdrawing, a hang and its recovery, a due time and a late answer.
fn scenario() -> Vec<Timed<Entry>> {
    let at = |at_us, input| Timed { at_us, input };
    vec![
        at(0, Input::ReaderReady(OUTPOST)),
        at(0, Input::Foreground(IN_FRONT)),
        at(1_000, read(2)),
        at(1_100, ask_outside()),
        at(1_200, read(1)),
        at(2_000, answer(OUTPOST, 1, result())),
        at(2_100, Input::Entry(Entry::Key)),
        at(2_200, answer(OUTPOST, 2, result())),
        at(2_300, answer(OUTPOST, 3, result())),
        at(
            2_400,
            Input::HangCheck {
                window: WINDOW.handle,
                hung: true,
            },
        ),
        at(1_000 + HANG_ROUND_US, Input::Tick),
        at(
            1_100 + HANG_ROUND_US,
            Input::HangCheck {
                window: WINDOW.handle,
                hung: false,
            },
        ),
        at(600_000, read(1)),
        at(600_000 + OTHER_DUE_US, Input::Tick),
        at(700_000 + OTHER_DUE_US, answer(OUTPOST, 5, result())),
    ]
}

#[test]
fn a_checkpoint_taken_at_any_input_replays_to_the_same_state_and_goes_on_alike() {
    let script = scenario();
    let mut straight = Core::<Test>::default();
    let effects: Vec<Vec<Effect<String>>> = script
        .iter()
        .map(|input| straight.feed(input.clone()))
        .collect();
    for cut in 0..=script.len() {
        let mut core = Core::<Test>::default();
        for input in &script[..cut] {
            let _ = core.feed(input.clone());
        }
        let mut resumed = core.checkpoint().unwrap();
        assert_eq!(resumed.dump(), core.dump(), "at input {cut}");
        for (place, input) in script.iter().enumerate().skip(cut) {
            assert_eq!(
                resumed.feed(input.clone()),
                effects[place],
                "input {place} after a checkpoint at {cut}"
            );
        }
        assert_eq!(resumed.dump(), straight.dump());
    }
}

#[test]
fn a_dump_shows_the_front_entry_its_answers_and_its_request_out() {
    let mut core = core_with(IN_FRONT);
    assert_eq!(feed(&mut core, 1_000, read(2)), [send(1)]);
    assert_eq!(
        feed(&mut core, 1_100, Input::Entry(Entry::Key)),
        [
            cancel(1, CancelReason::Withdrawn),
            said("NoLongerAwaited(Withdrawn)"),
            said("key after 0 results, 0 waiting")
        ]
    );
    assert_eq!(feed(&mut core, 2_000, read(2)), [send(2)]);
    assert_eq!(
        feed(&mut core, 3_000, answer(OUTPOST, 2, result())),
        [said_result(), send(3)]
    );
    assert_eq!(feed(&mut core, 3_100, read(1)), []);
    assert_eq!(
        core.dump(),
        concat!(
            "Core { now_us: 3100, ",
            "front: Some(Front { entry: Read { reads: 2, exempt: false }, ",
            "answers: [Result(PropertyValues([]))], asked: Some(RequestNumber(3)) }), ",
            "waiting: [One(Read { reads: 1, exempt: false })], ",
            "table: Table { last: 3, live: {RequestNumber(3): Live { reader: Outpost(Pid(1)), ",
            "window: RequestWindow { handle: WindowHandle(11), top_level: WindowHandle(10), ",
            "root_owner: WindowHandle(10), process: Pid(1), thread: 3, by_class: Some(Msaa) }, ",
            "due_us: 11002000, ",
            "request: Properties { node: NodeId { outpost: OutpostId(0), number: 1 }, set: [Name] }, ",
            "owner: Queue, stage: Out(Instance { reader: Outpost(Pid(1)), generation: 1 }), ",
            "probing: None }}, ",
            "late: {RequestNumber(1): Instance { reader: Outpost(Pid(1)), generation: 1 }}, ",
            "recovery: {}, round_due_us: Some(502000) }, ",
            "readers: {Outpost(Pid(1)): Instance { reader: Outpost(Pid(1)), generation: 1 }}, ",
            "foreground: Some(Foreground { window: WindowHandle(10), root_owner: WindowHandle(10), ",
            "process: Pid(1) }), backend: BackendCache { probed: {} }, facts: 1 }"
        )
    );
}

// Table 4, column D, and the probes requests answer.

/// A window of the tests' application whose classes give no verdict.
const PROBED: RequestWindow = RequestWindow {
    by_class: None,
    ..WINDOW
};

/// A read of the window whose classes give no verdict.
#[derive(Debug)]
struct ReadProbed;

impl Request for ReadProbed {
    type Reply = PropertyValues;

    fn reader(&self) -> Reader {
        OUTPOST.reader
    }

    fn window(&self) -> RequestWindow {
        PROBED
    }

    fn wire(&self) -> Wire {
        wire()
    }

    fn decode(reply: Reply) -> Option<PropertyValues> {
        ReadName::decode(reply)
    }
}

/// The logic of the probe tests: each entry reads the probed window once.
struct Probing;

impl Logic for Probing {
    type Entry = ();
    type Facts = ();
    type Said = String;

    fn run(mut cx: Cx<Self>, (): ()) -> Function {
        Box::pin(async move {
            let due_us = cx.due_in(OTHER_DUE_US);
            let outcome = cx.ask(ReadProbed, due_us).await;
            cx.emit(format!("{outcome:?}"));
        })
    }

    fn exempt_from_foreground((): &(), _: &RequestWindow) -> bool {
        false
    }

    fn withdraws((): &(), (): &(), (): &()) -> bool {
        false
    }

    fn recovered((): &(), _: &RequestWindow) -> Option<()> {
        None
    }

    fn place((): &(), (): &()) -> Place {
        Place::Other {
            merges: false,
            focus_kind: false,
            ends_run: true,
        }
    }

    fn merges((): &(), (): &(), (): &()) -> bool {
        false
    }

    fn exempt_from_cap((): &(), _: &MsaaKey) -> bool {
        false
    }

    fn run_entry(_: Vec<()>) {}

    fn ends_at_foreground((): &(), (): &(), _: &Foreground) -> bool {
        false
    }

    fn foreground_marker(_: &Foreground) -> Option<()> {
        None
    }
}

/// The send of request `number` to the probed window carrying `verdict`.
fn send_probed(number: u64, verdict: WindowVerdict) -> Effect<String> {
    Effect::Send {
        to: OUTPOST,
        ask: Ask {
            number: RequestNumber(number),
            window: PROBED.handle,
            verdict,
            request: wire(),
        },
    }
}

/// A probe of the probed window.
fn probe(provider: bool, began_us: u64) -> verbatim_model::request::Probe {
    verbatim_model::request::Probe {
        window: PROBED.handle,
        thread: PROBED.thread,
        provider,
        began_us,
    }
}

/// A core of the probe tests with the outpost ready.
fn probing_core() -> Core<Probing> {
    let mut core = Core::<Probing>::default();
    let _ = core.feed(Timed {
        at_us: 0,
        input: Input::ReaderReady(OUTPOST),
    });
    let _ = core.feed(Timed {
        at_us: 0,
        input: Input::Foreground(IN_FRONT),
    });
    core
}

/// Feeds `input` to a probe test's core at `at_us`.
fn feed_probing(core: &mut Core<Probing>, at_us: u64, input: Input<()>) -> Vec<Effect<String>> {
    core.feed(Timed { at_us, input })
}

/// The answer to request `number` with `probes`.
fn answer_probed(number: u64, probes: Vec<verbatim_model::request::Probe>) -> Input<()> {
    Input::Answer {
        from: OUTPOST,
        answer: Answer {
            number: RequestNumber(number),
            outcome: result(),
            probes,
            detail: String::new(),
        },
    }
}

#[test]
fn a_request_to_a_window_with_no_verdict_carries_probe_first_and_its_probe_decides_the_next() {
    let mut core = probing_core();
    assert_eq!(
        feed_probing(&mut core, 1_000, Input::Entry(())),
        [send_probed(1, WindowVerdict::ProbeFirst)]
    );
    assert_eq!(
        feed_probing(&mut core, 2_000, answer_probed(1, vec![probe(true, 1_100)])),
        [said_result()]
    );
    assert_eq!(
        feed_probing(&mut core, 3_000, Input::Entry(())),
        [send_probed(2, WindowVerdict::Uia)]
    );
}

#[test]
fn a_probe_verdict_lapses_half_a_second_after_the_probe_began() {
    let mut core = probing_core();
    assert_eq!(
        feed_probing(&mut core, 1_000, Input::Entry(())),
        [send_probed(1, WindowVerdict::ProbeFirst)]
    );
    assert_eq!(
        feed_probing(&mut core, 2_000, answer_probed(1, vec![probe(true, 1_100)])),
        [said_result()]
    );
    assert_eq!(
        feed_probing(&mut core, 601_100, Input::Entry(())),
        [send_probed(2, WindowVerdict::ProbeFirst)]
    );
}

#[test]
fn a_probe_first_request_answered_with_no_probe_counts_as_one_that_timed_out() {
    let mut core = probing_core();
    assert_eq!(
        feed_probing(&mut core, 1_000, Input::Entry(())),
        [send_probed(1, WindowVerdict::ProbeFirst)]
    );
    assert_eq!(
        feed_probing(&mut core, 2_000, answer_probed(1, Vec::new())),
        [said_result()]
    );
    assert_eq!(
        feed_probing(&mut core, 3_000, Input::Entry(())),
        [send_probed(2, WindowVerdict::Msaa)]
    );
}

#[test]
fn a_probe_first_request_no_longer_awaited_counts_as_a_probe_that_timed_out_from_its_send() {
    let mut core = probing_core();
    assert_eq!(
        feed_probing(&mut core, 1_000, Input::Entry(())),
        [send_probed(1, WindowVerdict::ProbeFirst)]
    );
    assert_eq!(
        feed_probing(&mut core, 2_000, Input::Foreground(SAME_APP_ELSEWHERE)),
        [
            Effect::Cancel {
                to: OUTPOST,
                cancel: Cancel {
                    number: RequestNumber(1),
                    reason: CancelReason::Foreground
                }
            },
            said("NoLongerAwaited(Foreground)")
        ]
    );
    assert_eq!(
        feed_probing(&mut core, 3_000, Input::Foreground(IN_FRONT)),
        []
    );
    assert_eq!(
        feed_probing(&mut core, 4_000, Input::Entry(())),
        [send_probed(2, WindowVerdict::Msaa)]
    );
    assert_eq!(
        feed_probing(&mut core, 5_000, answer_probed(2, Vec::new())),
        [said_result()]
    );
    assert_eq!(
        feed_probing(&mut core, 501_001, Input::Entry(())),
        [send_probed(3, WindowVerdict::ProbeFirst)]
    );
}

#[test]
fn a_destroyed_windows_probe_verdict_is_cleared() {
    let mut core = probing_core();
    assert_eq!(
        feed_probing(&mut core, 1_000, Input::Entry(())),
        [send_probed(1, WindowVerdict::ProbeFirst)]
    );
    assert_eq!(
        feed_probing(&mut core, 2_000, answer_probed(1, vec![probe(true, 1_100)])),
        [said_result()]
    );
    assert_eq!(
        feed_probing(&mut core, 2_100, Input::WindowDestroyed(PROBED.handle)),
        []
    );
    assert_eq!(
        feed_probing(&mut core, 3_000, Input::Entry(())),
        [send_probed(2, WindowVerdict::ProbeFirst)]
    );
}
