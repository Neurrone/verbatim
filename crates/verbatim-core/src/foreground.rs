//! The foreground record: where the user's attention is, which window and
//! object are in front, the window the user's focus line was last in, and
//! the stamps that order focuses (`books/dev/src/explanation/design/ownership.md`;
//! `books/dev/src/reference/adr/0001-focus.md`, "The foreground and the window title").
//!
//! Every fact of the record has one writer, a variant of
//! [`ForegroundFact`], applied through [`Foreground::apply`]; the reducer
//! reads the record through its accessors only, so a new writer is a new
//! variant, seen where the record is. The focus listener's foreground
//! change is the only report that a window came to the foreground, and the
//! only writer of attention and the front ([`ForegroundFact::Changed`]).

use serde::{Deserialize, Serialize};
use verbatim_model::{ForegroundWindow, ObjectKey, OutpostId, Pid, WindowFacts, WindowHandle};

/// The application and top-level window of the most recent foreground
/// change the focus listener reported, standing in for the system's
/// foreground window, which is what NVDA classifies events against. Every
/// event other than a focus is classified against it; attention moves only
/// on a foreground change (D14).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Attention {
    pub(crate) source: Pid,
    pub(crate) window: Option<WindowFacts>,
}

impl Attention {
    /// Attention on the window `change` reports in front, and its process.
    fn of(change: &ForegroundWindow) -> Self {
        Self {
            source: change.process,
            window: Some(WindowFacts {
                top_level: change.window,
                root_owner: change.root_owner,
                topmost: false,
                under_active_window: None,
                in_foreground: true,
                foreground_window: change.window,
                class: String::new(),
                application: String::new(),
                alt_held: false,
                foreground_is_shell: false,
            }),
        }
    }
}

/// The window the focus listener last reported in front, other than one of
/// Verbatim's own, and when it observed it: the front, which a focus is
/// judged against ([`Foreground::front`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Front {
    /// The window.
    pub(crate) window: WindowHandle,
    /// The top of its owner chain.
    pub(crate) root_owner: WindowHandle,
    /// When the listener observed it, in microseconds on the run's shared
    /// clock.
    pub(crate) observed_at_us: u64,
}

impl Front {
    /// Whether a focus in the window `facts` describe is related to the
    /// front: its top-level window or its root owner is the front's window
    /// or the front's root owner, as an owned dialog, a popup and a focus in
    /// an owned window are.
    pub(crate) fn relates(&self, facts: &WindowFacts) -> bool {
        [facts.top_level, facts.root_owner]
            .iter()
            .any(|window| *window == self.window || *window == self.root_owner)
    }

    /// Whether the front was observed after `observed_at_us`, an unknown
    /// observation (0) being no earlier than it.
    pub(crate) fn observed_after(&self, observed_at_us: u64) -> bool {
        observed_at_us != 0 && observed_at_us < self.observed_at_us
    }
}

/// A focus observation by which a focus from another outpost generation is
/// ordered ([`Foreground::latest_focus`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LatestFocus {
    /// The outpost generation that reported it.
    pub(crate) outpost: OutpostId,
    /// When it was observed, in microseconds on the run's shared clock.
    pub(crate) observed_at_us: u64,
    /// Its top-level window, when known.
    pub(crate) top_level: Option<WindowHandle>,
}

impl LatestFocus {
    /// Whether this focus was observed after a focus observed at
    /// `observed_at_us`, a tie being no later.
    pub(crate) fn observed_after(&self, observed_at_us: u64) -> bool {
        observed_at_us < self.observed_at_us
    }
}

/// The applied stamp as a dump holds it: an object, so a dump written
/// before the stamp existed, which has no such key, is told from one whose
/// stamp is still unset.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct AppliedStamp {
    observed_at_us: Option<u64>,
}

/// The record as it is serialized: the same keys the state held before the
/// record existed, so earlier dumps still load.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Stored {
    attention: Option<Attention>,
    #[serde(default)]
    foreground: Option<ObjectKey>,
    #[serde(default)]
    foreground_changed_at: Option<u64>,
    #[serde(default)]
    foreground_window: Option<ForegroundWindow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    front: Option<Front>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    entered: Option<WindowHandle>,
    #[serde(default)]
    latest_focus: Option<LatestFocus>,
    /// Absent in a dump written before the stamps were split, where the
    /// one stamp stood for both.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    applied_focus: Option<AppliedStamp>,
}

/// One writer of the foreground record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ForegroundFact {
    /// The focus listener reported this window in the system's foreground:
    /// the keyboard hook's view of it, and, unless it is one of Verbatim's
    /// own windows, attention, its process and window, and the front.
    Changed(ForegroundWindow),
    /// A whole-window focus was taken, or was refused as the window
    /// holding Core's focus: its window is the foreground object.
    Object(ObjectKey),
    /// A whole-window focus was taken: the window the user's focus line is
    /// in, the focus's own [`Object`](Self::Object) giving the foreground
    /// object.
    Entered(WindowHandle),
    /// A focus that entered the window in front was taken: the window the
    /// user's focus line is in, and its chain's outermost entry, if any, as
    /// the foreground object.
    EnteredBy(WindowHandle, Option<ObjectKey>),
    /// A focus taken cut speech off for a new foreground window, observed
    /// at this time.
    LineCut(u64),
    /// A focus observation passed the stale and foreground tests: the
    /// ordering stamp, whether or not the focus is then taken.
    Observed(LatestFocus),
    /// A focus observed at this time was taken: the applied stamp.
    Applied(u64),
}

/// The foreground record ([module docs](self)).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "Stored", into = "Stored")]
pub(crate) struct Foreground {
    attention: Option<Attention>,
    object: Option<ObjectKey>,
    changed_at: Option<u64>,
    window: Option<ForegroundWindow>,
    front: Option<Front>,
    entered: Option<WindowHandle>,
    latest_focus: Option<LatestFocus>,
    applied_at: Option<u64>,
}

impl From<Stored> for Foreground {
    fn from(stored: Stored) -> Self {
        let applied_at = match stored.applied_focus {
            Some(applied) => applied.observed_at_us,
            None => stored.latest_focus.map(|latest| latest.observed_at_us),
        };
        Self {
            attention: stored.attention,
            object: stored.foreground,
            changed_at: stored.foreground_changed_at,
            window: stored.foreground_window,
            front: stored.front,
            entered: stored.entered,
            latest_focus: stored.latest_focus,
            applied_at,
        }
    }
}

impl From<Foreground> for Stored {
    fn from(record: Foreground) -> Self {
        Self {
            attention: record.attention,
            foreground: record.object,
            foreground_changed_at: record.changed_at,
            foreground_window: record.window,
            front: record.front,
            entered: record.entered,
            latest_focus: record.latest_focus,
            applied_focus: Some(AppliedStamp {
                observed_at_us: record.applied_at,
            }),
        }
    }
}

impl Foreground {
    /// Applies one fact, the record's only write.
    pub(crate) fn apply(&mut self, fact: ForegroundFact) {
        match fact {
            ForegroundFact::Changed(change) => {
                self.window = Some(change);
                // Verbatim's own window in front, its menu's hidden frame
                // or one of its dialogs, takes the user out of the entered
                // window, so the focus of the return says its title even
                // when it is the return's only report.
                if change.own {
                    self.entered = None;
                } else {
                    self.attention = Some(Attention::of(&change));
                    self.front = Some(Front {
                        window: change.window,
                        root_owner: change.root_owner,
                        observed_at_us: change.observed_at_us,
                    });
                }
            }
            ForegroundFact::Object(object) => self.object = Some(object),
            ForegroundFact::Entered(window) => self.entered = Some(window),
            ForegroundFact::EnteredBy(window, object) => {
                self.entered = Some(window);
                self.object = object;
            }
            ForegroundFact::LineCut(at) => self.changed_at = Some(at),
            ForegroundFact::Observed(latest) => self.latest_focus = Some(latest),
            ForegroundFact::Applied(at) => self.applied_at = Some(at),
        }
    }

    /// The attention record; `None` before any foreground change.
    pub(crate) fn attention(&self) -> Option<&Attention> {
        self.attention.as_ref()
    }

    /// The window most recently reported as the foreground, as the
    /// identity rule compares it, whose speech stays valid while it is in
    /// front (`FocusNow`).
    pub(crate) fn object(&self) -> Option<ObjectKey> {
        self.object
    }

    /// When the newest foreground change applied was observed, in
    /// microseconds on the run's shared clock: a focus with a known
    /// observation time that brought a new foreground window and cut
    /// speech off. Speech from elsewhere observed before it, but reaching
    /// the reducer after it, is dropped, as NVDA's foreground change
    /// cancels speech queued before it (`books/dev/src/reference/adr/0001-focus.md`, "One input to
    /// Core, in arrival order").
    pub(crate) fn changed_at(&self) -> Option<u64> {
        self.changed_at
    }

    /// The window the focus listener last reported in the system's
    /// foreground (`Input::Foreground`), one of Verbatim's own included:
    /// the keyboard hook's view.
    pub(crate) fn window(&self) -> Option<ForegroundWindow> {
        self.window
    }

    /// The front: the window the focus listener last reported in front,
    /// other than one of Verbatim's own, with when it was observed.
    pub(crate) fn front(&self) -> Option<Front> {
        self.front
    }

    /// The entered window: the top-level window the user's focus line was
    /// last in, set by a whole-window focus taken and a focus taken in the
    /// window in front, never by a foreground change, and cleared by a
    /// foreground change into one of Verbatim's own windows.
    pub(crate) fn entered(&self) -> Option<WindowHandle> {
        self.entered
    }

    /// The ordering stamp: the newest focus observation that passed the
    /// stale and foreground tests, taken, a duplicate, or a refused
    /// foreground report. Each outpost keeps its own events in order, but
    /// two outpost generations can deliver theirs out of the order they
    /// were observed in, where NVDA handles every event in one queue; a
    /// focus from another generation observed before this one is stale.
    pub(crate) fn latest_focus(&self) -> Option<LatestFocus> {
        self.latest_focus
    }

    /// The applied stamp: when the newest focus taken was observed, which
    /// a menu's or the switcher's end compares with.
    pub(crate) fn applied_at(&self) -> Option<u64> {
        self.applied_at
    }
}

#[cfg(test)]
mod tests {
    use verbatim_model::NodeId;

    use super::*;

    fn key(number: u64) -> ObjectKey {
        ObjectKey::of_id(NodeId::new(number))
    }

    fn change(window: u64, own: bool) -> ForegroundWindow {
        ForegroundWindow {
            window: WindowHandle(window),
            root_owner: WindowHandle(window + 1),
            process: Pid(3),
            observed_at_us: 40,
            own,
            thread: 0,
            console: false,
            integrity: None,
            own_integrity: None,
        }
    }

    /// The listener's change is the keyboard hook's view, and moves
    /// attention and the front, but not the foreground object or the
    /// entered window.
    #[test]
    fn a_foreground_change_moves_attention_and_the_front() {
        let mut record = Foreground::default();
        record.apply(ForegroundFact::Object(key(1)));
        record.apply(ForegroundFact::Changed(change(7, false)));
        assert_eq!(record.window(), Some(change(7, false)));
        assert_eq!(
            record.attention().map(|attention| (
                attention.source,
                attention
                    .window
                    .as_ref()
                    .map(|window| (window.top_level, window.root_owner))
            )),
            Some((Pid(3), Some((WindowHandle(7), WindowHandle(8)))))
        );
        assert_eq!(
            record.front(),
            Some(Front {
                window: WindowHandle(7),
                root_owner: WindowHandle(8),
                observed_at_us: 40,
            })
        );
        assert_eq!((record.object(), record.entered()), (Some(key(1)), None));
    }

    /// A change to one of Verbatim's own windows is the keyboard hook's
    /// alone: attention and the front stay.
    #[test]
    fn a_change_to_verbatims_own_window_is_the_hooks_alone() {
        let mut record = Foreground::default();
        record.apply(ForegroundFact::Changed(change(7, false)));
        record.apply(ForegroundFact::Changed(change(9, true)));
        assert_eq!(record.window(), Some(change(9, true)));
        assert_eq!(
            record.front().map(|front| front.window),
            Some(WindowHandle(7))
        );
        assert_eq!(
            record
                .attention()
                .and_then(|attention| attention.window.as_ref())
                .map(|window| window.top_level),
            Some(WindowHandle(7))
        );
    }

    /// A focus entering a window sets the entered window and its chain's
    /// outermost entry as the foreground object; a whole-window focus sets
    /// the entered window alone.
    #[test]
    fn an_entered_window_is_kept_with_its_object() {
        let mut record = Foreground::default();
        record.apply(ForegroundFact::Object(key(1)));
        record.apply(ForegroundFact::Entered(WindowHandle(5)));
        assert_eq!(
            (record.entered(), record.object()),
            (Some(WindowHandle(5)), Some(key(1)))
        );
        record.apply(ForegroundFact::EnteredBy(WindowHandle(6), Some(key(2))));
        assert_eq!(
            (record.entered(), record.object()),
            (Some(WindowHandle(6)), Some(key(2)))
        );
    }

    #[test]
    fn the_foreground_object_is_its_reports_window() {
        let mut record = Foreground::default();
        record.apply(ForegroundFact::Object(key(5)));
        assert_eq!(record.object(), Some(key(5)));
    }

    #[test]
    fn a_line_cut_stamps_the_foreground_change() {
        let mut record = Foreground::default();
        record.apply(ForegroundFact::LineCut(1_200));
        assert_eq!(record.changed_at(), Some(1_200));
    }

    #[test]
    fn an_observation_moves_the_ordering_stamp_alone() {
        let latest = LatestFocus {
            outpost: OutpostId(1),
            observed_at_us: 2_000,
            top_level: None,
        };
        let mut record = Foreground::default();
        record.apply(ForegroundFact::Applied(1_000));
        record.apply(ForegroundFact::Observed(latest));
        assert_eq!(
            (record.latest_focus(), record.applied_at()),
            (Some(latest), Some(1_000))
        );
    }

    #[test]
    fn a_focus_taken_moves_the_applied_stamp_alone() {
        let mut record = Foreground::default();
        record.apply(ForegroundFact::Applied(3_000));
        assert_eq!(
            (record.latest_focus(), record.applied_at()),
            (None, Some(3_000))
        );
    }

    #[test]
    fn a_dump_from_before_the_split_takes_its_one_stamp_for_both() {
        let old = r#"{"attention":null,"latest_focus":{"outpost":1,"observed_at_us":5,"top_level":null}}"#;
        let record: Foreground = serde_json::from_str(old).expect("an old dump loads");
        assert_eq!(record.applied_at(), Some(5));
        let again: Foreground =
            serde_json::from_str(&serde_json::to_string(&record).expect("serializes"))
                .expect("loads");
        assert_eq!(again, record);
    }

    #[test]
    fn an_unset_applied_stamp_survives_a_dump() {
        let mut record = Foreground::default();
        record.apply(ForegroundFact::Observed(LatestFocus {
            outpost: OutpostId(1),
            observed_at_us: 9,
            top_level: None,
        }));
        let again: Foreground =
            serde_json::from_str(&serde_json::to_string(&record).expect("serializes"))
                .expect("loads");
        assert_eq!(again.applied_at(), None);
    }
}
