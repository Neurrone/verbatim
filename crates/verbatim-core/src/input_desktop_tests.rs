//! The secure desktop and the lock state (Table 8): a switch away handled
//! at once, the read after the return, and the lock state.

use verbatim_model::event_kind::EventKind;
use verbatim_model::property::PropertyValues;
use verbatim_model::request::{
    Follow, QueueEffect, QueueInput, Reply, Request as Wire, SessionLock,
};
use verbatim_model::tell::{Event, EventObject, Payload, TellWindow};
use verbatim_model::{Effect as Said, SegmentContent, TraceId, WindowHandle};

use super::{InputDesktop, RETURN_DELAY_US};
use crate::entries::tests::{
    APP, Held, LISTENER, OUTPOST, SPOKEN_FOLLOWED, answer, feed_at, focus_read, ready,
};
use crate::event_queue::{Foreground, Input};
use crate::focus::versions::Committed;

/// The application's window 4 in front.
const IN_FRONT: Foreground = Foreground {
    window: WindowHandle(4),
    root_owner: WindowHandle(4),
    process: APP,
    thread: 41,
    by_class: None,
    shell: false,
};

/// The button "OK" committed through UI Automation at startup and named
/// in `Follow`; the next request is 3.
fn started() -> Held {
    let mut held = ready();
    let _ = feed_at(&mut held, 500, Input::Foreground(IN_FRONT));
    assert_eq!(
        feed_at(
            &mut held,
            600,
            answer(OUTPOST, 1, Reply::Focus(Box::new(focus_read(None))))
        ),
        SPOKEN_FOLLOWED
    );
    held
}

/// The listener's tell of a switch of the input desktop, `open` when the
/// new one can be opened.
fn switch(open: bool) -> QueueInput {
    QueueInput::Tell {
        from: LISTENER,
        tell: Box::new(Event {
            trace_id: TraceId::from_raw(8),
            kind: EventKind::DesktopSwitch,
            process: APP,
            application: "app".to_owned(),
            thread: 41,
            at_us: 1_000,
            object: EventObject::Msaa {
                window: WindowHandle(0),
                object: 0,
                child: 0,
            },
            cached: PropertyValues::default(),
            payload: Payload::DesktopSwitch {
                input_desktop_open: open,
            },
            window: TellWindow::NoWindow,
            checked: None,
        }),
    }
}

/// What routing `input` at `at_us` did, as a test reads it: each request
/// by its reader and kind (a `Follow` by what it names), each utterance by
/// its segments, a cut as "cut", the rest left out.
fn route(held: &mut Held, at_us: u64, input: &QueueInput) -> Vec<String> {
    crate::entries::route(
        &mut held.state,
        &mut held.runtime,
        (TraceId::from_raw(1), at_us),
        input,
    )
    .said()
    .into_iter()
    .filter_map(|said| match said {
        Said::StopSpeech => Some("cut".to_owned()),
        Said::Queue(QueueEffect::Send { to, ask }) => Some(match ask.request {
            Wire::Follow(Follow { node }) => format!("send {:?} Follow {node:?}", to.reader),
            Wire::CurrentFocus { known, .. } => {
                format!("send {:?} CurrentFocus known {}", to.reader, known.len())
            }
            _ => format!("send {:?} other", to.reader),
        }),
        Said::Speak(utterance, _) => Some(
            utterance
                .segments
                .iter()
                .filter(|segment| !matches!(segment.content, SegmentContent::Mark(_)))
                .map(|segment| format!("{:?}", segment.content))
                .collect::<Vec<_>>()
                .join(" "),
        ),
        _ => None,
    })
    .collect()
}

/// Table 8, column A from the user's desktop: speech is cut, the sleeping
/// focus is committed with nothing followed, the hook's view counts the
/// switch away, and the old application is asked nothing more.
#[test]
fn a_switch_away_cuts_speech_and_commits_the_sleeping_focus_following_nothing() {
    let mut held = started();
    assert_eq!(
        route(&mut held, 1_000, &switch(false)),
        ["cut", "send Outpost(Pid(40)) Follow None"]
    );
    assert_eq!(
        held.state.versions.committed,
        Committed::Own { source: None }
    );
    assert!(held.state.focus.object().is_none());
    assert_eq!(held.state.desktop.input, InputDesktop::Other);
    assert_eq!(held.state.desktop.switches_away, 1);
    // No ancestor is known of the sleeping focus.
    let generation = verbatim_model::OutpostId(crate::entries::tests::GENERATION);
    assert_eq!(
        held.state.versions.known_ancestors(Some(generation)),
        Vec::new()
    );
}

/// Table 8, row "Another input desktop", column A: nothing more.
#[test]
fn a_second_switch_away_does_nothing_more() {
    let mut held = started();
    let _ = route(&mut held, 1_000, &switch(false));
    assert_eq!(
        route(&mut held, 1_100, &switch(false)),
        Vec::<String>::new()
    );
    assert_eq!(held.state.desktop.switches_away, 1);
}

/// Table 8, column B then G: 200 ms after the return the front
/// application's focus is read with none of the ancestors Core knows, and
/// spoken with its window's title, as after the sleeping focus.
#[test]
fn the_return_reads_the_front_focus_200_ms_later_and_says_its_title() {
    let mut held = started();
    let _ = route(&mut held, 1_000, &switch(false));
    assert_eq!(route(&mut held, 2_000, &switch(true)), Vec::<String>::new());
    assert_eq!(
        route(&mut held, 2_000 + RETURN_DELAY_US - 1, &QueueInput::Due),
        Vec::<String>::new()
    );
    assert_eq!(
        route(&mut held, 2_000 + RETURN_DELAY_US, &QueueInput::Due),
        ["send Outpost(Pid(40)) CurrentFocus known 0"]
    );
    assert_eq!(held.state.desktop.input, InputDesktop::User);
    assert_eq!(
        feed_at(
            &mut held,
            300_000,
            answer(OUTPOST, 4, Reply::Focus(Box::new(focus_read(None))))
        ),
        SPOKEN_FOLLOWED
    );
    assert!(matches!(held.state.versions.committed, Committed::Read(_)));
}

/// Table 8, row "Return pending", column A: the timer is cancelled and
/// the sleeping focus stays.
#[test]
fn a_switch_away_while_the_return_is_pending_cancels_the_read() {
    let mut held = started();
    let _ = route(&mut held, 1_000, &switch(false));
    let _ = route(&mut held, 2_000, &switch(true));
    assert_eq!(route(&mut held, 2_100, &switch(false)), ["cut"]);
    assert_eq!(held.state.desktop.input, InputDesktop::Other);
    assert_eq!(held.state.desktop.switches_away, 2);
    assert_eq!(
        route(&mut held, 2_000 + RETURN_DELAY_US, &QueueInput::Due),
        Vec::<String>::new()
    );
}

/// No re-take while another input desktop is up: the read after the
/// return covers it, and a re-take would replace the sleeping focus.
#[test]
fn an_outposts_end_on_another_desktop_takes_nothing_again() {
    let mut held = started();
    let _ = route(&mut held, 1_000, &switch(false));
    assert_eq!(
        route(
            &mut held,
            1_100,
            &QueueInput::ReaderEnded(crate::entries::tests::OUTPOST)
        ),
        Vec::<String>::new()
    );
    assert_eq!(
        held.state.versions.committed,
        Committed::Own { source: None }
    );
}

/// Table 8, columns C, D and H: the lock state is recorded, unknown
/// counting as locked.
#[test]
fn the_lock_state_counts_unknown_as_locked() {
    let mut held = ready();
    assert!(!held.state.session_locked());
    for (lock, locked) in [
        (SessionLock::Unknown, true),
        (SessionLock::Unlocked, false),
        (SessionLock::Locked, true),
    ] {
        assert_eq!(
            route(&mut held, 1_000, &QueueInput::Lock(lock)),
            Vec::<String>::new()
        );
        assert_eq!(held.state.session_locked(), locked);
    }
}
