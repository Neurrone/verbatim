//! The outpost generation record (Table 5), driven through Core's queue
//! inputs as the owner thread reports its process facts.

use verbatim_model::Pid;
use verbatim_model::request::{ProcessFact, QueueEffect, QueueInput, Reply, RunRead, Tried};
use verbatim_model::{Effect as Said, TraceId, WindowHandle};

use crate::entries::tests::{
    APP, GENERATION, Held, LISTENER, OUTPOST, focus_read, held, kind, msaa_tell,
};
use crate::event_queue::{Instance, Reader};
use crate::focus::versions::Committed;
use crate::supervisor::record::{LIVENESS_US, Retirement, Stage};

/// The generation after [`OUTPOST`].
const NEXT: Instance = Instance {
    reader: Reader::Outpost(APP),
    generation: GENERATION + 1,
};

/// What routing `input` at `at_us` did, as these tests read it: requests
/// by reader and kind, the record's effects, answers outside the queue and
/// utterances, the wake left out.
fn route(held: &mut Held, at_us: u64, input: &QueueInput) -> Vec<String> {
    crate::entries::route(
        &mut held.state,
        &mut held.runtime,
        (TraceId::from_raw(1), at_us),
        input,
    )
    .into_iter()
    .filter_map(|said| match said {
        Said::Queue(QueueEffect::Send { to, ask }) => {
            Some(format!("send {:?} {}", to.reader, kind(&ask)))
        }
        Said::Queue(QueueEffect::StartOutpost(process)) => Some(format!("start {}", process.0)),
        Said::Queue(QueueEffect::EndOutpost(instance)) => {
            Some(format!("end {}", instance.generation))
        }
        Said::Queue(QueueEffect::StaleFrame(instance)) => Some(format!(
            "stale {:?} {}",
            instance.reader, instance.generation
        )),
        Said::Queue(QueueEffect::Answered { outcome, .. }) => Some(format!("answered {outcome:?}")),
        Said::Speak(..) => Some("said".to_owned()),
        _ => None,
    })
    .collect()
}

fn started(generation: u64) -> QueueInput {
    QueueInput::Process(ProcessFact::OutpostStarted {
        process: APP,
        generation,
    })
}

/// An MSAA focus tell from the listener, which asks the application's
/// outpost for a run.
fn focus_tell() -> QueueInput {
    focus_tell_of(-4)
}

/// An MSAA focus tell of object `object` in window 5.
fn focus_tell_of(object: i32) -> QueueInput {
    QueueInput::Tell {
        from: LISTENER,
        tell: Box::new(msaa_tell(object)),
    }
}

/// A run's answer, from `from` to request `number`: the button "OK".
fn run_answer(from: Instance, number: u64) -> QueueInput {
    QueueInput::Answer {
        from,
        answer: verbatim_model::request::Answer {
            number: verbatim_model::request::RequestNumber(number),
            outcome: verbatim_model::request::Outcome::Result(Reply::Run(RunRead {
                tried: vec![Tried::Focus(Box::new(focus_read(Some(held(-4)))))],
            })),
            probes: Vec::new(),
            detail: String::new(),
        },
    }
}

/// The listener started and ready, no outpost asked for yet.
fn listening() -> Held {
    let mut held = Held::default();
    let _ = route(
        &mut held,
        0,
        &QueueInput::Process(ProcessFact::ListenerStarted(LISTENER.generation)),
    );
    let _ = route(&mut held, 0, &QueueInput::ReaderReady(LISTENER));
    held
}

/// The application's outpost [`OUTPOST`] ready, its focus "OK" committed
/// from request 1.
fn committed() -> Held {
    let mut held = listening();
    assert_eq!(route(&mut held, 1_000, &focus_tell()), ["start 40"]);
    assert_eq!(
        route(&mut held, 1_100, &started(GENERATION)),
        Vec::<String>::new()
    );
    assert_eq!(
        route(&mut held, 1_200, &QueueInput::ReaderReady(OUTPOST)),
        ["send Outpost(Pid(40)) MsaaRun"]
    );
    assert_eq!(
        route(&mut held, 1_300, &run_answer(OUTPOST, 1)),
        ["said", "said"]
    );
    held
}

fn stage(held: &Held) -> Option<Stage> {
    held.state
        .queue
        .record
        .applications
        .get(&APP)
        .map(|application| application.stage)
}

/// Columns A, B and D from none: a request for an application with no
/// outpost starts one; it waits through the start and is sent at the
/// generation's `Ready`.
#[test]
fn a_request_with_no_outpost_starts_one_and_is_sent_at_its_ready() {
    let held = committed();
    assert!(matches!(stage(&held), Some(Stage::Ready(_))));
}

/// Row "None", columns B and D, and row "Ready", column D: a start or a
/// `Ready` that is not the current generation's is dropped and logged.
#[test]
fn a_start_or_ready_of_no_current_generation_is_dropped() {
    let mut held = listening();
    assert_eq!(
        route(&mut held, 1_000, &started(GENERATION)),
        ["stale Outpost(Pid(40)) 3"]
    );
    assert_eq!(
        route(&mut held, 1_100, &QueueInput::ReaderReady(OUTPOST)),
        ["stale Outpost(Pid(40)) 3"]
    );
    let mut held = committed();
    assert_eq!(
        route(&mut held, 2_000, &QueueInput::ReaderReady(OUTPOST)),
        ["stale Outpost(Pid(40)) 3"]
    );
}

/// Frames of an instance that is not its reader's current one are
/// dropped: an ended generation's tell, and a listener's other than the
/// newest started.
#[test]
fn tells_of_an_old_generation_or_listener_are_dropped() {
    let mut held = committed();
    let old_listener = Instance {
        reader: Reader::Listener,
        generation: LISTENER.generation + 1,
    };
    assert_eq!(
        route(
            &mut held,
            2_000,
            &QueueInput::Process(ProcessFact::ListenerStarted(old_listener.generation))
        ),
        Vec::<String>::new()
    );
    assert_eq!(route(&mut held, 2_100, &focus_tell()), ["stale Listener 1"]);
}

/// Row "Launching", column C: the first failed launch starts again at
/// once; a later one waits out its backoff, then launches again for the
/// request still waiting (column H).
#[test]
fn a_failed_launch_starts_again_at_once_then_after_its_backoff() {
    let mut held = listening();
    let _ = route(&mut held, 1_000, &focus_tell());
    let failed = QueueInput::Process(ProcessFact::LaunchFailed {
        process: APP,
        application_gone: false,
    });
    assert_eq!(route(&mut held, 1_100, &failed), ["start 40"]);
    assert_eq!(route(&mut held, 1_200, &failed), Vec::<String>::new());
    assert_eq!(
        stage(&held),
        Some(Stage::WaitingOut {
            due_us: 1_200 + 1_000_000
        })
    );
    assert_eq!(
        route(&mut held, 1_199 + 1_000_000, &QueueInput::Due),
        Vec::<String>::new()
    );
    assert_eq!(
        route(&mut held, 1_200 + 1_000_000, &QueueInput::Due),
        ["start 40"]
    );
    assert_eq!(stage(&held), Some(Stage::Launching));
}

/// Row "Launching", column C with the application gone: as its exit, the
/// waiting request answered "not there" and the record gone.
#[test]
fn a_launch_failed_for_a_gone_application_ends_its_waiting_requests() {
    let mut held = listening();
    let _ = route(&mut held, 1_000, &focus_tell());
    assert_eq!(
        route(
            &mut held,
            1_100,
            &QueueInput::Process(ProcessFact::LaunchFailed {
                process: APP,
                application_gone: true,
            })
        ),
        Vec::<String>::new()
    );
    assert_eq!(stage(&held), None);
    assert!(held.state.queue.table.requests().next().is_none());
}

/// Row "Ready", column F: with no pong for 9 seconds the generation is
/// ended, its reader treated as ended at once and replaced; each pong
/// re-arms it.
#[test]
fn a_generation_with_no_pong_for_nine_seconds_is_ended_and_replaced() {
    let mut held = committed();
    assert_eq!(
        route(
            &mut held,
            5_000_000,
            &QueueInput::Process(ProcessFact::Pong(OUTPOST))
        ),
        Vec::<String>::new()
    );
    assert_eq!(
        route(&mut held, 4_999_999 + LIVENESS_US, &QueueInput::Due),
        Vec::<String>::new()
    );
    assert_eq!(
        route(&mut held, 5_000_000 + LIVENESS_US, &QueueInput::Due),
        ["end 3", "start 40"]
    );
    assert!(matches!(held.state.versions.committed, Committed::Ended(_)));
    assert_eq!(
        route(&mut held, 15_000_000, &started(NEXT.generation)),
        Vec::<String>::new()
    );
    assert_eq!(
        route(&mut held, 15_000_100, &QueueInput::ReaderEnded(OUTPOST)),
        ["stale Outpost(Pid(40)) 3"]
    );
}

/// Row "Ready", column E twice within a minute: the first end is replaced
/// at once, the second waits out its backoff; when it ends with nothing
/// waiting and the application not attended, the record goes (column
/// H).
#[test]
fn a_second_failure_waits_out_its_backoff_and_then_is_not_replaced_unasked() {
    let mut held = committed();
    assert_eq!(
        route(&mut held, 2_000, &QueueInput::ReaderEnded(OUTPOST)),
        ["start 40"]
    );
    let _ = route(&mut held, 2_100, &started(NEXT.generation));
    // The re-take waited for the replacement, sent at its `Ready`; it is
    // answered not there here, so nothing waits for the outpost after its
    // second end.
    assert_eq!(
        route(&mut held, 2_200, &QueueInput::ReaderReady(NEXT)),
        ["send Outpost(Pid(40)) CurrentFocus"]
    );
    let _ = route(
        &mut held,
        2_300,
        &QueueInput::Answer {
            from: NEXT,
            answer: verbatim_model::request::Answer {
                number: verbatim_model::request::RequestNumber(2),
                outcome: verbatim_model::request::Outcome::NotThere,
                probes: Vec::new(),
                detail: String::new(),
            },
        },
    );
    assert_eq!(
        route(&mut held, 3_000, &QueueInput::ReaderEnded(NEXT)),
        Vec::<String>::new()
    );
    assert_eq!(
        stage(&held),
        Some(Stage::WaitingOut {
            due_us: 3_000 + 1_000_000
        })
    );
    assert_eq!(
        route(&mut held, 3_000 + 1_000_000, &QueueInput::Due),
        Vec::<String>::new()
    );
    assert_eq!(stage(&held), None);
}

/// Column I: a foreground report of an application with no outpost starts
/// one, and one of an application waiting out a backoff ends the wait.
#[test]
fn a_foreground_report_starts_an_outpost_and_ends_a_backoff() {
    let mut held = listening();
    let foreground = QueueInput::Foreground(verbatim_model::request::ForegroundReport {
        window: WindowHandle(4),
        root_owner: WindowHandle(4),
        process: APP,
        thread: 41,
        class: "Frame".to_owned(),
    });
    let said = route(&mut held, 1_000, &foreground);
    assert_eq!(said.first().map(String::as_str), Some("start 40"));
    let failed = QueueInput::Process(ProcessFact::LaunchFailed {
        process: APP,
        application_gone: false,
    });
    let _ = route(&mut held, 1_100, &failed);
    let _ = route(&mut held, 1_200, &failed);
    assert!(matches!(stage(&held), Some(Stage::WaitingOut { .. })));
    assert_eq!(route(&mut held, 1_300, &foreground), ["start 40"]);
    assert_eq!(stage(&held), Some(Stage::Launching));
}

/// Row "Ready", column J: every request out is answered "not there", the
/// committed focus's ids are dropped, nothing is taken again and the
/// record goes.
#[test]
fn an_applications_exit_answers_its_requests_not_there_and_takes_nothing_again() {
    let mut held = committed();
    assert_eq!(
        route(&mut held, 2_000, &focus_tell_of(-8)),
        ["send Outpost(Pid(40)) MsaaRun"]
    );
    assert_eq!(
        route(
            &mut held,
            2_100,
            &QueueInput::Process(ProcessFact::ApplicationExited(APP))
        ),
        Vec::<String>::new()
    );
    assert_eq!(stage(&held), None);
    assert!(matches!(held.state.versions.committed, Committed::Ended(_)));
    assert!(held.state.queue.table.requests().next().is_none());
}

/// Row "Ready", column G: an outpost whose application has not held
/// attention for the idle time, of which Core holds nothing, is ended
/// and its record goes; one holding a request out is kept.
#[test]
fn an_idle_outpost_holding_nothing_is_retired_and_one_with_a_request_is_kept() {
    let mut held = listening();
    let retirement = Retirement {
        idle_us: 10_000,
        sweep_us: 1_000,
    };
    let _ = route(
        &mut held,
        0,
        &QueueInput::Process(ProcessFact::Retirement {
            idle_us: retirement.idle_us,
            sweep_us: retirement.sweep_us,
        }),
    );
    let _ = route(&mut held, 1_000, &focus_tell());
    let _ = route(&mut held, 1_100, &started(GENERATION));
    let _ = route(&mut held, 1_200, &QueueInput::ReaderReady(OUTPOST));
    // Request 1 is out: the outpost is held.
    assert_eq!(
        route(&mut held, 20_000, &QueueInput::Due),
        Vec::<String>::new()
    );
    assert!(matches!(stage(&held), Some(Stage::Ready(_))));
    // Answered not there, nothing holds it.
    let _ = route(
        &mut held,
        20_100,
        &QueueInput::Answer {
            from: OUTPOST,
            answer: verbatim_model::request::Answer {
                number: verbatim_model::request::RequestNumber(1),
                outcome: verbatim_model::request::Outcome::NotThere,
                probes: Vec::new(),
                detail: String::new(),
            },
        },
    );
    assert_eq!(route(&mut held, 21_200, &QueueInput::Due), ["end 3"]);
    assert_eq!(stage(&held), None);
}

/// Row "Ready", column G, today's `retirement_decision`: an application's
/// idle time counts from when it last held attention, so an outpost whose
/// application held attention for longer than the idle time is kept until
/// the idle time has passed since it lost attention, then retired.
#[test]
fn an_outpost_is_retired_only_once_the_idle_time_passes_after_its_application_loses_attention() {
    let mut held = listening();
    let _ = route(
        &mut held,
        0,
        &QueueInput::Process(ProcessFact::Retirement {
            idle_us: 10_000,
            sweep_us: 1_000,
        }),
    );
    let foreground_of = |process: Pid, window: u64| {
        QueueInput::Foreground(verbatim_model::request::ForegroundReport {
            window: WindowHandle(window),
            root_owner: WindowHandle(window),
            process,
            thread: 41,
            class: "Frame".to_owned(),
        })
    };
    let _ = route(&mut held, 1_000, &foreground_of(APP, 4));
    let _ = route(&mut held, 1_100, &started(GENERATION));
    let _ = route(&mut held, 1_200, &QueueInput::ReaderReady(OUTPOST));
    // The first foreground's read of the current focus, request 1, is
    // answered "not there": nothing holds the outpost.
    let _ = route(
        &mut held,
        1_300,
        &QueueInput::Answer {
            from: OUTPOST,
            answer: verbatim_model::request::Answer {
                number: verbatim_model::request::RequestNumber(1),
                outcome: verbatim_model::request::Outcome::NotThere,
                probes: Vec::new(),
                detail: String::new(),
            },
        },
    );
    assert!(held.state.queue.table.requests().next().is_none());
    // Attended for 50 ms, far past the idle time, then another
    // application takes the foreground.
    let ended = |said: Vec<String>| -> Vec<String> {
        said.into_iter()
            .filter(|said| said.starts_with("end"))
            .collect()
    };
    assert_eq!(
        ended(route(&mut held, 50_000, &foreground_of(Pid(50), 9))),
        Vec::<String>::new()
    );
    assert_eq!(
        ended(route(&mut held, 52_000, &QueueInput::Due)),
        Vec::<String>::new()
    );
    assert!(matches!(stage(&held), Some(Stage::Ready(_))));
    assert_eq!(ended(route(&mut held, 61_000, &QueueInput::Due)), ["end 3"]);
    assert_eq!(stage(&held), None);
}

/// Verbatim's own outpost is started at once and never retired.
#[test]
fn verbatims_own_outpost_is_started_at_once_and_never_retired() {
    let mut held = listening();
    let own = Pid(77);
    assert_eq!(
        route(
            &mut held,
            0,
            &QueueInput::Process(ProcessFact::OwnProcess(own))
        ),
        ["start 77"]
    );
    let _ = route(
        &mut held,
        0,
        &QueueInput::Process(ProcessFact::OutpostStarted {
            process: own,
            generation: 5,
        }),
    );
    let _ = route(
        &mut held,
        0,
        &QueueInput::ReaderReady(Instance {
            reader: Reader::Outpost(own),
            generation: 5,
        }),
    );
    assert_eq!(held.state.queue.record.sweep_due_us, None);
    assert_eq!(
        held.state.readiness(),
        crate::Readiness {
            spares_ready: 0,
            listener_ready: true,
            own_outpost_ready: true,
        }
    );
    assert_eq!(
        held.state.ready_outposts(),
        [(own, verbatim_model::OutpostId(5))]
    );
}
