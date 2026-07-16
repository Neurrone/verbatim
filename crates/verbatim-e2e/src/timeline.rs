//! A shared, timestamped record of everything a live scenario did and
//! heard: gestures and keystrokes injected through [`crate::Scenario`],
//! interleaved in time order with utterances collected by
//! [`crate::speech::SpeechCollector`].
//!
//! A speech-assertion failure without this is a transcript with a hole in
//! it: it shows what Verbatim said, but not what provoked it. The
//! [`Timeline`] fixes that by having both sides write to the same log —
//! `Scenario`'s inject methods and `SpeechCollector`'s frame loop each hold
//! a clone of the same handle — so a panic message can print, in order, the
//! last command sent before speech stopped.

use std::fmt::Write as _;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

/// One thing that happened during a scenario: an injected gesture, an
/// injected key combination, or a heard utterance.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TimelineKind {
    /// A gesture identifier routed through `Request::SendGesture` (for
    /// example `kb:verbatim+v`).
    Gesture(String),
    /// A key combination sent through `Request::SendKeys`, in the order
    /// they were sent to that single request.
    Keys(Vec<String>),
    /// An utterance's full rendered text, as heard on the speech
    /// connection at queue time — the frame assertions match against.
    Utterance(String),
    /// The audio-start follow-up for an utterance already recorded as an
    /// [`Utterance`](TimelineKind::Utterance): the same text again, arriving
    /// whenever the synthesizer actually began playing it. Recorded so the
    /// rendered timeline shows real audio timing, but never part of
    /// [`Timeline::utterances`] — under a loaded real synthesizer these
    /// arrive seconds late and interleaved with fresh queue-time frames, and
    /// matching them as utterances is exactly the off-by-one that broke the
    /// M1 walk on a cold guest.
    AudioStarted(String),
}

/// One [`TimelineKind`] paired with the [`Instant`] it was recorded at.
#[derive(Debug, Clone)]
struct TimelineEntry {
    at: Instant,
    kind: TimelineKind,
}

/// A cheaply cloneable handle onto one scenario's shared action-and-speech
/// log.
///
/// Every clone reads and writes the same underlying entries: [`Scenario`]
/// keeps one to record gestures and keys as it injects them, and hands a
/// clone to [`SpeechCollector`](crate::speech::SpeechCollector) to record
/// utterances as they arrive, so both sides can push concurrently without
/// either owning the other.
#[derive(Debug, Clone)]
pub struct Timeline {
    entries: Arc<Mutex<Vec<TimelineEntry>>>,
}

impl Timeline {
    /// A fresh, empty timeline.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Records a gesture identifier at the current instant.
    pub fn push_gesture(&self, identifier: &str) {
        self.push(TimelineKind::Gesture(identifier.to_owned()));
    }

    /// Records a key combination list at the current instant, in the order
    /// they were sent as one `SendKeys` request.
    pub fn push_keys(&self, keys: &[&str]) {
        self.push(TimelineKind::Keys(
            keys.iter().map(|key| (*key).to_owned()).collect(),
        ));
    }

    /// Records an utterance's full text at the current instant.
    pub fn push_utterance(&self, text: &str) {
        self.push(TimelineKind::Utterance(text.to_owned()));
    }

    /// Records an utterance's audio-start follow-up at the current instant.
    /// Rendered as its own `audio` line; excluded from [`utterances`]
    /// (see [`TimelineKind::AudioStarted`]).
    ///
    /// [`utterances`]: Self::utterances
    pub fn push_audio_started(&self, text: &str) {
        self.push(TimelineKind::AudioStarted(text.to_owned()));
    }

    fn push(&self, kind: TimelineKind) {
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        entries.push(TimelineEntry {
            at: Instant::now(),
            kind,
        });
    }

    /// Every utterance recorded so far, in arrival order — the substring of
    /// the timeline that [`SpeechCollector`](crate::speech::SpeechCollector)
    /// matches its matchers against.
    #[must_use]
    pub fn utterances(&self) -> Vec<String> {
        let entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        entries
            .iter()
            .filter_map(|entry| match &entry.kind {
                TimelineKind::Utterance(text) => Some(text.clone()),
                TimelineKind::Gesture(_)
                | TimelineKind::Keys(_)
                | TimelineKind::AudioStarted(_) => None,
            })
            .collect()
    }

    /// Renders every entry recorded so far, one per line and in time order,
    /// each tagged by kind and prefixed with elapsed time since the first
    /// entry (for example `+634ms speech "Settings... menu item"`) — the
    /// debugging artifact every `SpeechCollector::expect_*` panic message
    /// prints, so a human reading a failure sees the injected commands and
    /// the heard speech interleaved exactly as they happened.
    #[must_use]
    pub fn render(&self) -> String {
        let entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        if entries.is_empty() {
            return "(nothing recorded)".to_owned();
        }
        let start = entries[0].at;
        let mut out = String::new();
        for entry in entries.iter() {
            let elapsed = entry.at.saturating_duration_since(start).as_millis();
            let line = match &entry.kind {
                TimelineKind::Gesture(identifier) => {
                    format!("+{elapsed}ms gesture {identifier}")
                }
                TimelineKind::Keys(keys) => {
                    format!("+{elapsed}ms keys [{}]", keys.join(", "))
                }
                TimelineKind::Utterance(text) => {
                    format!("+{elapsed}ms speech {text:?}")
                }
                TimelineKind::AudioStarted(text) => {
                    format!("+{elapsed}ms audio {text:?}")
                }
            };
            let _ = writeln!(out, "{line}");
        }
        // Drop the trailing newline writeln! leaves so callers embedding
        // this in a larger panic message control their own spacing.
        out.pop();
        out
    }
}

impl Default for Timeline {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_is_empty_placeholder_when_nothing_recorded() {
        let timeline = Timeline::new();
        assert_eq!(timeline.render(), "(nothing recorded)");
    }

    #[test]
    fn render_interleaves_actions_and_speech_in_time_order_with_elapsed_prefixes() {
        let timeline = Timeline::new();
        timeline.push_gesture("kb:verbatim+v");
        timeline.push_keys(&["downarrow"]);
        timeline.push_utterance("Settings... menu item");

        let rendered = timeline.render();
        let lines: Vec<&str> = rendered.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with("+0ms gesture kb:verbatim+v"));
        assert!(lines[1].contains("keys [downarrow]"));
        assert!(lines[2].contains(r#"speech "Settings... menu item""#));

        // Time order: the gesture line precedes the keys line precedes the
        // speech line in the rendered text itself, not just by index.
        let gesture_pos = rendered.find("gesture").expect("gesture line present");
        let keys_pos = rendered.find("keys").expect("keys line present");
        let speech_pos = rendered.find("speech").expect("speech line present");
        assert!(gesture_pos < keys_pos);
        assert!(keys_pos < speech_pos);
    }

    #[test]
    fn utterances_filters_out_gestures_and_keys() {
        let timeline = Timeline::new();
        timeline.push_gesture("kb:verbatim+v");
        timeline.push_utterance("one");
        timeline.push_keys(&["tab"]);
        timeline.push_utterance("two");

        assert_eq!(
            timeline.utterances(),
            vec!["one".to_owned(), "two".to_owned()]
        );
    }

    #[test]
    fn audio_start_followups_render_tagged_and_never_count_as_utterances() {
        let timeline = Timeline::new();
        timeline.push_utterance("one");
        timeline.push_audio_started("one");

        assert_eq!(timeline.utterances(), vec!["one".to_owned()]);
        let rendered = timeline.render();
        assert!(rendered.contains(r#"speech "one""#));
        assert!(rendered.contains(r#"audio "one""#));
    }

    #[test]
    fn clones_share_the_same_underlying_log() {
        let timeline = Timeline::new();
        let clone = timeline.clone();
        clone.push_utterance("from the clone");
        assert_eq!(timeline.utterances(), vec!["from the clone".to_owned()]);
    }
}
