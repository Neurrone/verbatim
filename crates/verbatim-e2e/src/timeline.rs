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

use std::ops::Range;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use verbatim_model::{TraceId, UtteranceEnding};

/// One thing that happened during a scenario: an injected gesture, an
/// injected key combination, typed text, or a heard utterance.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TimelineKind {
    /// A gesture identifier routed through `Request::SendGesture` (for
    /// example `kb:verbatim+v`).
    Gesture(String),
    /// A key combination sent through `Request::SendKeys`, in the order
    /// they were sent to that single request.
    Keys(Vec<String>),
    /// Text typed as real key presses through the agent's `TypeText`.
    Text(String),
    /// An utterance's full rendered text, as heard on the speech
    /// connection at queue time — the frame assertions match against —
    /// and the trace ID of the event or key behind it, when it has one (a
    /// sound played at once has none).
    Utterance(String, Option<TraceId>),
    /// An utterance already recorded as an
    /// [`Utterance`](TimelineKind::Utterance) began to play: its text again,
    /// for reading. Recorded so the rendered timeline shows real audio
    /// timing, and never part of [`Timeline::utterances`].
    AudioStarted(String),
    /// An utterance ended: its text and how it ended.
    Ended(String, UtteranceEnding),
}

/// One [`TimelineKind`] paired with the [`Instant`] it was recorded at,
/// and the same moment as milliseconds since the Unix epoch, the clock
/// Verbatim stamps its speech frames with.
#[derive(Debug, Clone)]
struct TimelineEntry {
    at: Instant,
    unix_ms: u64,
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

    /// Records text typed through the agent's `TypeText` at the current
    /// instant.
    pub fn push_text(&self, text: &str) {
        self.push(TimelineKind::Text(text.to_owned()));
    }

    /// Records an utterance's full text, and the trace ID behind it, at
    /// the current instant.
    pub fn push_utterance(&self, text: &str, trace: Option<TraceId>) {
        self.push(TimelineKind::Utterance(text.to_owned(), trace));
    }

    /// Records an utterance's audio-start follow-up at the current instant.
    /// Rendered as its own `audio` line; excluded from [`utterances`]
    /// (see [`TimelineKind::AudioStarted`]).
    ///
    /// [`utterances`]: Self::utterances
    pub fn push_audio_started(&self, text: &str) {
        self.push(TimelineKind::AudioStarted(text.to_owned()));
    }

    /// Records that the utterance with this text ended as `ending` says.
    pub fn push_ended(&self, text: &str, ending: &UtteranceEnding) {
        self.push(TimelineKind::Ended(text.to_owned(), ending.clone()));
    }

    fn push(&self, kind: TimelineKind) {
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        entries.push(TimelineEntry {
            at: Instant::now(),
            unix_ms: unix_ms(),
            kind,
        });
    }

    /// The last gesture, keys, or typed text injected at or before
    /// `unix_ms` (milliseconds since the Unix epoch), described as the
    /// rendered timeline describes it: the step an utterance observed at
    /// that time answers, for the latency report.
    #[must_use]
    pub fn step_at(&self, unix_ms: u64) -> Option<String> {
        let entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        entries
            .iter()
            .rev()
            .filter(|entry| entry.unix_ms <= unix_ms)
            .find_map(|entry| match &entry.kind {
                TimelineKind::Gesture(identifier) => Some(format!("gesture {identifier}")),
                TimelineKind::Keys(keys) => Some(format!("keys {}", keys.join(" "))),
                TimelineKind::Text(text) => Some(format!("text {text:?}")),
                _ => None,
            })
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
                TimelineKind::Utterance(text, _) => Some(text.clone()),
                TimelineKind::Gesture(_)
                | TimelineKind::Keys(_)
                | TimelineKind::Text(_)
                | TimelineKind::AudioStarted(_)
                | TimelineKind::Ended(..) => None,
            })
            .collect()
    }

    /// Renders every entry recorded so far, one per line and in time order,
    /// each tagged by kind and prefixed with elapsed time since the first
    /// entry (for example `+634ms speech "Settings... menu item"`): the
    /// run's `timeline.txt`, so a human reading a failure sees the injected
    /// commands and the heard speech interleaved exactly as they happened.
    #[must_use]
    pub fn render(&self) -> String {
        let entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(first) = entries.first() else {
            return "(nothing recorded)".to_owned();
        };
        let start = first.at;
        let lines: Vec<String> = entries.iter().map(|entry| entry.line(start)).collect();
        lines.join("\n")
    }

    /// The entries of the step under way: from the last gesture, keys, or
    /// typed text injected (or from the start, before any) to the newest
    /// entry. Taken when an assertion fails, it is the step that failed.
    #[must_use]
    pub fn current_step(&self) -> Range<usize> {
        let entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        let start = entries
            .iter()
            .rposition(|entry| entry.kind.is_input())
            .unwrap_or(0);
        start..entries.len()
    }

    /// The trace ID of the newest utterance in `step` that has one.
    #[must_use]
    pub fn last_trace_in(&self, step: Range<usize>) -> Option<TraceId> {
        let entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        entries
            .get(step)
            .unwrap_or_default()
            .iter()
            .rev()
            .find_map(|entry| match entry.kind {
                TimelineKind::Utterance(_, trace) => trace,
                _ => None,
            })
    }

    /// The timeline up to the end of `failed`, a step
    /// [`current_step`](Self::current_step) gave, for a failure message:
    /// every earlier step on one line (when it started, what was injected,
    /// or `start` before any input, and every utterance queued in it), then
    /// the failed step's entries in full, as [`render`](Self::render)
    /// gives them.
    #[must_use]
    pub fn render_failed_step(&self, failed: Range<usize>) -> String {
        let entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(first) = entries.first() else {
            return "(nothing recorded)".to_owned();
        };
        let start = first.at;
        let end = failed.end.min(entries.len());
        let failed_start = failed.start.min(end);
        let mut lines = Vec::new();
        let mut step_start = 0;
        for index in 1..=failed_start {
            if index == failed_start || entries[index].kind.is_input() {
                if index > step_start {
                    lines.push(summarize_step(&entries[step_start..index], start));
                }
                step_start = index;
            }
        }
        lines.extend(
            entries[failed_start..end]
                .iter()
                .map(|entry| entry.line(start)),
        );
        lines.join("\n")
    }
}

impl TimelineKind {
    /// Whether this is something the scenario injected, which starts a
    /// step.
    fn is_input(&self) -> bool {
        matches!(self, Self::Gesture(_) | Self::Keys(_) | Self::Text(_))
    }

    /// What was injected, as the rendered timeline describes it; `None`
    /// for speech.
    fn input(&self) -> Option<String> {
        match self {
            Self::Gesture(identifier) => Some(format!("gesture {identifier}")),
            Self::Keys(keys) => Some(format!("keys [{}]", keys.join(", "))),
            Self::Text(text) => Some(format!("text {text:?}")),
            Self::Utterance(..) | Self::AudioStarted(_) | Self::Ended(..) => None,
        }
    }
}

impl TimelineEntry {
    /// This entry as one line of the rendered timeline, its time counted
    /// from `start`.
    fn line(&self, start: Instant) -> String {
        let elapsed = self.at.saturating_duration_since(start).as_millis();
        let what = match &self.kind {
            TimelineKind::Utterance(text, _) => format!("speech {text:?}"),
            TimelineKind::AudioStarted(text) => format!("audio {text:?}"),
            TimelineKind::Ended(text, UtteranceEnding::Completed) => {
                format!("completed {text:?}")
            }
            TimelineKind::Ended(text, UtteranceEnding::Cancelled) => {
                format!("cancelled {text:?}")
            }
            TimelineKind::Ended(text, UtteranceEnding::Failed(reason)) => {
                format!("failed {text:?}: {reason}")
            }
            input => input.input().unwrap_or_default(),
        };
        format!("+{elapsed}ms {what}")
    }
}

/// One step of the timeline on one line: when it started, what was
/// injected (`start` for what came before any input), and every utterance
/// queued in it.
fn summarize_step(step: &[TimelineEntry], start: Instant) -> String {
    let elapsed = step.first().map_or(0, |entry| {
        entry.at.saturating_duration_since(start).as_millis()
    });
    let input = step
        .first()
        .and_then(|entry| entry.kind.input())
        .unwrap_or_else(|| "start".to_owned());
    let said: Vec<String> = step
        .iter()
        .filter_map(|entry| match &entry.kind {
            TimelineKind::Utterance(text, _) => Some(format!("{text:?}")),
            _ => None,
        })
        .collect();
    if said.is_empty() {
        format!("+{elapsed}ms {input}, nothing said")
    } else {
        format!("+{elapsed}ms {input}, said {}", said.join(", "))
    }
}

/// Milliseconds since the Unix epoch, now.
pub(crate) fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
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
        timeline.push_utterance("Settings... menu item", None);

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
        timeline.push_utterance("one", None);
        timeline.push_keys(&["tab"]);
        timeline.push_utterance("two", None);

        assert_eq!(
            timeline.utterances(),
            vec!["one".to_owned(), "two".to_owned()]
        );
    }

    #[test]
    fn audio_start_followups_render_tagged_and_never_count_as_utterances() {
        let timeline = Timeline::new();
        timeline.push_utterance("one", None);
        timeline.push_audio_started("one");

        assert_eq!(timeline.utterances(), vec!["one".to_owned()]);
        let rendered = timeline.render();
        assert!(rendered.contains(r#"speech "one""#));
        assert!(rendered.contains(r#"audio "one""#));
    }

    #[test]
    fn typed_text_renders_tagged_and_never_counts_as_an_utterance() {
        let timeline = Timeline::new();
        timeline.push_text("echo hello");
        timeline.push_utterance("hello", None);

        assert_eq!(timeline.utterances(), vec!["hello".to_owned()]);
        assert!(timeline.render().contains(r#"text "echo hello""#));
    }

    /// Each line with its elapsed time removed, which varies from run to
    /// run.
    fn without_times(rendered: &str) -> Vec<String> {
        rendered
            .lines()
            .map(|line| {
                line.split_once("ms ")
                    .map_or(line, |(_, rest)| rest)
                    .to_owned()
            })
            .collect()
    }

    #[test]
    fn a_failed_step_is_shown_in_full_after_one_line_per_earlier_step() {
        let timeline = Timeline::new();
        timeline.push_utterance("Verbatim started", None);
        timeline.push_keys(&["downarrow"]);
        timeline.push_utterance("> banana", Some(TraceId::mint()));
        timeline.push_audio_started("> banana");
        timeline.push_ended("> banana", &UtteranceEnding::Completed);
        timeline.push_gesture("kb:verbatim+v");
        timeline.push_keys(&["uparrow"]);
        let failed_trace = TraceId::mint();
        timeline.push_utterance("> apple", Some(failed_trace));
        timeline.push_ended("> apple", &UtteranceEnding::Cancelled);
        let failed = timeline.current_step();
        // Pushed after the failure: not part of the failure's timeline.
        timeline.push_keys(&["enter"]);

        assert_eq!(timeline.last_trace_in(failed.clone()), Some(failed_trace));
        assert_eq!(
            without_times(&timeline.render_failed_step(failed)),
            vec![
                r#"start, said "Verbatim started""#,
                r#"keys [downarrow], said "> banana""#,
                "gesture kb:verbatim+v, nothing said",
                "keys [uparrow]",
                r#"speech "> apple""#,
                r#"cancelled "> apple""#,
            ]
        );
    }

    #[test]
    fn clones_share_the_same_underlying_log() {
        let timeline = Timeline::new();
        let clone = timeline.clone();
        clone.push_utterance("from the clone", None);
        assert_eq!(timeline.utterances(), vec!["from the clone".to_owned()]);
    }
}
