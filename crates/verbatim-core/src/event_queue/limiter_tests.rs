//! Table 1's waiting rules (`docs/design/readers-tell-core-asks-build1.md`):
//! the merge, MSAA runs, NVDA's limiter on the waiting MSAA entries, the
//! menu slot and the foreground change's ending of waiting entries, with a
//! logic whose entries say their names, behind a front entry that holds
//! the queue with one request.

use verbatim_model::event_kind::EventKind;
use verbatim_model::property::{PropertyId, PropertyValues};
use verbatim_model::request::{
    Answer, Outcome as WireOutcome, Reply, Request as Wire, RequestNumber,
};
use verbatim_model::{Effect, NodeId, Pid, WindowHandle};

use super::driver::{Core, Cx};
use super::inbox::{Input, Timed};
use super::limiter::{MsaaClass, MsaaKey, Place};
use super::request::Request;
use super::{Foreground, Function, Instance, Logic, OTHER_DUE_US, Reader, RequestWindow};

const APP: Pid = Pid(1);
const OUTPOST: Instance = Instance {
    reader: Reader::Outpost(APP),
    generation: 1,
};
const IN_FRONT: Foreground = Foreground {
    window: WindowHandle(10),
    root_owner: WindowHandle(10),
    process: APP,
    thread: 0,
    by_class: None,
    shell: false,
    ..Foreground::BLANK
};
/// The window whose entries end when the foreground changes.
const ELSEWHERE: u64 = 99;

/// The tests' logic.
struct Limited;

/// An MSAA tell.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Msaa {
    name: &'static str,
    kind: EventKind,
    window: u64,
    object: i32,
    thread: u32,
    accepted: bool,
    /// When the reader received it; the tests that do not name a time
    /// give 0, so all their tells fall in one pump window.
    at_us: u64,
}

#[derive(Clone, Debug, PartialEq)]
enum Entry {
    /// Holds the queue with one request until it is answered.
    Hold,
    /// An MSAA tell.
    Msaa(Msaa),
    /// Any other entry, merged with one of the same key.
    Other {
        name: &'static str,
        key: u32,
        focus_kind: bool,
        ends_run: bool,
    },
    /// A run of MSAA candidates, oldest first.
    Run(Vec<Msaa>),
    /// The foreground marker.
    Marker,
}

#[derive(Debug)]
struct HoldRead;

impl Request for HoldRead {
    type Reply = PropertyValues;

    fn reader(&self) -> Reader {
        OUTPOST.reader
    }

    fn window(&self) -> RequestWindow {
        RequestWindow {
            handle: WindowHandle(11),
            top_level: WindowHandle(10),
            root_owner: WindowHandle(10),
            process: APP,
            thread: 3,
            by_class: Some(verbatim_model::Backend::Msaa),
        }
    }

    fn wire(&self) -> verbatim_model::request::Question {
        verbatim_model::request::Question::Legacy(Wire::Properties {
            node: NodeId::new(1),
            set: vec![PropertyId::Name],
        })
    }

    fn decode(&self, reply: Reply) -> Option<PropertyValues> {
        match reply {
            Reply::Properties(values) => Some(values),
            _ => None,
        }
    }
}

fn class_of(kind: EventKind) -> MsaaClass {
    match kind {
        EventKind::MsaaFocus => MsaaClass::Focus,
        EventKind::Foreground => MsaaClass::Foreground,
        EventKind::MenuStart
        | EventKind::MenuEnd
        | EventKind::PopupStart
        | EventKind::PopupEnd
        | EventKind::SwitchEnd => MsaaClass::Menu,
        EventKind::Show => MsaaClass::Show,
        EventKind::Hide => MsaaClass::Hide,
        _ => MsaaClass::Other,
    }
}

fn key_of(tell: &Msaa) -> MsaaKey {
    MsaaKey {
        kind: tell.kind,
        window: WindowHandle(tell.window),
        object: tell.object,
        child: 0,
        thread: tell.thread,
    }
}

/// What an entry's function emits: its words, carried by a model effect
/// the queue passes on unread.
fn emitted(words: String) -> Effect {
    Effect::CopyToClipboard(words)
}

impl Logic for Limited {
    type Entry = Entry;
    /// The readers' last focus's object, exempt from the thread cap.
    type Facts = Option<i32>;

    fn run(mut cx: Cx<Self>, entry: Entry) -> Function {
        Box::pin(async move {
            match entry {
                Entry::Hold => {
                    let due_us = cx.due_in(OTHER_DUE_US);
                    let _ = cx.ask(HoldRead, due_us).await;
                    cx.emit(emitted("held".to_owned()));
                }
                Entry::Msaa(tell) => cx.emit(emitted(tell.name.to_owned())),
                Entry::Other { name, .. } => cx.emit(emitted(name.to_owned())),
                Entry::Run(candidates) => {
                    let names: Vec<&str> = candidates.iter().rev().map(|tell| tell.name).collect();
                    cx.emit(emitted(format!("run {}", names.join(" "))));
                }
                Entry::Marker => cx.emit(emitted("marker".to_owned())),
            }
        })
    }

    fn ends(
        _: &Option<i32>,
        _: (&Entry, &crate::event_queue::Live),
        _: crate::event_queue::Cause<'_, Entry>,
        _: (
            &crate::focus::backend::BackendCache,
            u64,
            Option<crate::event_queue::Foreground>,
        ),
    ) -> Option<verbatim_model::request::CancelReason> {
        None
    }

    fn recovered(_: &mut Option<i32>, _: &RequestWindow) -> Option<Entry> {
        None
    }

    fn place(_: &Option<i32>, entry: &Entry) -> Place {
        match entry {
            Entry::Msaa(tell) => Place::Msaa {
                key: key_of(tell),
                class: class_of(tell.kind),
                accepted: tell.accepted,
                at_us: tell.at_us,
            },
            Entry::Other {
                focus_kind,
                ends_run,
                ..
            } => Place::Other {
                merges: true,
                focus_kind: *focus_kind,
                ends_run: *ends_run,
            },
            Entry::Hold | Entry::Run(_) | Entry::Marker => Place::Other {
                merges: false,
                focus_kind: false,
                ends_run: true,
            },
        }
    }

    fn merges(_: &Option<i32>, waiting: &Entry, arriving: &Entry) -> bool {
        matches!(
            (waiting, arriving),
            (Entry::Other { key: one, .. }, Entry::Other { key: other, .. }) if one == other
        )
    }

    fn exempt_from_cap(facts: &Option<i32>, key: &MsaaKey) -> bool {
        *facts == Some(key.object)
    }

    fn run_entry(candidates: Vec<Entry>) -> Entry {
        Entry::Run(
            candidates
                .into_iter()
                .filter_map(|entry| match entry {
                    Entry::Msaa(tell) => Some(tell),
                    _ => None,
                })
                .collect(),
        )
    }

    fn ends_at_foreground(_: &Option<i32>, entry: &Entry, _: &Foreground) -> bool {
        matches!(entry, Entry::Msaa(tell) if tell.window == ELSEWHERE)
    }

    fn foreground_marker(_: &Foreground) -> Option<Entry> {
        Some(Entry::Marker)
    }
}

/// An MSAA tell of `kind` on `object`, from thread 1, accepted.
fn tell(name: &'static str, kind: EventKind, object: i32) -> Entry {
    Entry::Msaa(Msaa {
        name,
        kind,
        window: 5,
        object,
        thread: 1,
        accepted: true,
        at_us: 0,
    })
}

fn focus(name: &'static str, object: i32) -> Entry {
    tell(name, EventKind::MsaaFocus, object)
}

fn other(name: &'static str, key: u32) -> Entry {
    Entry::Other {
        name,
        key,
        focus_kind: false,
        ends_run: false,
    }
}

/// What the queue says once its front's request is answered, after `entries`
/// arrived behind it, with `exempt` the readers' last focus's object.
fn said_after(exempt: Option<i32>, entries: Vec<Entry>) -> Vec<String> {
    let mut core = Core::<Limited>::default();
    core.shared.borrow_mut().state.facts = exempt;
    let mut at_us = 0;
    let mut feed = |core: &mut Core<Limited>, input| {
        at_us += 1_000;
        core.feed(Timed { at_us, input })
    };
    core.shared
        .borrow_mut()
        .state
        .queue
        .start_for_tests(OUTPOST);
    let _ = feed(&mut core, Input::ReaderReady(OUTPOST));
    let _ = feed(&mut core, Input::Foreground(IN_FRONT));
    let _ = feed(&mut core, Input::Entry(Entry::Hold));
    for entry in entries {
        assert_eq!(feed(&mut core, Input::Entry(entry)), []);
    }
    let answer = Input::Answer {
        from: OUTPOST,
        answer: Answer {
            number: RequestNumber(1),
            outcome: WireOutcome::Result(Reply::Properties(PropertyValues::default())),
            probes: Vec::new(),
            detail: String::new(),
        },
    };
    feed(&mut core, answer)
        .into_iter()
        .filter_map(|effect| match effect {
            Effect::CopyToClipboard(words) => Some(words),
            _ => None,
        })
        .collect()
}

/// The words `names` said in order.
fn words(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

// The merge.

#[test]
fn a_waiting_entry_of_the_same_key_is_replaced_the_newest_keeping_its_place() {
    assert_eq!(
        said_after(
            None,
            vec![other("a", 1), other("b", 2), other("a again", 1)]
        ),
        words(&["held", "b", "a again"])
    );
}

#[test]
fn no_merge_reaches_past_a_focus_kind_entry() {
    let focus_kind = Entry::Other {
        name: "focus",
        key: 9,
        focus_kind: true,
        ends_run: false,
    };
    assert_eq!(
        said_after(None, vec![other("a", 1), focus_kind, other("a again", 1)]),
        words(&["held", "a", "focus", "a again"])
    );
    assert_eq!(
        said_after(
            None,
            vec![other("a", 1), focus("f", 1), other("a again", 1)]
        ),
        words(&["held", "a", "run f", "a again"])
    );
}

#[test]
fn the_asked_entry_is_not_merged() {
    let mut core = Core::<Limited>::default();
    let feed = |core: &mut Core<Limited>, at_us, input| core.feed(Timed { at_us, input });
    core.shared
        .borrow_mut()
        .state
        .queue
        .start_for_tests(OUTPOST);
    let _ = feed(&mut core, 0, Input::ReaderReady(OUTPOST));
    let _ = feed(&mut core, 0, Input::Entry(Entry::Hold));
    let _ = feed(&mut core, 1, Input::Entry(Entry::Hold));
    assert_eq!(core.shared.borrow().state.queue.waiting.len(), 1);
}

#[test]
fn an_msaa_tell_of_the_same_object_kind_and_thread_is_merged_and_of_another_thread_kept() {
    let name = |name, thread| {
        Entry::Msaa(Msaa {
            name,
            kind: EventKind::NameChange,
            window: 5,
            object: 3,
            thread,
            accepted: true,
            at_us: 0,
        })
    };
    assert_eq!(
        said_after(None, vec![name("one", 1), name("two", 2), name("three", 1)]),
        words(&["held", "two", "three"])
    );
}

// Runs.

#[test]
fn focus_and_foreground_tells_join_one_run_tried_newest_first_across_other_entries() {
    let foreground = tell("fg", EventKind::Foreground, 0);
    assert_eq!(
        said_after(
            None,
            vec![focus("f1", 1), other("u", 1), foreground, focus("f2", 2)]
        ),
        words(&["held", "run f2 fg f1", "u"])
    );
}

#[test]
fn an_accepted_msaa_tell_ends_a_run_and_a_refused_one_does_not() {
    let mut refused = tell("refused change", EventKind::NameChange, 7);
    if let Entry::Msaa(tell) = &mut refused {
        tell.accepted = false;
    }
    assert_eq!(
        said_after(
            None,
            vec![
                focus("f1", 1),
                tell("change", EventKind::NameChange, 7),
                focus("f2", 2)
            ]
        ),
        words(&["held", "run f1", "change", "run f2"])
    );
    assert_eq!(
        said_after(None, vec![focus("f1", 1), refused, focus("f2", 2)]),
        words(&["held", "run f2 f1", "refused change"])
    );
}

#[test]
fn a_repeated_candidate_moves_to_the_newer_place_in_its_run() {
    assert_eq!(
        said_after(
            None,
            vec![focus("f1", 1), focus("f2", 2), focus("f1 again", 1)]
        ),
        words(&["held", "run f1 again f2"])
    );
}

#[test]
fn a_foreground_tell_removes_a_waiting_focus_of_its_object_from_its_thread() {
    let other_thread = Entry::Msaa(Msaa {
        name: "f other thread",
        kind: EventKind::MsaaFocus,
        window: 5,
        object: 0,
        thread: 2,
        accepted: true,
        at_us: 0,
    });
    assert_eq!(
        said_after(
            None,
            vec![
                focus("f", 0),
                other_thread,
                tell("fg", EventKind::Foreground, 0)
            ]
        ),
        words(&["held", "run fg f other thread"])
    );
}

#[test]
fn the_newest_four_candidates_are_kept() {
    assert_eq!(
        said_after(
            None,
            (1..=6)
                .map(|object| focus(
                    ["f1", "f2", "f3", "f4", "f5", "f6"][object - 1],
                    i32::try_from(object).unwrap()
                ))
                .collect()
        ),
        words(&["held", "run f6 f5 f4 f3"])
    );
}

#[test]
fn candidates_arriving_more_than_a_pump_window_apart_are_not_capped_together() {
    // settings_dialog_keys: the desktop item's focus, then over 39 ms five
    // focus events from Verbatim's closing dialog while a read is out.
    // NVDA pumps at least every 10 ms, so they never meet in one flush of
    // its limiter, and the desktop item is kept.
    let at = |name, object, at_us| {
        Entry::Msaa(Msaa {
            name,
            kind: EventKind::MsaaFocus,
            window: 5,
            object,
            thread: 1,
            accepted: true,
            at_us,
        })
    };
    assert_eq!(
        said_after(
            None,
            vec![
                at("desktop item", 1, 0),
                at("d1", 2, 1_000),
                at("d2", 3, 10_000),
                at("d3", 4, 20_000),
                at("d4", 5, 30_000),
                at("d5", 6, 39_000),
            ]
        ),
        words(&["held", "run d5 d4 d3 d2 d1 desktop item"])
    );
}

#[test]
fn candidates_within_one_pump_window_are_capped_at_four() {
    let at = |name, object, at_us| {
        Entry::Msaa(Msaa {
            name,
            kind: EventKind::MsaaFocus,
            window: 5,
            object,
            thread: 1,
            accepted: true,
            at_us,
        })
    };
    assert_eq!(
        said_after(
            None,
            vec![
                at("f1", 1, 0),
                at("f2", 2, 30_000),
                at("f3", 3, 32_000),
                at("f4", 4, 35_000),
                at("f5", 5, 38_000),
                at("f6", 6, 40_000),
            ]
        ),
        words(&["held", "run f6 f5 f4 f3 f1"])
    );
}

#[test]
fn a_foreground_tell_beyond_the_four_is_kept_while_its_thread_keeps_it() {
    let mut entries = vec![tell("fg", EventKind::Foreground, 0)];
    entries.extend((1..=4).map(|object| {
        focus(
            ["f1", "f2", "f3", "f4"][object - 1],
            i32::try_from(object).unwrap(),
        )
    }));
    assert_eq!(
        said_after(None, entries),
        words(&["held", "run f4 f3 f2 f1 fg"])
    );
}

#[test]
fn a_foreground_tell_beyond_the_four_and_its_threads_ten_is_dropped() {
    let mut entries = vec![tell("fg", EventKind::Foreground, 0), focus("f1", 1)];
    let names = ["c1", "c2", "c3", "c4", "c5", "c6", "c7", "c8", "c9", "c10"];
    entries.extend(
        names
            .iter()
            .zip(20..)
            .map(|(name, object)| tell(name, EventKind::NameChange, object)),
    );
    entries.extend([focus("f2", 2), focus("f3", 3), focus("f4", 4)]);
    let mut expected = vec!["held", "run f1"];
    expected.extend(names);
    expected.push("run f4 f3 f2");
    assert_eq!(said_after(None, entries), words(&expected));
}

#[test]
fn the_newest_ten_other_msaa_entries_of_a_thread_are_kept_the_last_focus_exempt_but_counted() {
    let names = [
        "c1", "c2", "c3", "c4", "c5", "c6", "c7", "c8", "c9", "c10", "c11", "c12",
    ];
    let entries = || {
        names
            .iter()
            .zip(1..)
            .map(|(name, object)| tell(name, EventKind::NameChange, object))
            .collect::<Vec<_>>()
    };
    let mut kept = vec!["held"];
    kept.extend(&names[2..]);
    assert_eq!(said_after(None, entries()), words(&kept));
    let mut exempt = vec!["held", "c1"];
    exempt.extend(&names[2..]);
    assert_eq!(said_after(Some(1), entries()), words(&exempt));
}

// The menu slot.

#[test]
fn only_the_newest_menu_tell_waits() {
    assert_eq!(
        said_after(
            None,
            vec![
                tell("start", EventKind::MenuStart, 1),
                other("u", 1),
                tell("popup end", EventKind::PopupEnd, 2),
                tell("switch end", EventKind::SwitchEnd, 3)
            ]
        ),
        words(&["held", "u", "switch end"])
    );
}

#[test]
fn the_menu_slot_at_the_front_is_moved_once_behind_the_runs_behind_it() {
    assert_eq!(
        said_after(
            None,
            vec![
                tell("popup start", EventKind::PopupStart, 1),
                focus("f1", 1),
                other("u", 1),
                focus("f2", 2),
                other("v", 2)
            ]
        ),
        words(&["held", "run f2 f1", "popup start", "u", "v"])
    );
}

// Shows and hides.

#[test]
fn a_show_removes_a_waiting_hide_of_its_object_and_the_reverse() {
    assert_eq!(
        said_after(
            None,
            vec![
                tell("hide", EventKind::Hide, 1),
                tell("other hide", EventKind::Hide, 2),
                tell("show", EventKind::Show, 1)
            ]
        ),
        words(&["held", "other hide", "show"])
    );
    assert_eq!(
        said_after(
            None,
            vec![
                tell("show", EventKind::Show, 1),
                tell("hide", EventKind::Hide, 1)
            ]
        ),
        words(&["held", "hide"])
    );
}

// The foreground change.

#[test]
fn a_foreground_change_ends_waiting_entries_outside_it_and_a_marker_joins_the_back() {
    let elsewhere = |name, kind| {
        Entry::Msaa(Msaa {
            name,
            kind,
            window: ELSEWHERE,
            object: 1,
            thread: 1,
            accepted: true,
            at_us: 0,
        })
    };
    let mut core = Core::<Limited>::default();
    let feed = |core: &mut Core<Limited>, at_us, input| core.feed(Timed { at_us, input });
    core.shared
        .borrow_mut()
        .state
        .queue
        .start_for_tests(OUTPOST);
    let _ = feed(&mut core, 0, Input::ReaderReady(OUTPOST));
    let _ = feed(&mut core, 0, Input::Foreground(IN_FRONT));
    let _ = feed(&mut core, 1, Input::Entry(Entry::Hold));
    for entry in [
        elsewhere("gone", EventKind::NameChange),
        focus("f", 2),
        elsewhere("gone focus", EventKind::MsaaFocus),
        other("u", 1),
    ] {
        let _ = feed(&mut core, 2, Input::Entry(entry));
    }
    assert_eq!(feed(&mut core, 3, Input::Foreground(IN_FRONT)), []);
    let said: Vec<Effect> = feed(
        &mut core,
        4,
        Input::Answer {
            from: OUTPOST,
            answer: Answer {
                number: RequestNumber(1),
                outcome: WireOutcome::Result(Reply::Properties(PropertyValues::default())),
                probes: Vec::new(),
                detail: String::new(),
            },
        },
    );
    assert_eq!(
        said,
        ["held", "run f", "u", "marker"]
            .into_iter()
            .map(|words| emitted(words.to_owned()))
            .collect::<Vec<_>>()
    );
}
