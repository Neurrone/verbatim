//! Replays a flight-recorder dump (`books/dev/src/reference/crates/verbatim-core.md`, "The
//! dump format") through Core's reducer from the state it starts at, and
//! renders each input with the effects it produced on one compact line.
//!
//! `cargo xtask replay` prints a whole dump this way, and the end-to-end
//! suite prints the inputs around a failure the same way
//! (`books/dev/src/reference/crates/verbatim-replay.md`).

use std::fmt::Write as _;
use std::path::Path;

use serde_json::{Map, Value};
use verbatim_core::{DumpContents, replay};
use verbatim_model::{
    Arrival, Effect, Input, SpeechPriority, Theme, ThemeOptions, TraceId, UtteranceId,
};
use verbatim_speech::{ActiveTheme, ThemeHandle, ThemePresenter};

/// The file line a dump's first input is on: the header is line 1 and the
/// state the inputs start from is line 2.
pub const FIRST_INPUT_LINE: usize = 3;

/// The characters of a text kept before it is shortened.
const KEPT_CHARACTERS: usize = 80;

/// One recorded input, replayed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Replayed {
    /// The input's line in the dump file, counted from 1.
    pub line: usize,
    /// The input, with when it arrived.
    pub arrival: Arrival,
    /// How many effects the live session recorded for it.
    pub recorded: usize,
    /// The effects replaying it produced.
    pub effects: Vec<Effect>,
}

impl Replayed {
    /// Whether the replay produced a different number of effects from the
    /// live session: the code changed since the run, or the reducer is not
    /// deterministic.
    #[must_use]
    pub fn differs(&self) -> bool {
        self.effects.len() != self.recorded
    }
}

/// Replays every input of `contents` from its base state, in order.
#[must_use]
pub fn replay_dump(contents: &DumpContents) -> Vec<Replayed> {
    let arrivals: Vec<Arrival> = contents
        .inputs
        .iter()
        .map(|recorded| recorded.arrival.clone())
        .collect();
    let effects = replay(&contents.base, &arrivals);
    contents
        .inputs
        .iter()
        .zip(effects)
        .enumerate()
        .map(|(index, (recorded, effects))| Replayed {
            line: FIRST_INPUT_LINE + index,
            arrival: recorded.arrival.clone(),
            recorded: recorded.effect_count,
            effects,
        })
        .collect()
}

/// Whether `input` carries `trace` in a `trace_id` or `trace` field,
/// at any depth.
#[must_use]
pub fn carries_trace(input: &Input, trace: TraceId) -> bool {
    fn holds(value: &Value, trace: &Value) -> bool {
        match value {
            Value::Object(fields) => fields.iter().any(|(key, value)| {
                (matches!(key.as_str(), "trace_id" | "trace") && value == trace)
                    || holds(value, trace)
            }),
            Value::Array(items) => items.iter().any(|item| holds(item, trace)),
            _ => false,
        }
    }
    match (serde_json::to_value(input), serde_json::to_value(trace)) {
        (Ok(value), Ok(trace)) => holds(&value, &trace),
        _ => false,
    }
}

/// One input a failure's report shows ([`failure_inputs`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shown {
    /// When it arrived, in microseconds on the run's clock.
    pub time_us: u64,
    /// Its line in the dump file.
    pub line: usize,
    /// Its rendered line ([`Renderer::line`]), after `(failing trace)` for
    /// one carrying the failing step's trace.
    pub text: String,
}

/// The inputs of `replayed`, a whole dump's replay in order, that a
/// failure's report shows: each carrying `trace`, and each that arrived
/// from `from_us` to `to_us` on the run's shared clock.
#[must_use]
pub fn failure_inputs(
    replayed: &[Replayed],
    renderer: &Renderer,
    trace: Option<TraceId>,
    from_us: u64,
    to_us: u64,
) -> Vec<Shown> {
    replayed
        .iter()
        .filter_map(|replayed| {
            let time_us = replayed.arrival.at_us;
            let carries = trace.is_some_and(|trace| carries_trace(&replayed.arrival.input, trace));
            let within = (from_us..=to_us).contains(&time_us);
            (carries || within).then(|| Shown {
                time_us,
                line: replayed.line,
                text: if carries {
                    format!("(failing trace) {}", renderer.line(replayed))
                } else {
                    renderer.line(replayed)
                },
            })
        })
        .collect()
}

/// Renders inputs and effects as text, an utterance as the text Verbatim
/// reports as queued with the built-in default theme.
pub struct Renderer {
    presenter: ThemePresenter,
}

impl Renderer {
    /// A renderer whose theme finds its sound files in `sounds`, so a
    /// sound is rendered as `sound:` and its indication, as the end-to-end
    /// suite hears it; an indication whose file is not there is rendered
    /// in words.
    #[must_use]
    pub fn new(sounds: &Path) -> Self {
        let theme = ActiveTheme::new(
            Theme::builtin_default(),
            |file| Some(sounds.join(file)).filter(|path| path.is_file()),
            ThemeOptions::default(),
        );
        Self {
            presenter: ThemePresenter::new(ThemeHandle::new(theme)),
        }
    }

    /// One replayed input on one line: its line number, when it arrived,
    /// the input, and its effects, with the count the dump recorded when
    /// it differs.
    #[must_use]
    pub fn line(&self, replayed: &Replayed) -> String {
        let count = replayed.effects.len();
        let mut out = format!(
            "{}: at_us {}, {}. {count} effect{}",
            replayed.line,
            replayed.arrival.at_us,
            describe_input(&replayed.arrival.input),
            if count == 1 { "" } else { "s" }
        );
        if replayed.differs() {
            let _ = write!(out, " (differs: the dump recorded {})", replayed.recorded);
        }
        if count > 0 {
            let effects: Vec<String> = replayed
                .effects
                .iter()
                .map(|effect| self.effect(effect))
                .collect();
            out.push_str(": ");
            out.push_str(&effects.join("; "));
        }
        out
    }

    /// One effect: an utterance as its text, and anything else by its kind
    /// and fields.
    #[must_use]
    pub fn effect(&self, effect: &Effect) -> String {
        match effect {
            Effect::Speak(utterance, _) => {
                let text = self.presenter.flatten(utterance, UtteranceId(0)).text();
                match utterance.priority {
                    SpeechPriority::Queued => format!("Speak {}", quoted(&text)),
                    priority => format!("Speak {priority:?} {}", quoted(&text)),
                }
            }
            Effect::DropExpiredSpeech(_) => "DropExpiredSpeech".to_owned(),
            Effect::PlayEarcon(earcon) => format!("PlayEarcon {earcon:?}"),
            Effect::CopyToClipboard(text) => format!("CopyToClipboard {}", quoted(text)),
            Effect::ForKey(trace, effect) => {
                format!("ForKey trace {trace}: {}", self.effect(effect))
            }
            other => serde_json::to_value(other).map_or_else(
                |error| format!("{other:?} ({error})"),
                |value| variant(&value),
            ),
        }
    }
}

/// One input: its kind, then its fields, each as its name and value.
#[must_use]
pub fn describe_input(input: &Input) -> String {
    match serde_json::to_value(input) {
        Ok(value) => variant(&value),
        Err(error) => format!("{input:?} ({error})"),
    }
}

/// An enum's serialized value: its variant's name, then its fields.
fn variant(value: &Value) -> String {
    match value {
        Value::String(name) => name.clone(),
        Value::Object(map) if map.len() == 1 => {
            let (name, payload) = map.iter().next().expect("one entry");
            let fields = match payload {
                Value::Object(fields) => in_order(fields).into_iter().filter_map(field).collect(),
                other => value_text(other).into_iter().collect::<Vec<_>>(),
            };
            if fields.is_empty() {
                name.clone()
            } else {
                format!("{name} {}", fields.join(", "))
            }
        }
        other => value_text(other).unwrap_or_default(),
    }
}

/// An input's or effect's fields in the order they are printed: the
/// trace, the source, the times, then the rest by name.
fn in_order(fields: &Map<String, Value>) -> Vec<(&String, &Value)> {
    let rank = |key: &str| match key {
        "trace_id" => 0,
        "source" => 1,
        key if key.ends_with("_us") || key.ends_with("_ms") => 2,
        _ => 3,
    };
    let mut ordered: Vec<(&String, &Value)> = fields.iter().collect();
    ordered.sort_by_key(|(key, _)| rank(key));
    ordered
}

/// A field as its name and value, `trace_id` named `trace`; `None` for a
/// field with no value.
fn field((key, value): (&String, &Value)) -> Option<String> {
    let text = value_text(value)?;
    let key = if key == "trace_id" { "trace" } else { key };
    Some(format!("{key} {text}"))
}

/// A value's text: a string quoted and shortened, a node id as its
/// outpost and number, a node as its role and name, a list as its length,
/// and any other object as its fields in parentheses; `None` for a null or
/// an empty list or object.
fn value_text(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::Bool(flag) => Some(flag.to_string()),
        Value::Number(number) => Some(number.to_string()),
        Value::String(text) => Some(quoted(text)),
        Value::Array(items) if items.is_empty() => None,
        Value::Array(items) => Some(format!("{} items", items.len())),
        Value::Object(map) if map.is_empty() => None,
        Value::Object(map) => {
            if let (Some(outpost), Some(number), 2) =
                (map.get("outpost"), map.get("number"), map.len())
            {
                return Some(format!("{outpost}:{number}"));
            }
            if let (Some(Value::String(role)), Some(_)) = (map.get("role"), map.get("id")) {
                return Some(match map.get("name") {
                    Some(Value::String(name)) => format!("{role} {}", quoted(name)),
                    _ => role.clone(),
                });
            }
            if map.len() == 1 {
                return Some(variant(value));
            }
            let fields: Vec<String> = map.iter().filter_map(field).collect();
            Some(shortened(&format!("({})", fields.join(", "))))
        }
    }
}

/// `text` in quotation marks, shortened.
fn quoted(text: &str) -> String {
    let length = text.chars().count();
    if length <= KEPT_CHARACTERS {
        format!("{text:?}")
    } else {
        let kept: String = text.chars().take(KEPT_CHARACTERS).collect();
        format!("{:?} ({length} characters)", format!("{kept}..."))
    }
}

/// `text` cut to its first characters, with its length, when it is long.
fn shortened(text: &str) -> String {
    let length = text.chars().count();
    if length <= KEPT_CHARACTERS {
        text.to_owned()
    } else {
        let kept: String = text.chars().take(KEPT_CHARACTERS).collect();
        format!("{kept}... ({length} characters)")
    }
}

#[cfg(test)]
mod tests {
    use verbatim_model::TraceId;

    use super::*;

    #[test]
    fn a_long_text_is_shortened_with_its_length() {
        let arrival = typed(TraceId::from_raw(3), &"x".repeat(100), 5);
        assert_eq!(
            describe_input(&arrival.input),
            format!(
                "Queue trace 3, input Press pressed_at_us 5000, action Typed text \"{}...\" (100 characters), key_seq 1",
                "x".repeat(80)
            )
        );
    }

    /// A key typing `text` at `at_ms`, as the hook reports it to the
    /// event queue.
    fn typed(trace: TraceId, text: &str, at_ms: u64) -> Arrival {
        Arrival {
            at_us: at_ms * 1_000,
            input: Input::Queue {
                trace_id: trace,
                input: verbatim_model::request::QueueInput::Press(Box::new(
                    verbatim_model::request::Press {
                        key_seq: verbatim_model::KeySeq::new(1),
                        pressed_at_us: at_ms * 1_000,
                        action: verbatim_model::request::PressAction::Typed {
                            text: text.to_owned(),
                        },
                    },
                )),
            },
        }
    }

    /// The periodic tick, arriving at `at_ms`.
    fn tick(at_ms: u64) -> Arrival {
        Arrival {
            at_us: at_ms * 1_000,
            input: Input::Tick,
        }
    }

    /// `arrivals` replayed as a dump of them read back, each recorded with
    /// the effect count reducing it gives, from a new state.
    fn replayed(arrivals: Vec<Arrival>) -> Vec<Replayed> {
        let mut state = verbatim_core::SrState::new();
        let mut runtime = verbatim_core::Runtime::default();
        let recorded: Vec<verbatim_core::RecordedInput> = arrivals
            .into_iter()
            .map(|arrival| verbatim_core::RecordedInput {
                effect_count: verbatim_core::reduce(&mut state, &mut runtime, &arrival).len(),
                arrival,
            })
            .collect();
        let mut out = Vec::new();
        verbatim_core::dump::write_dump(
            &mut out,
            "0.1.0",
            verbatim_core::UNKNOWN_COMMIT,
            false,
            "2026-10-09T10:00:05Z",
            &verbatim_core::SrState::new(),
            &recorded,
        )
        .expect("writes the dump");
        let contents = verbatim_core::dump::read_dump(&mut out.as_slice()).expect("reads it back");
        replay_dump(&contents)
    }

    #[test]
    fn a_failure_shows_the_inputs_that_arrived_in_its_window_and_its_trace_s() {
        let trace = TraceId::from_raw(7);
        let other = TraceId::from_raw(8);
        let replayed = replayed(vec![
            typed(other, "a", 5_000),
            typed(trace, "b", 9_000),
            typed(other, "c", 11_000),
            tick(12_000),
            typed(other, "e", 14_000),
        ]);
        let renderer = Renderer::new(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../sounds")
                .as_path(),
        );
        let shown = failure_inputs(&replayed, &renderer, Some(trace), 10_000_000, 13_000_000);
        assert_eq!(
            shown,
            [
                Shown {
                    time_us: 9_000_000,
                    line: 4,
                    text: "(failing trace) 4: at_us 9000000, Queue trace 7, input Press pressed_at_us 9000000, action Typed text \"b\", key_seq 1. 1 effect: Queue".to_owned(),
                },
                Shown {
                    time_us: 11_000_000,
                    line: 5,
                    text: "5: at_us 11000000, Queue trace 8, input Press pressed_at_us 11000000, action Typed text \"c\", key_seq 1. 1 effect: Queue".to_owned(),
                },
                Shown {
                    time_us: 12_000_000,
                    line: 6,
                    text: "6: at_us 12000000, Tick. 0 effects".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn inputs_outside_the_window_without_the_failing_trace_are_not_shown() {
        let other = TraceId::from_raw(8);
        let renderer = Renderer::new(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../sounds")
                .as_path(),
        );
        let replayed_inputs = replayed(vec![tick(1_000), typed(other, "a", 20_000)]);
        let shown = failure_inputs(&replayed_inputs, &renderer, None, 10_000_000, 13_000_000);
        assert_eq!(shown, []);
    }
}
