//! The driver's five rules, every state and ending of a request in Core
//! and the recovery set, from `books/dev/src/reference/crates/verbatim-core.md`,
//! "Core's requests", against a small logic
//! whose entries each ask a chain of property reads and say each outcome.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Waker};

use verbatim_model::batch::{
    Batch, CaretAt, Done, Missing, Op, Placement, Step, StepOutcome, Subject,
};
use verbatim_model::property::{PropertyId, PropertyValues};
use verbatim_model::request::{
    Answer, Ask, Cancel, CancelReason, Outcome as WireOutcome, Refusal, Reply, RequestNumber,
    WindowVerdict,
};
use verbatim_model::{NodeId, Pid, WindowHandle};

use crate::batch::On;

use super::driver::{Core, Cx};
use super::inbox::{Input, Timed};
use super::limiter::{MsaaKey, Place};
use super::request::{Outcome, Request};
use super::table::Asker;
use super::{
    Cause, FOCUS_DUE_US, Foreground, Function, HANG_ROUND_US, Instance, Live, Logic, OTHER_DUE_US,
    Reader, RequestWindow,
};
use crate::focus::backend::BackendCache;
use verbatim_model::Backend;
use verbatim_model::Effect;
use verbatim_model::request::QueueEffect;

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
    thread: 0,
    by_class: None,
    ..Foreground::BLANK
};
/// Another window of the same application in front, outside the request
/// window's tree.
const SAME_APP_ELSEWHERE: Foreground = Foreground {
    window: WindowHandle(30),
    root_owner: WindowHandle(30),
    process: APP,
    thread: 0,
    by_class: None,
    ..Foreground::BLANK
};
/// Another application in front.
const ELSEWHERE: Foreground = Foreground {
    window: WindowHandle(20),
    root_owner: WindowHandle(20),
    process: Pid(2),
    thread: 0,
    by_class: None,
    ..Foreground::BLANK
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
    /// Asks a read of the plain node, which Core's plain reader answers at
    /// once, then a read of the outpost's node, saying each outcome.
    PlainThenRead,
    /// Asks one batch, a read of the application's focus and a caret, and
    /// says each step's outcome.
    Batch,
    /// Asks two numbered batches in a loop, the second given and read by
    /// the first's handle, and says what the second answers by it.
    Numbered,
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

    fn wire(&self) -> verbatim_model::request::Question {
        wire()
    }

    fn decode(&self, reply: Reply, _: super::Stamps) -> Option<PropertyValues> {
        values_of(reply)
    }
}

/// A read of the plain node, which the tests' plain reader answers.
#[derive(Debug)]
struct ReadPlain;

impl Request for ReadPlain {
    type Reply = PropertyValues;

    fn reader(&self) -> Reader {
        OUTPOST.reader
    }

    fn window(&self) -> RequestWindow {
        WINDOW
    }

    fn wire(&self) -> verbatim_model::request::Question {
        plain_wire()
    }

    fn decode(&self, reply: Reply, _: super::Stamps) -> Option<PropertyValues> {
        values_of(reply)
    }
}

/// A one-step read of `node`'s name, as the tests' requests ask.
fn read_of(node: NodeId) -> verbatim_model::request::Question {
    verbatim_model::batch::Batch {
        steps: vec![verbatim_model::batch::Step {
            op: verbatim_model::batch::Op::Read {
                subject: verbatim_model::batch::Subject::Node(node),
                set: vec![PropertyId::Name],
            },
            when: None,
        }],
        placement: verbatim_model::batch::Placement::Queue,
        spare: None,
    }
    .into()
}

/// The read of the plain node as it begins.
fn plain_wire() -> verbatim_model::request::Question {
    read_of(NodeId::plain(1, 1))
}

/// The read as it is sent.
fn wire() -> verbatim_model::request::Question {
    read_of(NodeId::new(1))
}

/// The values a one-step read's answer holds.
fn values_of(reply: Reply) -> Option<PropertyValues> {
    match reply {
        Reply::Steps(steps) => match steps.as_slice() {
            [verbatim_model::batch::StepOutcome::Done(verbatim_model::batch::Done::Read(read))] => {
                Some(read.values.clone())
            }
            _ => None,
        },
    }
}

impl Logic for Test {
    type Entry = Entry;
    type Facts = u32;

    /// Only the tests' application's outpost is started; another
    /// application in front is one whose outpost these tests do not
    /// follow.
    fn watched(_: &u32, process: Pid) -> bool {
        process == APP
    }

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
                        cx.emit(emitted(format!("{outcome:?}")));
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
                    cx.emit(emitted(words));
                }
                Entry::Recovered(window) => cx.emit(emitted(format!("recovered {}", window.0))),
                Entry::Stray => std::future::pending::<()>().await,
                Entry::Batch => {
                    let due_us = cx.due_in(OTHER_DUE_US);
                    let mut read = cx.batch(|| ());
                    let focus = read.read(On::focus(), &[PropertyId::Name]);
                    let caret = read.caret(NodeId::new(1), None);
                    let asked = cx.ask(read.finish().of(OUTPOST.reader, WINDOW), due_us);
                    let words = match asked.await {
                        Outcome::Result(answers) => format!(
                            "in front {}, focus {}, caret {}",
                            answers.in_front().is_some(),
                            got(&answers.get(focus)),
                            got(&answers.get(caret)),
                        ),
                        other => format!("{other:?}"),
                    };
                    cx.emit(emitted(words));
                }
                Entry::Numbered => {
                    let due_us = cx.due_in(OTHER_DUE_US);
                    let mut earlier = None;
                    for _ in 0..2 {
                        let mut read = cx.batch_numbered(|| ());
                        let caret = read.caret(NodeId::new(1), None);
                        if let Some(earlier) = earlier {
                            let _ = read.read_text(
                                NodeId::new(1),
                                crate::batch::Ref::caret(earlier),
                                None,
                                verbatim_model::TextUnit::Line,
                                1,
                                verbatim_model::TextDetails::NONE,
                            );
                        }
                        let asked = cx.ask(read.finish().of(OUTPOST.reader, WINDOW), due_us);
                        let answers = asked.await;
                        if let (Some(earlier), Outcome::Result(answers)) = (earlier, &answers) {
                            let words = format!(
                                "own {}, another's {}",
                                got(&answers.get(caret)),
                                got(&answers.get(earlier))
                            );
                            cx.emit(emitted(words));
                        }
                        earlier = Some(caret);
                    }
                }
                Entry::PlainThenRead => {
                    let due_us = cx.due_in(OTHER_DUE_US);
                    let plain = cx.ask(ReadPlain, due_us).await;
                    cx.emit(emitted(format!("plain {plain:?}")));
                    let read = cx.ask(ReadName, due_us).await;
                    cx.emit(emitted(format!("{read:?}")));
                }
            }
        })
    }

    fn ends(
        facts: &u32,
        (front, live): (&Entry, &Live),
        cause: Cause<'_, Entry>,
        (_, _, record): (&BackendCache, u64, Option<Foreground>),
    ) -> Option<CancelReason> {
        match cause {
            Cause::Arrived(arriving) => (matches!(front, Entry::Read { .. })
                && *arriving == Entry::Key)
                .then_some(CancelReason::Withdrawn),
            Cause::Foreground => (!matches!(front, Entry::Read { exempt: true, .. })
                && record.is_some_and(|record| !record.holds(&live.window)))
            .then_some(CancelReason::Foreground),
            Cause::Settled => (*facts == UNNEEDED).then_some(CancelReason::Withdrawn),
        }
    }

    fn recovered(_: &mut u32, window: &RequestWindow) -> Option<Entry> {
        Some(Entry::Recovered(window.handle))
    }

    /// The plain node's read is answered with a result.
    fn plain_answer(_: &u32, request: &verbatim_model::request::Question) -> Option<WireOutcome> {
        (*request == plain_wire()).then(result)
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
fn send(number: u64) -> Effect {
    Effect::Queue(QueueEffect::Send {
        to: OUTPOST,
        ask: Ask {
            number: RequestNumber(number),
            window: WINDOW.handle,
            verdict: WindowVerdict::Msaa,
            request: wire(),
        },
    })
}

/// The cancel of request `number` for `reason`.
fn cancel(number: u64, reason: CancelReason) -> Effect {
    Effect::Queue(QueueEffect::Cancel {
        to: OUTPOST,
        cancel: Cancel {
            number: RequestNumber(number),
            reason,
        },
    })
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
            began_us: 0,
            ended_us: 0,
        },
    }
}

/// A result.
fn result() -> WireOutcome {
    WireOutcome::Result(crate::entries::tests::properties(PropertyValues::default()))
}

/// What the tests' entry says of a result.
fn said_result() -> Effect {
    said("Result(PropertyValues([]))")
}

/// What an entry said.
fn said(text: &str) -> Effect {
    emitted(text.to_owned())
}

/// What an entry's function emits: its words, carried by a model effect
/// the queue passes on unread.
fn emitted(words: String) -> Effect {
    Effect::CopyToClipboard(words)
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
fn feed(core: &mut Core<Test>, at_us: u64, input: Input<Entry>) -> Vec<Effect> {
    core.feed(Timed { at_us, input })
}

/// `instance` made ready at `at_us`: its start set up as the owner thread
/// reports it, then its `Ready`.
fn ready_at(core: &mut Core<Test>, at_us: u64, instance: Instance) -> Vec<Effect> {
    core.shared
        .borrow_mut()
        .state
        .queue
        .start_for_tests(instance);
    feed(core, at_us, Input::ReaderReady(instance))
}

/// The owner thread's report that `instance`'s process started.
fn started(instance: Instance) -> Input<Entry> {
    let Reader::Outpost(process) = instance.reader else {
        unreachable!("only an outpost is started by Core");
    };
    Input::Process(verbatim_model::request::ProcessFact::OutpostStarted {
        process,
        generation: instance.generation,
    })
}

/// A core whose outpost is ready, with `foreground` in front.
fn core_with(foreground: Foreground) -> Core<Test> {
    let mut core = Core::default();
    assert_eq!(ready_at(&mut core, 0, OUTPOST), []);
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
    assert_eq!(
        feed(&mut core, 0, Input::Foreground(IN_FRONT)),
        [Effect::Queue(QueueEffect::StartOutpost(APP))]
    );
    assert_eq!(feed(&mut core, 1_000, read(1)), []);
    assert_eq!(ready_at(&mut core, 2_000, OUTPOST), [send(1)]);
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
        [
            Effect::PlayEarcon(verbatim_model::Earcon::AppNotResponding),
            said("NotResponding")
        ]
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
    assert_eq!(core.shared.borrow().state.queue.table.next_due(), None);
}

#[test]
fn a_waiting_request_is_not_checked_for_a_hang() {
    let mut core = Core::<Test>::default();
    assert_eq!(
        feed(&mut core, 0, Input::Foreground(IN_FRONT)),
        [Effect::Queue(QueueEffect::StartOutpost(APP))]
    );
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
    assert_eq!(
        feed(&mut core, 0, Input::Foreground(IN_FRONT)),
        [Effect::Queue(QueueEffect::StartOutpost(APP))]
    );
    assert_eq!(feed(&mut core, 1_000, read(1)), []);
    assert_eq!(
        feed(&mut core, 1_000 + OTHER_DUE_US, Input::Tick),
        [said("NoLongerAwaited(DueTime)")]
    );
    assert_eq!(ready_at(&mut core, OTHER_DUE_US + 2_000, OUTPOST), []);
}

#[test]
fn a_waiting_request_waits_on_when_a_starting_reader_fails() {
    let mut core = Core::<Test>::default();
    assert_eq!(
        feed(&mut core, 0, Input::Foreground(IN_FRONT)),
        [Effect::Queue(QueueEffect::StartOutpost(APP))]
    );
    assert_eq!(feed(&mut core, 1_000, read(1)), []);
    assert_eq!(feed(&mut core, 1_500, started(OUTPOST)), []);
    // A first failure is replaced at once (Table 5, column E).
    assert_eq!(
        feed(&mut core, 2_000, Input::ReaderEnded(OUTPOST)),
        [Effect::Queue(QueueEffect::StartOutpost(APP))]
    );
    assert_eq!(feed(&mut core, 2_500, started(NEXT_OUTPOST)), []);
    assert_eq!(
        feed(&mut core, 3_000, Input::ReaderReady(NEXT_OUTPOST)),
        [Effect::Queue(QueueEffect::Send {
            to: NEXT_OUTPOST,
            ask: Ask {
                number: RequestNumber(1),
                window: WINDOW.handle,
                verdict: WindowVerdict::Msaa,
                request: wire(),
            },
        })]
    );
}

#[test]
fn a_waiting_request_ends_unsent_when_the_foreground_leaves_its_window() {
    let mut core = Core::<Test>::default();
    assert_eq!(
        feed(&mut core, 0, Input::Foreground(IN_FRONT)),
        [Effect::Queue(QueueEffect::StartOutpost(APP))]
    );
    assert_eq!(feed(&mut core, 1_000, read(1)), []);
    assert_eq!(
        feed(&mut core, 2_000, Input::Foreground(SAME_APP_ELSEWHERE)),
        [said("NoLongerAwaited(Foreground)")]
    );
    assert_eq!(ready_at(&mut core, 3_000, OUTPOST), []);
}

#[test]
fn a_waiting_request_stays_when_the_foreground_holds_its_window() {
    let mut core = Core::<Test>::default();
    assert_eq!(feed(&mut core, 0, Input::Foreground(ELSEWHERE)), []);
    assert_eq!(
        feed(&mut core, 1_000, read(1)),
        [Effect::Queue(QueueEffect::StartOutpost(APP))]
    );
    assert_eq!(feed(&mut core, 2_000, Input::Foreground(IN_FRONT)), []);
    assert_eq!(ready_at(&mut core, 3_000, OUTPOST), [send(1)]);
}

#[test]
fn a_waiting_request_withdrawn_by_its_entry_is_dropped_unsent() {
    let mut core = Core::<Test>::default();
    assert_eq!(
        feed(&mut core, 0, Input::Foreground(IN_FRONT)),
        [Effect::Queue(QueueEffect::StartOutpost(APP))]
    );
    assert_eq!(feed(&mut core, 1_000, read(1)), []);
    assert_eq!(
        feed(&mut core, 2_000, Input::Entry(Entry::Key)),
        [
            said("NoLongerAwaited(Withdrawn)"),
            said("key after 0 results, 0 waiting")
        ]
    );
    assert_eq!(ready_at(&mut core, 3_000, OUTPOST), []);
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
            Effect::PlayEarcon(verbatim_model::Earcon::AppNotResponding),
            said("NoLongerAwaited(Hung)")
        ]
    );
}

/// A console host's window in front: the foreground names the console
/// window, which the console host's process owns, while attention is on its
/// client's process, which the foreground names.
const CONSOLE_IN_FRONT: Foreground = Foreground {
    window: WindowHandle(10),
    root_owner: WindowHandle(10),
    process: Pid(2),
    thread: 0,
    by_class: None,
    ..Foreground::BLANK
};

/// A request to the console host, out while its client holds attention,
/// found hung: the console window is the foreground window, so the
/// not-responding sound plays though the processes differ.
#[test]
fn a_console_hosts_request_found_hung_while_its_client_holds_attention_plays_the_sound() {
    let mut core = core_with(CONSOLE_IN_FRONT);
    assert_eq!(feed(&mut core, 1_000, read(1)), [send(1)]);
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
            Effect::PlayEarcon(verbatim_model::Earcon::AppNotResponding),
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
    assert_eq!(core.shared.borrow().state.queue.table.next_due(), None);
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
        [
            said("ReaderEnded"),
            Effect::Queue(QueueEffect::StartOutpost(APP))
        ]
    );
}

/// The facts of the tests' logic when no read is needed any more.
const UNNEEDED: u32 = u32::MAX;

/// The one judge ends the front's read for each cause: a key arriving, the
/// foreground leaving it in its own application, and an unneeded read.
#[test]
fn the_one_judge_ends_the_front_for_each_cause() {
    type Of = fn(&mut Core<Test>) -> Input<Entry>;
    let unneeded = |core: &mut Core<Test>| {
        *core.cx_for_tests().facts_mut() = UNNEEDED;
        Input::Settle
    };
    let ended = |reason: CancelReason| {
        let said = said(&format!("NoLongerAwaited({reason:?})"));
        vec![cancel(1, reason), said]
    };
    let mut key = ended(CancelReason::Withdrawn);
    key.push(said("key after 0 results, 0 waiting"));
    let causes: [(Of, Vec<Effect>); 3] = [
        (|_| Input::Entry(Entry::Key), key),
        (
            |_| Input::Foreground(SAME_APP_ELSEWHERE),
            ended(CancelReason::Foreground),
        ),
        (unneeded, ended(CancelReason::Withdrawn)),
    ];
    for (cause, expected) in causes {
        let mut core = core_with_read_out();
        let input = cause(&mut core);
        assert_eq!(feed(&mut core, 2_000, input), expected);
    }
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

// Table 2, outside the queue.

#[test]
fn a_request_outside_the_queue_waits_for_its_reader_and_is_sent_when_ready() {
    let mut core = Core::<Test>::default();
    assert_eq!(
        feed(&mut core, 1_000, ask_outside()),
        [Effect::Queue(QueueEffect::StartOutpost(APP))]
    );
    assert_eq!(ready_at(&mut core, 2_000, OUTPOST), [send(1)]);
}

#[test]
fn a_request_outside_the_queue_found_hung_by_the_shell_is_answered_not_responding() {
    let mut core = core_with_outside_out();
    assert_eq!(
        feed(&mut core, 1_500, Input::RefusedHung(RequestNumber(1))),
        [
            Effect::PlayEarcon(verbatim_model::Earcon::AppNotResponding),
            Effect::Queue(QueueEffect::Answered {
                asker: ASKER.0,
                outcome: WireOutcome::NotResponding
            })
        ]
    );
}

#[test]
fn a_waiting_request_outside_the_queue_past_its_due_time_answers_its_asker_unsent() {
    let mut core = Core::<Test>::default();
    assert_eq!(
        feed(&mut core, 1_000, ask_outside()),
        [Effect::Queue(QueueEffect::StartOutpost(APP))]
    );
    assert_eq!(
        feed(&mut core, 1_000 + OTHER_DUE_US, Input::Tick),
        [Effect::Queue(QueueEffect::Answered {
            asker: ASKER.0,
            outcome: WireOutcome::Ended
        })]
    );
    assert_eq!(ready_at(&mut core, OTHER_DUE_US + 2_000, OUTPOST), []);
}

#[test]
fn a_waiting_request_outside_the_queue_waits_for_the_next_reader_when_one_fails() {
    let mut core = Core::<Test>::default();
    assert_eq!(
        feed(&mut core, 1_000, ask_outside()),
        [Effect::Queue(QueueEffect::StartOutpost(APP))]
    );
    assert_eq!(feed(&mut core, 1_500, started(OUTPOST)), []);
    assert_eq!(
        feed(&mut core, 2_000, Input::ReaderEnded(OUTPOST)),
        [Effect::Queue(QueueEffect::StartOutpost(APP))]
    );
    assert_eq!(feed(&mut core, 2_500, started(NEXT_OUTPOST)), []);
    assert_eq!(
        feed(&mut core, 3_000, Input::ReaderReady(NEXT_OUTPOST)),
        [Effect::Queue(QueueEffect::Send {
            to: NEXT_OUTPOST,
            ask: Ask {
                number: RequestNumber(1),
                window: WINDOW.handle,
                verdict: WindowVerdict::Msaa,
                request: wire(),
            },
        })]
    );
}

#[test]
fn a_waiting_request_outside_the_queue_is_untouched_by_the_foreground() {
    let mut core = Core::<Test>::default();
    assert_eq!(
        feed(&mut core, 1_000, ask_outside()),
        [Effect::Queue(QueueEffect::StartOutpost(APP))]
    );
    assert_eq!(feed(&mut core, 2_000, Input::Foreground(ELSEWHERE)), []);
    assert_eq!(ready_at(&mut core, 3_000, OUTPOST), [send(1)]);
}

#[test]
fn a_waiting_request_outside_the_queue_withdrawn_is_dropped_unsent() {
    let mut core = Core::<Test>::default();
    assert_eq!(
        feed(&mut core, 1_000, ask_outside()),
        [Effect::Queue(QueueEffect::StartOutpost(APP))]
    );
    assert_eq!(
        feed(&mut core, 2_000, Input::WithdrawOutside(ASKER)),
        [Effect::Queue(QueueEffect::Answered {
            asker: ASKER.0,
            outcome: WireOutcome::Ended
        })]
    );
    assert_eq!(ready_at(&mut core, 3_000, OUTPOST), []);
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
            Effect::PlayEarcon(verbatim_model::Earcon::AppNotResponding),
            Effect::Queue(QueueEffect::Answered {
                asker: ASKER.0,
                outcome: WireOutcome::Ended
            })
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
            Effect::Queue(QueueEffect::Answered {
                asker: ASKER.0,
                outcome: WireOutcome::Ended
            })
        ]
    );
}

#[test]
fn a_request_outside_the_queue_passes_its_answer_to_its_asker() {
    let mut core = core_with_outside_out();
    assert_eq!(
        feed(&mut core, 2_000, answer(OUTPOST, 1, result())),
        [Effect::Queue(QueueEffect::Answered {
            asker: ASKER.0,
            outcome: WireOutcome::Result(crate::entries::tests::properties(
                PropertyValues::default()
            ))
        })]
    );
}

#[test]
fn a_request_outside_the_queue_answers_reader_ended_to_its_asker() {
    let mut core = core_with_outside_out();
    assert_eq!(
        feed(&mut core, 2_000, Input::ReaderEnded(OUTPOST)),
        [
            Effect::Queue(QueueEffect::Answered {
                asker: ASKER.0,
                outcome: WireOutcome::Ended
            }),
            Effect::Queue(QueueEffect::StartOutpost(APP))
        ]
    );
}

#[test]
fn a_request_outside_the_queue_out_is_untouched_by_the_foreground() {
    let mut core = core_with_outside_out();
    assert_eq!(feed(&mut core, 2_000, Input::Foreground(ELSEWHERE)), []);
    assert_eq!(
        feed(&mut core, 3_000, answer(OUTPOST, 1, result())),
        [Effect::Queue(QueueEffect::Answered {
            asker: ASKER.0,
            outcome: WireOutcome::Result(crate::entries::tests::properties(
                PropertyValues::default()
            ))
        })]
    );
}

#[test]
fn a_request_outside_the_queue_out_withdrawn_is_cancelled() {
    let mut core = core_with_outside_out();
    assert_eq!(
        feed(&mut core, 2_000, Input::WithdrawOutside(ASKER)),
        [
            cancel(1, CancelReason::Withdrawn),
            Effect::Queue(QueueEffect::Answered {
                asker: ASKER.0,
                outcome: WireOutcome::Ended
            })
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
        [Effect::Queue(QueueEffect::AnswerDropped {
            number: RequestNumber(1),
            late: true
        })]
    );
}

#[test]
fn a_request_no_longer_awaited_is_forgotten_when_its_reader_ends() {
    let mut core = core_with_late_request();
    assert_eq!(
        feed(&mut core, OTHER_DUE_US + 2_000, Input::ReaderEnded(OUTPOST)),
        [Effect::Queue(QueueEffect::StartOutpost(APP))]
    );
    assert_eq!(
        feed(
            &mut core,
            OTHER_DUE_US + 3_000,
            answer(OUTPOST, 1, result())
        ),
        [Effect::Queue(QueueEffect::AnswerDropped {
            number: RequestNumber(1),
            late: false
        })]
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
        [Effect::Queue(QueueEffect::AnswerDropped {
            number: RequestNumber(1),
            late: false
        })]
    );
    // The outpost's end starts its replacement, and its `Ready` again is
    // an ended generation's, dropped; neither touches the request.
    for (input, effects) in [
        (
            Input::HangCheck {
                window: WINDOW.handle,
                hung: true,
            },
            Vec::new(),
        ),
        (Input::Tick, Vec::new()),
        (
            Input::ReaderEnded(OUTPOST),
            vec![Effect::Queue(QueueEffect::StartOutpost(APP))],
        ),
        (Input::Foreground(ELSEWHERE), Vec::new()),
        (
            Input::ReaderReady(OUTPOST),
            vec![Effect::Queue(QueueEffect::StaleFrame(OUTPOST))],
        ),
    ] {
        assert_eq!(feed(&mut core, 4_000 + OTHER_DUE_US, input), effects);
    }
}

#[test]
fn an_answer_from_an_instance_the_request_did_not_go_to_is_dropped() {
    let mut core = core_with_read_out();
    assert_eq!(
        feed(&mut core, 2_000, answer(NEXT_OUTPOST, 1, result())),
        [Effect::Queue(QueueEffect::AnswerDropped {
            number: RequestNumber(1),
            late: false
        })]
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
        [Effect::Queue(QueueEffect::CheckHung(vec![WINDOW.handle]))]
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
            Effect::PlayEarcon(verbatim_model::Earcon::AppNotResponding),
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
        [Effect::Queue(QueueEffect::CheckHung(vec![WINDOW.handle]))]
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

/// A refusal for a window whose application is in the recovery set plays
/// nothing: the sound plays once per application joining the set
/// (Claude, 2026-11-21).
#[test]
fn a_watched_window_stays_when_another_request_to_it_is_found_hung() {
    let mut core = core_watching();
    assert_eq!(feed(&mut core, 2_000, read(1)), [send(2)]);
    assert_eq!(
        feed(&mut core, 2_100, Input::RefusedHung(RequestNumber(2))),
        [said("NotResponding")]
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

/// A destroy with no process, as the listener sends one whose raising
/// thread it cannot open, still takes the window out of the recovery set,
/// and sends nothing to an outpost.
#[test]
fn a_watched_window_leaves_the_set_when_it_is_destroyed() {
    let mut core = core_watching();
    assert_eq!(
        feed(
            &mut core,
            2_000,
            Input::WindowDestroyed {
                window: WINDOW.handle,
                process: None,
            }
        ),
        []
    );
    assert_eq!(core.next_due(), None);
}

#[test]
fn a_watched_window_leaves_the_set_when_its_application_loses_attention() {
    let mut core = core_watching();
    assert_eq!(feed(&mut core, 2_000, Input::Foreground(ELSEWHERE)), []);
    assert_eq!(core.shared.borrow().state.queue.table.next_due(), None);
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
    assert_eq!(
        feed(&mut core, 2_000, Input::ReaderEnded(OUTPOST)),
        [Effect::Queue(QueueEffect::StartOutpost(APP))]
    );
    assert_eq!(core.next_due(), None);
}

// A reader's report of a hung window (ADR 0003, "A frozen window").

/// Another window of the tests' application, under the same top-level
/// window.
const SIBLING: RequestWindow = RequestWindow {
    handle: WindowHandle(12),
    ..WINDOW
};

/// A window of another application.
const OTHER_APP_WINDOW: RequestWindow = RequestWindow {
    handle: WindowHandle(21),
    top_level: WindowHandle(20),
    root_owner: WindowHandle(20),
    process: Pid(2),
    ..WINDOW
};

#[test]
fn a_report_while_attended_plays_the_sound_and_watches_the_window() {
    let mut core = core_with(IN_FRONT);
    assert_eq!(
        feed(&mut core, 1_000, Input::WindowHung(WINDOW)),
        [Effect::PlayEarcon(verbatim_model::Earcon::AppNotResponding)]
    );
    assert_eq!(core.next_due(), Some(1_000 + HANG_ROUND_US));
    assert_eq!(
        feed(&mut core, 1_000 + HANG_ROUND_US, Input::Tick),
        [Effect::Queue(QueueEffect::CheckHung(vec![WINDOW.handle]))]
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
        [said("recovered 11")]
    );
}

#[test]
fn a_second_window_of_the_same_application_is_watched_with_no_sound() {
    let mut core = core_with(IN_FRONT);
    assert_eq!(
        feed(&mut core, 1_000, Input::WindowHung(WINDOW)),
        [Effect::PlayEarcon(verbatim_model::Earcon::AppNotResponding)]
    );
    assert_eq!(feed(&mut core, 1_100, Input::WindowHung(SIBLING)), []);
    assert_eq!(feed(&mut core, 1_200, Input::WindowHung(WINDOW)), []);
    assert_eq!(
        feed(&mut core, 1_000 + HANG_ROUND_US, Input::Tick),
        [Effect::Queue(QueueEffect::CheckHung(vec![
            WINDOW.handle,
            SIBLING.handle
        ]))]
    );
}

#[test]
fn a_report_while_its_application_does_not_hold_attention_does_nothing() {
    let mut core = core_with(IN_FRONT);
    assert_eq!(
        feed(&mut core, 1_000, Input::WindowHung(OTHER_APP_WINDOW)),
        []
    );
    assert_eq!(core.next_due(), None);
}

/// Once an application's windows leave the set, the next report sounds
/// again.
#[test]
fn a_report_after_the_application_left_the_set_sounds_again() {
    let mut core = core_with(IN_FRONT);
    assert_eq!(
        feed(&mut core, 1_000, Input::WindowHung(WINDOW)),
        [Effect::PlayEarcon(verbatim_model::Earcon::AppNotResponding)]
    );
    assert_eq!(feed(&mut core, 1_100, Input::Foreground(ELSEWHERE)), []);
    assert_eq!(feed(&mut core, 1_200, Input::Foreground(IN_FRONT)), []);
    assert_eq!(
        feed(&mut core, 1_300, Input::WindowHung(WINDOW)),
        [Effect::PlayEarcon(verbatim_model::Earcon::AppNotResponding)]
    );
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
            answer(OUTPOST, 1, WireOutcome::Result(Reply::Steps(Vec::new())))
        ),
        [
            Effect::Queue(QueueEffect::Undecodable(RequestNumber(1))),
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
    let effects: Vec<Vec<Effect>> = script
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
            "waiting: [One(Read { reads: 1, exempt: false })], timers: [], ",
            "table: Table { last: 3, live: {RequestNumber(3): Live { reader: Reader(Outpost(Pid(1))), ",
            "window: RequestWindow { handle: WindowHandle(11), top_level: WindowHandle(10), ",
            "root_owner: WindowHandle(10), process: Pid(1), thread: 3, by_class: Some(Msaa) }, ",
            "due_us: 11002000, ",
            "request: Batch(Batch { steps: [Step { op: Read { subject: Node(NodeId { ",
            "issuer: Outpost(OutpostId(0)), number: 1 }), set: [Name] }, when: None }], ",
            "placement: Queue, spare: None }), ",
            "owner: Queue, stage: Out { to: Instance { reader: Outpost(Pid(1)), generation: 1 }, ",
            "probing: None } }}, ",
            "late: {RequestNumber(1): Instance { reader: Outpost(Pid(1)), generation: 1 }}, ",
            "recovery: {}, round_due_us: Some(502000) }, ",
            "readers: {Outpost(Pid(1)): Instance { reader: Outpost(Pid(1)), generation: 1 }}, ",
            "foreground: Some(Foreground { window: WindowHandle(10), root_owner: WindowHandle(10), ",
            "process: Pid(1), thread: 0, by_class: None, ghost: None, console: false, ",
            "integrity: None, own_integrity: None }), backend: BackendCache { probed: {} }, facts: 1 }"
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

    fn wire(&self) -> verbatim_model::request::Question {
        wire()
    }

    fn decode(&self, reply: Reply, _: super::Stamps) -> Option<PropertyValues> {
        ReadName.decode(reply, super::Stamps::default())
    }
}

/// The logic of the probe tests: each entry reads the probed window once.
struct Probing;

/// A probe test's entry: whether, joining the back, it withdraws the
/// front's request.
type Withdraws = bool;

impl Logic for Probing {
    type Entry = Withdraws;
    type Facts = ();

    fn run(mut cx: Cx<Self>, _: Withdraws) -> Function {
        Box::pin(async move {
            let due_us = cx.due_in(OTHER_DUE_US);
            let outcome = cx.ask(ReadProbed, due_us).await;
            cx.emit(emitted(format!("{outcome:?}")));
        })
    }

    fn ends(
        (): &(),
        (_, live): (&Withdraws, &Live),
        cause: Cause<'_, Withdraws>,
        (_, _, record): (&BackendCache, u64, Option<Foreground>),
    ) -> Option<CancelReason> {
        match cause {
            Cause::Arrived(arriving) => arriving.then_some(CancelReason::Withdrawn),
            Cause::Foreground => record
                .is_some_and(|record| !record.holds(&live.window))
                .then_some(CancelReason::Foreground),
            Cause::Settled => None,
        }
    }

    fn recovered((): &mut (), _: &RequestWindow) -> Option<Withdraws> {
        None
    }

    fn place((): &(), _: &Withdraws) -> Place {
        Place::Other {
            merges: false,
            focus_kind: false,
            ends_run: true,
        }
    }

    fn merges((): &(), _: &Withdraws, _: &Withdraws) -> bool {
        false
    }

    fn exempt_from_cap((): &(), _: &MsaaKey) -> bool {
        false
    }

    fn run_entry(_: Vec<Withdraws>) -> Withdraws {
        false
    }

    fn ends_at_foreground((): &(), _: &Withdraws, _: &Foreground) -> bool {
        false
    }

    fn foreground_marker(_: &Foreground) -> Option<Withdraws> {
        None
    }
}

/// The send of request `number` to the probed window carrying `verdict`.
fn send_probed(number: u64, verdict: WindowVerdict) -> Effect {
    Effect::Queue(QueueEffect::Send {
        to: OUTPOST,
        ask: Ask {
            number: RequestNumber(number),
            window: PROBED.handle,
            verdict,
            request: wire(),
        },
    })
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
    core.shared
        .borrow_mut()
        .state
        .queue
        .start_for_tests(OUTPOST);
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
fn feed_probing(core: &mut Core<Probing>, at_us: u64, input: Input<Withdraws>) -> Vec<Effect> {
    core.feed(Timed { at_us, input })
}

/// The answer to request `number` with `probes`.
fn answer_probed(number: u64, probes: Vec<verbatim_model::request::Probe>) -> Input<Withdraws> {
    Input::Answer {
        from: OUTPOST,
        answer: Answer {
            number: RequestNumber(number),
            outcome: result(),
            probes,
            detail: String::new(),
            began_us: 0,
            ended_us: 0,
        },
    }
}

#[test]
fn a_request_to_a_window_with_no_verdict_carries_probe_first_and_its_probe_decides_the_next() {
    let mut core = probing_core();
    assert_eq!(
        feed_probing(&mut core, 1_000, Input::Entry(false)),
        [send_probed(1, WindowVerdict::ProbeFirst)]
    );
    assert_eq!(
        feed_probing(&mut core, 2_000, answer_probed(1, vec![probe(true, 1_100)])),
        [said_result()]
    );
    assert_eq!(
        feed_probing(&mut core, 3_000, Input::Entry(false)),
        [send_probed(2, WindowVerdict::Uia)]
    );
}

/// A console window's probe names the thread `GetWindowThreadProcessId`
/// gives, its client's, while its tells name the console host's thread
/// that raised the event: the probe still decides the request's window.
#[test]
fn a_probe_of_the_requests_window_naming_another_owner_thread_decides_it() {
    let mut core = probing_core();
    assert_eq!(
        feed_probing(&mut core, 1_000, Input::Entry(false)),
        [send_probed(1, WindowVerdict::ProbeFirst)]
    );
    let client_thread = verbatim_model::request::Probe {
        thread: PROBED.thread + 1,
        ..probe(true, 1_100)
    };
    assert_eq!(
        feed_probing(&mut core, 2_000, answer_probed(1, vec![client_thread])),
        [said_result()]
    );
    assert_eq!(
        feed_probing(&mut core, 3_000, Input::Entry(false)),
        [send_probed(2, WindowVerdict::Uia)]
    );
}

#[test]
fn a_probe_verdict_lapses_half_a_second_after_the_probe_began() {
    let mut core = probing_core();
    assert_eq!(
        feed_probing(&mut core, 1_000, Input::Entry(false)),
        [send_probed(1, WindowVerdict::ProbeFirst)]
    );
    assert_eq!(
        feed_probing(&mut core, 2_000, answer_probed(1, vec![probe(true, 1_100)])),
        [said_result()]
    );
    assert_eq!(
        feed_probing(&mut core, 601_100, Input::Entry(false)),
        [send_probed(2, WindowVerdict::ProbeFirst)]
    );
}

#[test]
fn a_probe_first_request_answered_with_no_probe_counts_as_one_that_timed_out() {
    let mut core = probing_core();
    assert_eq!(
        feed_probing(&mut core, 1_000, Input::Entry(false)),
        [send_probed(1, WindowVerdict::ProbeFirst)]
    );
    assert_eq!(
        feed_probing(&mut core, 2_000, answer_probed(1, Vec::new())),
        [said_result()]
    );
    assert_eq!(
        feed_probing(&mut core, 3_000, Input::Entry(false)),
        [send_probed(2, WindowVerdict::Msaa)]
    );
}

#[test]
fn a_probe_first_request_no_longer_awaited_counts_as_a_probe_that_timed_out_from_its_send() {
    let mut core = probing_core();
    assert_eq!(
        feed_probing(&mut core, 1_000, Input::Entry(false)),
        [send_probed(1, WindowVerdict::ProbeFirst)]
    );
    assert_eq!(
        feed_probing(&mut core, 2_000, Input::Foreground(SAME_APP_ELSEWHERE)),
        [
            Effect::Queue(QueueEffect::Cancel {
                to: OUTPOST,
                cancel: Cancel {
                    number: RequestNumber(1),
                    reason: CancelReason::Foreground
                }
            }),
            said("NoLongerAwaited(Foreground)")
        ]
    );
    assert_eq!(
        feed_probing(&mut core, 3_000, Input::Foreground(IN_FRONT)),
        []
    );
    assert_eq!(
        feed_probing(&mut core, 4_000, Input::Entry(false)),
        [send_probed(2, WindowVerdict::Msaa)]
    );
    assert_eq!(
        feed_probing(&mut core, 5_000, answer_probed(2, Vec::new())),
        [said_result()]
    );
    assert_eq!(
        feed_probing(&mut core, 501_001, Input::Entry(false)),
        [send_probed(3, WindowVerdict::ProbeFirst)]
    );
}

/// A request withdrawn by its entry is left to run in its reader, which
/// may still answer its probe: Core has not waited it out, so its window
/// keeps no verdict and the next request probes again, and a UI
/// Automation focus told from the window is judged by its own probe at
/// its arrival.
#[test]
fn a_probe_first_request_withdrawn_leaves_its_window_with_no_verdict() {
    let mut core = probing_core();
    assert_eq!(
        feed_probing(&mut core, 1_000, Input::Entry(false)),
        [send_probed(1, WindowVerdict::ProbeFirst)]
    );
    assert_eq!(
        feed_probing(&mut core, 2_000, Input::Entry(true)),
        [
            Effect::Queue(QueueEffect::Cancel {
                to: OUTPOST,
                cancel: Cancel {
                    number: RequestNumber(1),
                    reason: CancelReason::Withdrawn
                }
            }),
            said("NoLongerAwaited(Withdrawn)"),
            send_probed(2, WindowVerdict::ProbeFirst)
        ]
    );
}

#[test]
fn a_destroyed_windows_probe_verdict_is_cleared() {
    let mut core = probing_core();
    assert_eq!(
        feed_probing(&mut core, 1_000, Input::Entry(false)),
        [send_probed(1, WindowVerdict::ProbeFirst)]
    );
    assert_eq!(
        feed_probing(&mut core, 2_000, answer_probed(1, vec![probe(true, 1_100)])),
        [said_result()]
    );
    assert_eq!(
        feed_probing(
            &mut core,
            2_100,
            Input::WindowDestroyed {
                window: PROBED.handle,
                process: None,
            }
        ),
        []
    );
    assert_eq!(
        feed_probing(&mut core, 3_000, Input::Entry(false)),
        [send_probed(2, WindowVerdict::ProbeFirst)]
    );
}

/// A request on a plain node is answered by Core's plain reader as it
/// begins, with nothing sent, the function going on to its next ask in the
/// same step; a checkpoint taken after it rebuilds the function from the
/// recorded answer and goes on alike.
#[test]
fn a_plain_read_is_answered_at_once_and_a_checkpoint_after_it_rebuilds() {
    let mut core = core_with(IN_FRONT);
    assert_eq!(
        feed(&mut core, 1_000, Input::Entry(Entry::PlainThenRead)),
        [said("plain Result(PropertyValues([]))"), send(2)]
    );
    let mut resumed = core.checkpoint().unwrap();
    assert_eq!(resumed.dump(), core.dump());
    let answered = feed(&mut core, 2_000, answer(OUTPOST, 2, result()));
    assert_eq!(answered, [said_result()]);
    assert_eq!(
        feed(&mut resumed, 2_000, answer(OUTPOST, 2, result())),
        answered
    );
    assert_eq!(resumed.dump(), core.dump());
}

/// A request from outside the queue on a plain node is answered at once,
/// with nothing sent and no outpost started.
#[test]
fn a_plain_read_from_outside_the_queue_is_answered_at_once() {
    let mut core = core_with(IN_FRONT);
    assert_eq!(
        feed(
            &mut core,
            1_000,
            Input::AskOutside {
                asker: ASKER,
                reader: Reader::Outpost(Pid(9)),
                window: WINDOW,
                request: plain_wire(),
                due_after_us: OTHER_DUE_US,
            }
        ),
        [Effect::Queue(QueueEffect::Answered {
            asker: ASKER.0,
            outcome: WireOutcome::Result(crate::entries::tests::properties(
                PropertyValues::default()
            )),
        })]
    );
    assert!(core.shared.borrow().state.queue.idle());
}

/// A step's outcome as an entry says it.
fn got<T>(got: &crate::batch::Got<'_, T>) -> &'static str {
    match got {
        crate::batch::Got::Done(_) => "done",
        crate::batch::Got::Refused(_) => "refused",
        crate::batch::Got::NotThere(_) => "not there",
        crate::batch::Got::Failed => "failed",
        crate::batch::Got::Skipped => "skipped",
    }
}

/// The batch [`Entry::Batch`] asks: a read of the focus, then a caret.
fn focus_and_caret() -> Batch {
    Batch {
        steps: vec![
            Step {
                op: Op::Read {
                    subject: Subject::Focus,
                    set: vec![PropertyId::Name],
                },
                when: None,
            },
            caret_step(),
        ],
        placement: Placement::Queue,
        spare: None,
    }
}

/// A caret read of node 1.
fn caret_step() -> Step {
    Step {
        op: Op::Caret {
            subject: Subject::Node(NodeId::new(1)),
            adopt: None,
        },
        when: None,
    }
}

/// The send of request `number`, asking `batch`.
fn send_batch(number: u64, batch: Batch) -> Effect {
    Effect::Queue(QueueEffect::Send {
        to: OUTPOST,
        ask: Ask {
            number: RequestNumber(number),
            window: WINDOW.handle,
            verdict: WindowVerdict::Msaa,
            request: batch.into(),
        },
    })
}

/// A caret read's result.
fn caret_done() -> StepOutcome {
    StepOutcome::Done(Done::Caret(CaretAt {
        caret: verbatim_model::TextAnchor(4),
        selection: None,
        held: verbatim_model::TextAnchor(5),
        offsets: None,
        reported: true,
    }))
}

/// The answer to request `number`: these steps' outcomes.
fn steps(number: u64, outcomes: Vec<StepOutcome>) -> Input<Entry> {
    answer(OUTPOST, number, WireOutcome::Result(Reply::Steps(outcomes)))
}

/// A batch is asked as one request and its answer read step by step, each
/// outcome of its own step's kind.
#[test]
fn a_batch_is_asked_as_one_request_and_read_by_its_handles() {
    let mut core = core_with(IN_FRONT);
    assert_eq!(
        feed(&mut core, 1_000, Input::Entry(Entry::Batch)),
        [send_batch(1, focus_and_caret())]
    );
    assert_eq!(
        feed(
            &mut core,
            2_000,
            steps(
                1,
                vec![StepOutcome::NotThere(Missing::Object), caret_done()]
            )
        ),
        [said("in front false, focus not there, caret done")]
    );
}

/// An answer that does not fit the batch sent, a step's result of another
/// operation's kind or a step too few, is undecodable.
#[test]
fn an_answer_that_does_not_fit_its_batch_is_undecodable() {
    for outcomes in [vec![caret_done(), caret_done()], vec![StepOutcome::Failed]] {
        let mut core = core_with(IN_FRONT);
        assert_eq!(
            feed(&mut core, 1_000, Input::Entry(Entry::Batch)),
            [send_batch(1, focus_and_caret())]
        );
        assert_eq!(
            feed(&mut core, 2_000, steps(1, outcomes)),
            [
                Effect::Queue(QueueEffect::Undecodable(RequestNumber(1))),
                said("Refused(Undecodable)"),
            ]
        );
    }
}

/// A batch whose first step reads the application's focus takes what the
/// shell read in front, no step of it run.
#[test]
fn a_batch_reading_the_focus_first_takes_what_is_in_front() {
    let mut core = core_with(IN_FRONT);
    assert_eq!(
        feed(&mut core, 1_000, Input::Entry(Entry::Batch)),
        [send_batch(1, focus_and_caret())]
    );
    assert_eq!(
        feed(
            &mut core,
            1_500,
            Input::InFront {
                number: RequestNumber(1),
                front: verbatim_model::request::Front::Hung,
            }
        ),
        [said("in front true, focus skipped, caret skipped")]
    );
}

/// In the loop that numbers its batches, the first batch's handle given to
/// the second leaves its step off the second, and read from the second's
/// answers is failed: each a defect, logged.
#[test]
fn a_numbered_batch_takes_no_handle_of_another() {
    let mut core = core_with(IN_FRONT);
    let one_caret = Batch {
        steps: vec![caret_step()],
        placement: Placement::Queue,
        spare: None,
    };
    assert_eq!(
        feed(&mut core, 1_000, Input::Entry(Entry::Numbered)),
        [send_batch(1, one_caret.clone())]
    );
    let foreign = Effect::Queue(QueueEffect::ForeignHandle {
        batch: Some(2),
        handle: Some(1),
    });
    assert_eq!(
        feed(&mut core, 2_000, steps(1, vec![caret_done()])),
        [send_batch(2, one_caret), foreign.clone()]
    );
    assert_eq!(
        feed(&mut core, 3_000, steps(2, vec![caret_done()])),
        [said("own done, another's failed"), foreign]
    );
}
