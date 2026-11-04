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
use verbatim_model::{Effect, Input, SpeechPriority, Theme, ThemeOptions, TraceId, UtteranceId};
use verbatim_speech::{ActiveTheme, Presenter, ThemeHandle, ThemePresenter};

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
    /// The input.
    pub input: Input,
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
    let inputs: Vec<Input> = contents
        .inputs
        .iter()
        .map(|recorded| recorded.input.clone())
        .collect();
    let effects = replay(&contents.base, &inputs);
    contents
        .inputs
        .iter()
        .zip(effects)
        .enumerate()
        .map(|(index, (recorded, effects))| Replayed {
            line: FIRST_INPUT_LINE + index,
            input: recorded.input.clone(),
            recorded: recorded.effect_count,
            effects,
        })
        .collect()
}

/// When `input` happened, in microseconds on the run's clock, from the
/// time it carries (`observed_at_us`, `ended_at_us`, `pressed_at_ms` or
/// `at_ms`); `None` for an input that carries none, such as an outpost's
/// answer, or whose time is 0, which no stamp taken on the run's clock
/// reads.
#[must_use]
pub fn input_time_us(input: &Input) -> Option<u64> {
    let value = serde_json::to_value(input).ok()?;
    let Value::Object(variant) = value else {
        return None;
    };
    let (_, Value::Object(fields)) = variant.into_iter().next()? else {
        return None;
    };
    fields
        .iter()
        .find_map(|(key, value)| {
            let number = value.as_u64()?;
            if key.ends_with("_us") {
                Some(number)
            } else if key.ends_with("_ms") {
                number.checked_mul(1_000)
            } else {
                None
            }
        })
        .filter(|&time| time != 0)
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
    /// The time it is ordered by, in microseconds on the run's clock: its
    /// own, or for an untimed input the time of the nearest timed input
    /// before it, or else after it; `None` when no input in the dump is
    /// timed.
    pub time_us: Option<u64>,
    /// Its line in the dump file.
    pub line: usize,
    /// Its rendered line ([`Renderer::line`]), after its marks in
    /// parentheses: `failing trace` for one carrying the failing step's
    /// trace, `untimed` for one with no time of its own.
    pub text: String,
}

/// The inputs of `replayed`, a whole dump's replay in order, that a
/// failure's report shows: each carrying `trace`, and each received from
/// `from_us` to `to_us` on the run's shared clock. An untimed input
/// ([`input_time_us`]) is judged by the time [`Shown::time_us`] gives it,
/// and is shown when no input of the dump is timed, since then nothing
/// places it outside the window.
#[must_use]
pub fn failure_inputs(
    replayed: &[Replayed],
    renderer: &Renderer,
    trace: Option<TraceId>,
    from_us: u64,
    to_us: u64,
) -> Vec<Shown> {
    let own: Vec<Option<u64>> = replayed
        .iter()
        .map(|replayed| input_time_us(&replayed.input))
        .collect();
    let mut placed = own.clone();
    let mut before = None;
    for time in &mut placed {
        *time = time.or(before);
        before = *time;
    }
    let mut after = None;
    for (time, own) in placed.iter_mut().zip(&own).rev() {
        after = own.or(after);
        *time = time.or(after);
    }
    replayed
        .iter()
        .zip(own.iter().zip(placed))
        .filter_map(|(replayed, (own, time))| {
            let carries = trace.is_some_and(|trace| carries_trace(&replayed.input, trace));
            let within = time.is_none_or(|time| (from_us..=to_us).contains(&time));
            let marks: Vec<&str> = [
                carries.then_some("failing trace"),
                own.is_none().then_some("untimed"),
            ]
            .into_iter()
            .flatten()
            .collect();
            (carries || within).then(|| Shown {
                time_us: time,
                line: replayed.line,
                text: if marks.is_empty() {
                    renderer.line(replayed)
                } else {
                    format!("({}) {}", marks.join(", "), renderer.line(replayed))
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

    /// One replayed input on one line: its line number, the input, and
    /// its effects, with the count the dump recorded when it differs.
    #[must_use]
    pub fn line(&self, replayed: &Replayed) -> String {
        let count = replayed.effects.len();
        let mut out = format!(
            "{}: {}. {count} effect{}",
            replayed.line,
            describe_input(&replayed.input),
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

    /// One effect: an utterance as its text, a query by what it asks of
    /// which node, and anything else by its kind and fields.
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
            Effect::Fetch(query) => format!(
                "Fetch {:?} of {}, query {}",
                query.kind,
                serde_json::to_value(query.node_id)
                    .ok()
                    .and_then(|id| value_text(&id))
                    .unwrap_or_default(),
                query.query_id.0
            ),
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

/// One input: its kind, and for an event the event's kind and node, then
/// its fields, each as its name and value.
#[must_use]
pub fn describe_input(input: &Input) -> String {
    let value = match serde_json::to_value(input) {
        Ok(value) => value,
        Err(error) => return format!("{input:?} ({error})"),
    };
    if let Input::Event { .. } = input
        && let Some(Value::Object(fields)) = value.get("Event")
    {
        return describe_event(fields);
    }
    variant(&value)
}

/// An event: `Event`, the event's kind, its node, the event's own fields,
/// then the input's.
fn describe_event(fields: &Map<String, Value>) -> String {
    let mut parts = vec!["Event".to_owned()];
    let mut rest = Vec::new();
    if let Some(Value::Object(event)) = fields.get("event")
        && let Some((kind, Value::Object(event_fields))) = event.iter().next()
    {
        parts[0].push(' ');
        parts[0].push_str(kind);
        for key in ["node", "node_id", "controller", "bar"] {
            if let Some(node) = event_fields.get(key).and_then(value_text) {
                parts.push(format!("{key} {node}"));
            }
        }
        rest.extend(
            event_fields
                .iter()
                .filter(|(key, _)| {
                    !matches!(key.as_str(), "node" | "node_id" | "controller" | "bar")
                })
                .filter_map(field),
        );
    }
    parts.extend(rest);
    parts.extend(
        in_order(fields)
            .into_iter()
            .filter(|(key, _)| key.as_str() != "event")
            .filter_map(field),
    );
    parts.join(", ")
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
    use verbatim_model::{
        Backend, NodeDetails, NodeId, NodeSnapshot, NormalizedEvent, Pid, Role, StateSet, TraceId,
    };

    use super::*;

    #[test]
    fn an_event_names_its_kind_and_node_first() {
        let input = Input::Event {
            trace_id: TraceId::from_raw(7),
            observed_at_us: 12,
            source: Pid(700),
            backend: Backend::Uia,
            window: None,
            event: NormalizedEvent::FocusChanged {
                whole_window: false,
                node: NodeSnapshot {
                    id: NodeId::new(1),
                    backend: Backend::Uia,
                    role: Role::Button,
                    name: Some("OK".to_owned()),
                    value: None,
                    states: StateSet::new(),
                    details: NodeDetails::default(),
                },
                ancestors: Vec::new(),
                selected_child: None,
                text: None,
                earlier: None,
                group: None,
            },
        };
        assert_eq!(
            describe_input(&input),
            "Event FocusChanged, node Button \"OK\", whole_window false, trace 7, source 700, observed_at_us 12, backend \"Uia\""
        );
        assert_eq!(input_time_us(&input), Some(12));
    }

    #[test]
    fn a_long_text_is_shortened_with_its_length() {
        let input = Input::CharacterTyped {
            trace_id: TraceId::from_raw(3),
            key_seq: verbatim_model::KeySeq::none_yet(),
            text: "x".repeat(100),
            at_ms: 5,
        };
        assert_eq!(
            describe_input(&input),
            format!(
                "CharacterTyped trace 3, at_ms 5, key_seq 0, text \"{}...\" (100 characters)",
                "x".repeat(80)
            )
        );
        assert_eq!(input_time_us(&input), Some(5_000));
    }

    fn typed(trace: TraceId, text: &str, at_ms: u64) -> Input {
        Input::CharacterTyped {
            trace_id: trace,
            key_seq: verbatim_model::KeySeq::none_yet(),
            text: text.to_owned(),
            at_ms,
        }
    }

    /// `inputs` replayed as a dump of them read back, each recorded with
    /// the effect count reducing it gives, from a new state.
    fn replayed(inputs: Vec<Input>) -> Vec<Replayed> {
        let mut state = verbatim_core::SrState::new();
        let recorded: Vec<verbatim_core::RecordedInput> = inputs
            .into_iter()
            .map(|input| verbatim_core::RecordedInput {
                effect_count: verbatim_core::reduce(&mut state, &input).len(),
                input,
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
    fn a_failure_shows_the_inputs_in_its_window_and_its_trace_s_with_the_untimed_placed_by_their_neighbours()
     {
        let trace = TraceId::from_raw(7);
        let other = TraceId::from_raw(8);
        let replayed = replayed(vec![
            typed(other, "a", 5_000),
            typed(trace, "b", 9_000),
            typed(other, "c", 11_000),
            Input::Tick,
            // A zero stamp is no time: placed, like the tick, after "c".
            typed(other, "d", 0),
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
                    time_us: Some(9_000_000),
                    line: 4,
                    text: "(failing trace) 4: CharacterTyped trace 7, at_ms 9000, key_seq 0, text \"b\". 0 effects".to_owned(),
                },
                Shown {
                    time_us: Some(11_000_000),
                    line: 5,
                    text: "5: CharacterTyped trace 8, at_ms 11000, key_seq 0, text \"c\". 0 effects".to_owned(),
                },
                Shown {
                    time_us: Some(11_000_000),
                    line: 6,
                    text: "(untimed) 6: Tick. 0 effects".to_owned(),
                },
                Shown {
                    time_us: Some(11_000_000),
                    line: 7,
                    text: "(untimed) 7: CharacterTyped trace 8, at_ms 0, key_seq 0, text \"d\". 0 effects".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn untimed_inputs_before_any_timed_one_take_the_next_one_s_time_and_with_none_timed_are_shown()
    {
        let other = TraceId::from_raw(8);
        let renderer = Renderer::new(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../sounds")
                .as_path(),
        );
        let replayed_inputs = replayed(vec![Input::Tick, typed(other, "a", 20_000)]);
        let shown = failure_inputs(&replayed_inputs, &renderer, None, 10_000_000, 13_000_000);
        assert_eq!(shown, []);

        let shown = failure_inputs(
            &replayed(vec![Input::Tick]),
            &renderer,
            None,
            10_000_000,
            13_000_000,
        );
        assert_eq!(
            shown,
            [Shown {
                time_us: None,
                line: 3,
                text: "(untimed) 3: Tick. 0 effects".to_owned(),
            }]
        );
    }
}
