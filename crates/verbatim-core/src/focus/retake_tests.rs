//! The reads of the current focus Core makes itself: the startup read
//! at startup, the re-take after a reader's replacement
//! and the read after a hung window answers again
//! (`books/dev/src/reference/crates/verbatim-core.md`, "Focus, events and objects", the committed focus, and "Core's requests", the recovery set).
use crate::entries::tests::found_at;
use crate::readers::Found;
use verbatim_model::property::{PropertyAnswer, PropertyId, PropertyValue};

use verbatim_model::request::{FocusCaret, FocusCaretRead, FocusRead, KnownAncestor};
use verbatim_model::tell::{ElementToken, Read};
use verbatim_model::{
    CaretReport, NodeId, OutpostId, Pid, Role, TextAnchor, TextChunk, TextUnit, TraceId,
    WindowHandle,
};

use crate::entries::tests::{
    APP, GENERATION, Held, LISTENER, NOTHING, OUTPOST, SPOKEN_FOLLOWED, asked_completion, entry,
    feed_at, focus_read, held, msaa_tell, node, ready, ready_at, uia_tell, values,
};
use crate::event_queue::{Foreground, Input, Instance, Reader};
use crate::focus::versions::{Committed, ReadBy};

/// The application's window 4 comes in front.
const IN_FRONT: Foreground = Foreground {
    window: WindowHandle(4),
    root_owner: WindowHandle(4),
    process: Some(APP),
    thread: 41,
    by_class: None,
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
        found_at(&mut state, 600, (OUTPOST, 1), Found::Read(focus_read(None))),
        SPOKEN_FOLLOWED
    );
    state
}

#[test]
fn the_first_foreground_report_reads_the_focus_and_speaks_it_as_a_switch() {
    let state = started();
    assert_eq!(state.versions.focus().unwrap().context.snapshot.id, node(2));
    assert_eq!(
        state.versions.focus().unwrap().context.trace_id,
        Some(TraceId::from_raw(11))
    );
    let Some(ReadBy::Uia { token, .. }) = state.versions.last.as_ref().map(|last| &last.read_by)
    else {
        panic!("the startup read's UI Automation focus is the readers' last");
    };
    assert_eq!(*token, None);
}

/// A first foreground report whose window no process could be read as
/// serving, a console whose client runs as another user, starts no outpost
/// and asks no reader: no startup read waits on a process that does not
/// exist, and no application holds attention.
#[test]
fn a_foreground_whose_process_cannot_be_read_starts_no_outpost_and_asks_nothing() {
    let mut state = Held::default();
    state.state.queue.start_for_tests(LISTENER);
    assert_eq!(
        feed_at(&mut state, 0, Input::ReaderReady(LISTENER)),
        NOTHING
    );
    let unread = Foreground {
        process: None,
        ..IN_FRONT
    };
    let effects = crate::entries::feed(
        &mut state.state,
        &mut state.runtime,
        crate::event_queue::Timed {
            at_us: 500,
            input: Input::Foreground(unread),
        },
    )
    .unwrap();
    assert_eq!(effects, Vec::<verbatim_model::Effect>::new());
    assert_eq!(state.state.attention(), None);
    assert_eq!(
        state.state.queue.last_number_for_tests(),
        0,
        "no request begun"
    );
    assert!(state.state.queue.front_for_tests().is_none());
}

#[test]
fn a_later_foreground_report_reads_nothing() {
    let mut state = started();
    let elsewhere = Foreground {
        window: WindowHandle(70),
        root_owner: WindowHandle(70),
        process: Some(Pid(41)),
        ..IN_FRONT
    };
    assert_eq!(
        feed_at(&mut state, 700, Input::Foreground(elsewhere)),
        NOTHING
    );
}

/// The re-take keeps its read's own fence, not a key's that joined while
/// it was out.
#[test]
fn the_committed_focus_read_again_after_its_outpost_ends_is_taken_silently() {
    let mut state = started();
    state.arrivals.key_joined(verbatim_model::KeySeq::new(3));
    let fence = state.arrivals.fence();
    assert_eq!(
        feed_at(&mut state, 1_000, Input::ReaderEnded(OUTPOST)),
        NOTHING
    );
    assert!(matches!(state.versions.committed, Committed::Ended(_)));
    assert_eq!(
        ready_at(&mut state, 1_100, REPLACEMENT),
        ["send Outpost(Pid(40)) CurrentFocus"]
    );
    state.arrivals.key_joined(verbatim_model::KeySeq::new(4));
    let read = minted_anew(focus_read(None));
    assert_eq!(
        found_at(&mut state, 1_200, (REPLACEMENT, 3), Found::Read(read)),
        // Taken silently, and named to the replacement generation.
        ["send Outpost(Pid(40)) Follow"]
    );
    let new_id = NodeId::in_outpost(OutpostId(GENERATION + 1), 2);
    assert_eq!(state.versions.focus().unwrap().context.snapshot.id, new_id);
    let Committed::Read(focus) = &state.versions.committed else {
        panic!("the re-take is committed as a reader's focus");
    };
    assert_eq!(focus.context.snapshot.id, new_id);
    assert_eq!(since(&state), Some(Some(fence)));
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
    let _ = found_at(&mut state, 600, (OUTPOST, 1), Found::Read(read));
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
        found_at(&mut state, 1_200, (REPLACEMENT, 3), Found::Read(read)),
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
    found_at(
        &mut state,
        3_000,
        (OUTPOST, 2),
        Found::Read(focus_read(None)),
    );
    // The foreground report's startup read, answered the same.
    assert_eq!(
        feed_at(&mut state, 3_100, Input::Foreground(IN_FRONT)),
        ["send Outpost(Pid(40)) CurrentFocus"]
    );
    found_at(
        &mut state,
        3_200,
        (OUTPOST, 4),
        Found::Read(focus_read(None)),
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
            .batches_for_tests()
            .iter()
            .any(|(_, batch)| crate::entries::tests::batch_kind(batch) == "ReadObject")
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
        found_at(
            &mut state,
            1_700,
            (OUTPOST, 4),
            Found::Read(focus_read(None))
        ),
        NOTHING
    );
    assert!(state.queue.front_for_tests().is_none());
}

/// The silent re-take mints the baseline in the new
/// generation, says nothing of it, and keeps the focus's text-changed
/// flag.
#[test]
fn the_silent_retake_mints_the_baseline_in_the_new_generation_and_keeps_the_flag() {
    let mut state = started();
    if let Committed::Read(focus) = &mut state.versions.committed {
        focus.kept.selection.text_changed = true;
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
        },
        selected_text: None,
        baseline: TextAnchor(90),
    }));
    assert_eq!(
        found_at(&mut state, 1_200, (REPLACEMENT, 3), Found::Read(read)),
        ["send Outpost(Pid(40)) Follow"]
    );
    let Committed::Read(focus) = &state.versions.committed else {
        panic!("the re-take is committed as a reader's focus");
    };
    assert_eq!(
        focus.kept.selection,
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
        found_at(&mut state, 600, (OUTPOST, 1), Found::Read(focus_read(None))),
        SPOKEN_FOLLOWED
    );
    assert_eq!(since(&state), Some(Some(state.arrivals.fence())));
}

/// The Run dialog read through MSAA: the edit "Open" focused in the
/// dialog "Run", a window object of top-level window 4.
fn run_dialog() -> FocusRead {
    let mut read = focus_read(Some(held(-4)));
    read.focus.read.values = values("Open", Role::EditableText);
    let dialog = &mut read.ancestors[0].node.read.values;
    *dialog = values("Run", Role::Dialog);
    dialog.0.push(PropertyAnswer {
        id: PropertyId::NativeWindow,
        value: Some(PropertyValue::Window(WindowHandle(4))),
    });
    read
}

/// A re-take keeps the old generation's dialog entry, met by its window;
/// the next Tab's MSAA read meets the dialog by that window, and the
/// button alone is said, as NVDA's capture at the pinned build says it in
/// the Run dialog (`tests/fixtures/run_dialog_tab/run-tab-1.jsonl`, "OK
/// button").
#[test]
fn a_tab_after_a_retake_that_kept_the_old_dialog_entry_says_the_button() {
    let focused = Found::Read;
    let mut state = ready();
    feed_at(&mut state, 500, Input::Foreground(IN_FRONT));
    found_at(&mut state, 600, (OUTPOST, 1), focused(run_dialog()));
    feed_at(&mut state, 1_000, Input::ReaderEnded(OUTPOST));
    ready_at(&mut state, 1_100, REPLACEMENT);
    let retaken = focused(minted_anew(run_dialog()));
    assert_eq!(
        found_at(&mut state, 1_200, (REPLACEMENT, 2), retaken),
        NOTHING
    );
    feed_at(&mut state, 2_000, entry(msaa_tell(-5)));
    let Some(known) = state
        .queue
        .batches_for_tests()
        .into_iter()
        .flat_map(|(_, batch)| batch.steps)
        .find_map(|step| match step.op {
            verbatim_model::batch::Op::FocusRead { known, .. } => Some(known),
            _ => None,
        })
    else {
        panic!("the Tab's MSAA run is out");
    };
    let dialog = KnownAncestor::Window(WindowHandle(4));
    let met = known.iter().position(|known| *known == dialog).unwrap();
    let mut ok = minted_anew(focus_read(Some(held(-5))));
    ok.focus.read.node = Some(NodeId::in_outpost(OutpostId(GENERATION + 1), 3));
    ok.ancestors.clear();
    ok.met = Some(u32::try_from(met).unwrap());
    assert_eq!(
        found_at(&mut state, 2_100, (REPLACEMENT, 3), focused(ok)),
        ["Label(\"OK\") Role(Button)"]
    );
}

/// The outpost of a focus that is not a window node is replaced and the
/// replacement reports the same focus: it is taken silently, and speech
/// is told where the focus now is, its new node, so that what is said of
/// it later is not dropped against the ended node, as NVDA's speech stays
/// valid while its object equals the focus (`eventHandler.py` 208 to 216
/// and 229 to 232).
#[test]
fn the_silent_retake_tells_speech_the_focus_s_new_node() {
    let mut state = started();
    let _ = feed_at(&mut state, 1_000, Input::ReaderEnded(OUTPOST));
    assert_eq!(
        ready_at(&mut state, 1_100, REPLACEMENT),
        ["send Outpost(Pid(40)) CurrentFocus"]
    );
    let input = crate::entries::tests::found(
        &state,
        REPLACEMENT,
        3,
        Found::Read(minted_anew(focus_read(None))),
    );
    let effects = crate::entries::feed(
        &mut state.state,
        &mut state.runtime,
        crate::event_queue::Timed {
            at_us: 1_200,
            input,
        },
    )
    .unwrap();
    let new_id = NodeId::in_outpost(OutpostId(GENERATION + 1), 2);
    let told: Vec<_> = effects
        .iter()
        .filter_map(|effect| match effect {
            verbatim_model::Effect::DropExpiredSpeech(now) => Some((
                now.focus.id,
                now.replaced.iter().map(|key| key.id).collect::<Vec<_>>(),
            )),
            _ => None,
        })
        .collect();
    // The ended nodes of the focus and its line are the same objects as
    // the new ones, so what was said of them before still holds.
    assert_eq!(told, [(new_id, vec![node(2), node(1)])]);
}
