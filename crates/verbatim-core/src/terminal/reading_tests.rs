//! The focused terminal's life and its reading (`docs/design/readers-tell-core-asks-build3.md`,
//! Tables 1 and 2), driven through the reading's own transitions on
//! Core's state: the requests each wants, by kind, and the state it
//! leaves. The flood policy's columns are tested with it
//! (`super::super::tests`), and whole sessions through the event queue
//! (`tests/terminal_reading.rs`, `tests/terminal_floods.rs`).

use verbatim_model::request::{CancelReason, Refusal, Reply, Request as Wire};
use verbatim_model::{NodeId, OutpostId, Pid, TerminalReadReply, TextAnchor, TextOp, TextReply};
use verbatim_text::terminal_rows::{Read, ReadText};

use super::*;

fn node() -> NodeId {
    NodeId::in_outpost(OutpostId(1), 12)
}

fn window() -> RequestWindow {
    RequestWindow {
        handle: verbatim_model::WindowHandle(5),
        top_level: verbatim_model::WindowHandle(4),
        root_owner: verbatim_model::WindowHandle(4),
        process: Pid(1),
        thread: 41,
        by_class: None,
    }
}

fn terminal() -> Terminal {
    Terminal {
        node: node(),
        window: window(),
        console: false,
    }
}

/// What a wanted request is, as these tests name it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Want {
    Follow,
    /// A full read in the event queue's order: the baseline.
    Baseline,
    /// An output read.
    Output,
}

/// The requests wanted since the last call, each with its token.
fn wants(state: &mut SrState) -> Vec<(Want, u64)> {
    wanted_for_tests(state)
        .into_iter()
        .map(|(token, request)| {
            let want = match request {
                Wire::Follow(_) => Want::Follow,
                Wire::Text {
                    op: TextOp::ReadTerminal(read),
                    ..
                } if read.output => Want::Output,
                Wire::Text {
                    op: TextOp::ReadTerminal(_),
                    ..
                } => Want::Baseline,
                other => panic!("an unexpected request: {other:?}"),
            };
            (
                want,
                token.expect("every request of the terminal's is awaited"),
            )
        })
        .collect()
}

/// The kinds of `wants`.
fn kinds(wants: &[(Want, u64)]) -> Vec<Want> {
    wants.iter().map(|(want, _)| *want).collect()
}

/// A read of the terminal showing `text`, begun at `started_us`.
fn read(text: &str, started_us: u64) -> Outcome<Reply> {
    let read = Read {
        text: ReadText::unpadded(text),
        screen: text.to_owned(),
        from_start: false,
        shrank: false,
        at_document_start: true,
        alternate: true,
        first_row: text.to_owned(),
        caret_rows_to_end: 1,
        caret_line: text.to_owned(),
        caret_column: 0,
        at_ms: started_us / 1_000,
        above: 0,
    };
    Outcome::Result(Reply::Text(TextReply::TerminalRead(Box::new(
        TerminalReadReply {
            snapshot: crate::snapshot_of(&read, false, None),
            start: TextAnchor(1),
            started_us,
            point: None,
        },
    ))))
}

/// Answers `token` with `outcome` at `now_us`, returning the entries that
/// join.
fn answer(state: &mut SrState, token: u64, outcome: Outcome<Reply>, now_us: u64) -> Vec<Entry> {
    let mut said = Vec::new();
    answered(
        state,
        token,
        outcome,
        (now_us, TraceId::from_raw(1)),
        &mut said,
    )
}

/// The token of the `want` among `wants`.
fn token(wants: &[(Want, u64)], want: Want) -> u64 {
    wants
        .iter()
        .find(|(each, _)| *each == want)
        .map(|(_, token)| *token)
        .expect("the request wanted")
}

/// A state whose focus is the terminal, just committed: its `Follow` and
/// baseline wanted, their tokens.
fn baselining() -> (SrState, u64, u64) {
    let mut state = SrState::new();
    committed(&mut state, Some(terminal()), Vec::new(), 1_000);
    let wanted = wants(&mut state);
    assert_eq!(kinds(&wanted), [Want::Baseline]);
    (state, 0, token(&wanted, Want::Baseline))
}

/// A modelled terminal, its baseline taken at 1 ms, quiet.
fn modelled() -> SrState {
    let (mut state, _, baseline) = baselining();
    let entries = answer(&mut state, baseline, read("$ ", 1_000), 2_000);
    assert_eq!(entries.len(), 1, "the baseline's speech entry");
    assert_eq!(wants(&mut state), []);
    assert!(matches!(
        state.terminal_focus.focused,
        FocusedTerminal::Modelled {
            reading: Reading::Quiet,
            ..
        }
    ));
    state
}

fn reading(state: &SrState) -> Reading {
    match state.terminal_focus.focused {
        FocusedTerminal::Modelled { reading, .. } => reading,
        other => panic!("not modelled: {other:?}"),
    }
}

/// Table 1, column A from none: the terminal committed is baselining, its
/// `Follow` named among the commit's and the baseline sent after it.
#[test]
fn a_terminal_committed_is_followed_then_baselined() {
    let mut state = SrState::new();
    let follow = (
        window(),
        Wire::Follow(Follow {
            node: Some(Followed {
                node: node(),
                group: FollowGroup::Terminal { console: false },
                extra: None,
            }),
        }),
    );
    committed(&mut state, Some(terminal()), vec![follow], 1_000);
    assert_eq!(kinds(&wants(&mut state)), [Want::Follow, Want::Baseline]);
    assert!(matches!(
        state.terminal_focus.focused,
        FocusedTerminal::Baselining {
            follow: Some(_),
            read: Some(_),
            told: None,
            ..
        }
    ));
}

/// Table 1, columns D and E: a change told while the baseline is out is
/// read once the baseline answers, when it came after the baseline began.
#[test]
fn a_change_told_before_the_baseline_answers_is_read_after_it() {
    let (mut state, _, baseline) = baselining();
    told(&mut state, node(), 1_500, 1_500);
    assert_eq!(wants(&mut state), [], "the baseline is out");
    let _ = answer(&mut state, baseline, read("$ ", 1_000), 2_000);
    assert_eq!(kinds(&wants(&mut state)), [Want::Output]);
}

/// Table 1, column F, then D: a failed baseline is sent at the next change.
#[test]
fn a_failed_baseline_is_sent_at_the_next_change() {
    let (mut state, _, baseline) = baselining();
    let _ = answer(
        &mut state,
        baseline,
        Outcome::Refused(crate::event_queue::Refused::Reader(Refusal::Unsupported)),
        2_000,
    );
    assert_eq!(wants(&mut state), []);
    told(&mut state, node(), 3_000, 3_000);
    assert_eq!(kinds(&wants(&mut state)), [Want::Baseline]);
}

/// Table 2, column A from quiet and B: a change told while the read is
/// out is read once more after it, unless the read covered it.
#[test]
fn a_change_told_while_a_read_is_out_is_read_after_it_unless_covered() {
    let mut state = modelled();
    told(&mut state, node(), 10_000, 10_000);
    let first = token(&wants(&mut state), Want::Output);
    told(&mut state, node(), 10_500, 10_500);
    assert_eq!(wants(&mut state), [], "one read out at a time");
    // The read began after the second change: it covered it.
    let _ = answer(&mut state, first, read("$ a", 11_000), 11_500);
    assert_eq!(wants(&mut state), []);
    assert_eq!(reading(&state), Reading::Quiet);
    // A change told after the read began is read once more.
    told(&mut state, node(), 12_000, 12_000);
    let second = token(&wants(&mut state), Want::Output);
    told(&mut state, node(), 12_600, 12_600);
    let _ = answer(&mut state, second, read("$ ab", 12_500), 13_000);
    assert_eq!(kinds(&wants(&mut state)), [Want::Output]);
}

/// Table 2, column A from quiet: a change the last read covered sends
/// nothing.
#[test]
fn a_change_the_last_read_covered_sends_nothing() {
    let mut state = modelled();
    told(&mut state, node(), 500, 3_000);
    assert_eq!(wants(&mut state), []);
}

/// Table 2, column D: a read stopped at its due time is sent again with
/// its mark.
#[test]
fn a_read_stopped_at_its_due_time_is_sent_again() {
    let mut state = modelled();
    told(&mut state, node(), 10_000, 10_000);
    let first = token(&wants(&mut state), Want::Output);
    let _ = answer(
        &mut state,
        first,
        Outcome::NoLongerAwaited(CancelReason::DueTime),
        11_010_000,
    );
    assert_eq!(kinds(&wants(&mut state)), [Want::Output]);
    assert!(matches!(
        reading(&state),
        Reading::Out {
            mark: Mark::Spoken,
            ..
        }
    ));
}

/// Table 2, column C: a refused read is not sent again until a change is
/// told.
#[test]
fn a_refused_read_waits_for_the_next_change() {
    let mut state = modelled();
    told(&mut state, node(), 10_000, 10_000);
    let first = token(&wants(&mut state), Want::Output);
    let _ = answer(
        &mut state,
        first,
        Outcome::Refused(crate::event_queue::Refused::Reader(Refusal::Unsupported)),
        11_000,
    );
    assert_eq!(wants(&mut state), []);
    assert_eq!(reading(&state), Reading::Quiet);
}

/// Table 2, columns E and K: a read whose window hangs stalls the reading
/// until the window answers again; then `Follow` is sent again and one
/// output read covers everything written meanwhile.
#[test]
fn a_hung_read_stalls_until_the_window_answers_then_reads_once() {
    let mut state = modelled();
    told(&mut state, node(), 10_000, 10_000);
    let first = token(&wants(&mut state), Want::Output);
    let _ = answer(&mut state, first, Outcome::NotResponding, 10_500);
    assert_eq!(reading(&state), Reading::Stalled { holding: false });
    told(&mut state, node(), 20_000, 20_000);
    assert_eq!(wants(&mut state), [], "nothing is read while stalled");
    recovered(&mut state, &window(), 600_000);
    assert_eq!(kinds(&wants(&mut state)), [Want::Follow, Want::Output]);
}

/// Table 2, columns A and F: a change told while an entry's full read is
/// out waits behind it, and is read after it when it came after the
/// entry's read began.
#[test]
fn a_change_told_while_an_entrys_read_is_out_waits_behind_it() {
    let mut state = modelled();
    let asked = entry_read(&mut state, node(), None, 10_000).expect("the focused terminal");
    told(&mut state, node(), 10_500, 10_500);
    assert_eq!(reading(&state), Reading::Behind { told: 10_500 });
    assert_eq!(wants(&mut state), []);
    let Outcome::Result(Reply::Text(TextReply::TerminalRead(reply))) = read("$ x", 10_200) else {
        unreachable!("a read");
    };
    let _ = entry_read_ended(
        &mut state,
        &asked,
        Some(&reply),
        (11_000, TraceId::from_raw(1)),
    );
    assert_eq!(kinds(&wants(&mut state)), [Want::Output]);
}

/// Table 2, column J from holding: a cut with a change told reads it at
/// once by a silent read, and the policy goes live.
#[test]
fn a_cut_while_holding_with_a_change_reads_it_silently() {
    let mut state = modelled();
    lines_full(&mut state);
    told(&mut state, node(), 10_000, 10_000);
    assert_eq!(reading(&state), Reading::Holding { told: Some(10_000) });
    cut(&mut state);
    assert_eq!(kinds(&wants(&mut state)), [Want::Output]);
    assert!(matches!(
        reading(&state),
        Reading::Out {
            mark: Mark::Silent,
            ..
        }
    ));
}

/// Table 2, column J from a release read out: it becomes silent.
#[test]
fn a_release_read_out_at_a_cut_becomes_silent() {
    let mut state = modelled();
    lines_full(&mut state);
    told(&mut state, node(), 10_000, 10_000);
    group_last_line(&mut state);
    assert!(matches!(
        reading(&state),
        Reading::Out {
            mark: Mark::Waited { release: true },
            ..
        }
    ));
    assert!(state.terminal.release_waiting);
    let _ = wants(&mut state);
    cut(&mut state);
    assert_eq!(wants(&mut state), []);
    assert!(matches!(
        reading(&state),
        Reading::Out {
            mark: Mark::Silent,
            ..
        }
    ));
    assert!(!state.terminal.release_waiting);
}

/// Table 1, column B: `Follow` refused ends the focused terminal's state.
#[test]
fn a_refused_follow_ends_the_focused_terminal() {
    let mut state = SrState::new();
    let follow = (
        window(),
        Wire::Follow(Follow {
            node: Some(Followed {
                node: node(),
                group: FollowGroup::Terminal { console: false },
                extra: None,
            }),
        }),
    );
    committed(&mut state, Some(terminal()), vec![follow], 1_000);
    let follow = token(&wants(&mut state), Want::Follow);
    let _ = answer(
        &mut state,
        follow,
        Outcome::Refused(crate::event_queue::Refused::Reader(Refusal::Unsupported)),
        2_000,
    );
    assert_eq!(state.terminal_focus.focused, FocusedTerminal::None);
}

/// Table 1, columns K and J: `Follow` hung before the baseline answered
/// leaves the terminal waiting for its window, read again from `Follow`
/// and its baseline when the window answers.
#[test]
fn a_hung_follow_waits_for_the_window_then_follows_and_baselines() {
    let mut state = SrState::new();
    let follow = (
        window(),
        Wire::Follow(Follow {
            node: Some(Followed {
                node: node(),
                group: FollowGroup::Terminal { console: false },
                extra: None,
            }),
        }),
    );
    committed(&mut state, Some(terminal()), vec![follow], 1_000);
    let follow = token(&wants(&mut state), Want::Follow);
    let _ = answer(&mut state, follow, Outcome::NotResponding, 2_000);
    assert!(matches!(
        state.terminal_focus.focused,
        FocusedTerminal::WaitingForWindow { .. }
    ));
    recovered(&mut state, &window(), 600_000);
    assert_eq!(kinds(&wants(&mut state)), [Want::Follow, Want::Baseline]);
}

/// Table 1, column D: a change tell of another node is dropped.
#[test]
fn a_change_of_another_node_is_dropped() {
    let mut state = modelled();
    told(
        &mut state,
        NodeId::in_outpost(OutpostId(1), 13),
        10_000,
        10_000,
    );
    assert_eq!(wants(&mut state), []);
    assert_eq!(reading(&state), Reading::Quiet);
}

/// Table 2, column B: a read answering "not there" ends the focused
/// terminal and its model.
#[test]
fn a_read_of_a_terminal_not_there_ends_it() {
    let mut state = modelled();
    told(&mut state, node(), 10_000, 10_000);
    let first = token(&wants(&mut state), Want::Output);
    let _ = answer(&mut state, first, Outcome::NotThere, 11_000);
    assert_eq!(state.terminal_focus.focused, FocusedTerminal::None);
    assert!(state.terminals.is_empty());
}

/// Table 1, column H: a focus that leaves the terminal ends its state; an
/// answer of its read after that is taken by nothing.
#[test]
fn an_answer_after_the_focus_left_is_taken_by_nothing() {
    let mut state = modelled();
    told(&mut state, node(), 10_000, 10_000);
    let first = token(&wants(&mut state), Want::Output);
    committed(&mut state, None, Vec::new(), 10_500);
    let before = state.clone();
    assert_eq!(answer(&mut state, first, read("$ a", 10_000), 11_000), []);
    assert_eq!(state, before);
}
