//! The foreground record: where the user's attention is, which window and
//! object are in front, and the stamps that order focuses
//! (`docs/design/ownership-one-judge.md`, "One foreground record").
//!
//! Every fact of the record has one writer, a variant of
//! [`ForegroundFact`], applied through [`Foreground::apply`]; the reducer
//! reads the record through its accessors only, so a new writer is a new
//! variant, seen where the record is.

use serde::{Deserialize, Serialize};
use verbatim_model::{ForegroundWindow, ObjectKey, OutpostId, Pid, WindowFacts, WindowHandle};

/// The application and top-level window of the most recent foreground
/// change, standing in for the system's foreground window, which is what
/// NVDA classifies events against. Every event other than a foreground
/// change is classified against it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Attention {
    pub(crate) source: Pid,
    pub(crate) window: Option<WindowFacts>,
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
    #[serde(default)]
    latest_focus: Option<LatestFocus>,
    /// Absent in a dump written before the stamps were split, where the
    /// one stamp stood for both.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    applied_focus: Option<AppliedStamp>,
}

/// One writer of the foreground record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ForegroundFact {
    /// A foreground report reached the reducer: attention moves to its
    /// application and window, whether or not the report is taken.
    Reported(Attention),
    /// A focus in the system's foreground window unrelated to the
    /// attention window was taken: attention follows it, and the
    /// foreground object moves to its top-level window, `object`.
    Unreported(Attention, Option<ObjectKey>),
    /// A foreground report was taken: its window is the foreground object.
    Object(ObjectKey),
    /// A focus taken cut speech off for a new foreground window, observed
    /// at this time.
    LineCut(u64),
    /// The focus listener reported this window in the system's foreground.
    Window(ForegroundWindow),
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
            ForegroundFact::Reported(attention) => self.attention = Some(attention),
            ForegroundFact::Unreported(attention, object) => {
                self.attention = Some(attention);
                self.object = object;
            }
            ForegroundFact::Object(object) => self.object = Some(object),
            ForegroundFact::LineCut(at) => self.changed_at = Some(at),
            ForegroundFact::Window(window) => self.window = Some(window),
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
    /// cancels speech queued before it (`docs/design/focus-pipeline.md`,
    /// section 7).
    pub(crate) fn changed_at(&self) -> Option<u64> {
        self.changed_at
    }

    /// The window the focus listener last reported in the system's
    /// foreground (`Input::ForegroundWindow`).
    pub(crate) fn window(&self) -> Option<ForegroundWindow> {
        self.window
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

    fn attention(pid: u32) -> Attention {
        Attention {
            source: Pid(pid),
            window: None,
        }
    }

    fn key(number: u64) -> ObjectKey {
        ObjectKey::of_id(NodeId::new(number))
    }

    #[test]
    fn a_reported_foreground_moves_attention_alone() {
        let mut record = Foreground::default();
        record.apply(ForegroundFact::Object(key(1)));
        record.apply(ForegroundFact::Reported(attention(2)));
        assert_eq!(
            (record.attention(), record.object()),
            (Some(&attention(2)), Some(key(1)))
        );
    }

    #[test]
    fn an_unreported_foreground_moves_attention_and_the_object() {
        let mut record = Foreground::default();
        record.apply(ForegroundFact::Object(key(1)));
        record.apply(ForegroundFact::Unreported(attention(3), Some(key(4))));
        assert_eq!(
            (record.attention(), record.object()),
            (Some(&attention(3)), Some(key(4)))
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
    fn the_listeners_window_is_kept() {
        let window = ForegroundWindow {
            window: WindowHandle(7),
            thread: 0,
            console: false,
            integrity: None,
            own_integrity: None,
        };
        let mut record = Foreground::default();
        record.apply(ForegroundFact::Window(window));
        assert_eq!(record.window(), Some(window));
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
