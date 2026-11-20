//! The reads of the current focus Core makes itself: the startup read
//! (Table 3, row "Startup"), the re-take after a reader's replacement
//! (column F) and the read after a hung window answers again (Table 18,
//! column B).
use verbatim_model::request::CandidateAnswer;

use verbatim_model::request::{FocusCaret, FocusCaretRead, FocusRead, Reply, Request as Wire};
use verbatim_model::tell::{ElementToken, Read};
use verbatim_model::{
    CaretReport, NodeId, OutpostId, Pid, TextAnchor, TextChunk, TextUnit, TraceId, WindowHandle,
};

use crate::entries::tests::{
    APP, GENERATION, Held, LISTENER, NOTHING, OUTPOST, SPOKEN_FOLLOWED, answer, asked_completion,
    entry, feed_at, focus_read, node, ready, ready_at, uia_tell,
};
use crate::event_queue::{Foreground, Input, Instance, Reader};
use crate::focus::versions::{Committed, ReadBy};

/// The application's window 4 comes in front.
const IN_FRONT: Foreground = Foreground {
    window: WindowHandle(4),
    root_owner: WindowHandle(4),
    process: APP,
    thread: 41,
    by_class: None,
    shell: false,
    ..Foreground::BLANK
};

/// The outpost that replaces [`OUTPOST`].
const REPLACEMENT: Instance = Instance {
    reader: Reader::Outpost(APP),
    generation: GENERATION + 1,
};

/// `read` as the replacement outpost answers it, its nodes minted anew.
fn minted_anew(mut read: FocusRead) -> FocusRead {
    let anew = |id: NodeId| NodeId::in_outpost(OutpostId(GENERATION + 1), id.number());
    read.focus.read.node = read.focus.read.node.map(anew);
    for ancestor in &mut read.ancestors {
        ancestor.node.read.node = ancestor.node.read.node.map(anew);
    }
    read
}

/// Startup, its read answered and spoken as a switch: the window's title
/// and the focus.
fn started() -> Held {
    let mut state = ready();
    state.queue_trace = crate::entries::InputTrace(TraceId::from_raw(11));
    assert_eq!(
        feed_at(&mut state, 500, Input::Foreground(IN_FRONT)),
        ["send Outpost(Pid(40)) CurrentFocus"]
    );
    assert_eq!(
        feed_at(
            &mut state,
            600,
            answer(
                OUTPOST,
                1,
                Reply::Candidate(CandidateAnswer::focused(focus_read(None)))
            )
        ),
        SPOKEN_FOLLOWED
    );
    state
}

#[test]
fn the_first_foreground_report_reads_the_focus_and_speaks_it_as_a_switch() {
    let state = started();
    assert_eq!(state.focus.object().unwrap().snapshot.id, node(2));
    assert_eq!(
        state.focus.object().unwrap().trace_id,
        Some(TraceId::from_raw(11))
    );
    let Some(ReadBy::Uia { token, .. }) = state.versions.last.as_ref().map(|last| &last.read_by)
    else {
        panic!("the startup read's UI Automation focus is the readers' last");
    };
    assert_eq!(*token, None);
}

#[test]
fn a_later_foreground_report_reads_nothing() {
    let mut state = started();
    let elsewhere = Foreground {
        window: WindowHandle(70),
        root_owner: WindowHandle(70),
        process: Pid(41),
        ..IN_FRONT
    };
    assert_eq!(
        feed_at(&mut state, 700, Input::Foreground(elsewhere)),
        NOTHING
    );
}

#[test]
fn the_committed_focus_read_again_after_its_outpost_ends_is_taken_silently() {
    let mut state = started();
    assert_eq!(
        feed_at(&mut state, 1_000, Input::ReaderEnded(OUTPOST)),
        NOTHING
    );
    assert!(matches!(state.versions.committed, Committed::Ended(_)));
    assert_eq!(
        ready_at(&mut state, 1_100, REPLACEMENT),
        ["send Outpost(Pid(40)) CurrentFocus"]
    );
    let read = minted_anew(focus_read(None));
    assert_eq!(
        feed_at(
            &mut state,
            1_200,
            answer(
                REPLACEMENT,
                3,
                Reply::Candidate(CandidateAnswer::focused(read))
            )
        ),
        // Taken silently, and named to the replacement generation.
        ["send Outpost(Pid(40)) Follow"]
    );
    let new_id = NodeId::in_outpost(OutpostId(GENERATION + 1), 2);
    assert_eq!(state.focus.object().unwrap().snapshot.id, new_id);
    let Committed::Read(kept) = &state.versions.committed else {
        panic!("the re-take is committed as a reader's focus");
    };
    assert_eq!(kept.node, new_id);
}

/// The edit field "Body" committed by the startup read: the hook holds
/// caret keys for it and a character typed is echoed; once its outpost
/// ends, the focus is dead (its committed version ended), so the hook
/// holds no caret key for it and nothing typed is echoed against it.
#[test]
fn a_focus_whose_outpost_ended_holds_no_caret_key_and_echoes_nothing() {
    let mut state = ready();
    state.settings.speak_typed_characters = verbatim_model::TypingEcho::EditControls;
    let _ = feed_at(&mut state, 500, Input::Foreground(IN_FRONT));
    let mut read = focus_read(None);
    read.focus.read.values =
        crate::entries::tests::values("Body", verbatim_model::Role::EditableText);
    let _ = feed_at(
        &mut state,
        600,
        answer(OUTPOST, 1, Reply::Candidate(CandidateAnswer::focused(read))),
    );
    assert!(state.holds_caret_keys());
    assert_eq!(
        crate::editing::character_typed(&mut state, TraceId::from_raw(20), "a").len(),
        1
    );
    assert_eq!(
        feed_at(&mut state, 1_000, Input::ReaderEnded(OUTPOST)),
        NOTHING
    );
    assert!(matches!(state.versions.committed, Committed::Ended(_)));
    assert!(!state.holds_caret_keys());
    assert_eq!(
        crate::editing::character_typed(&mut state, TraceId::from_raw(21), "b"),
        Vec::new()
    );
    assert_eq!(state.focused(), None);
}

#[test]
fn a_different_focus_read_after_its_outpost_ends_is_spoken() {
    let mut state = started();
    feed_at(&mut state, 1_000, Input::ReaderEnded(OUTPOST));
    ready_at(&mut state, 1_100, REPLACEMENT);
    let mut read = minted_anew(focus_read(None));
    read.focus.read.values = crate::entries::tests::values("Cancel", verbatim_model::Role::Button);
    assert_eq!(
        feed_at(
            &mut state,
            1_200,
            answer(
                REPLACEMENT,
                3,
                Reply::Candidate(CandidateAnswer::focused(read))
            )
        ),
        // None of the ended generation's ancestors is sent as known, so
        // the walk goes to the top and the window is said again.
        [
            "Label(\"Editor\") Role(Window)",
            "Label(\"Cancel\") Role(Button)",
            "send Outpost(Pid(40)) Follow",
        ]
    );
}

#[test]
fn the_listeners_end_clears_its_token_and_reads_the_attended_applications_focus() {
    let mut state = asked_completion();
    feed_at(
        &mut state,
        3_000,
        answer(OUTPOST, 2, Reply::Focus(Box::new(focus_read(None)))),
    );
    // The foreground report's startup read, answered the same.
    assert_eq!(
        feed_at(&mut state, 3_100, Input::Foreground(IN_FRONT)),
        ["send Outpost(Pid(40)) CurrentFocus"]
    );
    feed_at(
        &mut state,
        3_200,
        answer(
            OUTPOST,
            4,
            Reply::Candidate(CandidateAnswer::focused(focus_read(None))),
        ),
    );
    // A later focus of the listener's waits behind the re-take and is
    // dropped with the listener's element.
    feed_at(&mut state, 3_300, Input::ReaderEnded(OUTPOST));
    feed_at(&mut state, 3_400, entry(uia_tell(2, Vec::new())));
    assert_eq!(
        feed_at(&mut state, 3_500, Input::ReaderEnded(LISTENER)),
        NOTHING
    );
    assert!(
        !state
            .queue
            .table_for_tests()
            .iter()
            .any(|request| matches!(request, Wire::ReadObject(_)))
    );
    let Some(ReadBy::Uia { token, .. }) = state.versions.last.as_ref().map(|last| &last.read_by)
    else {
        panic!("the readers' last focus stays a UI Automation focus");
    };
    assert_eq!(*token, None);
}

#[test]
fn a_recovered_window_reads_the_focus_and_a_uia_focus_on_the_last_runtime_id_is_a_duplicate() {
    let mut state = started();
    // A second focus is stopped as hung while its application holds
    // attention; the window answering again reads the current focus.
    let kept = vec![(ElementToken(3), Read::Answered(true))];
    assert_eq!(
        feed_at(&mut state, 1_000, entry(uia_tell(2, kept))),
        ["send Listener ReadObject"]
    );
    feed_at(
        &mut state,
        1_100,
        Input::HangCheck {
            window: WindowHandle(5),
            hung: true,
        },
    );
    assert_eq!(
        feed_at(
            &mut state,
            1_600,
            Input::HangCheck {
                window: WindowHandle(5),
                hung: false,
            }
        ),
        ["send Outpost(Pid(40)) CurrentFocus"]
    );
    assert_eq!(
        feed_at(
            &mut state,
            1_700,
            answer(
                OUTPOST,
                4,
                Reply::Candidate(CandidateAnswer::focused(focus_read(None)))
            )
        ),
        NOTHING
    );
    assert!(state.queue.front_for_tests().is_none());
}

/// Table 17, column J: the silent re-take mints the baseline in the new
/// generation, says nothing of it, and keeps the focus's text-changed
/// flag.
#[test]
fn the_silent_retake_mints_the_baseline_in_the_new_generation_and_keeps_the_flag() {
    let mut state = started();
    if let Committed::Read(kept) = &mut state.versions.committed {
        kept.selection.text_changed = true;
    }
    let _ = feed_at(&mut state, 1_000, Input::ReaderEnded(OUTPOST));
    let _ = ready_at(&mut state, 1_100, REPLACEMENT);
    let mut read = minted_anew(focus_read(None));
    read.caret = FocusCaret::Read(Box::new(FocusCaretRead {
        caret: CaretReport {
            line: TextChunk {
                unit: TextUnit::Line,
                text: "OK".to_owned(),
                start: TextAnchor(12),
                offset: 0,
                languages: Vec::new(),
                first: true,
                last: true,
                truncated: false,
                formats: Vec::new(),
            },
            selection: None,
            read_at_us: 0,
        },
        selected_text: None,
        baseline: TextAnchor(90),
    }));
    assert_eq!(
        feed_at(
            &mut state,
            1_200,
            answer(
                REPLACEMENT,
                3,
                Reply::Candidate(CandidateAnswer::focused(read))
            )
        ),
        ["send Outpost(Pid(40)) Follow"]
    );
    let Committed::Read(kept) = &state.versions.committed else {
        panic!("the re-take is committed as a reader's focus");
    };
    assert_eq!(
        kept.selection,
        crate::text_facts::SelectionFacts {
            baseline: Some(TextAnchor(90)),
            text_changed: true,
        }
    );
}

/// The readers' last focus's fence, `None` with no last focus.
#[expect(
    clippy::option_option,
    reason = "no last focus, its fence cleared, or its fence"
)]
fn since(state: &Held) -> Option<Option<crate::keys::entry::Fence>> {
    state.versions.last.as_ref().map(|last| last.since)
}

/// The startup read executed keeps its entry's fence.
#[test]
fn the_startup_read_keeps_its_entrys_fence() {
    let mut state = ready();
    state.arrivals.key_joined(verbatim_model::KeySeq::new(2));
    feed_at(&mut state, 500, Input::Foreground(IN_FRONT));
    assert_eq!(
        feed_at(
            &mut state,
            600,
            answer(
                OUTPOST,
                1,
                Reply::Candidate(CandidateAnswer::focused(focus_read(None)))
            )
        ),
        SPOKEN_FOLLOWED
    );
    assert_eq!(since(&state), Some(Some(state.arrivals.fence())));
}

/// The re-take taken silently keeps its entry's fence; a re-take answered
/// not there clears it.
#[test]
fn the_retake_keeps_its_fence_and_one_not_there_clears_it() {
    let mut state = started();
    state.arrivals.key_joined(verbatim_model::KeySeq::new(3));
    feed_at(&mut state, 1_000, Input::ReaderEnded(OUTPOST));
    ready_at(&mut state, 1_100, REPLACEMENT);
    let read = minted_anew(focus_read(None));
    feed_at(
        &mut state,
        1_200,
        answer(
            REPLACEMENT,
            3,
            Reply::Candidate(CandidateAnswer::focused(read)),
        ),
    );
    assert_eq!(since(&state), Some(Some(state.arrivals.fence())));
    let second = Instance {
        reader: Reader::Outpost(APP),
        generation: GENERATION + 2,
    };
    feed_at(&mut state, 2_000, Input::ReaderEnded(REPLACEMENT));
    ready_at(&mut state, 2_100, second);
    feed_at(
        &mut state,
        2_200,
        crate::entries::tests::answer_outcome(
            second,
            5,
            verbatim_model::request::Outcome::NotThere,
        ),
    );
    assert_eq!(since(&state), Some(None));
}
