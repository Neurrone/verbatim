//! The watch (build 9's design, 5.2), one test per cell, the places' table
//! (5.6), and the defects the rebuild fixes, each fed in the order the
//! listener and the outpost give: a tell, the read Core wants, its answer,
//! the speech entry's turn, playback's marks. Windows are written by hand,
//! forty cells wide, one row a line.

use verbatim_model::batch::{Batch, Missing, Op, StepOutcome, StepRefusal};
use verbatim_model::request::{CancelReason, Question, Refusal, Reply};
use verbatim_model::{
    CaretReport, Effect, Role, SegmentContent, SpeechMark, TerminalText, TextAnchor, TextChunk,
    TextUnit, TraceId,
};

use crate::terminal_window::{MarkPlace, MarkSearch, TerminalWindow, WindowCaret, outcomes_making};

use super::*;
use crate::event_queue::Outcome;
use crate::keys::entry::Fence;

const WIDTH: u32 = 40;
const TRACE: TraceId = TraceId::from_raw(9);

/// A state whose committed focus is a terminal, its baseline wanted.
fn terminal() -> SrState {
    let mut state = SrState::new();
    let mut focus = crate::focus::versions::tests::focus(10);
    focus.context.snapshot.role = Role::Terminal;
    let terminal = Terminal {
        node: focus.context.snapshot.id,
        window: focus.kept.asked,
        console: false,
    };
    state.versions.execute(focus, Fence::default());
    committed(&mut state, Some(terminal), Vec::new());
    state
}

/// The tokens and batches of the window reads Core wants, taken: the
/// batches whose first step reads the visible range.
fn wanted(state: &mut SrState) -> Vec<(u64, Batch)> {
    state
        .wants
        .take_for_tests()
        .into_iter()
        .filter_map(|(token, question)| match question {
            Question::Batch(batch) => match batch.steps.first().map(|step| &step.op) {
                Some(Op::Visible { .. }) => Some((token?, batch)),
                _ => None,
            },
        })
        .collect()
}

/// The one window read Core wants.
fn one_read(state: &mut SrState) -> u64 {
    let reads = wanted(state);
    assert_eq!(reads.len(), 1, "one read wanted: {reads:?}");
    reads[0].0
}

/// A window of `lines`, each one row, the last the caret's at its end,
/// read at `at_ms`, the whole window on the screen, its document
/// `document_rows` rows long, and the mark Core named showing `place`.
fn window(
    lines: &[&str],
    at_ms: u64,
    (document_rows, place): (u32, Option<MarkPlace>),
) -> TerminalWindow {
    let rows = u32::try_from(lines.len()).unwrap();
    let last = lines.last().copied().unwrap_or_default();
    TerminalWindow {
        rows: TerminalText {
            text: lines.join("\r\n"),
            lengths: vec![(rows, WIDTH)],
        },
        above: rows,
        screen_top: rows,
        screen_rows: rows,
        width: WIDTH,
        screen_at_start: document_rows == rows,
        caret: Some(WindowCaret {
            report: CaretReport {
                line: TextChunk {
                    unit: TextUnit::Line,
                    text: last.to_owned(),
                    start: TextAnchor(1),
                    offset: u32::try_from(last.len()).unwrap(),
                    languages: Vec::new(),
                    first: false,
                    last: true,
                    truncated: false,
                    formats: Vec::new(),
                },
                selection: None,
            },
            point: TextAnchor(2),
        }),
        caret_rows_to_end: Some(1),
        end: TextAnchor(at_ms),
        first_row: TerminalText::unpadded(lines.first().copied().unwrap_or_default()),
        document_rows,
        end_place: place,
        aside_place: None,
        found: MarkSearch::NotAsked,
        colours: None,
        began_us: at_ms * 1_000,
        ended_us: at_ms * 1_000,
    }
}

/// A window with the mark showing `written` rows past it.
fn grown(lines: &[&str], at_ms: u64, (document_rows, written): (u32, u32)) -> TerminalWindow {
    window(
        lines,
        at_ms,
        (
            document_rows,
            Some(MarkPlace::Rows {
                written,
                opening: None,
            }),
        ),
    )
}

/// How a test answers a read: with a window its steps make, or with an
/// outcome as it is.
enum Answered {
    Window(Box<TerminalWindow>),
    Outcome(Outcome<Reply>),
}

impl From<Outcome<Reply>> for Answered {
    fn from(outcome: Outcome<Reply>) -> Self {
        Self::Outcome(outcome)
    }
}

/// A read answering `window`, its steps' outcomes made for what Core
/// names when it answers ([`outcomes_making`]).
fn read_of(window: TerminalWindow) -> Answered {
    Answered::Window(Box::new(window))
}

/// `answered` as the outcome Core takes, Core's time moved to the
/// window's when it is a window.
fn outcome_of(state: &mut SrState, answered: impl Into<Answered>) -> Outcome<Reply> {
    match answered.into() {
        Answered::Outcome(outcome) => outcome,
        Answered::Window(window) => {
            state.queue.set_now_for_tests(window.ended_us);
            let focus = focus_of(state).expect("a terminal read").clone();
            let ask = ask_now(state, &focus, false);
            Outcome::Result(Reply::Steps(outcomes_making(&window, &ask)))
        }
    }
}

/// Answers the read under `token` with `answered`, at the window's time
/// when it is a window: the speech entries that join, spoken at once at
/// their turns, and what was said.
fn answer(state: &mut SrState, token: u64, answered: impl Into<Answered>) -> Vec<String> {
    let outcome = outcome_of(state, answered);
    let mut said = Vec::new();
    let at_us = state.queue.now_us();
    let entries = facts_answered(state, token, outcome, (at_us, TRACE, &mut said));
    for entry in entries {
        let Entry::TerminalSpeech(entry) = entry else {
            panic!("a speech entry");
        };
        said.extend(speak(state, *entry));
    }
    words(&said)
}

/// What `effects` say, each utterance's words, marks left out.
fn words(effects: &[Effect]) -> Vec<String> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Speak(utterance, _) => Some(&utterance.segments),
            Effect::ForKey(_, effect) => match &**effect {
                Effect::Speak(utterance, _) => Some(&utterance.segments),
                _ => None,
            },
            _ => None,
        })
        .map(|segments| {
            segments
                .iter()
                .filter(|segment| !matches!(segment.content, SegmentContent::Mark(_)))
                .map(|segment| match &segment.content {
                    SegmentContent::Text(text) | SegmentContent::Character(text) => text.clone(),
                    other => format!("{other:?}"),
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect()
}

/// Every mark of every utterance in `effects`, in order.
fn marks(effects: &[Effect]) -> Vec<SpeechMark> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Speak(utterance, _) => Some(&utterance.segments),
            _ => None,
        })
        .flatten()
        .filter_map(|segment| match segment.content {
            SegmentContent::Mark(mark) => Some(mark),
            _ => None,
        })
        .collect()
}

/// A terminal live on `lines`, its baseline answered, nothing out.
fn live(lines: &[&str]) -> SrState {
    let mut state = terminal();
    let token = one_read(&mut state);
    let rows = u32::try_from(lines.len()).unwrap();
    assert_eq!(
        answer(&mut state, token, read_of(window(lines, 10, (rows, None)))),
        Vec::<String>::new()
    );
    state
}

/// The watch.
fn watch(state: &SrState) -> Watch {
    focus_of(state).expect("a terminal's record").watch.clone()
}

/// The read a live watch has, its window left out.
fn read_state(state: &SrState) -> Read {
    match watch(state) {
        Watch::Live { read, .. } => read,
        other => panic!("not live: {other:?}"),
    }
}

/// The focused terminal's node.
fn node_of(state: &SrState) -> NodeId {
    focused(state).expect("a focused terminal").node
}

/// A change of the focused terminal told.
fn tell(state: &mut SrState) {
    let node = node_of(state);
    let _ = told(state, node);
}

/// A change told, and the read it sends.
fn changed(state: &mut SrState) -> u64 {
    tell(state);
    one_read(state)
}

/// Every effect speech handed on from now, playback reaching each mark in
/// turn until nothing more is handed on.
fn play(state: &mut SrState, mut effects: Vec<Effect>) -> Vec<String> {
    let mut heard = Vec::new();
    while !effects.is_empty() {
        heard.extend(words(&effects));
        let reached = marks(&effects);
        effects = reached
            .into_iter()
            .flat_map(|mark| mark_reached(state, mark))
            .collect();
    }
    heard
}

// The watch, 5.2: one test a cell.

#[test]
fn unread_clean_on_a_change_is_unread_dirty_and_sends_its_baseline_at_settle() {
    let mut state = terminal();
    let token = one_read(&mut state);
    let _ = answer(
        &mut state,
        token,
        Outcome::Refused(crate::event_queue::Refused::Reader(Refusal::Unsupported)),
    );
    assert_eq!(watch(&state), Watch::Unread { dirty: false });
    tell(&mut state);
    let baseline = one_read(&mut state);
    assert_eq!(
        watch(&state),
        Watch::Baselining {
            token: baseline,
            dirty: false
        }
    );
}

#[test]
fn unread_on_the_window_answering_again_sends_its_baseline() {
    let mut state = terminal();
    let token = one_read(&mut state);
    let _ = answer(&mut state, token, Outcome::NotResponding);
    assert_eq!(watch(&state), Watch::Unread { dirty: false });
    let window = focused(&state).unwrap().window;
    let _ = recovered(&mut state, &window);
    let baseline = one_read(&mut state);
    assert_eq!(
        watch(&state),
        Watch::Baselining {
            token: baseline,
            dirty: false
        }
    );
}

#[test]
fn baselining_on_a_change_is_dirty_and_reads_again_after_its_answer() {
    let mut state = terminal();
    let token = one_read(&mut state);
    tell(&mut state);
    assert_eq!(wanted(&mut state), []);
    let _ = answer(&mut state, token, read_of(window(&["$"], 10, (1, None))));
    // Live, dirty, so settle sends the output read at once.
    assert!(matches!(
        read_state(&state),
        Read::Out {
            speaks: Speaks::Output,
            ..
        }
    ));
    assert_eq!(wanted(&mut state).len(), 1);
}

#[test]
fn baselining_answered_is_live_and_silent() {
    let mut state = terminal();
    let token = one_read(&mut state);
    assert_eq!(
        answer(
            &mut state,
            token,
            read_of(window(&["one", "$"], 10, (2, None)))
        ),
        Vec::<String>::new()
    );
    assert_eq!(read_state(&state), Read::Idle { dirty: false });
}

#[test]
fn baselining_abandoned_is_unread_with_its_dirt() {
    for (outcome, dirty) in [
        (
            Outcome::Refused(crate::event_queue::Refused::Reader(Refusal::Unsupported)),
            false,
        ),
        (Outcome::NoLongerAwaited(CancelReason::DueTime), true),
        (Outcome::NoLongerAwaited(CancelReason::Withdrawn), true),
        (Outcome::NoLongerAwaited(CancelReason::Foreground), true),
    ] {
        let mut state = terminal();
        let token = one_read(&mut state);
        if dirty {
            tell(&mut state);
        }
        let _ = answer(&mut state, token, outcome);
        // A dirty unread watch sends its baseline at once (settle).
        if dirty {
            assert!(matches!(
                watch(&state),
                Watch::Baselining { dirty: false, .. }
            ));
        } else {
            assert_eq!(watch(&state), Watch::Unread { dirty: false });
        }
    }
}

#[test]
fn baselining_failed_takes_the_empty_baseline_and_speaks_the_windows_lines_after() {
    // Source-derived, as NVDA's monitor starts from empty text: a first read
    // that fails leaves an empty baseline, so the next read's lines are
    // all new, through the pacing.
    for failed in [
        StepOutcome::Failed,
        StepOutcome::Refused(StepRefusal::NoText),
    ] {
        let mut state = terminal();
        let token = one_read(&mut state);
        let _ = answer(
            &mut state,
            token,
            Outcome::Result(Reply::Steps(vec![failed])),
        );
        assert!(matches!(
            watch(&state),
            Watch::Live {
                read: Read::Idle { dirty: false },
                ..
            }
        ));
        let read = changed(&mut state);
        assert_eq!(
            answer(
                &mut state,
                read,
                read_of(window(&["one", "two", "$"], 20, (3, None)))
            ),
            ["one", "two"]
        );
    }
}

#[test]
fn baselining_hung_is_unread_clean() {
    let mut state = terminal();
    let token = one_read(&mut state);
    tell(&mut state);
    let _ = answer(
        &mut state,
        token,
        Outcome::NoLongerAwaited(CancelReason::Hung),
    );
    assert_eq!(watch(&state), Watch::Unread { dirty: false });
}

#[test]
fn baselining_not_there_is_gone() {
    let mut state = terminal();
    let token = one_read(&mut state);
    let _ = answer(&mut state, token, Outcome::NotThere);
    assert_eq!(watch(&state), Watch::Gone);
    let mut state = terminal();
    let token = one_read(&mut state);
    let gone = Outcome::Result(Reply::Steps(vec![StepOutcome::NotThere(Missing::Node(
        focused(&state).unwrap().node,
    ))]));
    let _ = answer(&mut state, token, gone);
    assert_eq!(watch(&state), Watch::Gone);
}

#[test]
fn baselining_on_its_reader_ending_stays() {
    let mut state = terminal();
    let token = one_read(&mut state);
    let _ = answer(&mut state, token, Outcome::ReaderEnded);
    assert_eq!(
        watch(&state),
        Watch::Baselining {
            token,
            dirty: false
        }
    );
}

#[test]
fn an_answer_whose_token_is_not_the_watchs_is_dropped() {
    let mut state = live(&["$"]);
    let read = changed(&mut state);
    assert_eq!(
        answer(
            &mut state,
            read.wrapping_add(100),
            read_of(grown(&["$", "x"], 20, (2, 1)))
        ),
        Vec::<String>::new()
    );
    assert!(matches!(read_state(&state), Read::Out { token, .. } if token == read));
}

#[test]
fn live_idle_on_a_change_sends_an_output_read_at_settle() {
    let mut state = live(&["$"]);
    tell(&mut state);
    let reads = wanted(&mut state);
    assert_eq!(reads.len(), 1);
    assert!(matches!(
        read_state(&state),
        Read::Out {
            speaks: Speaks::Output,
            dirty: false,
            ..
        }
    ));
}

#[test]
fn live_out_on_a_change_is_dirty_and_reads_again_after_its_answer() {
    let mut state = live(&["$"]);
    let read = changed(&mut state);
    tell(&mut state);
    assert_eq!(wanted(&mut state), []);
    assert!(matches!(read_state(&state), Read::Out { dirty: true, .. }));
    assert_eq!(
        answer(&mut state, read, read_of(grown(&["$", "one"], 20, (2, 1)))),
        ["one"]
    );
    assert_eq!(wanted(&mut state).len(), 1);
}

#[test]
fn live_out_answered_speaks_the_diff_and_keeps_the_window() {
    let mut state = live(&["$ ls"]);
    let read = changed(&mut state);
    assert_eq!(
        answer(
            &mut state,
            read,
            read_of(grown(&["$ ls", "a.txt", "$"], 20, (3, 2)))
        ),
        ["a.txt", "$"]
    );
    assert_eq!(read_state(&state), Read::Idle { dirty: false });
}

#[test]
fn live_out_past_its_due_time_reads_again() {
    // Q5: a read past its due time is read again, and "Finished" is heard.
    let mut state = live(&["$ make"]);
    let read = changed(&mut state);
    let _ = answer(
        &mut state,
        read,
        Outcome::NoLongerAwaited(CancelReason::DueTime),
    );
    let again = one_read(&mut state);
    assert_eq!(
        answer(
            &mut state,
            again,
            read_of(grown(&["$ make", "Finished"], 30, (2, 1)))
        ),
        ["Finished"]
    );
}

#[test]
fn live_out_abandoned_otherwise_is_idle_with_its_dirt() {
    let mut state = live(&["$"]);
    let read = changed(&mut state);
    let _ = answer(
        &mut state,
        read,
        Outcome::Refused(crate::event_queue::Refused::Reader(Refusal::Unsupported)),
    );
    assert_eq!(read_state(&state), Read::Idle { dirty: false });
    assert_eq!(wanted(&mut state), []);
}

#[test]
fn live_out_failed_keeps_the_last_window() {
    // NVDA keeps its last text when a read fails, so here too:
    // the next read is judged against it.
    let mut state = live(&["$ ls"]);
    let read = changed(&mut state);
    let _ = answer(
        &mut state,
        read,
        Outcome::Result(Reply::Steps(vec![StepOutcome::Failed])),
    );
    assert_eq!(read_state(&state), Read::Idle { dirty: false });
    let again = changed(&mut state);
    assert_eq!(
        answer(
            &mut state,
            again,
            read_of(grown(&["$ ls", "a.txt"], 30, (2, 1)))
        ),
        ["a.txt"]
    );
}

#[test]
fn a_hung_terminal_speaks_its_output_once_the_window_answers() {
    // Q3: a hang leaves the read idle and clean; the window answering again
    // reads, and what was written meanwhile is spoken once.
    let mut state = live(&["$ build"]);
    let read = changed(&mut state);
    tell(&mut state);
    let _ = answer(&mut state, read, Outcome::NotResponding);
    assert_eq!(read_state(&state), Read::Idle { dirty: false });
    assert_eq!(wanted(&mut state), []);
    let window = focused(&state).unwrap().window;
    let _ = recovered(&mut state, &window);
    let again = one_read(&mut state);
    assert_eq!(
        answer(
            &mut state,
            again,
            read_of(grown(&["$ build", "done"], 900, (2, 1)))
        ),
        ["done"]
    );
}

#[test]
fn live_out_not_there_is_gone_and_a_gone_watch_reads_nothing() {
    let mut state = live(&["$"]);
    let read = changed(&mut state);
    let _ = answer(&mut state, read, Outcome::NotThere);
    assert_eq!(watch(&state), Watch::Gone);
    tell(&mut state);
    assert_eq!(wanted(&mut state), []);
    let node = node_of(&state);
    assert_eq!(key_read_sent(&mut state, node), None);
}

#[test]
fn live_out_on_its_reader_ending_stays() {
    let mut state = live(&["$"]);
    let read = changed(&mut state);
    let _ = answer(&mut state, read, Outcome::ReaderEnded);
    assert!(matches!(read_state(&state), Read::Out { token, .. } if token == read));
}

#[test]
fn any_commit_replaces_the_watch_and_an_answer_still_out_is_stale() {
    // Q16: the answer out at the commit is dropped.
    let mut state = live(&["line 74"]);
    let read = changed(&mut state);
    let terminal = focused(&state).unwrap();
    committed(&mut state, Some(terminal), Vec::new());
    let baseline = one_read(&mut state);
    assert_ne!(baseline, read);
    assert_eq!(
        answer(
            &mut state,
            read,
            read_of(grown(&["line 74", "line 75"], 20, (2, 1)))
        ),
        Vec::<String>::new()
    );
}

#[test]
fn a_keys_read_takes_over_the_read_out_whose_answer_goes_stale() {
    let mut state = live(&["$"]);
    let read = changed(&mut state);
    let node = focused(&state).unwrap().node;
    let key = key_read_sent(&mut state, node).unwrap().token;
    assert!(matches!(read_state(&state), Read::Out { token, .. } if token == key));
    assert_eq!(
        answer(&mut state, read, read_of(grown(&["$", "x"], 20, (2, 1)))),
        Vec::<String>::new()
    );
    assert_eq!(
        answer(&mut state, key, read_of(grown(&["$", "x"], 21, (2, 1)))),
        ["x"]
    );
}

#[test]
fn a_keys_read_in_unread_or_baselining_is_the_silent_baseline() {
    // Up Arrow after a refused baseline, and before the baseline answers:
    // the window is not spoken.
    for refused in [true, false] {
        let mut state = terminal();
        let token = one_read(&mut state);
        if refused {
            let _ = answer(
                &mut state,
                token,
                Outcome::Refused(crate::event_queue::Refused::Reader(Refusal::Unsupported)),
            );
        }
        let node = focused(&state).unwrap().node;
        let key = key_read_sent(&mut state, node).unwrap().token;
        assert!(matches!(watch(&state), Watch::Baselining { token, .. } if token == key));
        let _ = answer(&mut state, token, read_of(window(&["old"], 10, (1, None))));
        assert_eq!(
            answer(
                &mut state,
                key,
                read_of(window(&["history", "$ ls"], 20, (2, None)))
            ),
            Vec::<String>::new()
        );
        assert_eq!(read_state(&state), Read::Idle { dirty: false });
    }
}

// The pacing's hold and the cut (5.2's notes, 5.3).

/// `count` numbered lines named `name`, from `first`.
fn numbered(name: &str, first: u32, count: u32) -> Vec<String> {
    (first..first + count)
        .map(|n| format!("{name} {n}"))
        .collect()
}

#[test]
fn a_flood_is_held_while_a_group_speaks_and_read_at_its_last_line() {
    // F1 and F2: thirty lines whole, the read held, then the read at the
    // group's last line counts what went by ("skipped N lines") and the
    // newest thirty are the next group.
    let mut state = live(&["$"]);
    let first = numbered("line", 1, 70);
    let lines: Vec<&str> = first.iter().map(String::as_str).collect();
    let read = changed(&mut state);
    let mut said = Vec::new();
    let outcome = outcome_of(&mut state, read_of(grown(&lines, 20, (71, 70))));
    let at_us = state.queue.now_us();
    let entries = facts_answered(&mut state, read, outcome, (at_us, TRACE, &mut said));
    let Some(Entry::TerminalSpeech(entry)) = entries.into_iter().next() else {
        panic!("speech");
    };
    let handed = speak(&mut state, *entry);
    assert_eq!(words(&handed), ["line 1", "line 2"]);
    // Thirty lines wait past the group: held, so a change reads nothing.
    tell(&mut state);
    assert_eq!(wanted(&mut state), []);
    let mut effects = handed;
    let mut heard = Vec::new();
    // Play the group: once its last line is handed on the hold lifts.
    let mut release = None;
    while release.is_none() {
        heard.extend(words(&effects));
        let reached = marks(&effects);
        effects = reached
            .into_iter()
            .flat_map(|mark| mark_reached(&mut state, mark))
            .collect();
        release = wanted(&mut state)
            .first()
            .map(|(token, ask)| (*token, ask.clone()));
    }
    // The read is sent once the group's last line was handed on, not
    // before.
    assert!(focus_of(&state).unwrap().pacing.released());
    assert!(!heard.iter().any(|line| line == "line 30"));
    let (release, _) = release.unwrap();
    let many = numbered("line", 4975, 30);
    let window_lines: Vec<&str> = many.iter().map(String::as_str).collect();
    // The rest of the group plays; the look waits for the release read.
    let rest = play(&mut state, effects);
    assert_eq!(rest.last().map(String::as_str), Some("line 30"));
    let after = answer(
        &mut state,
        release,
        read_of(grown(&window_lines, 900, (5005, 4934))),
    );
    assert_eq!(
        after.first().map(String::as_str),
        Some("Phrase(SkippedLines(4944))")
    );
}

#[test]
fn a_cut_while_held_reads_to_the_end_speaking_only_keys() {
    let mut state = live(&["$"]);
    let first = numbered("line", 1, 70);
    let lines: Vec<&str> = first.iter().map(String::as_str).collect();
    let read = changed(&mut state);
    let mut said = Vec::new();
    let outcome = outcome_of(&mut state, read_of(grown(&lines, 20, (71, 70))));
    let at_us = state.queue.now_us();
    let entries = facts_answered(&mut state, read, outcome, (at_us, TRACE, &mut said));
    let Some(Entry::TerminalSpeech(entry)) = entries.into_iter().next() else {
        panic!("speech");
    };
    let _ = speak(&mut state, *entry);
    tell(&mut state);
    assert_eq!(wanted(&mut state), []);
    let _ = cut(&mut state);
    let reads = wanted(&mut state);
    assert_eq!(reads.len(), 1);
    assert!(matches!(
        read_state(&state),
        Read::Out {
            speaks: Speaks::KeysOnly,
            ..
        }
    ));
    let more = numbered("line", 71, 5);
    let more: Vec<&str> = lines
        .iter()
        .copied()
        .chain(more.iter().map(String::as_str))
        .collect();
    assert_eq!(
        answer(&mut state, reads[0].0, read_of(grown(&more, 50, (76, 5)))),
        Vec::<String>::new()
    );
}

#[test]
fn a_read_out_at_a_cut_while_not_held_is_spoken() {
    // Q15: `ping -t` in the console host, Control alone while a read of a
    // new line is out: the line is spoken (q15-ping.jsonl, steps 8 to 26).
    let mut state = live(&["Reply from ::1: time<1ms"]);
    let read = changed(&mut state);
    state.arrivals.speech_cut(verbatim_model::KeySeq::new(1));
    let _ = cut(&mut state);
    let mut said = Vec::new();
    let outcome = outcome_of(
        &mut state,
        read_of(grown(
            &["Reply from ::1: time<1ms", "Reply from ::1: time<2ms"],
            1000,
            (2, 1),
        )),
    );
    let at_us = state.queue.now_us();
    let entries = facts_answered(&mut state, read, outcome, (at_us, TRACE, &mut said));
    let Some(Entry::TerminalSpeech(mut entry)) = entries.into_iter().next() else {
        panic!("speech");
    };
    // It joins after the cut, under the cut's fence.
    entry.fence = state.arrivals.fence();
    assert_eq!(
        words(&speak(&mut state, *entry)),
        ["Reply from ::1: time<2ms"]
    );
}

#[test]
fn output_fenced_off_by_a_later_cut_speaks_nothing() {
    // An entry whose fence a later cut passed hands no output on, as NVDA's
    // queued cut drops it.
    let mut state = live(&["$"]);
    let read = changed(&mut state);
    let mut said = Vec::new();
    let outcome = outcome_of(&mut state, read_of(grown(&["$", "out"], 20, (2, 1))));
    let at_us = state.queue.now_us();
    let entries = facts_answered(&mut state, read, outcome, (at_us, TRACE, &mut said));
    state.arrivals.speech_cut(verbatim_model::KeySeq::new(3));
    let Some(Entry::TerminalSpeech(entry)) = entries.into_iter().next() else {
        panic!("speech");
    };
    assert_eq!(words(&speak(&mut state, *entry)), Vec::<String>::new());
}

// The places, 5.6: one test a row.

/// Sixty rows written past the mark, no more than the window's thirty and
/// "Lines spoken in full"'s thirty together, so the read gives no opening
/// rows, as a window read's steps never do past more.
#[test]
fn no_rotation_counts_the_rows_written_exactly() {
    let mut state = live(&["$"]);
    let read = changed(&mut state);
    let lines = numbered("line", 31, 30);
    let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
    let said = answer(&mut state, read, read_of(grown(&lines, 20, (61, 60))));
    assert_eq!(said[0], "Phrase(SkippedLines(30))");
    assert_eq!(said[1], "line 31");
}

#[test]
fn rotation_with_the_marks_line_in_the_window_speaks_the_diff() {
    let mut state = live(&["a", "b", "$"]);
    let read = changed(&mut state);
    let mut rotated = grown(&["b", "$", "c"], 20, (3, 0));
    rotated.first_row = TerminalText::unpadded("b");
    assert_eq!(answer(&mut state, read, read_of(rotated)), ["c"]);
}

/// The rows a full history holds in the tests.
const FULL: u32 = 9001;

/// A live terminal whose history is full: its baseline and one read that
/// rotated it.
fn full_history() -> SrState {
    let mut state = terminal();
    let token = one_read(&mut state);
    let _ = answer(
        &mut state,
        token,
        read_of(window(&["a", "b", "$"], 10, (FULL, None))),
    );
    let read = changed(&mut state);
    let mut rotated = grown(&["b", "$", "c"], 20, (FULL, 0));
    rotated.first_row = TerminalText::unpadded("b");
    let _ = answer(&mut state, read, read_of(rotated));
    state
}

#[test]
fn a_held_read_after_rotation_asks_for_the_search() {
    let state = full_history();
    let focus = focus_of(&state).unwrap();
    let ask = places_of(&state, focus.terminal.node)
        .unwrap()
        .ask(true, 30, false);
    assert!(ask.end.is_some_and(|end| end.find && end.mark.line == "c"));
    let ask = places_of(&state, focus.terminal.node)
        .unwrap()
        .ask(false, 30, false);
    assert!(ask.end.is_some_and(|end| !end.find));
}

#[test]
fn rotation_with_the_line_found_counts_from_its_distance() {
    let mut state = full_history();
    let read = changed(&mut state);
    let lines = numbered("x", 1, 3);
    let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
    let mut found = grown(&lines, 30, (FULL, 0));
    found.first_row = TerminalText::unpadded("y");
    found.found = MarkSearch::Found(12);
    let said = answer(&mut state, read, read_of(found));
    assert_eq!(said[0], "Phrase(SkippedLines(9))");
}

#[test]
fn rotation_with_the_line_missed_skips_more_than_the_document_holds() {
    // The blank-mark case of `conhost_scrollback_overflow_during_group`:
    // the mark's own row is no evidence, the first row changed with the
    // count the same, and the line is gone.
    let mut state = full_history();
    let read = changed(&mut state);
    let lines = numbered("x", 1, 3);
    let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
    let mut missed = grown(&lines, 30, (FULL, 0));
    missed.first_row = TerminalText::unpadded("y");
    missed.found = MarkSearch::Missed;
    let said = answer(&mut state, read, read_of(missed));
    assert_eq!(said, ["Phrase(SkippedMoreThanLines(8998))"]);
}

#[test]
fn a_flood_of_identical_lines_over_a_full_history_skips_uncounted_lines() {
    let mut state = full_history();
    let read = changed(&mut state);
    let mut same = grown(&["b", "$", "c"], 30, (FULL, 0));
    same.first_row = TerminalText::unpadded("z");
    same.found = MarkSearch::InWindow;
    let said = answer(&mut state, read, read_of(same));
    assert_eq!(said[0], "Phrase(SkippedUncountedLines)");
}

#[test]
fn a_mark_refused_after_a_clear_skips_about_the_documents_rows() {
    let mut state = live(&["$ clear; dir"]);
    let read = changed(&mut state);
    let lines = numbered("file", 1, 3);
    let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
    let said = answer(
        &mut state,
        read,
        read_of(window(&lines, 20, (50, Some(MarkPlace::Refused)))),
    );
    assert_eq!(said[0], "Phrase(SkippedAboutLines(47))");
}

#[test]
fn a_read_the_pacing_did_not_hold_after_rotation_is_spoken_by_its_diff() {
    let mut state = full_history();
    let read = changed(&mut state);
    let mut next = grown(&["$", "c", "d"], 30, (FULL, 0));
    next.first_row = TerminalText::unpadded("$");
    assert_eq!(answer(&mut state, read, read_of(next)), ["d"]);
}

#[test]
fn a_new_screen_speaks_its_screen_and_sets_the_shells_mark_aside() {
    // F14 and Q10: `less` over a long history, then closed: its screen,
    // then the shell's screen with no skip phrase.
    let mut state = live(&["$ less file"]);
    let read = changed(&mut state);
    let mut pager = grown(&["page one", "page two"], 20, (2, 0));
    pager.screen_at_start = true;
    assert_eq!(
        answer(&mut state, read, read_of(pager)),
        ["page one", "page two"]
    );
    let node = focused(&state).unwrap().node;
    let ask = places_of(&state, node).unwrap().ask(false, 30, false);
    assert!(ask.aside.is_some_and(|aside| aside.line == "$ less file"));
    // Speech cut off, so the next screen is handed on at once.
    let _ = cut(&mut state);
    let read = changed(&mut state);
    let mut closed = window(&["$ less file", "$"], 30, (9000, None));
    closed.screen_at_start = false;
    closed.aside_place = Some(MarkPlace::Rows {
        written: 1,
        opening: None,
    });
    assert_eq!(
        answer(&mut state, read, read_of(closed)),
        ["$ less file", "$"]
    );
    let ask = places_of(&state, node).unwrap().ask(false, 30, false);
    assert_eq!(ask.aside, None);
}

// Defects.

#[test]
fn a_retaken_terminal_reads_its_waiting_lines_after_the_listener_dies() {
    // T1: the read at a group's last line is out when the listener dies and
    // the re-take commits: the new watch's baseline is not an output read,
    // so the look runs at the next settle and the waiting lines are
    // spoken. Output between the dropped read and the new baseline is not
    // heard (pending Dickson in `docs/parity/terminals.md`).
    let mut state = live(&["$"]);
    let first = numbered("line", 1, 34);
    let lines: Vec<&str> = first.iter().map(String::as_str).collect();
    let read = changed(&mut state);
    let mut said = Vec::new();
    let outcome = outcome_of(&mut state, read_of(grown(&lines, 20, (35, 34))));
    let at_us = state.queue.now_us();
    let entries = facts_answered(&mut state, read, outcome, (at_us, TRACE, &mut said));
    let Some(Entry::TerminalSpeech(entry)) = entries.into_iter().next() else {
        panic!("speech");
    };
    let mut effects = speak(&mut state, *entry);
    tell(&mut state);
    let mut heard = Vec::new();
    while wanted(&mut state).is_empty() {
        heard.extend(words(&effects));
        let reached = marks(&effects);
        effects = reached
            .into_iter()
            .flat_map(|mark| mark_reached(&mut state, mark))
            .collect();
    }
    // The re-take of the same node commits: the pacing is carried.
    let terminal = focused(&state).unwrap();
    let carried = focus_of(&state).unwrap().carried();
    state.versions.focus_mut().unwrap().context.terminal = Some(Box::new(carried));
    committed(&mut state, Some(terminal), Vec::new());
    let _ = wanted(&mut state);
    heard.extend(play(&mut state, effects));
    assert_eq!(
        heard.iter().filter(|line| line.starts_with("line")).count(),
        34
    );
    assert_eq!(heard.last().map(String::as_str), Some("line 34"));
}

#[test]
fn typing_shown_by_a_keys_final_read_joins_under_the_later_keys_fence() {
    // T2: Windows Terminal, a caret key's final read out under key 8, "z"
    // typed as key 9: the echo's entry joins when the answer arrives, under
    // key 9's fence, so key 9's cut does not drop it.
    let mut state = live(&["$ "]);
    let node = focused(&state).unwrap().node;
    let key = key_read_sent(&mut state, node).unwrap().token;
    state.arrivals.key_joined(verbatim_model::KeySeq::new(8));
    key_passed(&mut state, (KeyKind::Typed, Some("z")), 15_000, TRACE);
    state.arrivals.key_joined(verbatim_model::KeySeq::new(9));
    state.arrivals.speech_cut(verbatim_model::KeySeq::new(9));
    let mut shown = window(
        &["$ z"],
        20,
        (
            1,
            Some(MarkPlace::Rows {
                written: 0,
                opening: None,
            }),
        ),
    );
    shown.caret_rows_to_end = Some(1);
    let entry = with_focus(&mut state, |state, focus| {
        answered(state, focus, (key, Ended::Read(Box::new(shown))), TRACE)
    })
    .flatten()
    .unwrap();
    let entry = SpeechEntry {
        fence: state.arrivals.fence(),
        ..entry
    };
    assert_eq!(entry.echo, ["z"]);
    assert!(!state.arrivals.cut_since(entry.fence));
}

#[test]
fn a_modifiers_cut_then_output_is_spoken() {
    // 6a9e55dc: a modifier's cut with nothing held drops nothing later.
    let mut state = live(&["$"]);
    state.arrivals.speech_cut(verbatim_model::KeySeq::new(1));
    let _ = cut(&mut state);
    let read = changed(&mut state);
    let mut said = Vec::new();
    let outcome = outcome_of(&mut state, read_of(grown(&["$", "out"], 20, (2, 1))));
    let at_us = state.queue.now_us();
    let entries = facts_answered(&mut state, read, outcome, (at_us, TRACE, &mut said));
    let Some(Entry::TerminalSpeech(mut entry)) = entries.into_iter().next() else {
        panic!("speech");
    };
    entry.fence = state.arrivals.fence();
    assert_eq!(words(&speak(&mut state, *entry)), ["out"]);
}

#[test]
fn a_no_text_window_read_takes_the_failed_cell() {
    // 75b5fb5c: a read answering "no text" is a failed read, not a hang.
    let mut state = live(&["$ ls"]);
    let read = changed(&mut state);
    let no_text = Outcome::Result(Reply::Steps(vec![StepOutcome::Refused(
        StepRefusal::NoText,
    )]));
    let _ = answer(&mut state, read, no_text);
    assert_eq!(read_state(&state), Read::Idle { dirty: false });
}

#[test]
fn what_was_read_is_spoken_after_the_outpost_ends() {
    // b7038a48 (Q11): the outpost ending leaves the pacing to speak what
    // was read.
    let mut state = live(&["$"]);
    let read = changed(&mut state);
    let mut said = Vec::new();
    let outcome = outcome_of(
        &mut state,
        read_of(grown(&["$", "a", "b", "c"], 20, (4, 3))),
    );
    let at_us = state.queue.now_us();
    let entries = facts_answered(&mut state, read, outcome, (at_us, TRACE, &mut said));
    let Some(Entry::TerminalSpeech(entry)) = entries.into_iter().next() else {
        panic!("speech");
    };
    let effects = speak(&mut state, *entry);
    let outpost = node_of(&state).outpost().unwrap();
    outpost_ended(&mut state, outpost);
    assert_eq!(play(&mut state, effects), ["a", "b", "c"]);
}

#[test]
fn up_arrow_then_a_typed_letter_speak_once_in_their_window() {
    // f4fb5828 and 1adc1421: Up Arrow recalls a line, "t" is typed at once
    // after: the recalled line with the letter is the keys' answer, the
    // letter not echoed on its own.
    let mut state = live(&["$ "]);
    let node = focused(&state).unwrap().node;
    key_passed(
        &mut state,
        (KeyKind::Caret(Unit::Line), None),
        15_000,
        TRACE,
    );
    key_passed(&mut state, (KeyKind::Typed, Some("t")), 16_000, TRACE);
    let key = key_read_sent(&mut state, node).unwrap().token;
    let recalled = window(
        &["$ echo onet"],
        20,
        (
            1,
            Some(MarkPlace::Rows {
                written: 0,
                opening: None,
            }),
        ),
    );
    let entry = with_focus(&mut state, |state, focus| {
        answered(state, focus, (key, Ended::Read(Box::new(recalled))), TRACE)
    })
    .flatten()
    .unwrap();
    assert_eq!(entry.echo, Vec::<String>::new());
    assert_eq!(
        entry
            .keys
            .iter()
            .map(|said| said.text.as_str())
            .collect::<Vec<_>>(),
        ["$ echo onet"]
    );
}
