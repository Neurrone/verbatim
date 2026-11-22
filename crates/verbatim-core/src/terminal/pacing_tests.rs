//! The pacing (build 9's design, 5.3), one test per cell, with the limits
//! at four lines in full and two last lines.

use verbatim_model::{
    Effect, SegmentContent, SpeechMark, SpeechPriority, TraceId, Utterance, UtteranceSegment,
};

use super::*;

const UNDER: Under = Under {
    trace: TraceId::from_raw(3),
    fence: None,
};

/// Runs `then` with a voice of four lines in full and two last lines.
fn voiced<R>(marks: &mut u64, then: impl FnOnce(&mut Voice<'_>) -> R) -> R {
    let utterance = |trace_id, segments| Utterance {
        trace_id,
        priority: SpeechPriority::Queued,
        segments,
        source: None,
        say_all: false,
        validity: None,
    };
    let mut voice = Voice {
        full: 4,
        last: 2,
        next_mark: marks,
        utterance: &utterance,
    };
    then(&mut voice)
}

/// Lines `from` to `to`, named "l".
fn lines(from: u32, to: u32) -> Vec<Waiting> {
    (from..=to)
        .map(|n| Waiting::Line(format!("l{n}")))
        .collect()
}

/// What `effects` say, and their marks.
fn said(effects: &[Effect]) -> (Vec<String>, Vec<SpeechMark>) {
    let mut words = Vec::new();
    let mut marks = Vec::new();
    for effect in effects {
        let Effect::Speak(utterance, _) = effect else {
            continue;
        };
        let mut text = Vec::new();
        for segment in &utterance.segments {
            match &segment.content {
                SegmentContent::Mark(mark) => marks.push(*mark),
                SegmentContent::Text(line) => text.push(line.clone()),
                other => text.push(format!("{other:?}")),
            }
        }
        words.push(text.join(" "));
    }
    (words, marks)
}

/// Plays everything handed on: each mark reached in turn.
fn play(pacing: &mut Pacing, marks: &mut u64, effects: &[Effect]) -> Vec<String> {
    let (mut heard, mut pending) = said(effects);
    while let Some(mark) = (!pending.is_empty()).then(|| pending.remove(0)) {
        let more = voiced(marks, |voice| pacing.mark_reached(mark, voice));
        let (words, new) = said(&more);
        heard.extend(words);
        pending.extend(new);
    }
    heard
}

#[test]
fn idle_on_an_entrys_lines_speaks_two_ahead() {
    let (mut pacing, mut marks) = (Pacing::Idle, 0);
    let effects = voiced(&mut marks, |voice| pacing.take(lines(1, 3), UNDER, voice));
    assert_eq!(said(&effects).0, ["l1", "l2"]);
    assert!(matches!(pacing, Pacing::Speaking(_)));
}

#[test]
fn idle_ignores_a_mark_and_stays_on_a_cut_or_settle() {
    let (mut pacing, mut marks) = (Pacing::Idle, 0);
    let effects = voiced(&mut marks, |voice| {
        pacing.mark_reached(SpeechMark(5), voice)
    });
    assert_eq!(effects, []);
    pacing.drop_waiting();
    let effects = voiced(&mut marks, |voice| pacing.look(false, voice));
    assert_eq!((pacing, effects), (Pacing::Idle, Vec::new()));
}

#[test]
fn speaking_holds_while_its_group_has_lines_and_a_full_backlog_waits() {
    let (mut pacing, mut marks) = (Pacing::Idle, 0);
    let _ = voiced(&mut marks, |voice| pacing.take(lines(1, 9), UNDER, voice));
    assert!(pacing.holds(4));
    assert!(!pacing.released());
}

#[test]
fn speaking_appends_an_entrys_lines_keeping_the_backlog_to_the_limits() {
    let (mut pacing, mut marks) = (Pacing::Idle, 0);
    let first = voiced(&mut marks, |voice| pacing.take(lines(1, 4), UNDER, voice));
    let more = voiced(&mut marks, |voice| pacing.take(lines(5, 20), UNDER, voice));
    assert_eq!(more, []);
    let Pacing::Speaking(group) = &pacing else {
        panic!("speaking");
    };
    // The group's two left, then the backlog cut to four lines behind one
    // count of those skipped.
    assert_eq!(group.waiting.len(), 7);
    assert_eq!(group.waiting[2], Waiting::Skipped(Skipped::Count(12)));
    assert_eq!(said(&first).0, ["l1", "l2"]);
}

#[test]
fn speaking_on_an_opening_mark_hands_the_next_line_on() {
    let (mut pacing, mut marks) = (Pacing::Idle, 0);
    let effects = voiced(&mut marks, |voice| pacing.take(lines(1, 3), UNDER, voice));
    let (_, reached) = said(&effects);
    let next = voiced(&mut marks, |voice| pacing.mark_reached(reached[0], voice));
    assert_eq!(said(&next).0, ["l3"]);
}

#[test]
fn speaking_on_the_last_lines_closing_mark_is_due_and_lifts_the_hold() {
    let (mut pacing, mut marks) = (Pacing::Idle, 0);
    let effects = voiced(&mut marks, |voice| pacing.take(lines(1, 6), UNDER, voice));
    let heard = play(&mut pacing, &mut marks, &effects);
    assert_eq!(heard, ["l1", "l2", "l3", "l4"]);
    assert!(matches!(&pacing, Pacing::Due { waiting, .. } if waiting.len() == 2));
    assert!(pacing.released());
    assert!(!pacing.holds(4));
}

#[test]
fn speaking_on_a_cut_is_idle_and_drops_what_waits() {
    let (mut pacing, mut marks) = (Pacing::Idle, 0);
    let effects = voiced(&mut marks, |voice| pacing.take(lines(1, 6), UNDER, voice));
    let (_, reached) = said(&effects);
    assert!(pacing.cancelled(&reached));
    assert!(!pacing.cancelled(&[SpeechMark(99)]));
    pacing = Pacing::Idle;
    let effects = voiced(&mut marks, |voice| pacing.mark_reached(reached[1], voice));
    assert_eq!(effects, []);
}

#[test]
fn speaking_with_report_new_output_off_drops_what_waits() {
    let (mut pacing, mut marks) = (Pacing::Idle, 0);
    let effects = voiced(&mut marks, |voice| pacing.take(lines(1, 6), UNDER, voice));
    pacing.drop_waiting();
    let heard = play(&mut pacing, &mut marks, &effects);
    assert_eq!(heard, ["l1", "l2"]);
    assert!(matches!(&pacing, Pacing::Due { waiting, .. } if waiting.is_empty()));
}

#[test]
fn due_on_an_entrys_lines_appends_for_the_look() {
    let (mut pacing, mut marks) = (Pacing::Idle, 0);
    let effects = voiced(&mut marks, |voice| pacing.take(lines(1, 5), UNDER, voice));
    let _ = play(&mut pacing, &mut marks, &effects);
    let more = voiced(&mut marks, |voice| pacing.take(lines(6, 7), UNDER, voice));
    assert_eq!(more, []);
    assert!(matches!(&pacing, Pacing::Due { waiting, .. } if waiting.len() == 3));
}

#[test]
fn due_waits_at_settle_while_an_output_read_is_out() {
    // Q9: the next group is never built from lines read before the hold.
    let (mut pacing, mut marks) = (Pacing::Idle, 0);
    let effects = voiced(&mut marks, |voice| pacing.take(lines(1, 9), UNDER, voice));
    let _ = play(&mut pacing, &mut marks, &effects);
    let effects = voiced(&mut marks, |voice| pacing.look(true, voice));
    assert_eq!(effects, []);
    assert!(matches!(pacing, Pacing::Due { .. }));
}

#[test]
fn due_at_settle_skips_all_but_the_last_lines_past_the_full_limit() {
    let (mut pacing, mut marks) = (Pacing::Idle, 0);
    let effects = voiced(&mut marks, |voice| pacing.take(lines(1, 9), UNDER, voice));
    let _ = play(&mut pacing, &mut marks, &effects);
    let effects = voiced(&mut marks, |voice| pacing.look(false, voice));
    assert_eq!(said(&effects).0, ["Phrase(SkippedLines(3))", "l8"]);
}

#[test]
fn due_at_settle_speaks_a_short_backlog_whole() {
    let (mut pacing, mut marks) = (Pacing::Idle, 0);
    let effects = voiced(&mut marks, |voice| pacing.take(lines(1, 7), UNDER, voice));
    let _ = play(&mut pacing, &mut marks, &effects);
    let effects = voiced(&mut marks, |voice| pacing.look(false, voice));
    assert_eq!(said(&effects).0, ["l5", "l6"]);
}

#[test]
fn due_with_nothing_waiting_is_idle_at_settle() {
    let (mut pacing, mut marks) = (Pacing::Idle, 0);
    let effects = voiced(&mut marks, |voice| pacing.take(lines(1, 2), UNDER, voice));
    let _ = play(&mut pacing, &mut marks, &effects);
    let effects = voiced(&mut marks, |voice| pacing.look(false, voice));
    assert_eq!((pacing, effects), (Pacing::Idle, Vec::new()));
}

#[test]
fn due_with_report_new_output_off_is_idle() {
    let (mut pacing, mut marks) = (Pacing::Idle, 0);
    let effects = voiced(&mut marks, |voice| pacing.take(lines(1, 6), UNDER, voice));
    let _ = play(&mut pacing, &mut marks, &effects);
    pacing.drop_waiting();
    assert_eq!(pacing, Pacing::Idle);
}

#[test]
fn a_blank_line_counts_and_is_not_spoken() {
    // Q1: blank lines count as lines toward a skip.
    let (mut pacing, mut marks) = (Pacing::Idle, 0);
    let output = vec![
        Waiting::Line("a".into()),
        Waiting::Line(String::new()),
        Waiting::Line("b".into()),
    ];
    let effects = voiced(&mut marks, |voice| pacing.take(output, UNDER, voice));
    assert_eq!(play(&mut pacing, &mut marks, &effects), ["a", "b"]);
}

#[test]
fn a_line_past_the_bound_is_cut_saying_so() {
    let (mut pacing, mut marks) = (Pacing::Idle, 0);
    let long = "x".repeat(MAX_WAITING_BYTES + 3);
    let effects = voiced(&mut marks, |voice| {
        pacing.take(vec![Waiting::Line(long)], UNDER, voice)
    });
    let (words, _) = said(&effects);
    assert_eq!(words.len(), 2);
    assert_eq!(words[1], "Message(TerminalLineCut)");
}

#[test]
fn skipped_counts_add_and_stay_exact_only_when_both_are() {
    assert_eq!(Skipped::Count(3).plus(Skipped::Count(4)), Skipped::Count(7));
    assert_eq!(
        Skipped::Count(3).plus(Skipped::Uncounted),
        Skipped::Uncounted
    );
    assert_eq!(
        Skipped::Count(40).plus(Skipped::MoreThan(9001)),
        Skipped::MoreThan(9001)
    );
    assert_eq!(
        Skipped::MoreThan(9001).plus(Skipped::Count(10)),
        Skipped::MoreThan(9011)
    );
    assert_eq!(Skipped::About(4).plus(Skipped::Count(1)), Skipped::About(5));
}

#[test]
fn an_utterance_carries_its_trace_and_marks() {
    let (mut pacing, mut marks) = (Pacing::Idle, 7);
    let effects = voiced(&mut marks, |voice| pacing.take(lines(1, 1), UNDER, voice));
    let Some(Effect::Speak(utterance, None)) = effects.first() else {
        panic!("an utterance");
    };
    assert_eq!(utterance.trace_id, UNDER.trace);
    assert_eq!(
        utterance.segments.first(),
        Some(&UtteranceSegment::new(SegmentContent::Mark(SpeechMark(7))))
    );
}
