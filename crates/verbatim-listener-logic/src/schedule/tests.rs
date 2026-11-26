//! The reading threads' schedule's tests: one per rule of
//! `books/dev/src/reference/crates/verbatim-listener-logic.md`, "The reading threads' schedule".

use super::*;

/// Jobs are numbered; a request's number is its job's.
type Under = Schedule<u64, u64>;

fn request(number: u64) -> JobKind {
    JobKind::Request(RequestNumber(number))
}

fn app(pid: u32, generation: u64) -> AppKey {
    AppKey {
        pid: Pid(pid),
        generation,
    }
}

/// The jobs `effects` ended, with why.
fn ended(effects: &Effects<u64, u64>) -> Vec<(u64, Ending)> {
    effects
        .ended
        .iter()
        .map(|ended| (ended.job, ended.ending))
        .collect()
}

/// Pushes `job` of `pid`'s, of `kind`, at `now`.
fn push(schedule: &mut Under, pid: u32, job: u64, kind: JobKind, now: u64) -> Effects<u64, u64> {
    schedule.push(Pid(pid), kind, job, now)
}

/// The job thread `thread` of `app` runs next at `now`.
fn run(schedule: &mut Under, app: AppKey, reader: u64, now: u64) -> Option<u64> {
    match schedule.next(app, reader, now).0 {
        Next::Run { job, .. } => Some(job),
        Next::Wait { .. } | Next::Exit => None,
    }
}

/// Application 1 with both its threads abandoned to requests 1 and 2,
/// request 3 waiting.
fn at_the_limit(schedule: &mut Under) {
    assert_eq!(push(schedule, 1, 1, request(1), 0).start, [(app(1, 1), 1)]);
    assert_eq!(run(schedule, app(1, 1), 1, 0), Some(1));
    push(schedule, 1, 2, request(2), 0);
    let effects = schedule.cancel(RequestNumber(1), CancelReason::Foreground, 1);
    assert_eq!(effects.abandoned, [RequestNumber(1)]);
    assert_eq!(effects.start, [(app(1, 1), 2)]);
    assert_eq!(run(schedule, app(1, 1), 2, 1), Some(2));
    push(schedule, 1, 3, request(3), 2);
    let effects = schedule.cancel(RequestNumber(2), CancelReason::Foreground, 3);
    assert_eq!(effects.abandoned, [RequestNumber(2)]);
    assert_eq!(effects.start, [], "two abandoned: no reader");
}

/// An application's jobs run in order on one thread, Core's requests
/// ahead of releases queued before them.
#[test]
fn requests_run_ahead_of_releases() {
    let mut schedule = Under::default();
    push(&mut schedule, 1, 1, request(1), 0);
    assert_eq!(run(&mut schedule, app(1, 1), 1, 0), Some(1));
    push(&mut schedule, 1, 3, JobKind::Release, 0);
    push(&mut schedule, 1, 4, request(4), 0);
    push(&mut schedule, 1, 5, request(5), 0);
    let mut order = Vec::new();
    for now in 1..=4 {
        let (returned, _) = schedule.returned(app(1, 1), 1, now);
        assert_eq!(returned.reader, Some(1));
        order.extend(run(&mut schedule, app(1, 1), 1, now));
    }
    assert_eq!(order, [4, 5, 3]);
}

/// Each application has a thread of its own: one whose thread is held in
/// a call holds no other.
#[test]
fn an_application_whose_thread_is_held_does_not_hold_another() {
    let mut schedule = Under::default();
    push(&mut schedule, 1, 1, request(1), 0);
    assert_eq!(run(&mut schedule, app(1, 1), 1, 0), Some(1));
    let effects = push(&mut schedule, 2, 2, request(2), 0);
    assert_eq!(effects.start, [(app(2, 2), 2)]);
    assert_eq!(run(&mut schedule, app(2, 2), 2, 0), Some(2));
}

/// A queued request cancelled for any reason is ended cancelled, and the
/// rest still run.
#[test]
fn a_queued_request_is_cancelled_for_every_reason() {
    for reason in [
        CancelReason::Foreground,
        CancelReason::Hung,
        CancelReason::DueTime,
        CancelReason::Withdrawn,
    ] {
        let mut schedule = Under::default();
        push(&mut schedule, 1, 1, request(1), 0);
        assert_eq!(run(&mut schedule, app(1, 1), 1, 0), Some(1));
        push(&mut schedule, 1, 2, request(2), 0);
        push(&mut schedule, 1, 3, request(3), 0);
        let effects = schedule.cancel(RequestNumber(2), reason, 1);
        assert_eq!(ended(&effects), [(2, Ending::Cancelled)], "{reason:?}");
        assert_eq!(effects.abandoned, [], "{reason:?}");
        let _ = schedule.returned(app(1, 1), 1, 2);
        assert_eq!(run(&mut schedule, app(1, 1), 1, 2), Some(3), "{reason:?}");
    }
}

/// A running request cancelled for the foreground has its thread
/// abandoned, answered abandoned, and a fresh thread runs what waits; its
/// late return is discarded.
#[test]
fn a_running_request_cancelled_for_the_foreground_is_abandoned() {
    let mut schedule = Under::default();
    push(&mut schedule, 1, 1, request(1), 0);
    assert_eq!(run(&mut schedule, app(1, 1), 1, 0), Some(1));
    push(&mut schedule, 1, 2, request(2), 0);
    let effects = schedule.cancel(RequestNumber(1), CancelReason::Foreground, 1);
    assert_eq!(effects.abandoned, [RequestNumber(1)]);
    assert_eq!(effects.start, [(app(1, 1), 2)]);
    assert!(!schedule.in_charge(app(1, 1), 1));
    assert_eq!(run(&mut schedule, app(1, 1), 2, 1), Some(2));
    let (returned, _) = schedule.returned(app(1, 1), 1, 2);
    assert_eq!(
        returned,
        Returned {
            send: false,
            reader: None
        }
    );
}

/// A running request cancelled for any other reason is left alone: its
/// answer is sent when its call returns.
#[test]
fn a_running_request_cancelled_for_another_reason_is_left_alone() {
    for reason in [
        CancelReason::Hung,
        CancelReason::DueTime,
        CancelReason::Withdrawn,
    ] {
        let mut schedule = Under::default();
        push(&mut schedule, 1, 1, request(1), 0);
        assert_eq!(run(&mut schedule, app(1, 1), 1, 0), Some(1));
        let effects = schedule.cancel(RequestNumber(1), reason, 1);
        assert_eq!(effects.abandoned, [], "{reason:?}");
        assert_eq!(effects.start, [], "{reason:?}");
        assert!(schedule.in_charge(app(1, 1), 1), "{reason:?}");
        let (returned, _) = schedule.returned(app(1, 1), 1, 2);
        assert_eq!(
            returned,
            Returned {
                send: true,
                reader: Some(1)
            },
            "{reason:?}"
        );
    }
}

/// A request already answered, or never sent, is cancelled with nothing
/// done.
#[test]
fn a_cancel_of_an_answered_request_does_nothing() {
    let mut schedule = Under::default();
    push(&mut schedule, 1, 1, request(1), 0);
    assert_eq!(run(&mut schedule, app(1, 1), 1, 0), Some(1));
    let _ = schedule.returned(app(1, 1), 1, 1);
    let effects = schedule.cancel(RequestNumber(1), CancelReason::Foreground, 2);
    assert_eq!(ended(&effects), Vec::new());
    assert_eq!(effects.abandoned, []);
    assert!(schedule.in_charge(app(1, 1), 1));
}

/// A job at the abandoned limit waits with no thread, and is run by the
/// first abandoned thread that returns, under a new thread number.
#[test]
fn a_job_at_the_abandoned_limit_is_run_by_a_returning_thread() {
    let mut schedule = Under::default();
    at_the_limit(&mut schedule);
    let (returned, _) = schedule.returned(app(1, 1), 1, 4);
    assert_eq!(
        returned,
        Returned {
            send: false,
            reader: Some(3)
        }
    );
    assert_eq!(run(&mut schedule, app(1, 1), 3, 4), Some(3));
}

/// An abandoned thread's return at the limit with nothing waiting leaves
/// the record idle below the limit, and the next job starts a thread.
#[test]
fn a_return_at_the_limit_with_nothing_waiting_lets_the_next_job_start_a_thread() {
    let mut schedule = Under::default();
    at_the_limit(&mut schedule);
    let effects = schedule.cancel(RequestNumber(3), CancelReason::Withdrawn, 4);
    assert_eq!(ended(&effects), [(3, Ending::Cancelled)]);
    let (returned, _) = schedule.returned(app(1, 1), 1, 5);
    assert_eq!(returned.reader, None);
    let effects = push(&mut schedule, 1, 4, request(4), 6);
    assert_eq!(effects.start, [(app(1, 1), 3)]);
}

/// A record whose thread ended idle while an abandoned thread is out is
/// let go once that thread returns.
#[test]
fn an_idle_record_is_let_go_once_its_last_abandoned_thread_returns() {
    let mut schedule = Under::default();
    push(&mut schedule, 1, 1, request(1), 0);
    assert_eq!(run(&mut schedule, app(1, 1), 1, 0), Some(1));
    let effects = schedule.cancel(RequestNumber(1), CancelReason::Foreground, 1);
    assert_eq!(effects.start, [], "nothing waits: no fresh reader");
    let (_, effects) = schedule.returned(app(1, 1), 1, 2);
    assert_eq!(effects.forget, [app(1, 1)]);
}

/// A thread with nothing to run waits for the idle time, then ends and
/// lets its record go; a job within the idle time is run by it.
#[test]
fn a_thread_waits_the_idle_time_then_lets_its_record_go() {
    let mut schedule = Under::default();
    push(&mut schedule, 1, 1, JobKind::Release, 0);
    assert_eq!(run(&mut schedule, app(1, 1), 1, 0), Some(1));
    let _ = schedule.returned(app(1, 1), 1, 10);
    assert_eq!(
        schedule.next(app(1, 1), 1, 10).0,
        Next::Wait {
            until_us: 10 + IDLE_US
        }
    );
    let effects = push(&mut schedule, 1, 2, request(2), 20);
    assert_eq!(effects.wake, [app(1, 1)]);
    assert_eq!(run(&mut schedule, app(1, 1), 1, 20), Some(2));
    let _ = schedule.returned(app(1, 1), 1, 30);
    let (next, effects) = schedule.next(app(1, 1), 1, 30 + IDLE_US);
    assert_eq!(next, Next::Exit);
    assert_eq!(effects.forget, [app(1, 1)]);
}

/// The idle waiter runs once every job queued before it has ended,
/// whatever was queued after it.
#[test]
fn an_idle_waiter_runs_with_later_jobs_queued() {
    let mut schedule = Under::default();
    push(&mut schedule, 1, 1, request(1), 0);
    assert_eq!(run(&mut schedule, app(1, 1), 1, 0), Some(1));
    assert_eq!(schedule.when_idle(7).waiters, Vec::<u64>::new());
    push(&mut schedule, 1, 2, request(2), 1);
    let (_, effects) = schedule.returned(app(1, 1), 1, 2);
    assert_eq!(effects.waiters, [7]);
}

/// An abandoned thread does not hold the idle waiter: a request it was
/// abandoned to no longer runs.
#[test]
fn an_abandoned_thread_does_not_hold_the_idle_waiter() {
    let mut schedule = Under::default();
    push(&mut schedule, 1, 1, request(1), 0);
    assert_eq!(run(&mut schedule, app(1, 1), 1, 0), Some(1));
    assert_eq!(schedule.when_idle(7).waiters, Vec::<u64>::new());
    let effects = schedule.cancel(RequestNumber(1), CancelReason::Foreground, 1);
    assert_eq!(effects.waiters, [7]);
}

/// Every request is answered once: one cancelled queued, one abandoned
/// running whose late return is discarded, one returned.
#[test]
fn every_request_is_answered_once() {
    let mut schedule = Under::default();
    let mut answered: Vec<u64> = Vec::new();
    push(&mut schedule, 1, 1, request(1), 0);
    assert_eq!(run(&mut schedule, app(1, 1), 1, 0), Some(1));
    push(&mut schedule, 1, 2, request(2), 0);
    push(&mut schedule, 1, 3, request(3), 0);
    let effects = schedule.cancel(RequestNumber(2), CancelReason::DueTime, 1);
    answered.extend(
        effects
            .ended
            .iter()
            .filter_map(|ended| ended.number.map(|n| n.0)),
    );
    let effects = schedule.cancel(RequestNumber(1), CancelReason::Foreground, 2);
    answered.extend(effects.abandoned.iter().map(|number| number.0));
    assert_eq!(run(&mut schedule, app(1, 1), 2, 2), Some(3));
    let (late, _) = schedule.returned(app(1, 1), 1, 3);
    assert!(!late.send);
    let (current, _) = schedule.returned(app(1, 1), 2, 4);
    assert!(current.send);
    answered.push(3);
    answered.sort_unstable();
    assert_eq!(answered, [1, 2, 3]);
}

/// A process's exit ends its waiting jobs "not there" when no thread is
/// in charge, and a thread in charge runs what waits, then ends with no
/// idle time; a later job of the reused id starts a new generation.
#[test]
fn an_exit_drains_its_thread_and_a_reused_id_has_a_record_of_its_own() {
    let mut schedule = Under::default();
    at_the_limit(&mut schedule);
    let effects = schedule.application_exited(Pid(1));
    assert_eq!(ended(&effects), [(3, Ending::Exited)]);
    let effects = push(&mut schedule, 1, 4, request(4), 5);
    assert_eq!(effects.start, [(app(1, 2), 3)]);
    assert_eq!(run(&mut schedule, app(1, 2), 3, 5), Some(4));
    push(&mut schedule, 1, 5, request(5), 5);
    let effects = schedule.application_exited(Pid(1));
    assert_eq!(ended(&effects), Vec::new());
    let _ = schedule.returned(app(1, 2), 3, 6);
    assert_eq!(run(&mut schedule, app(1, 2), 3, 6), Some(5));
    let _ = schedule.returned(app(1, 2), 3, 7);
    let (next, effects) = schedule.next(app(1, 2), 3, 7);
    assert_eq!(next, Next::Exit);
    assert_eq!(effects.forget, [app(1, 2)]);
}

/// A thread that cannot be started ends what waits for it, and its record
/// goes.
#[test]
fn a_thread_that_cannot_start_ends_its_jobs_and_lets_its_record_go() {
    let mut schedule = Under::default();
    push(&mut schedule, 1, 1, request(1), 0);
    let effects = schedule.spawn_failed(app(1, 1), 1);
    assert_eq!(ended(&effects), [(1, Ending::NoThread)]);
    assert_eq!(effects.forget, [app(1, 1)]);
    assert_eq!(
        push(&mut schedule, 1, 2, request(2), 1).start,
        [(app(1, 2), 2)]
    );
}

/// The listener's stop ends every waiting job and wakes every thread to
/// exit; a running request's answer is dropped.
#[test]
fn the_stop_ends_what_waits_and_drops_a_running_answer() {
    let mut schedule = Under::default();
    push(&mut schedule, 1, 1, request(1), 0);
    assert_eq!(run(&mut schedule, app(1, 1), 1, 0), Some(1));
    push(&mut schedule, 1, 2, request(2), 0);
    let effects = schedule.stop();
    assert_eq!(ended(&effects), [(2, Ending::Stopped)]);
    assert_eq!(effects.wake, [app(1, 1)]);
    let (returned, _) = schedule.returned(app(1, 1), 1, 1);
    assert!(!returned.send);
    assert_eq!(schedule.next(app(1, 1), 1, 1).0, Next::Exit);
    assert_eq!(
        ended(&push(&mut schedule, 1, 3, request(3), 2)),
        [(3, Ending::Stopped)]
    );
}
