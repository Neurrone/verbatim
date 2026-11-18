//! Scripted-input tests for milestone M4's text handling in the reducer:
//! caret keys and the selection, typed character and word echo, the
//! review cursor over text, say-all, and formatting spoken as it changes,
//! each driven through Core's event queue by the public reducer: a field
//! committed through the outpost's focus read, keys and commands pressed,
//! caret tells told, and each text request Core sends answered as the test
//! scripts it.

mod desk;
mod terminal_support;

use desk::{App, Desk, ONE};
use verbatim_model::event_kind::EventKind;
use verbatim_model::request::{
    FocusCaret, FocusCaretRead, FocusRead, Instance, Outcome, Press, PressAction, QueueEffect,
    QueueInput, Reply, Request as Wire,
};
use verbatim_model::stroke::{HeldStroke, Stroke};
use verbatim_model::tell::{AcceptanceFacts, Event, EventObject, Payload, Relation, TellWindow};
use verbatim_model::{
    Backend, CaretKey, CaretMotion, CaretReport, CaretSelection, EditSelection, Effect, Input,
    KeySeq, Message, NodeDetails, NodeId, NodeSnapshot, Phrase, ReaderSettings, ReviewCommand,
    Role, SegmentContent, Selection, SelectionChange, SelectionDifference, SelectionReadReply,
    SelectionText, SpeechMark, SpeechPriority, Started, StateSet, TextAnchor, TextChunk, TextOp,
    TextPoint, TextPosition, TextProvider, TextReply, TextUnit, TraceId, UtteranceSegment, Watched,
    WatchedRead,
};

/// The edit field of most tests, node 5 of [`ONE`].
const FIELD: u64 = 5;

/// Node `number` of [`ONE`].
fn id(number: u64) -> NodeId {
    ONE.node(number)
}

/// Node `number`, named "Body", of `role` with `states`.
fn node(number: u64, role: Role, states: StateSet) -> NodeSnapshot {
    NodeSnapshot {
        id: id(number),
        backend: Backend::Uia,
        role,
        name: Some("Body".to_owned()),
        value: None,
        states,
        details: NodeDetails::default(),
    }
}

/// A line chunk starting at `anchor`, with the point of interest at
/// `offset`.
fn line(text: &str, anchor: u64, offset: u32) -> TextChunk {
    TextChunk {
        unit: TextUnit::Line,
        text: text.to_owned(),
        start: TextAnchor(anchor),
        offset,
        languages: Vec::new(),
        first: false,
        last: false,
        truncated: false,
        formats: Vec::new(),
    }
}

/// The caret on `line`, with `selection`.
fn caret_on(line: TextChunk, selection: Option<CaretSelection>) -> CaretReport {
    CaretReport {
        line,
        selection,
        read_at_us: 0,
    }
}

fn caret_selection(selection: Selection) -> CaretSelection {
    CaretSelection { range: selection }
}

/// A focus's caret read of `caret`, with the text selected, and baseline
/// `baseline` minted.
fn caret_read(caret: CaretReport, selected_text: Option<&str>, baseline: u64) -> FocusCaret {
    FocusCaret::Read(Box::new(FocusCaretRead {
        caret,
        selected_text: selected_text.map(str::to_owned),
        baseline: TextAnchor(baseline),
    }))
}

/// One effect as these tests compare it: every effect becomes exactly one
/// of these, so comparing the whole list checks every effect a step had.
#[derive(Clone, Debug, PartialEq)]
enum Heard {
    /// An utterance, not say-all's: its priority and segments.
    Say(SpeechPriority, Vec<UtteranceSegment>),
    /// A say-all utterance: its segments, its marks among them.
    SayAll(Vec<UtteranceSegment>),
    /// A text request: what it asks.
    Asked(TextOp),
    /// Another request, by its kind.
    Sent(&'static str),
    /// Held strokes sent to the application: how many.
    Strokes(usize),
    /// A request withdrawn.
    Cancelled,
    /// Speech stopped.
    Stop,
    /// Expired speech dropped.
    Expire,
    /// Text copied.
    Copied(String),
    /// Settings to save.
    Settings(ReaderSettings),
    /// The display kept on, or let sleep.
    Display(bool),
}

/// A queued utterance of `segments`.
fn queued(segments: Vec<UtteranceSegment>) -> Heard {
    Heard::Say(SpeechPriority::Queued, segments)
}

/// One effect as [`Heard`], `None` for the queue's own business and the
/// control plane's stream.
fn heard_of(effect: Effect) -> Option<Heard> {
    match effect {
        Effect::ForKey(_, effect) => heard_of(*effect),
        Effect::Speak(utterance, _) if utterance.say_all => Some(Heard::SayAll(utterance.segments)),
        Effect::Speak(utterance, _) => Some(Heard::Say(utterance.priority, utterance.segments)),
        Effect::StopSpeech => Some(Heard::Stop),
        Effect::DropExpiredSpeech(_) => Some(Heard::Expire),
        Effect::CopyToClipboard(text) => Some(Heard::Copied(text)),
        Effect::SettingsChanged(settings) => Some(Heard::Settings(settings)),
        Effect::KeepDisplayOn(on) => Some(Heard::Display(on)),
        Effect::Queue(QueueEffect::Send { ask, .. }) => Some(match ask.request {
            Wire::Text { op, .. } => Heard::Asked(op),
            other => Heard::Sent(desk::kind(&other)),
        }),
        Effect::Queue(QueueEffect::SendStrokes(sent)) => Some(Heard::Strokes(sent.len())),
        Effect::Queue(QueueEffect::Cancel { .. }) => Some(Heard::Cancelled),
        Effect::Observed(_) | Effect::Queue(_) => None,
        other => panic!("an effect these tests do not expect: {other:?}"),
    }
}

/// A desk with the field's application, each text request Core sends
/// kept until the test answers it, and a read of the review cursor's own
/// line, unchanged, answered as the state holds it.
struct Text {
    desk: Desk,
    /// The requests not yet answered, but `Follow`: number, reader and
    /// request.
    out: Vec<(u64, Instance, Wire)>,
    /// The last held stroke's sequence.
    strokes: u32,
    /// The caret as the application has it, which a key's starting read
    /// finds.
    caret: CaretReport,
    /// The selection baseline the outpost minted last.
    baseline: u64,
    /// How the committed field's text is read: through UI Automation, or
    /// through a classic edit control's messages.
    provider: TextProvider,
    /// The key number each utterance carried, since the last press or
    /// answer.
    stamps: Vec<Option<KeySeq>>,
}

impl Text {
    fn new() -> Self {
        Self {
            desk: Desk::new(),
            out: Vec::new(),
            strokes: 0,
            caret: caret_on(line("", 100, 0), None),
            baseline: 1,
            provider: TextProvider::Uia,
            stamps: Vec::new(),
        }
    }

    /// Focuses `snapshot` of [`ONE`] with `caret`: every utterance its
    /// announcement spoke, each by its segments.
    fn focus(&mut self, snapshot: &NodeSnapshot, caret: FocusCaret) -> Vec<Vec<UtteranceSegment>> {
        if let FocusCaret::Read(read) = &caret {
            self.caret = read.caret.clone();
        }
        let read = FocusRead {
            caret,
            ..desk::focus_read_of(ONE, snapshot, &[])
        };
        self.focus_read(ONE, read)
    }

    /// Focuses the outpost's `read` in `app`: every utterance spoken.
    fn focus_read(&mut self, app: App, read: FocusRead) -> Vec<Vec<UtteranceSegment>> {
        self.out.clear();
        self.desk
            .focus_read(app, read)
            .into_iter()
            .filter_map(|heard| match heard {
                desk::Heard::Say(_, segments) => Some(segments),
                _ => None,
            })
            .collect()
    }

    /// Feeds `input` at the next moment: every effect.
    fn step(&mut self, input: QueueInput) -> Vec<Heard> {
        self.desk.at_us += 1_000;
        let effects = self.desk.feed(input);
        self.take(effects)
    }

    /// `effects` as heard, a read of the review cursor's line answered at
    /// once in the place of its request.
    fn take(&mut self, effects: Vec<Effect>) -> Vec<Heard> {
        let mut heard = Vec::new();
        for effect in effects {
            if let Effect::Queue(QueueEffect::Send { to, ask }) = &effect
                && let Wire::Text { op, .. } = &ask.request
                && let Some(chunk) = self.unchanged(op)
            {
                let answer = QueueInput::Answer {
                    from: *to,
                    answer: verbatim_model::request::Answer {
                        number: ask.number,
                        outcome: Outcome::Result(Reply::Text(TextReply::Read { moved: 0, chunk })),
                        probes: Vec::new(),
                        detail: String::new(),
                    },
                };
                heard.extend(self.step(answer));
                continue;
            }
            if let Effect::Queue(QueueEffect::Send { to, ask }) = &effect
                && !matches!(ask.request, Wire::Follow(_))
            {
                self.out.push((ask.number.0, *to, ask.request.clone()));
            }
            let own = match &effect {
                Effect::ForKey(_, effect) => effect.as_ref(),
                effect => effect,
            };
            if let Effect::Speak(_, key) = own {
                self.stamps.push(*key);
            }
            if let Effect::Queue(QueueEffect::Cancel { cancel, .. }) = &effect {
                self.out.retain(|(number, ..)| *number != cancel.number.0);
            }
            heard.extend(heard_of(effect));
        }
        heard
    }

    /// The review cursor's line as the state holds it, when `op` reads it
    /// again where the review cursor is, in text that is not a terminal's.
    fn unchanged(&self, op: &TextOp) -> Option<TextChunk> {
        let TextOp::Read(verbatim_model::TextRead {
            at: TextPoint::At(position),
            movement: None,
            unit: TextUnit::Line,
        }) = op
        else {
            return None;
        };
        let whole = serde_json::to_value(&self.desk.state).expect("the state serializes");
        let navigator = &whole["navigator"];
        if navigator["object"]["role"] == "Terminal"
            || navigator["text"]["At"]["offset"].as_u64() != Some(u64::from(position.offset))
        {
            return None;
        }
        let mut chunk: TextChunk =
            serde_json::from_value(navigator["text"]["At"]["line"].clone()).ok()?;
        if chunk.start != position.anchor {
            return None;
        }
        chunk.offset = position.offset;
        Some(chunk)
    }

    /// Answers the one request out with `outcome`: every effect.
    fn outcome(&mut self, outcome: Outcome) -> Vec<Heard> {
        let [(number, from, _)] = self.out[..] else {
            panic!("one request out: {:?}", self.out);
        };
        self.out.clear();
        self.stamps.clear();
        self.step(QueueInput::Answer {
            from,
            answer: verbatim_model::request::Answer {
                number: verbatim_model::request::RequestNumber(number),
                outcome,
                probes: Vec::new(),
                detail: String::new(),
            },
        })
    }

    /// Answers the one request out, a live read of `snapshot`'s
    /// properties, with its values.
    fn properties(&mut self, snapshot: &NodeSnapshot) -> Vec<Heard> {
        self.outcome(Outcome::Result(Reply::Properties(desk::values_of(
            snapshot,
        ))))
    }

    /// Answers the one text request out with `reply`.
    fn reply(&mut self, reply: TextReply) -> Vec<Heard> {
        self.outcome(Outcome::Result(Reply::Text(reply)))
    }

    /// A key press doing `action`.
    fn press(&mut self, action: PressAction) -> Vec<Heard> {
        self.stamps.clear();
        self.desk.keys += 1;
        let press = Press {
            key_seq: KeySeq::new(self.desk.keys),
            pressed_at_us: self.desk.at_us + 1_000,
            action,
        };
        self.step(QueueInput::Press(Box::new(press)))
    }

    /// Caret key `motion`, held by the hook unless it selects, bound to
    /// the committed focus's record.
    fn key(&mut self, motion: CaretMotion, select: bool) -> Vec<Heard> {
        self.strokes += 1;
        let held = (!select).then(|| HeldStroke {
            stroke: Stroke {
                vk: 0x27,
                scan_code: 0x4D,
                extended: true,
                sequence: self.strokes,
            },
            modifiers: Vec::new(),
        });
        let bound = self.desk.state.caret_binding().map(Box::new);
        self.press(PressAction::CaretKey {
            key: CaretKey { motion, select },
            held,
            bound,
        })
    }

    /// Caret key `motion`, held, its starting read finding the caret where
    /// the application has it, and its watch finding the key's effect: the
    /// caret on `after`, with `unit` at it, by default the key's unit
    /// there. Every effect of the watch's answer.
    fn moved(
        &mut self,
        motion: CaretMotion,
        after: TextChunk,
        unit: Option<TextChunk>,
    ) -> Vec<Heard> {
        let unit = unit.unwrap_or_else(|| unit_at(&after, motion.unit()));
        self.watched(
            motion,
            Watched::Effect(WatchedRead {
                caret: caret_on(after, None),
                unit,
            }),
        )
    }

    /// Caret key `motion`, held and passed as [`Text::moved`] says, its
    /// watch answering `watched`: every effect of the answer.
    fn watched(&mut self, motion: CaretMotion, watched: Watched) -> Vec<Heard> {
        let passed = self.passed(motion);
        assert_eq!(
            passed,
            [
                Heard::Strokes(1),
                Heard::Asked(watch(motion, self.provider, self.desk.at_us))
            ]
        );
        if let Watched::Effect(read) | Watched::Bound(read) = &watched {
            self.caret = read.caret.clone();
        }
        self.reply(TextReply::Watched(Box::new(watched)))
    }

    /// Caret key `motion`, held, and its starting read answered with the
    /// caret where the application has it: every effect of the answer.
    fn passed(&mut self, motion: CaretMotion) -> Vec<Heard> {
        assert_eq!(
            self.key(motion, false),
            [Heard::Asked(starting_read(motion, self.provider))]
        );
        let before = self.caret.clone();
        let at = if motion.deletes_back() {
            unit_before(&before, motion.unit())
        } else {
            unit_at(&before.line, motion.unit())
        };
        self.reply(TextReply::Started(Box::new(Started {
            point: TextAnchor(50),
            caret: before,
            unit: at,
            word: None,
        })))
    }

    /// A UI Automation value change of node `number`, told by the outpost
    /// following it.
    fn value_changed(&mut self, number: u64) -> Vec<Heard> {
        if self.provider == TextProvider::EditMessages {
            let tell = self.edit_tell(EventKind::ValueChange, -4);
            return self.step(QueueInput::Tell {
                from: desk::LISTENER,
                tell: Box::new(tell),
            });
        }
        let tell = Event {
            kind: EventKind::UiaPropertyChanged,
            payload: Payload::PropertyChanged {
                property: verbatim_model::property::PropertyId::Value,
                value: Some(verbatim_model::property::PropertyValue::Text(String::new())),
            },
            ..self.followed(number, EventKind::UiaPropertyChanged)
        };
        self.step(QueueInput::Tell {
            from: ONE.instance(),
            tell: Box::new(tell),
        })
    }

    /// A tell of `kind` of MSAA object `object` of [`ONE`]'s window, as
    /// the listener tells one.
    fn edit_tell(&self, kind: EventKind, object: i32) -> Event {
        Event {
            kind,
            object: EventObject::Msaa {
                window: ONE.window(),
                object,
                child: 0,
            },
            window: TellWindow::Found(Box::new(ONE.facts())),
            ..self.followed(FIELD, kind)
        }
    }

    /// A tell of `kind` of node `number`, as the outpost following it
    /// tells one.
    fn followed(&self, number: u64, kind: EventKind) -> Event {
        Event {
            trace_id: TraceId::from_raw(8),
            kind,
            process: ONE.pid,
            application: "app".to_owned(),
            thread: 41,
            at_us: self.desk.at_us + 1_000,
            object: EventObject::Followed {
                runtime_id: desk::runtime_of(number),
            },
            cached: verbatim_model::property::PropertyValues::default(),
            payload: Payload::None,
            window: TellWindow::NoWindow,
            checked: None,
        }
    }

    /// The caret moved to `caret` by no key of Verbatim's: its caret tell,
    /// whose read names the baseline, answered with `difference` from it
    /// and the next baseline. Every effect of the answer.
    fn selection(
        &mut self,
        caret: CaretReport,
        difference: Option<SelectionDifference>,
    ) -> Vec<Heard> {
        assert_eq!(
            self.caret_tell(FIELD),
            [Heard::Asked(TextOp::ReadSelection(
                verbatim_model::SelectionRead {
                    provider: self.provider,
                    baseline: Some(TextAnchor(self.baseline)),
                    adopt: None,
                }
            ))]
        );
        self.baseline += 1;
        self.caret = caret.clone();
        let edit_selection = (self.provider == TextProvider::EditMessages).then(|| {
            caret.selection.as_ref().map_or(
                EditSelection {
                    start: caret.line.offset,
                    end: caret.line.offset,
                },
                |selection| {
                    // A position on another line counts as past this one.
                    let past = u32::try_from(caret.line.text.len()).expect("a short line");
                    let offset = |position: TextPosition| {
                        if position.anchor == caret.line.start {
                            position.offset
                        } else {
                            past + position.offset
                        }
                    };
                    EditSelection {
                        start: offset(selection.range.start),
                        end: offset(selection.range.end),
                    }
                },
            )
        });
        self.reply(TextReply::SelectionRead(Box::new(SelectionReadReply {
            caret,
            baseline: TextAnchor(self.baseline),
            difference,
            edit_selection,
        })))
    }

    /// Review command `command`, pressed `repeat` extra times.
    fn command(&mut self, command: ReviewCommand, repeat: u8) -> Vec<Heard> {
        self.press(PressAction::Command { command, repeat })
    }

    /// `text` typed.
    fn typed(&mut self, text: &str) -> Vec<Heard> {
        self.press(PressAction::Typed {
            text: text.to_owned(),
        })
    }

    /// The reader's settings changed to `settings`.
    fn settings(&mut self, settings: ReaderSettings) {
        let _ = verbatim_core::reduce(
            &mut self.desk.state,
            &mut self.desk.runtime,
            &Input::Settings(settings),
        );
    }

    /// A caret tell of node `number`, told by the outpost following it.
    fn caret_tell(&mut self, number: u64) -> Vec<Heard> {
        if self.provider == TextProvider::EditMessages {
            let tell = self.edit_tell(EventKind::SystemCaret, -8);
            return self.step(QueueInput::Tell {
                from: desk::LISTENER,
                tell: Box::new(tell),
            });
        }
        let tell = self.followed(number, EventKind::UiaTextSelectionChanged);
        self.step(QueueInput::Tell {
            from: ONE.instance(),
            tell: Box::new(tell),
        })
    }
}

/// The starting read's answer: point 50, the caret on `line` with
/// `selection`, `unit` at it.
fn started(line: TextChunk, selection: Option<CaretSelection>, unit: TextChunk) -> TextReply {
    TextReply::Started(Box::new(Started {
        point: TextAnchor(50),
        caret: caret_on(line, selection),
        unit,
        word: None,
    }))
}

/// `text` read as `unit` at anchor `anchor`.
fn unit(unit: TextUnit, text: &str, anchor: u64) -> TextChunk {
    TextChunk {
        unit,
        ..line(text, anchor, 0)
    }
}

/// The unit `unit` at the caret of `line`, as the outpost cuts it: a
/// character (a carriage return and line feed as one), a word with its
/// trailing spaces, or the line with its formatting.
fn unit_at(line: &TextChunk, unit: TextUnit) -> TextChunk {
    let rest = &line.text[line.offset as usize..];
    let text = match unit {
        TextUnit::Character if rest.starts_with("\r\n") => "\r\n",
        TextUnit::Character => rest
            .chars()
            .next()
            .map_or("", |first| &rest[..first.len_utf8()]),
        TextUnit::Word => {
            let end = rest.find(' ').map_or(rest.len(), |space| {
                rest.len() - rest[space..].trim_start_matches(' ').len()
            });
            &rest[..end]
        }
        _ => {
            return TextChunk {
                unit,
                offset: 0,
                ..line.clone()
            };
        }
    };
    self::unit(unit, text, line.start.0 + 1_000)
}

/// The `unit` around the character before the caret of `caret`, or
/// before its selection's start when that lies on its line, as the
/// outpost's starting read reads it for a Backspace
/// (`verbatim_model::StartingUnit::BeforeCaret`): the character, or the
/// word with the spaces after it, as [`unit_at`] finds them; empty at the
/// line's start.
fn unit_before(caret: &CaretReport, unit: TextUnit) -> TextChunk {
    let line = &caret.line;
    let at = caret
        .selection
        .as_ref()
        .filter(|selection| selection.range.start.anchor == line.start)
        .map_or(line.offset, |selection| selection.range.start.offset) as usize;
    let before = &line.text[..at];
    let Some(last) = before.chars().last() else {
        return self::unit(unit, "", line.start.0 + 1_000);
    };
    let start = match unit {
        TextUnit::Word => before
            .trim_end_matches(' ')
            .rfind(' ')
            .map_or(0, |space| space + 1),
        _ => at - last.len_utf8(),
    };
    let from = TextChunk {
        offset: u32::try_from(start).expect("a short line"),
        ..line.clone()
    };
    unit_at(&from, unit)
}

fn character(text: &str) -> UtteranceSegment {
    UtteranceSegment::new(SegmentContent::Character(text.to_owned()))
}

fn message(message: Message) -> UtteranceSegment {
    UtteranceSegment::new(SegmentContent::Message(message))
}

fn phrase(phrase: Phrase) -> UtteranceSegment {
    UtteranceSegment::new(SegmentContent::Phrase(phrase))
}

/// The edit field focused, its caret on `text` at `offset`, at anchor 100.
fn editing(text: &str, offset: u32) -> Text {
    let mut field = Text::new();
    let _ = field.focus(
        &node(FIELD, Role::EditableText, StateSet::new()),
        caret_read(caret_on(line(text, 100, offset), None), None, 1),
    );
    field
}

/// What a key of `motion`'s reads fetch: everything, but no spelling or
/// grammar errors for a paragraph.
fn details(motion: CaretMotion) -> verbatim_model::TextDetails {
    if motion.unit() == TextUnit::Paragraph {
        verbatim_model::TextDetails {
            spelling_errors: false,
            grammar_errors: false,
            ..verbatim_model::TextDetails::ALL
        }
    } else {
        verbatim_model::TextDetails::ALL
    }
}

/// A field's starting read for `motion`.
fn starting_read(motion: CaretMotion, provider: TextProvider) -> TextOp {
    TextOp::StartingRead(verbatim_model::StartingRead {
        provider,
        details: details(motion),
        unit: verbatim_model::StartingUnit::for_motion(motion),
    })
}

/// A field's watch for `motion`, passed at `passed_at_us`.
fn watch(motion: CaretMotion, provider: TextProvider, passed_at_us: u64) -> TextOp {
    TextOp::Watch(verbatim_model::Watch {
        provider,
        point: TextAnchor(50),
        motion,
        passed_at_us,
        poll_us: 10_000,
        events: provider == TextProvider::Uia,
        bound_us: 100_000,
        final_read: verbatim_model::WatchFinalRead::Unit {
            unit: motion.unit(),
            details: details(motion),
        },
    })
}

/// The one utterance of a key's words: `segments`, queued.
fn words(segments: Vec<UtteranceSegment>) -> Vec<Heard> {
    vec![queued(segments)]
}

/// The difference from the baseline: `changes`, each selected or not with
/// its text, and the whole new selection's text.
fn differing(changes: &[(bool, &str)], new_selection: Option<&str>) -> SelectionDifference {
    let change = |selected, text: &str| SelectionChange {
        selected,
        text: text.to_owned(),
        characters: u32::try_from(text.chars().count()).expect("a short text"),
    };
    SelectionDifference {
        changes: changes
            .iter()
            .map(|&(selected, text)| change(selected, text))
            .collect(),
        new_selection: new_selection.map(|text| change(true, text)),
    }
}

/// The edit field as [`editing`] makes it, with the review cursor
/// settled at the caret by a review of the current character.
fn reviewing(text: &str, offset: u32) -> Text {
    let mut field = editing(text, offset);
    field.settle(line(text, 100, offset));
    field
}

impl Text {
    /// Settles the review cursor, following the caret, on the caret's
    /// `line`, as the first review command after the caret moved does.
    fn settle(&mut self, line: TextChunk) {
        assert_eq!(
            self.command(ReviewCommand::ReviewCurrentCharacter, 0),
            [Heard::Asked(line_read(TextPoint::Caret, None))]
        );
        let _ = self.reply(TextReply::Read {
            moved: 0,
            chunk: line,
        });
    }
}

/// A read of the line at `at`, after moving by `movement` when given.
fn line_read(at: TextPoint, movement: Option<(TextUnit, i32)>) -> TextOp {
    TextOp::Read(verbatim_model::TextRead {
        at,
        movement: movement.map(|(unit, count)| verbatim_model::TextMovement { unit, count }),
        unit: TextUnit::Line,
    })
}

/// A classic edit control focused, read through its messages, its caret
/// on `text` at `offset`, at anchor 100.
fn edit_control(text: &str, offset: u32) -> Text {
    let mut field = Text::new();
    field.provider = TextProvider::EditMessages;
    field.desk.app(ONE);
    let tell = field.edit_tell(EventKind::MsaaFocus, -4);
    let _ = field.desk.tell(desk::LISTENER, tell);
    let [(run, to, Wire::MsaaRun(_))] = field.desk.sent[..] else {
        panic!("one run asked: {:?}", field.desk.sent);
    };
    let caret = caret_on(line(text, 100, offset), None);
    let mut read = FocusRead {
        caret: caret_read(caret.clone(), None, 1),
        msaa: Some(verbatim_model::request::HeldIds {
            event: verbatim_model::request::MsaaIds {
                window: ONE.window(),
                object: -4,
                child: 0,
            },
            held_child: 0,
        }),
        ..desk::focus_read_of(ONE, &node(FIELD, Role::EditableText, StateSet::new()), &[])
    };
    read.focus.runtime_id = None;
    let tried = verbatim_model::request::CandidateAnswer::focused(read);
    let _ = field
        .desk
        .answer(to, run, Outcome::Result(Reply::Candidate(tried)));
    field.caret = caret;
    field
}

#[test]
fn an_arrow_key_watches_for_evidence_then_speaks_the_character_at_the_caret() {
    let mut text = editing("Hello\r\n", 0);
    assert_eq!(
        text.moved(CaretMotion::NextCharacter, line("Hello\r\n", 100, 1), None),
        words(vec![character("e")])
    );
    // At the end of the line, the caret is on the line break, named by its
    // first character, as NVDA names it.
    assert_eq!(
        text.moved(CaretMotion::EndOfLine, line("Hello\r\n", 100, 5), None),
        words(vec![character("\r")])
    );
    // At the end of the text there is no character: blank.
    assert_eq!(
        text.moved(CaretMotion::EndOfLine, line("Bye", 101, 3), None),
        words(vec![message(Message::Blank)])
    );
}

#[test]
fn a_backspace_in_a_run_of_one_character_watches_its_line() {
    // Deleting one star of a password field's mask changes neither the
    // characters beside the caret nor, where positions follow edits, the
    // caret: the watch says the key deletes, so the outpost takes the line
    // changing as evidence, and the star removed is spoken.
    let mut text = editing("******", 5);
    assert_eq!(
        text.moved(CaretMotion::Backspace, line("*****", 100, 4), None),
        words(vec![character("*")])
    );
}

#[test]
fn a_line_break_the_caret_reaches_is_named_with_its_formatting_read() {
    // The outpost cuts the character from the line, a carriage return and
    // line feed as one cluster; it is named by its carriage return.
    let mut text = editing("Hello\r\n", 0);
    let unit = TextChunk {
        unit: TextUnit::Character,
        formats: vec![verbatim_model::FormatRun {
            start: 0,
            end: 2,
            attributes: verbatim_model::TextAttributes::default(),
        }],
        ..line("\r\n", 102, 0)
    };
    assert_eq!(
        text.moved(
            CaretMotion::EndOfLine,
            line("Hello\r\n", 100, 5),
            Some(unit)
        ),
        words(vec![character("\r")])
    );
}

#[test]
fn line_word_and_paragraph_keys_speak_their_unit() {
    let mut text = editing("one two\n", 0);
    assert_eq!(
        text.moved(CaretMotion::NextLine, line("second line\n", 101, 0), None),
        words(vec![UtteranceSegment::text("second line")])
    );
    // The provider's word, as the application moved the caret by it.
    assert_eq!(
        text.moved(
            CaretMotion::NextWord,
            line("second line\n", 101, 7),
            Some(unit(TextUnit::Word, "line ", 102))
        ),
        words(vec![UtteranceSegment::text("line")])
    );
    // A provider without paragraphs reads the line as the paragraph.
    assert_eq!(
        text.moved(CaretMotion::NextParagraph, line("third\n", 103, 0), None),
        words(vec![UtteranceSegment::text("third")])
    );
}

#[test]
fn a_word_of_one_character_is_spoken_as_that_character() {
    // Notepad's word unit makes a sentence's full stop a word of its own;
    // spoken as text, it would say nothing.
    let mut text = editing("the river bank.\r\n", 11);
    assert_eq!(
        text.moved(
            CaretMotion::NextWord,
            line("the river bank.\r\n", 101, 14),
            Some(unit(TextUnit::Word, ".\r\n", 102))
        ),
        words(vec![character(".")])
    );
}

#[test]
fn a_word_of_one_space_is_spoken_as_that_space() {
    // A provider's word that is a single space (the space before the first
    // word of an indented line) is spelled, as NVDA keeps a unit's text
    // that is only white space when it decides whether to spell it
    // (`speech/speech.py` 1753 to 1763): "space", not "blank".
    let mut text = editing(
        "a  b
", 0,
    );
    assert_eq!(
        text.moved(
            CaretMotion::NextWord,
            line(
                "a  b
", 101, 1
            ),
            Some(unit(TextUnit::Word, " ", 102))
        ),
        words(vec![character(" ")])
    );
    // Two spaces are not one character: "blank", as NVDA speaks them.
    assert_eq!(
        text.moved(
            CaretMotion::NextWord,
            line(
                "a  b
", 101, 1
            ),
            Some(unit(TextUnit::Word, "  ", 103))
        ),
        words(vec![message(Message::Blank)])
    );
}

#[test]
fn the_review_cursor_speaks_a_word_of_one_character_by_its_name() {
    let mut text = reviewing("the river bank.\r\n", 11);
    assert_eq!(
        text.command(ReviewCommand::ReviewNextWord, 0),
        words(vec![character(".")])
    );
    assert_eq!(
        text.command(ReviewCommand::ReviewCurrentWord, 0),
        words(vec![character(".")])
    );
}

#[test]
fn shift_movement_speaks_what_was_selected_and_unselected() {
    // A selection key is passed at once and says nothing; its caret tell
    // says what it selected and unselected, each part its own utterance,
    // as NVDA speaks each selection message on its own: every selected
    // part first, then every unselected one, whatever order the outpost
    // listed them in (`speech/speech.py` 1389 to 1407).
    let mut text = editing("hello, world\n", 5);
    assert_eq!(text.key(CaretMotion::NextCharacter, true), []);
    let long = "x".repeat(600);
    assert_eq!(
        text.selection(
            caret_on(line("hello, world\n", 100, 6), None),
            Some(differing(
                &[(true, ","), (false, "hello"), (true, &long)],
                None
            ))
        ),
        [
            queued(vec![phrase(Phrase::Selected(SelectionText::Character(
                ",".to_owned()
            )))]),
            queued(vec![phrase(Phrase::Selected(SelectionText::Characters(
                600
            )))]),
            queued(vec![phrase(Phrase::Unselected(SelectionText::Text(
                "hello".to_owned()
            )))]),
        ]
    );
}

/// A modifier pressed after a selection key, in the hook's order: its cut
/// of speech reaches Core before the caret tell, whose selection speech is
/// the selection key's words and carries its number, so the speech queue,
/// which drops speech numbered below the cut, silences it, as NVDA's
/// queued cut silences the key's script (`docs/parity/text.md`, "Speech
/// cut off by a modifier").
#[test]
fn a_selection_said_after_a_modifiers_cut_carries_the_selection_keys_number() {
    let mut text = editing(
        "hello, world
",
        5,
    );
    // Shift+Right Arrow is key 1, Control key 2.
    assert_eq!(text.key(CaretMotion::NextCharacter, true), []);
    assert_eq!(text.cut(), []);
    assert_eq!(
        text.selection(
            caret_on(
                line(
                    "hello, world
",
                    100,
                    6
                ),
                None
            ),
            Some(differing(&[(true, ",")], None))
        ),
        [queued(vec![phrase(Phrase::Selected(
            SelectionText::Character(",".to_owned())
        ))])]
    );
    assert_eq!(text.stamps, [Some(KeySeq::new(1))]);
}

/// A caret tell asks its read, then Shift+Control+Right Arrow comes, in
/// the hook's order: Control's cut, the arrow's cut and its press. The
/// read, answered after the arrow joined the queue, sees the selection
/// the arrow made, so its words are the arrow's and carry its number,
/// which the arrow's own cut does not silence (`notepad_selection_keys`,
/// Control+X then Shift+Control+Right Arrow, whose Control+X caret tell
/// was read after the arrow).
#[test]
fn a_selection_read_after_a_key_joined_carries_that_keys_number() {
    let mut text = editing("hello, world\n", 7);
    assert_eq!(
        text.caret_tell(FIELD),
        [Heard::Asked(TextOp::ReadSelection(
            verbatim_model::SelectionRead {
                provider: TextProvider::Uia,
                baseline: Some(TextAnchor(1)),
                adopt: None,
            }
        ))]
    );
    // Control is key 1; the arrow cuts speech and joins as key 2.
    assert_eq!(text.cut(), []);
    assert_eq!(
        text.step(QueueInput::SpeechCut {
            key: KeySeq::new(2)
        }),
        []
    );
    assert_eq!(text.key(CaretMotion::NextWord, true), []);
    assert_eq!(
        text.reply(TextReply::SelectionRead(Box::new(SelectionReadReply {
            caret: caret_on(line("hello, world\n", 100, 12), Some(selected(7, 12))),
            baseline: TextAnchor(2),
            difference: Some(differing(&[(true, "world")], None)),
            edit_selection: None,
        }))),
        [queued(vec![phrase(Phrase::Selected(SelectionText::Text(
            "world".to_owned()
        )))])]
    );
    assert_eq!(text.stamps, [Some(KeySeq::new(2))]);
}

/// A modifier's cut and then an event the application raised on its own:
/// its speech carries the cut's number, so the speech queue speaks it, as
/// NVDA runs an event that arrives after the cut it queued
/// (`docs/parity/text.md`, "Speech cut off by a modifier").
#[test]
fn an_event_after_a_modifiers_cut_carries_the_cuts_number() {
    let mut text = editing("ab", 2);
    // "c" typed is key 1, Control key 2.
    let _ = text.typed("c");
    assert_eq!(text.cut(), []);
    text.stamps.clear();
    let tell = Event {
        payload: Payload::PropertyChanged {
            property: verbatim_model::property::PropertyId::Name,
            value: Some(verbatim_model::property::PropertyValue::Text(
                "Renamed".to_owned(),
            )),
        },
        ..text.followed(FIELD, EventKind::UiaPropertyChanged)
    };
    let heard = text.step(QueueInput::Tell {
        from: ONE.instance(),
        tell: Box::new(tell),
    });
    assert_eq!(heard, [queued(vec![UtteranceSegment::label("Renamed")])]);
    assert_eq!(text.stamps, [Some(KeySeq::new(2))]);
}

/// The selection of bytes `start` to `end` of the line at anchor 100.
fn selected(start: u32, end: u32) -> CaretSelection {
    let at = |offset| TextPosition {
        anchor: TextAnchor(100),
        offset,
    };
    caret_selection(Selection {
        start: at(start),
        end: at(end),
    })
}

#[test]
fn a_movement_that_leaves_a_selection_speaks_the_unit_then_what_it_unselected() {
    let text_line = |offset| line("hello, world\n", 100, offset);
    let mut text = editing("hello, world\n", 5);
    // Shift+Home selects "hello".
    assert_eq!(text.key(CaretMotion::StartOfLine, true), []);
    let _ = text.selection(
        caret_on(text_line(0), Some(selected(0, 5))),
        Some(differing(&[(true, "hello")], None)),
    );
    // Right Arrow speaks the character, then its caret tell the text
    // unselected.
    assert_eq!(
        text.moved(CaretMotion::NextCharacter, text_line(5), None),
        words(vec![character(",")])
    );
    assert_eq!(
        text.selection(
            caret_on(text_line(5), None),
            Some(differing(&[(false, "hello")], None))
        ),
        words(vec![phrase(Phrase::Unselected(SelectionText::Text(
            "hello".to_owned()
        )))])
    );

    // A deletion's starting read compares no selection either.
    let mut text = editing("hello, world\n", 0);
    assert_eq!(text.key(CaretMotion::NextWord, true), []);
    let _ = text.selection(
        caret_on(text_line(5), Some(selected(0, 5))),
        Some(differing(&[(true, "hello")], None)),
    );
    assert_eq!(
        text.key(CaretMotion::Delete, false),
        [Heard::Asked(starting_read(
            CaretMotion::Delete,
            TextProvider::Uia
        ))]
    );
}

#[test]
fn backspace_at_a_lines_start_names_the_line_break_it_deleted() {
    // Windows 11 Notepad's text breaks lines with a carriage return.
    let mut text = editing("def\r", 0);
    assert_eq!(
        text.moved(CaretMotion::Backspace, line("abcdef\r", 99, 3), None),
        words(vec![character("\r")])
    );

    // With Control too, as Notepad deletes the break before a line's start.
    let mut text = editing("def\r", 0);
    assert_eq!(
        text.moved(CaretMotion::BackspaceWord, line("abcdef\r", 99, 3), None),
        words(vec![character("\r")])
    );

    // A standard edit control's carriage return and line feed are spoken
    // as the line feed. On the text's last line, which has no break, the
    // break is the one the text was last seen to use.
    let mut text = editing("abc\r\n", 1);
    assert_eq!(
        text.selection(caret_on(line("def", 101, 0), None), None),
        []
    );
    assert_eq!(
        text.moved(CaretMotion::Backspace, line("abcdef", 100, 3), None),
        words(vec![character("\n")])
    );

    // On the text's first line there is no break to delete.
    let mut text = Text::new();
    let first = TextChunk {
        first: true,
        ..line("abc\r\n", 100, 0)
    };
    let _ = text.focus(
        &node(FIELD, Role::EditableText, StateSet::new()),
        caret_read(caret_on(first, None), None, 1),
    );
    let read = WatchedRead {
        caret: caret_on(line("abc\r\n", 100, 0), None),
        unit: unit(TextUnit::Character, "a", 101),
    };
    assert_eq!(
        text.watched(CaretMotion::Backspace, Watched::Bound(read)),
        []
    );
}

#[test]
fn delete_compares_the_character_and_speaks_the_new_one() {
    let mut text = editing("abc\n", 1);
    assert_eq!(
        text.moved(CaretMotion::Delete, line("ac\n", 100, 1), None),
        words(vec![character("c")])
    );
}

/// A selection from byte `start` of the line whose anchor is 100 (or of
/// another chunk, `on_line` false) to an end elsewhere.
fn selection_from(start: u32, on_line: bool) -> Selection {
    Selection {
        start: TextPosition {
            anchor: TextAnchor(if on_line { 100 } else { 200 }),
            offset: start,
        },
        end: TextPosition {
            anchor: TextAnchor(201),
            offset: 0,
        },
    }
}

impl Text {
    /// Backspace or Control+Backspace, `motion`, over the selection the
    /// caret has in a classic edit control, leaving the caret on `after`:
    /// the key's words, then the words of the value change and caret tell
    /// its edit raises, the selection's removal said in general terms, as
    /// an edit control's offsets do not follow the edit.
    fn deleted_selection(&mut self, motion: CaretMotion, after: TextChunk) -> [Vec<Heard>; 2] {
        let said = self.moved(motion, after.clone(), None);
        assert_eq!(self.value_changed(FIELD), []);
        let removed = self.selection(caret_on(after, None), Some(differing(&[(false, "")], None)));
        [said, removed]
    }
}

#[test]
fn backspace_over_a_selection_says_what_was_before_its_start_then_the_selection_change() {
    // What comes before the selection's start, where NVDA's caret is,
    // whichever end the caret is
    // (`target/e2e-evidence/m42-selection-keys-2026-10-22/`): a classic
    // edit control's caret is the start of "beta " (`textbox-second-word-backspace.txt`,
    // `textbox-second-word-control-backspace.txt`); Notepad's is the
    // end of "beta " (`notepad-second-word-control-backspace.txt`) and of
    // "gamma" up to the line's end
    // (`notepad-third-word-to-end-backspace.txt`,
    // `notepad-third-word-to-end-control-backspace.txt`).
    let text = "alpha beta gamma\r\n";
    let space = character(" ");
    let alpha = UtteranceSegment::text("alpha".to_owned());
    let beta = UtteranceSegment::text("beta".to_owned());
    for (caret, start, motion, said) in [
        (6, 6, CaretMotion::Backspace, &space),
        (6, 6, CaretMotion::BackspaceWord, &alpha),
        (11, 6, CaretMotion::BackspaceWord, &alpha),
        (16, 11, CaretMotion::Backspace, &space),
        (16, 11, CaretMotion::BackspaceWord, &beta),
    ] {
        let mut field = edit_control(text, 0);
        let selection = Some(caret_selection(selection_from(start, true)));
        assert_eq!(
            field.selection(caret_on(line(text, 100, caret), selection), None),
            []
        );
        assert_eq!(
            field.deleted_selection(motion, line("alpha gamma\r\n", 100, 6)),
            [
                words(vec![said.clone()]),
                words(vec![message(Message::SelectionRemoved)])
            ],
            "{motion:?} with the caret at {caret} and the selection from {start}"
        );
    }

    // With nothing before the start, "blank" (`textbox-word-backspace.txt`).
    let mut field = edit_control(text, 0);
    let first = TextChunk {
        first: true,
        ..line(text, 100, 0)
    };
    let selection = Some(caret_selection(selection_from(0, true)));
    assert_eq!(field.selection(caret_on(first, selection), None), []);
    assert_eq!(
        field.deleted_selection(CaretMotion::Backspace, line("beta gamma\r\n", 100, 0)),
        [
            words(vec![message(Message::Blank)]),
            words(vec![message(Message::SelectionRemoved)])
        ]
    );
}

#[test]
fn backspace_over_a_selection_whose_start_is_in_no_line_reported_says_only_the_change() {
    // A start in no line reported with it has nothing known before it:
    // only the selection's change is said.
    let text = "alpha beta gamma\r\n";
    let mut field = edit_control(text, 0);

    let selection = Some(caret_selection(selection_from(3, false)));
    assert_eq!(
        field.selection(caret_on(line(text, 100, 6), selection), None),
        []
    );
    assert_eq!(
        field.deleted_selection(CaretMotion::Backspace, line("gamma\r\n", 100, 0)),
        [Vec::new(), words(vec![message(Message::SelectionRemoved)])]
    );
}

/// An undo that restores a selection, an editing chord, says the selection
/// alone from the application's caret event and not the line it changed
/// (`docs/design/caret-selection-rework.md`).
#[test]
fn an_undo_restoring_a_selection_says_only_the_selection() {
    let after_cut = "beta gamma\r\n";
    let restored = "alpha beta gamma\r\n";
    let mut text = editing(after_cut, 0);
    assert_eq!(
        text.press(PressAction::EditingChord),
        [],
        "a chord asks nothing"
    );
    assert_eq!(text.value_changed(FIELD), []);
    let selection = Some(caret_selection(selection_from(6, true)));
    assert_eq!(
        text.selection(
            caret_on(line(restored, 100, 0), selection),
            Some(differing(&[(true, "alpha ")], Some("alpha ")))
        ),
        words(vec![phrase(Phrase::Selected(SelectionText::Text(
            "alpha ".to_owned()
        )))])
    );
}

#[test]
fn a_newer_key_or_a_focus_change_supersedes_a_waiting_key() {
    // A key with a later key behind it is sent silently at its starting
    // read's answer; the later key says where it landed.
    let mut text = editing("abc\n", 0);
    let first = starting_read(CaretMotion::NextCharacter, TextProvider::Uia);
    assert_eq!(
        text.key(CaretMotion::NextCharacter, false),
        [Heard::Asked(first.clone())]
    );
    assert_eq!(text.key(CaretMotion::NextCharacter, false), []);
    let at = |offset| line("abc\n", 100, offset);
    assert_eq!(
        text.reply(started(at(0), None, unit_at(&at(0), TextUnit::Character))),
        [Heard::Strokes(1), Heard::Asked(first)]
    );
    assert_eq!(
        text.reply(started(at(1), None, unit_at(&at(1), TextUnit::Character))),
        [
            Heard::Strokes(1),
            Heard::Asked(watch(
                CaretMotion::NextCharacter,
                TextProvider::Uia,
                text.desk.at_us
            ))
        ]
    );
    assert_eq!(
        text.reply(TextReply::Watched(Box::new(Watched::Effect(WatchedRead {
            caret: caret_on(at(1), None),
            unit: unit_at(&at(1), TextUnit::Character),
        })))),
        words(vec![character("b")])
    );
    // A focus moving while a key is watched withdraws the watch, and the
    // key says nothing.
    text.caret = caret_on(at(1), None);
    let _ = text.passed(CaretMotion::NextCharacter);
    let focused = text
        .desk
        .focus(ONE, &node(6, Role::Button, StateSet::new()), &[]);
    assert_eq!(
        focused,
        desk::focus_heard(
            ONE,
            desk::focus_now_in(ONE, 6, &[]),
            vec![vec![
                UtteranceSegment::label("Body"),
                UtteranceSegment::new(SegmentContent::Role(Role::Button)),
            ]]
        )
    );
}

#[test]
fn caret_keys_are_watched_in_a_terminal_and_not_at_all_without_text() {
    let mut text = Text::new();
    let _ = text.focus(&node(8, Role::Button, StateSet::new()), FocusCaret::NoText);
    // Bound to no text, the key is sent at once and says nothing.
    assert_eq!(
        text.key(CaretMotion::PreviousLine, false),
        [Heard::Strokes(1)]
    );
}

#[test]
fn typed_characters_are_echoed_and_a_protected_field_echoes_stars() {
    let mut text = editing("", 0);
    assert_eq!(text.typed("a"), words(vec![character("a")]));
    // Word echo is off by default.
    assert_eq!(text.typed(" "), words(vec![character(" ")]));

    let mut text = Text::new();
    let protected = node(
        9,
        Role::EditableText,
        StateSet::new().with(verbatim_model::State::Protected),
    );
    let _ = text.focus(
        &protected,
        caret_read(caret_on(line("", 100, 0), None), None, 1),
    );
    assert_eq!(text.typed("p"), words(vec![character("*")]));
}

/// Word echo always on, character echo off.
fn words_only() -> ReaderSettings {
    ReaderSettings {
        speak_typed_characters: verbatim_model::TypingEcho::Off,
        speak_typed_words: verbatim_model::TypingEcho::Always,
        ..ReaderSettings::default()
    }
}

#[test]
fn word_echo_speaks_a_word_when_it_ends() {
    let mut text = editing("", 0);
    text.settings(words_only());
    for letter in ["h", "i"] {
        assert_eq!(text.typed(letter), []);
    }
    assert_eq!(text.typed(" "), words(vec![UtteranceSegment::text("hi")]));
}

/// Combining marks and the zero-width non-joiner, each typed as a key of
/// its own, continue the word: a Devanagari conjunct (ka, virama, ssa), a
/// Thai syllable with a tone mark (ko kai, mai ek, sara aa), and Persian
/// with a zero-width non-joiner (mim, yeh, the non-joiner, khah) are each
/// spoken as one word when a space ends it.
#[test]
fn word_echo_keeps_combining_marks_and_joiners_in_the_word() {
    let mut text = editing("", 0);
    text.settings(words_only());
    for (keys, word) in [
        (&["क", "\u{94D}", "ष"][..], "क्ष"),
        (&["ก", "\u{E48}", "า"][..], "ก่า"),
        (&["م", "ی", "\u{200C}", "خ"][..], "می\u{200C}خ"),
    ] {
        for key in keys {
            assert_eq!(text.typed(key), [], "{word}");
        }
        assert_eq!(text.typed(" "), words(vec![UtteranceSegment::text(word)]));
    }
}

/// The word being typed goes on past a key that selects and a key outside
/// the caret table, and starts again after a caret movement without Shift
/// (`docs/parity/input.md`, "Typed-character echo").
#[test]
fn word_echo_forgets_the_word_only_when_the_caret_moves_without_shift() {
    let mut text = editing("", 0);
    text.settings(words_only());
    let type_word = |text: &mut Text, between: &mut dyn FnMut(&mut Text)| {
        for letter in ["h", "e", "l"] {
            assert_eq!(text.typed(letter), []);
        }
        between(text);
        for letter in ["l", "o"] {
            assert_eq!(text.typed(letter), []);
        }
        text.typed(" ")
    };
    let mut other = |text: &mut Text| assert_eq!(text.press(PressAction::Passed), []);
    assert_eq!(
        type_word(&mut text, &mut other),
        words(vec![UtteranceSegment::text("hello")])
    );
    let mut selecting =
        |text: &mut Text| assert_eq!(text.key(CaretMotion::PreviousCharacter, true), []);
    assert_eq!(
        type_word(&mut text, &mut selecting),
        words(vec![UtteranceSegment::text("hello")])
    );
    let mut moving = |text: &mut Text| {
        assert_eq!(
            text.moved(CaretMotion::PreviousCharacter, line("hel", 100, 2), None),
            words(vec![character("l")])
        );
    };
    assert_eq!(
        type_word(&mut text, &mut moving),
        words(vec![UtteranceSegment::text("lo")])
    );
}

#[test]
fn control_characters_end_a_word_but_are_never_spelled() {
    let mut text = editing("", 0);
    text.settings(ReaderSettings {
        speak_typed_words: verbatim_model::TypingEcho::Always,
        ..ReaderSettings::default()
    });
    assert_eq!(text.typed("a"), words(vec![character("a")]));
    // Tab ends the word, which is spoken, and is not spelled itself.
    assert_eq!(text.typed("\t"), words(vec![UtteranceSegment::text("a")]));
    assert_eq!(text.typed("\r"), []);
}

#[test]
fn echo_only_in_edit_controls_stays_quiet_elsewhere() {
    let mut text = Text::new();
    text.settings(ReaderSettings {
        speak_typed_characters: verbatim_model::TypingEcho::EditControls,
        ..ReaderSettings::default()
    });
    let _ = text.focus(&node(10, Role::Button, StateSet::new()), FocusCaret::NoText);
    assert_eq!(text.typed("a"), []);
    let _ = text.focus(
        &node(11, Role::EditableText, StateSet::new()),
        caret_read(caret_on(line("", 100, 0), None), None, 1),
    );
    assert_eq!(text.typed("a"), words(vec![character("a")]));
}

impl Text {
    /// Answers the one text request out with a line read that moved
    /// `moved` lines and found `chunk`.
    fn read(&mut self, moved: i32, chunk: TextChunk) -> Vec<Heard> {
        self.reply(TextReply::Read { moved, chunk })
    }
}

/// Node `number` of `role` focused, its caret on `line`.
fn focused(number: u64, role: Role, line: TextChunk) -> Text {
    let mut field = Text::new();
    let _ = field.focus(
        &node(number, role, StateSet::new()),
        caret_read(caret_on(line, None), None, 1),
    );
    field
}

#[test]
fn a_review_cursor_moved_off_the_caret_reviews_from_where_it_is() {
    let mut text = reviewing("one two\n", 0);
    assert_eq!(
        text.command(ReviewCommand::ReviewNextWord, 0),
        words(vec![UtteranceSegment::text("two")])
    );
    assert_eq!(
        text.command(ReviewCommand::ReviewCurrentCharacter, 0),
        words(vec![character("t")])
    );
}

#[test]
fn the_review_cursor_follows_the_caret_and_keeps_its_column_across_lines() {
    let mut text = editing("abcdef\n", 4);
    // Following the caret: the line at the caret is read afresh.
    assert_eq!(
        text.command(ReviewCommand::ReviewCurrentCharacter, 0),
        [Heard::Asked(line_read(TextPoint::Caret, None))]
    );
    assert_eq!(
        text.read(0, line("abcdef\n", 100, 4)),
        words(vec![character("e")])
    );
    assert_eq!(
        text.command(ReviewCommand::ReviewNextLine, 0),
        [Heard::Asked(line_read(
            TextPoint::At(TextPosition::at(TextAnchor(100))),
            Some((TextUnit::Line, 1))
        ))]
    );
    // A shorter line puts the cursor on its last character.
    assert_eq!(
        text.read(1, line("xy\n", 101, 0)),
        words(vec![UtteranceSegment::text("xy")])
    );
    assert_eq!(
        text.command(ReviewCommand::ReviewCurrentCharacter, 0),
        words(vec![character("y")])
    );

    // The next longer line returns to the remembered column.
    let _ = text.command(ReviewCommand::ReviewNextLine, 0);
    let _ = text.read(1, line("012345678\n", 102, 0));
    assert_eq!(
        text.command(ReviewCommand::ReviewCurrentCharacter, 0),
        words(vec![character("4")])
    );
}

#[test]
fn the_review_cursor_meets_each_character_of_a_line_break() {
    // A standard edit control's line: the end of the line is its line
    // feed, and moving by character crosses the carriage return first.
    let mut text = reviewing("ab\r\n", 1);
    for (command, said) in [
        (ReviewCommand::ReviewEndOfLine, vec![character("\n")]),
        (
            ReviewCommand::ReviewPreviousCharacter,
            vec![character("\r")],
        ),
        (ReviewCommand::ReviewPreviousCharacter, vec![character("b")]),
        (ReviewCommand::ReviewNextCharacter, vec![character("\r")]),
        (ReviewCommand::ReviewNextCharacter, vec![character("\n")]),
        // The line feed is the line's last character.
        (
            ReviewCommand::ReviewNextCharacter,
            vec![message(Message::Right), character("\n")],
        ),
    ] {
        assert_eq!(text.command(command, 0), words(said), "{command:?}");
    }

    // Windows 11 Notepad's line ends with a carriage return alone, and the
    // text's last line, with no break, ends on its last character.
    let mut text = reviewing("ab\r", 0);
    assert_eq!(
        text.command(ReviewCommand::ReviewEndOfLine, 0),
        words(vec![character("\r")])
    );
    let mut text = reviewing("ab", 0);
    assert_eq!(
        text.command(ReviewCommand::ReviewEndOfLine, 0),
        words(vec![character("b")])
    );
}

#[test]
fn review_edges_are_known_from_the_line_or_from_a_movement_that_did_not_move() {
    let only = TextChunk {
        first: true,
        ..line("only\n", 300, 0)
    };
    let mut text = focused(14, Role::EditableText, only.clone());
    text.settle(only);
    let _ = text.command(ReviewCommand::ReviewPreviousLine, 0);
    assert_eq!(
        text.read(0, line("only\n", 300, 0)),
        words(vec![message(Message::Top), UtteranceSegment::text("only")])
    );
    let _ = text.command(ReviewCommand::ReviewNextLine, 0);
    assert_eq!(
        text.read(0, line("only\n", 300, 0)),
        words(vec![
            message(Message::Bottom),
            UtteranceSegment::text("only")
        ])
    );
}

#[test]
fn repeated_presses_describe_and_spell_with_descriptions() {
    let mut text = reviewing("ab\n", 0);
    let description =
        |text: &str| UtteranceSegment::new(SegmentContent::CharacterDescription(text.to_owned()));
    assert_eq!(
        text.command(ReviewCommand::ReviewCurrentCharacter, 1),
        words(vec![description("a")])
    );
    assert_eq!(
        text.command(ReviewCommand::ReviewCurrentLine, 2),
        words(vec![description("a"), description("b")])
    );
}

#[test]
fn a_navigator_without_a_caret_reads_its_line_first_and_falls_back_to_flat_text() {
    let mut text = Text::new();
    let mut edit = node(15, Role::EditableText, StateSet::new());
    edit.value = Some("flat value".to_owned());
    let _ = text.focus(&edit, FocusCaret::NotRead);
    assert_eq!(
        text.command(ReviewCommand::ReviewCurrentLine, 0),
        [Heard::Asked(line_read(TextPoint::Caret, None))]
    );
    // With no text, the object is read afresh and its value reviewed.
    assert_eq!(text.reply(TextReply::NoText), [Heard::Sent("Properties")]);
    assert_eq!(
        text.properties(&edit),
        words(vec![UtteranceSegment::text("flat value")])
    );
}

/// The point `offset` bytes into the line [`reviewing`] reads, anchor 100.
fn at(offset: u32) -> TextPoint {
    TextPoint::At(TextPosition {
        anchor: TextAnchor(100),
        offset,
    })
}

/// The range select then copy selects or copies: from the review cursor
/// or the marker at `start` to the other at `end`, each with the point
/// past its character.
fn through(start: u32, end: u32) -> verbatim_model::TextThrough {
    verbatim_model::TextThrough {
        start: at(start + 1),
        end: at(end + 1),
    }
}

/// "Hello w" selected, as the outpost answers select then copy's first
/// press.
fn hello_w_selected() -> TextReply {
    TextReply::Selected(vec![SelectionChange {
        selected: true,
        text: "hello w".to_owned(),
        characters: 7,
    }])
}

#[test]
fn select_then_copy_selects_from_the_start_marker_says_so_copies_and_clears_the_marker() {
    let mut text = reviewing("hello world\n", 0);
    assert_eq!(
        text.command(ReviewCommand::SelectThenCopy, 0),
        words(vec![message(Message::NoStartMarker)])
    );
    assert_eq!(
        text.command(ReviewCommand::SetStartMarker, 0),
        words(vec![message(Message::StartMarked)])
    );
    let _ = text.command(ReviewCommand::ReviewNextWord, 0);

    // The marker on "h" and the review cursor on "w", each with the point
    // just past its character, for the range from the earlier through the
    // character at the later.
    assert_eq!(
        text.command(ReviewCommand::SelectThenCopy, 0),
        [Heard::Asked(TextOp::Select {
            start: at(0),
            end: at(6),
            through: Some(through(0, 6)),
        })]
    );
    // The selection made is spoken as a selecting key's is.
    assert_eq!(
        text.reply(hello_w_selected()),
        words(vec![phrase(Phrase::Selected(SelectionText::Text(
            "hello w".to_owned()
        )))])
    );
    // The caret the selection moved takes the review cursor to the range's
    // start; the second press still copies the range selected.
    assert_eq!(
        text.selection(caret_on(line("hello world\n", 101, 0), None), None),
        []
    );
    assert_eq!(
        text.command(ReviewCommand::SelectThenCopy, 1),
        [Heard::Asked(line_read(TextPoint::Caret, None))]
    );
    assert_eq!(
        text.read(0, line("hello world\n", 101, 0)),
        [Heard::Asked(TextOp::ReadRange {
            start: at(0),
            end: at(6),
            through: Some(through(0, 6)),
        })]
    );
    assert_eq!(
        text.reply(TextReply::Range {
            text: "hello w".to_owned(),
            truncated: false,
        }),
        [Heard::Copied("hello w".to_owned())]
    );
    // The copy cleared the start marker: a further press has none.
    assert_eq!(
        text.command(ReviewCommand::SelectThenCopy, 0),
        words(vec![message(Message::NoStartMarker)])
    );
}

/// A start marker on "h" of "hello world", the review cursor moved to
/// "w", and select then copy's first press made, its selection of "hello
/// w" answered.
fn selected_from_the_marker(text: &mut Text) {
    let _ = text.command(ReviewCommand::SetStartMarker, 0);
    let _ = text.command(ReviewCommand::ReviewNextWord, 0);
    assert_eq!(
        text.command(ReviewCommand::SelectThenCopy, 0),
        [Heard::Asked(TextOp::Select {
            start: at(0),
            end: at(6),
            through: Some(through(0, 6)),
        })]
    );
    let _ = text.reply(hello_w_selected());
}

#[test]
fn a_first_press_after_a_selection_not_yet_copied_asks_for_the_second() {
    // NVDA keeps the range a first press selected: a further first press
    // says so and selects nothing, and the second press copies that range
    // (`docs/parity/review.md`, "Select then copy from the review cursor").
    let mut text = reviewing("hello world\n", 0);
    selected_from_the_marker(&mut text);
    assert_eq!(
        text.command(ReviewCommand::SelectThenCopy, 0),
        words(vec![message(Message::PressTwiceToCopy)])
    );
    assert_eq!(
        text.command(ReviewCommand::SelectThenCopy, 1),
        [Heard::Asked(TextOp::ReadRange {
            start: at(0),
            end: at(6),
            through: Some(through(0, 6)),
        })]
    );
}

#[test]
fn a_copy_keeps_a_start_marker_set_while_its_read_was_in_flight() {
    // The copy clears the marker it copied from, not one set after the
    // second press and before its read was answered
    // (`docs/parity/review.md`, "Select then copy from the review cursor").
    let mut text = reviewing("hello world\n", 0);
    selected_from_the_marker(&mut text);
    let _ = text.command(ReviewCommand::SelectThenCopy, 1);
    assert_eq!(
        text.command(ReviewCommand::SetStartMarker, 0),
        [],
        "the press waits behind the copy"
    );
    assert_eq!(
        text.reply(TextReply::Range {
            text: "hello w".to_owned(),
            truncated: false,
        }),
        [
            Heard::Copied("hello w".to_owned()),
            queued(vec![message(Message::StartMarked)])
        ]
    );
    // The new marker, on "w", is kept: a first press selects from it.
    assert_eq!(
        text.command(ReviewCommand::SelectThenCopy, 0),
        [Heard::Asked(TextOp::Select {
            start: at(6),
            end: at(6),
            through: Some(through(6, 6)),
        })]
    );
}

#[test]
fn a_copy_of_nothing_clears_the_start_marker_and_says_nothing() {
    let mut text = reviewing("hello world\n", 0);
    selected_from_the_marker(&mut text);
    let _ = text.command(ReviewCommand::SelectThenCopy, 1);
    assert_eq!(
        text.reply(TextReply::Range {
            text: String::new(),
            truncated: false,
        }),
        []
    );
    assert_eq!(
        text.command(ReviewCommand::SelectThenCopy, 0),
        words(vec![message(Message::NoStartMarker)])
    );
}

#[test]
fn a_copy_the_text_cannot_answer_clears_the_start_marker() {
    let mut text = reviewing("hello world\n", 0);
    selected_from_the_marker(&mut text);
    let _ = text.command(ReviewCommand::SelectThenCopy, 1);
    assert_eq!(
        text.reply(TextReply::NoText),
        words(vec![message(Message::NotSupported)])
    );
    assert_eq!(
        text.command(ReviewCommand::SelectThenCopy, 0),
        words(vec![message(Message::NoStartMarker)])
    );
}

#[test]
fn the_start_marker_in_another_object_is_no_start_marker_and_is_kept() {
    // NVDA says "No start marker set" with the navigator elsewhere, and
    // the marker is there again when it returns.
    let mut text = reviewing("hello world\n", 0);
    let _ = text.command(ReviewCommand::SetStartMarker, 0);
    let _ = text.focus(&node(6, Role::Button, StateSet::new()), FocusCaret::NoText);
    assert_eq!(
        text.command(ReviewCommand::SelectThenCopy, 0),
        [Heard::Sent("Properties")]
    );
    assert_eq!(
        text.properties(&node(6, Role::Button, StateSet::new())),
        words(vec![message(Message::NoStartMarker)])
    );
    let _ = text.focus(
        &node(FIELD, Role::EditableText, StateSet::new()),
        caret_read(caret_on(line("hello world\n", 100, 0), None), None, 1),
    );
    assert_eq!(
        text.command(ReviewCommand::SelectThenCopy, 0),
        [Heard::Asked(line_read(TextPoint::Caret, None))]
    );
    assert_eq!(
        text.read(0, line("hello world\n", 100, 0)),
        [Heard::Asked(TextOp::Select {
            start: at(0),
            end: at(0),
            through: Some(through(0, 0)),
        })]
    );
}

#[test]
fn moving_to_the_start_marker_in_another_object_is_no_start_marker_and_keeps_it() {
    // Verbatim+Shift+F9 with the navigator elsewhere says "No start marker
    // set" and asks for no text; back in the text, the marker on "w" is
    // there to move to, though the caret is now on "h".
    let mut text = reviewing("hello world\n", 6);
    let _ = text.command(ReviewCommand::SetStartMarker, 0);
    let _ = text.focus(&node(6, Role::Button, StateSet::new()), FocusCaret::NoText);
    assert_eq!(
        text.command(ReviewCommand::MoveToStartMarker, 0),
        [Heard::Sent("Properties")]
    );
    assert_eq!(
        text.properties(&node(6, Role::Button, StateSet::new())),
        words(vec![message(Message::NoStartMarker)])
    );
    let _ = text.focus(
        &node(FIELD, Role::EditableText, StateSet::new()),
        caret_read(caret_on(line("hello world\n", 100, 0), None), None, 1),
    );
    assert_eq!(
        text.command(ReviewCommand::MoveToStartMarker, 0),
        [Heard::Asked(line_read(TextPoint::Caret, None))]
    );
    assert_eq!(
        text.read(0, line("hello world\n", 100, 0)),
        [Heard::Asked(line_read(at(6), None))]
    );
}

#[test]
fn select_then_copy_back_from_the_marker_runs_from_the_cursor_through_the_marker() {
    // The marker on "w", the review cursor moved back to "h": the range
    // runs from the cursor through the character at the marker, both
    // ends' characters included (`docs/parity/review.md`, "Select then
    // copy from the review cursor"). The outpost, which alone can tell
    // which end comes first, is sent both ends and the points just past
    // their characters.
    let mut text = reviewing("hello world\n", 6);
    let _ = text.command(ReviewCommand::SetStartMarker, 0);
    let _ = text.command(ReviewCommand::ReviewPreviousWord, 0);
    assert_eq!(
        text.command(ReviewCommand::SelectThenCopy, 0),
        [Heard::Asked(TextOp::Select {
            start: at(6),
            end: at(0),
            through: Some(through(6, 0)),
        })]
    );
    // The text cannot be selected: said as NVDA says it, and the second
    // press copies the same range.
    assert_eq!(
        text.reply(TextReply::Unsupported),
        words(vec![message(Message::CannotSelect)])
    );
    assert_eq!(
        text.command(ReviewCommand::SelectThenCopy, 1),
        [Heard::Asked(TextOp::ReadRange {
            start: at(6),
            end: at(0),
            through: Some(through(6, 0)),
        })]
    );
}

/// The settings saved when `settings` differ from the defaults.
fn saved(settings: ReaderSettings) -> Heard {
    Heard::Settings(settings)
}

#[test]
fn the_follow_caret_toggle_is_spoken_saved_and_obeyed() {
    let mut text = reviewing("abc\n", 0);
    assert_eq!(
        text.command(ReviewCommand::ToggleFollowCaret, 0),
        [
            queued(vec![message(Message::CaretDoesNotMoveReview)]),
            saved(ReaderSettings {
                follow_caret: false,
                ..ReaderSettings::default()
            })
        ]
    );
    assert_eq!(
        text.selection(caret_on(line("xyz\n", 101, 2), None), None),
        []
    );
    assert_eq!(
        text.command(ReviewCommand::ReviewCurrentCharacter, 0),
        words(vec![character("a")]),
        "the review stayed put"
    );
    // From the default, only in edit controls, the next setting is always.
    assert_eq!(
        text.command(ReviewCommand::ToggleTypedCharacters, 0),
        [
            queued(vec![phrase(Phrase::SpeakTypedCharacters(
                verbatim_model::TypingEcho::Always
            ))]),
            saved(ReaderSettings {
                follow_caret: false,
                speak_typed_characters: verbatim_model::TypingEcho::Always,
                ..ReaderSettings::default()
            })
        ]
    );
}

#[test]
fn a_unit_the_text_does_not_have_is_not_supported() {
    let mut text = editing("abc\n", 0);
    let _ = text.command(ReviewCommand::ReviewNextPage, 0);
    assert_eq!(
        text.reply(TextReply::UnsupportedUnit(TextUnit::Page)),
        words(vec![message(Message::NotSupported)])
    );
}

#[test]
fn the_next_word_past_the_line_moves_to_the_next_line_first_word() {
    let mut text = reviewing("last\n", 0);
    let _ = text.command(ReviewCommand::ReviewNextWord, 0);
    assert_eq!(
        text.read(1, line("  next one\n", 101, 0)),
        words(vec![UtteranceSegment::text("next")])
    );
}

/// The start of the line anchored at `anchor`.
fn line_start(anchor: u64) -> TextPoint {
    TextPoint::At(TextPosition::at(TextAnchor(anchor)))
}

#[test]
fn review_bottom_reads_the_last_line_from_its_start() {
    let mut text = reviewing("first\n", 3);
    assert_eq!(
        text.command(ReviewCommand::ReviewBottom, 0),
        [Heard::Asked(line_read(TextPoint::End, None))]
    );
    assert_eq!(
        text.read(0, line("last line", 102, 9)),
        words(vec![UtteranceSegment::text("last line")])
    );
    assert_eq!(
        text.command(ReviewCommand::ReviewCurrentCharacter, 0),
        words(vec![character("l")])
    );
}

/// Page movement keeps the review cursor's column, as moving by line does
/// (`docs/parity/review.md`, "Review cursor columns"), and a page movement
/// that did not move says "Top" or "Bottom", as NVDA does, and leaves the
/// review cursor where it was. Each page here is one line, so the speech
/// is the whole page's, as NVDA's is; a page of several lines is read
/// differently from NVDA, which `docs/parity/review.md` records, and this
/// test asserts nothing about it.
#[test]
fn review_pages_keep_the_column_and_name_the_edges() {
    let mut text = reviewing("abcdef\n", 2);
    let page = |text: &mut Text, command, from: u64, count, (moved, chunk): (i32, TextChunk)| {
        assert_eq!(
            text.command(command, 0),
            [Heard::Asked(line_read(
                line_start(from),
                Some((TextUnit::Page, count))
            ))]
        );
        text.read(moved, chunk)
    };
    assert_eq!(
        page(
            &mut text,
            ReviewCommand::ReviewNextPage,
            100,
            1,
            (1, line("012345\n", 140, 0))
        ),
        words(vec![UtteranceSegment::text("012345")])
    );
    assert_eq!(
        text.command(ReviewCommand::ReviewCurrentCharacter, 0),
        words(vec![character("2")])
    );
    assert_eq!(
        page(
            &mut text,
            ReviewCommand::ReviewNextPage,
            140,
            1,
            (0, line("012345\n", 140, 0))
        ),
        words(vec![
            message(Message::Bottom),
            UtteranceSegment::text("012345")
        ])
    );
    assert_eq!(
        page(
            &mut text,
            ReviewCommand::ReviewPreviousPage,
            140,
            -1,
            (-1, line("abcdef\n", 100, 0))
        ),
        words(vec![UtteranceSegment::text("abcdef")])
    );
    assert_eq!(
        page(
            &mut text,
            ReviewCommand::ReviewPreviousPage,
            100,
            -1,
            (0, line("abcdef\n", 100, 0))
        ),
        words(vec![
            message(Message::Top),
            UtteranceSegment::text("abcdef")
        ])
    );
    assert_eq!(
        text.command(ReviewCommand::ReviewCurrentCharacter, 0),
        words(vec![character("c")])
    );
}

/// The selection's start is its first character, and its end the
/// character before the point past the selection, its last.
#[test]
fn the_review_cursor_moves_to_the_selections_first_and_last_characters() {
    let mut text = reviewing("hello world\n", 0);
    assert_eq!(
        text.command(ReviewCommand::ReviewSelectionStart, 0),
        [Heard::Asked(line_read(TextPoint::SelectionStart, None))]
    );
    assert_eq!(
        text.read(0, line("hello world\n", 100, 6)),
        words(vec![character("w")])
    );
    assert_eq!(
        text.command(ReviewCommand::ReviewSelectionEnd, 0),
        [Heard::Asked(line_read(TextPoint::SelectionEnd, None))]
    );
    assert_eq!(
        text.read(0, line("hello world\n", 100, 11)),
        words(vec![character("d")])
    );
    assert_eq!(
        text.command(ReviewCommand::ReviewCurrentCharacter, 0),
        words(vec![character("d")])
    );
}

/// Start of line moves to the line's first character and says it, from
/// the review cursor's line.
#[test]
fn start_of_line_says_the_lines_first_character() {
    let mut text = reviewing("hello world\n", 8);
    assert_eq!(
        text.command(ReviewCommand::ReviewStartOfLine, 0),
        words(vec![character("h")])
    );
    assert_eq!(
        text.command(ReviewCommand::ReviewNextCharacter, 0),
        words(vec![character("e")])
    );
}

/// Previous word reads the word before on the line; from the line's first
/// word it reads the line above and lands on its last word; on the first
/// line it says "Top" and the word again.
#[test]
fn previous_word_crosses_a_line_start_and_stops_at_the_top() {
    let mut text = reviewing("big world\n", 4);
    assert_eq!(
        text.command(ReviewCommand::ReviewPreviousWord, 0),
        words(vec![UtteranceSegment::text("big")])
    );
    assert_eq!(
        text.command(ReviewCommand::ReviewPreviousWord, 0),
        [Heard::Asked(line_read(
            line_start(100),
            Some((TextUnit::Line, -1))
        ))]
    );
    let first = TextChunk {
        first: true,
        ..line("one two\n", 90, 0)
    };
    assert_eq!(
        text.read(-1, first),
        words(vec![UtteranceSegment::text("two")])
    );
    assert_eq!(
        text.command(ReviewCommand::ReviewPreviousWord, 0),
        words(vec![UtteranceSegment::text("one")])
    );
    assert_eq!(
        text.command(ReviewCommand::ReviewPreviousWord, 0),
        words(vec![message(Message::Top), UtteranceSegment::text("one")])
    );
}

/// The review cursor's location is asked at the review cursor, not the
/// caret, and said the same way.
#[test]
fn review_location_asks_for_the_review_cursors_place_and_says_it() {
    let mut text = reviewing("hello world\n", 0);
    let _ = text.command(ReviewCommand::ReviewNextWord, 0);
    assert_eq!(
        text.command(ReviewCommand::ReportReviewLocation, 0),
        [Heard::Asked(TextOp::Location(at(6)))]
    );
    assert_eq!(
        text.reply(TextReply::Location { x: 148, y: 200 }),
        words(vec![phrase(Phrase::Positioned { x: 148, y: 200 })])
    );
}

/// Moving to a start marker in the same text reads the marker's line and
/// says the character at the marker, where the review cursor then is.
#[test]
fn moving_to_the_start_marker_in_the_same_text_says_its_character() {
    let mut text = reviewing("hello world\n", 6);
    let _ = text.command(ReviewCommand::SetStartMarker, 0);
    let _ = text.command(ReviewCommand::ReviewPreviousWord, 0);
    assert_eq!(
        text.command(ReviewCommand::MoveToStartMarker, 0),
        [Heard::Asked(line_read(at(6), None))]
    );
    assert_eq!(
        text.read(0, line("hello world\n", 100, 6)),
        words(vec![character("w")])
    );
    assert_eq!(
        text.command(ReviewCommand::ReviewNextCharacter, 0),
        words(vec![character("o")])
    );
}

/// Verbatim+3 cycles word echo from its default, off, through only in edit
/// controls and always back to off, saying each and reporting the settings
/// to save.
#[test]
fn the_typed_words_toggle_cycles_says_and_saves_each_setting() {
    let mut text = editing("abc\n", 0);
    for echo in [
        verbatim_model::TypingEcho::EditControls,
        verbatim_model::TypingEcho::Always,
        verbatim_model::TypingEcho::Off,
    ] {
        assert_eq!(
            text.command(ReviewCommand::ToggleTypedWords, 0),
            [
                queued(vec![phrase(Phrase::SpeakTypedWords(echo))]),
                saved(ReaderSettings {
                    speak_typed_words: echo,
                    ..ReaderSettings::default()
                })
            ],
            "{echo:?}"
        );
    }
}

/// Verbatim+6 pressed twice turns following the caret off and on again,
/// saying and saving each, and once on the review cursor follows the
/// caret again.
#[test]
fn the_follow_caret_toggle_turns_following_off_and_back_on() {
    let mut text = reviewing("abc\n", 0);
    assert_eq!(
        text.command(ReviewCommand::ToggleFollowCaret, 0),
        [
            queued(vec![message(Message::CaretDoesNotMoveReview)]),
            saved(ReaderSettings {
                follow_caret: false,
                ..ReaderSettings::default()
            })
        ]
    );
    assert_eq!(
        text.command(ReviewCommand::ToggleFollowCaret, 0),
        [
            queued(vec![message(Message::CaretMovesReview)]),
            saved(ReaderSettings::default())
        ]
    );
    assert_eq!(
        text.selection(caret_on(line("xyz\n", 101, 2), None), None),
        []
    );
    assert_eq!(
        text.command(ReviewCommand::ReviewCurrentCharacter, 0),
        [Heard::Asked(line_read(TextPoint::Caret, None))]
    );
    assert_eq!(
        text.read(0, line("xyz\n", 101, 2)),
        words(vec![character("z")])
    );
}

/// In plain text a line move leaves the column remembered, so a move
/// back to a longer line finds it, whatever command read the short line.
#[test]
fn a_column_remembered_across_a_short_line_comes_back_in_plain_text() {
    let mut text = reviewing("hello world\n", 7);
    let _ = text.command(ReviewCommand::ReviewNextLine, 0);
    assert_eq!(
        text.read(1, line("ok\n", 101, 0)),
        words(vec![UtteranceSegment::text("ok")])
    );
    // The line read again keeps the column past the short line's text.
    assert_eq!(
        text.command(ReviewCommand::ReviewCurrentCharacter, 0),
        words(vec![character("k")])
    );
    let _ = text.command(ReviewCommand::ReviewPreviousLine, 0);
    assert_eq!(
        text.read(-1, line("hello world\n", 100, 0)),
        words(vec![UtteranceSegment::text("hello world")])
    );
    assert_eq!(
        text.command(ReviewCommand::ReviewCurrentCharacter, 0),
        words(vec![character("o")])
    );
}

/// The document node 5 holding `value`, with `states`.
fn document_node(states: StateSet, value: &str) -> NodeSnapshot {
    NodeSnapshot {
        value: Some(value.to_owned()),
        ..node(FIELD, Role::Document, states)
    }
}

impl Text {
    /// Focuses the document node 5 holding `value`, with what the
    /// outpost's caret read found, `caret`: the segments of each utterance
    /// its announcement spoke, in order.
    fn focus_with_value(
        &mut self,
        states: StateSet,
        value: &str,
        caret: FocusCaret,
    ) -> Vec<Vec<UtteranceSegment>> {
        self.focus(&document_node(states, value), caret)
    }
}

/// The caret read of a focus whose caret is on `text`, at its start, with
/// `selection` and its `selected_text`, baseline 1 minted.
fn text_read(selection: Option<Selection>, text: &str, selected_text: Option<&str>) -> FocusCaret {
    caret_read(
        caret_on(line(text, 100, 0), selection.map(caret_selection)),
        selected_text,
        1,
    )
}

/// The focus announcement of the document `focus_with_value` focuses.
fn document() -> Vec<UtteranceSegment> {
    vec![
        UtteranceSegment::label("Body"),
        UtteranceSegment::new(SegmentContent::Role(Role::Document)),
    ]
}

#[test]
fn a_focused_document_says_its_caret_line_and_not_its_value() {
    let mut text = Text::new();
    // The line the focus came with ends its announcement, from the one
    // message.
    assert_eq!(
        text.focus_with_value(
            StateSet::new(),
            "gamma\rdelta\r",
            text_read(None, "gamma\r", None)
        ),
        vec![document(), vec![UtteranceSegment::text("gamma")]]
    );
    // A later caret tell says nothing by itself.
    assert_eq!(
        text.selection(
            caret_on(line("delta\r", 100, 0), None),
            Some(differing(&[], None))
        ),
        []
    );
}

#[test]
fn an_empty_focused_field_is_blank() {
    let mut text = Text::new();
    assert_eq!(
        text.focus_with_value(StateSet::new(), "", text_read(None, "", None)),
        vec![document(), vec![message(Message::Blank)]]
    );
}

#[test]
fn a_focused_field_with_text_selected_says_the_selection() {
    let mut text = Text::new();
    let selection = Selection {
        start: TextPosition::at(TextAnchor(101)),
        end: TextPosition::at(TextAnchor(102)),
    };
    // Nothing is asked of the outpost beyond the focus's reads.
    assert_eq!(
        text.focus_with_value(
            StateSet::new(),
            "alpha\rdelta epsilon\r",
            text_read(Some(selection), "delta epsilon\r", Some("delta epsilon"))
        ),
        vec![
            document(),
            vec![phrase(Phrase::Preselected(SelectionText::Text(
                "delta epsilon".to_owned()
            )))]
        ]
    );
}

/// The selection of "alpha" in "alpha beta\r".
fn alpha_selected() -> Selection {
    Selection {
        start: TextPosition::at(TextAnchor(100)),
        end: TextPosition {
            anchor: TextAnchor(100),
            offset: 5,
        },
    }
}

/// The caret at byte 5 of "alpha beta\r", with `selection`.
fn after_alpha(selection: Option<Selection>) -> CaretReport {
    caret_on(line("alpha beta\r", 100, 5), selection.map(caret_selection))
}

/// A focused field whose announcement has ended with its caret's line.
fn announced_field() -> Text {
    let mut text = Text::new();
    assert_eq!(
        text.focus_with_value(
            StateSet::new(),
            "alpha beta\r",
            text_read(None, "alpha beta\r", None)
        ),
        vec![document(), vec![UtteranceSegment::text("alpha beta")]]
    );
    text
}

#[test]
fn a_selection_no_key_made_says_what_was_selected() {
    let mut text = announced_field();
    assert_eq!(
        text.selection(
            after_alpha(Some(alpha_selected())),
            Some(differing(&[(true, "alpha")], Some("alpha")))
        ),
        words(vec![phrase(Phrase::Selected(SelectionText::Text(
            "alpha".to_owned()
        )))])
    );
}

#[test]
fn a_caret_report_with_no_selection_change_says_nothing() {
    let mut text = announced_field();
    assert_eq!(
        text.selection(after_alpha(None), Some(differing(&[], None))),
        []
    );
}

/// A change with no text: "x" typed over a selection, its old range in a
/// UIA text holding nothing once the text it held was cut.
#[test]
fn a_selection_change_with_no_text_says_nothing() {
    let mut text = announced_field();
    assert_eq!(
        text.selection(after_alpha(None), Some(differing(&[(false, "")], None))),
        []
    );
}

/// A classic edit control on "alpha beta\r" whose text changed: its value
/// change told, so its next caret tell's changes are said in general
/// terms.
fn changed_edit_control() -> Text {
    let mut text = edit_control("alpha beta\r", 0);
    assert_eq!(text.value_changed(FIELD), []);
    text
}

/// The changes `changes` (each selected or not, with no text), as the
/// general terms judge them, and the new selection's text `new`.
fn general(changes: &[bool], new: Option<&str>) -> SelectionDifference {
    let changes: Vec<(bool, &str)> = changes.iter().map(|&selected| (selected, "")).collect();
    differing(&changes, new)
}

#[test]
fn a_selection_unselected_after_a_text_change_with_nothing_selected_says_selection_removed() {
    let mut text = changed_edit_control();
    assert_eq!(
        text.selection(after_alpha(None), Some(general(&[false], None))),
        words(vec![message(Message::SelectionRemoved)])
    );
}

#[test]
fn a_selection_unselected_after_a_text_change_with_another_selected_says_selected_instead() {
    let mut text = changed_edit_control();
    assert_eq!(
        text.selection(
            after_alpha(Some(alpha_selected())),
            Some(general(&[false], Some("alpha")))
        ),
        words(vec![phrase(Phrase::SelectedInstead(SelectionText::Text(
            "alpha".to_owned()
        )))])
    );
}

#[test]
fn a_selection_extended_after_a_text_change_says_the_whole_new_selection_selected() {
    let mut text = changed_edit_control();
    assert_eq!(
        text.selection(
            after_alpha(Some(alpha_selected())),
            Some(general(&[true], Some("alpha")))
        ),
        words(vec![phrase(Phrase::Selected(SelectionText::Text(
            "alpha".to_owned()
        )))])
    );
}

/// A selection replaced by one apart from it after a text change: both
/// general words, each with the whole new selection, each its own
/// utterance.
#[test]
fn a_selection_replaced_after_a_text_change_says_selected_then_selected_instead() {
    let mut text = changed_edit_control();
    assert_eq!(
        text.selection(
            after_alpha(Some(alpha_selected())),
            Some(general(&[false, true], Some("alpha")))
        ),
        [
            queued(vec![phrase(Phrase::Selected(SelectionText::Text(
                "alpha".to_owned()
            )))]),
            queued(vec![phrase(Phrase::SelectedInstead(SelectionText::Text(
                "alpha".to_owned()
            )))]),
        ]
    );
}

/// A selecting key's caret tell after a text change: the whole new
/// selection, one character by its name.
#[test]
fn a_selecting_keys_answer_after_a_text_change_says_the_whole_new_selection() {
    let mut text = edit_control("hello, world\n", 5);
    assert_eq!(text.key(CaretMotion::NextCharacter, true), []);
    assert_eq!(text.value_changed(FIELD), []);
    assert_eq!(
        text.selection(
            caret_on(line("hello, world\n", 100, 6), Some(selected(5, 6))),
            Some(general(&[true], Some(",")))
        ),
        words(vec![phrase(Phrase::Selected(SelectionText::Character(
            ",".to_owned()
        )))])
    );
}

/// Control+X on a selected line in a classic edit control: the caret
/// tell says the selection's change in general terms, and the text the
/// key removed, the selection's, is not said as well, as NVDA says only
/// "selection removed".
#[test]
fn a_keys_answer_removing_the_selection_with_its_text_says_only_selection_removed() {
    let mut text = edit_control("hello world\n", 0);
    assert_eq!(text.key(CaretMotion::EndOfLine, true), []);
    let _ = text.selection(
        caret_on(line("hello world\n", 100, 0), Some(selected(0, 11))),
        Some(differing(&[(true, "hello world")], Some("hello world"))),
    );
    assert_eq!(text.press(PressAction::EditingChord), []);
    assert_eq!(text.value_changed(FIELD), []);
    assert_eq!(
        text.selection(
            caret_on(line("\n", 101, 0), None),
            Some(general(&[false], None))
        ),
        words(vec![message(Message::SelectionRemoved)])
    );
}

#[test]
fn a_selecting_keys_change_reported_while_it_waits_is_said_with_its_answer() {
    // A selecting key is passed at once; its caret tell says what it
    // selected.
    let mut text = announced_field();
    assert_eq!(text.key(CaretMotion::StartOfLine, true), []);
    assert_eq!(
        text.selection(
            caret_on(
                line("alpha beta\r", 100, 0),
                Some(caret_selection(alpha_selected()))
            ),
            Some(differing(&[(true, "alpha")], Some("alpha")))
        ),
        words(vec![phrase(Phrase::Selected(SelectionText::Text(
            "alpha".to_owned()
        )))])
    );
}

#[test]
fn a_focus_without_text_says_its_value_instead() {
    let mut text = Text::new();
    assert_eq!(
        text.focus_with_value(StateSet::new(), "plain value", FocusCaret::NoText),
        vec![document(), vec![UtteranceSegment::value("plain value")]]
    );
}

#[test]
fn a_focus_whose_text_was_not_read_says_neither_its_line_nor_its_value() {
    let mut text = Text::new();
    assert_eq!(
        text.focus_with_value(StateSet::new(), "plain value", FocusCaret::NotRead),
        vec![document()]
    );
}

#[test]
fn a_protected_field_says_its_masked_line_selection_or_value_as_any_field() {
    // The masked text the control gives is read as any field's text
    // (`docs/parity/focus-announcement.md`, "Text on focus").
    let protected = StateSet::new().with(verbatim_model::State::Protected);
    let announcement = vec![
        UtteranceSegment::label("Body"),
        UtteranceSegment::new(SegmentContent::Role(Role::Document)),
        UtteranceSegment::new(SegmentContent::State(verbatim_model::State::Protected)),
    ];
    let mut text = Text::new();
    assert_eq!(
        text.focus_with_value(protected, "******", text_read(None, "******", None)),
        vec![announcement.clone(), vec![UtteranceSegment::text("******")]]
    );
    let mut text = Text::new();
    assert_eq!(
        text.focus_with_value(
            protected,
            "******",
            text_read(Some(selection_from(0, true)), "******", Some("******"))
        ),
        vec![
            announcement.clone(),
            vec![phrase(Phrase::Preselected(SelectionText::Text(
                "******".to_owned()
            )))]
        ]
    );
    // With no text to read, its value is spoken as any field's.
    let mut text = Text::new();
    assert_eq!(
        text.focus_with_value(protected, "******", FocusCaret::NoText),
        vec![announcement, vec![UtteranceSegment::value("******")]]
    );
}

/// The caret read of a focus whose caret is at the start of a line minted
/// at `anchor`, with nothing selected, and baseline `baseline` minted.
fn caret_at_anchor(anchor: u64, baseline: u64) -> FocusCaret {
    caret_read(caret_on(line("gamma\n", anchor, 0), None), None, baseline)
}

/// The text anchors the state holds, by outpost.
fn anchors_held(text: &Text) -> Vec<(verbatim_model::OutpostId, Vec<TextAnchor>)> {
    text.desk
        .state
        .held_anchors()
        .into_iter()
        .map(|(outpost, anchors)| (outpost, anchors.into_iter().collect()))
        .collect()
}

#[test]
fn held_anchors_name_every_position_the_state_keeps() {
    let mut text = editing("abc\n", 1);
    let _ = text.command(ReviewCommand::SetStartMarker, 0);
    // The selection baseline, and the caret and the start marker both on
    // the line at anchor 100, in the one edit field.
    let outpost = verbatim_model::OutpostId(ONE.generation);
    assert_eq!(
        anchors_held(&text),
        [(outpost, vec![TextAnchor(1), TextAnchor(100)])]
    );
    let nodes: Vec<(verbatim_model::OutpostId, Vec<NodeId>)> = text
        .desk
        .state
        .held_nodes()
        .into_iter()
        .map(|(outpost, nodes)| (outpost, nodes.into_iter().collect()))
        .collect();
    assert_eq!(nodes, [(outpost, vec![id(FIELD)])]);
}

impl Text {
    /// [`ONE`]'s outpost ended, and its replacement started and answered
    /// its focus read with `snapshot`, in its own generation, and `caret`:
    /// every utterance spoken.
    fn replaced(
        &mut self,
        snapshot: &NodeSnapshot,
        caret: FocusCaret,
    ) -> Vec<Vec<UtteranceSegment>> {
        let next = ONE.replaced();
        self.desk.app(next);
        let _ = self.desk.step(QueueInput::ReaderEnded(ONE.instance()));
        let [(number, to, Wire::CurrentFocus { .. })] = self.desk.sent[..] else {
            panic!("the replacement asked for the focus: {:?}", self.desk.sent);
        };
        let read = FocusRead {
            caret,
            ..desk::focus_read_of(next, snapshot, &[])
        };
        self.desk
            .answer(
                to,
                number,
                Outcome::Result(Reply::Candidate(
                    verbatim_model::request::CandidateAnswer::focused(read),
                )),
            )
            .into_iter()
            .filter_map(|heard| match heard {
                desk::Heard::Say(_, segments) => Some(segments),
                _ => None,
            })
            .collect()
    }
}

#[test]
fn a_replacement_outposts_silent_re_read_takes_its_caret_without_speaking() {
    let mut text = Text::new();
    let _ = text.focus_with_value(StateSet::new(), "gamma\n", caret_at_anchor(100, 1));
    // The replacement numbers its nodes and anchors afresh and reports the
    // focus the user already heard: nothing is said, and its caret is
    // Core's.
    let next = ONE.replaced();
    let snapshot = NodeSnapshot {
        id: next.node(FIELD),
        ..document_node(StateSet::new(), "gamma\n")
    };
    assert_eq!(
        (
            text.replaced(&snapshot, caret_at_anchor(200, 2)),
            anchors_held(&text)
        ),
        (
            Vec::new(),
            vec![(
                verbatim_model::OutpostId(next.generation),
                vec![TextAnchor(2), TextAnchor(200)]
            )]
        )
    );
}

/// `chunk` with formatting: each stretch a byte range and its attributes.
fn formatted(
    mut chunk: TextChunk,
    stretches: Vec<(u32, u32, verbatim_model::TextAttributes)>,
) -> TextChunk {
    chunk.formats = stretches
        .into_iter()
        .map(|(start, end, attributes)| verbatim_model::FormatRun {
            start,
            end,
            attributes,
        })
        .collect();
    chunk
}

/// `chunk` with formatting: each stretch a byte range, a spelling error or
/// not.
fn with_errors(chunk: TextChunk, stretches: &[(u32, u32, bool)]) -> TextChunk {
    formatted(
        chunk,
        stretches
            .iter()
            .map(|&(start, end, spelling_error)| {
                let attributes = verbatim_model::TextAttributes {
                    spelling_error,
                    ..verbatim_model::TextAttributes::default()
                };
                (start, end, attributes)
            })
            .collect(),
    )
}

fn format(format: verbatim_model::TextFormat) -> UtteranceSegment {
    UtteranceSegment::new(SegmentContent::Format(format))
}

use verbatim_model::TextFormat;

#[test]
fn a_line_speaks_a_spelling_error_where_it_starts() {
    let mut text = editing("first\n", 0);
    let error_line = with_errors(
        line("hello wrold there\n", 101, 0),
        &[(0, 6, false), (6, 11, true), (11, 18, false)],
    );
    let said = words(vec![
        UtteranceSegment::text("hello"),
        format(TextFormat::SpellingError),
        UtteranceSegment::text("wrold there"),
    ]);
    assert_eq!(
        text.moved(CaretMotion::NextLine, error_line.clone(), None),
        said
    );
    // Leaving the error inside a line says nothing, and reading the same
    // line again starts out of the error, as the last stretch left it.
    assert_eq!(text.moved(CaretMotion::NextLine, error_line, None), said);
}

#[test]
fn a_line_joins_its_stretches_as_the_text_has_them() {
    let mut text = editing("first\n", 0);
    let error_line = with_errors(
        line("Ths line has a tset.\n", 101, 0),
        &[
            (0, 3, true),
            (3, 15, false),
            (15, 19, true),
            (19, 21, false),
        ],
    );
    // Only a spoken change parts the text; the dot after the error follows
    // it with no space, as NVDA says the line.
    assert_eq!(
        text.moved(CaretMotion::NextLine, error_line, None),
        words(vec![
            format(TextFormat::SpellingError),
            UtteranceSegment::text("Ths line has a"),
            format(TextFormat::SpellingError),
            UtteranceSegment::text("tset."),
        ])
    );
}

#[test]
fn characters_report_entering_and_leaving_an_error() {
    let text_of = "ab cd\n";
    let mut text = editing(text_of, 0);
    let letter = |text: &str, error: bool| {
        let length = u32::try_from(text.len()).expect("short");
        with_errors(unit(TextUnit::Character, text, 200), &[(0, length, error)])
    };
    let at = |offset| line(text_of, 100, offset);
    assert_eq!(
        text.moved(CaretMotion::NextCharacter, at(1), Some(letter("b", true))),
        words(vec![format(TextFormat::SpellingError), character("b")])
    );
    // Within the error, the character alone.
    assert_eq!(
        text.moved(
            CaretMotion::PreviousCharacter,
            at(0),
            Some(letter("a", true))
        ),
        words(vec![character("a")])
    );
    // Out of it, said, as a character is extra detail.
    assert_eq!(
        text.moved(CaretMotion::NextCharacter, at(2), Some(letter(" ", false))),
        words(vec![format(TextFormat::NotSpellingError), character(" ")])
    );
}

#[test]
fn a_word_carries_the_change_its_trailing_space_makes() {
    let mut text = editing("tset of it\n", 0);
    let word = |text: &str, stretches: &[(u32, u32, bool)]| {
        with_errors(unit(TextUnit::Word, text, 200), stretches)
    };
    let at = |offset| line("tset of it\n", 100, offset);
    // The misspelt word, then its space, out of the error.
    assert_eq!(
        text.moved(
            CaretMotion::NextWord,
            at(0),
            Some(word("tset ", &[(0, 4, true), (4, 5, false)]))
        ),
        words(vec![
            format(TextFormat::SpellingError),
            UtteranceSegment::text("tset"),
            format(TextFormat::NotSpellingError),
        ])
    );
    // The next word, already out of it, says only itself.
    assert_eq!(
        text.moved(
            CaretMotion::NextWord,
            at(5),
            Some(word("of ", &[(0, 3, false)]))
        ),
        words(vec![UtteranceSegment::text("of")])
    );
}

#[test]
fn a_focus_reports_the_formatting_at_its_line_start_afresh() {
    let mut text = Text::new();
    let error_line = with_errors(line("Ths is\r", 100, 0), &[(0, 3, true), (3, 7, false)]);
    assert_eq!(
        text.focus_with_value(
            StateSet::new(),
            "Ths is\r",
            caret_read(caret_on(error_line, None), None, 1)
        ),
        vec![
            document(),
            vec![
                format(TextFormat::SpellingError),
                UtteranceSegment::text("Ths is"),
            ]
        ]
    );
}

#[test]
fn bold_starts_and_ends_and_a_font_change_is_named() {
    let mut text = editing("first\n", 0);
    let calibri = |bold| verbatim_model::TextAttributes {
        bold: Some(bold),
        font_name: Some("Calibri".to_owned()),
        ..verbatim_model::TextAttributes::default()
    };
    let bold_line = formatted(
        line("plain bold\n", 101, 0),
        vec![(0, 6, calibri(false)), (6, 11, calibri(true))],
    );
    assert_eq!(
        text.moved(CaretMotion::NextLine, bold_line, None),
        words(vec![
            format(TextFormat::FontName("Calibri".to_owned())),
            UtteranceSegment::text("plain"),
            format(TextFormat::Bold),
            UtteranceSegment::text("bold"),
        ])
    );
    let next = formatted(line("next\n", 102, 0), vec![(0, 5, calibri(false))]);
    assert_eq!(
        text.moved(CaretMotion::NextLine, next, None),
        words(vec![
            format(TextFormat::NotBold),
            UtteranceSegment::text("next")
        ])
    );
}

/// `text` as a line all in bold.
fn bold_line(text: &str, anchor: u64) -> TextChunk {
    let end = u32::try_from(text.len()).expect("short");
    let bold = verbatim_model::TextAttributes {
        bold: Some(true),
        ..verbatim_model::TextAttributes::default()
    };
    formatted(line(text, anchor, 0), vec![(0, end, bold)])
}

/// The formatting is kept on the focus object, so the silent re-take
/// after an outpost is replaced, a new focus object, starts afresh and the
/// next line says its bold again (Dickson, 2026-11-08).
#[test]
fn the_silent_re_take_after_a_replacement_reports_formatting_afresh() {
    let mut text = editing("first\n", 0);
    let bold = |said: &str| words(vec![format(TextFormat::Bold), UtteranceSegment::text(said)]);
    assert_eq!(
        text.moved(CaretMotion::NextLine, bold_line("first bold\n", 101), None),
        bold("first bold")
    );
    let snapshot = NodeSnapshot {
        id: ONE.replaced().node(FIELD),
        ..node(FIELD, Role::EditableText, StateSet::new())
    };
    let caret = caret_on(bold_line("first bold\n", 200), None);
    assert_eq!(
        text.replaced(&snapshot, caret_read(caret.clone(), None, 2)),
        Vec::<Vec<UtteranceSegment>>::new()
    );
    // The replacement answers the key.
    text.caret = caret;
    assert_eq!(
        text.moved(CaretMotion::NextLine, bold_line("second line\n", 201), None),
        bold("second line")
    );
}

/// A heading's size and a list item's bullet: the key's words alone.
#[test]
fn a_heading_says_its_size_and_a_list_item_its_bullet_before_its_text() {
    let mut text = editing("first\n", 0);
    let size = |points: &str| verbatim_model::TextAttributes {
        font_size: Some(points.to_owned()),
        ..verbatim_model::TextAttributes::default()
    };
    let heading = formatted(
        line("Title\n", 101, 0),
        vec![(0, 5, size("28.0 pt")), (5, 6, size("11.0 pt"))],
    );
    assert_eq!(
        text.moved(CaretMotion::NextLine, heading, None),
        words(vec![
            format(TextFormat::FontSize("28.0 pt".to_owned())),
            UtteranceSegment::text("Title"),
        ]),
        "the line break, not part of the line's content, changes nothing"
    );
    let bulleted = verbatim_model::TextAttributes {
        bullet: Some(verbatim_model::BulletStyle::FilledRound),
        ..size("11.0 pt")
    };
    let item = || formatted(line("item\n", 102, 0), vec![(0, 5, bulleted.clone())]);
    assert_eq!(
        text.moved(CaretMotion::NextLine, item(), None),
        words(vec![
            format(TextFormat::FontSize("11.0 pt".to_owned())),
            format(TextFormat::Bullet(verbatim_model::BulletStyle::FilledRound)),
            UtteranceSegment::text("item"),
        ]),
        "the bullet after the changes"
    );
    // Every line read says its bullet, which is no change of formatting.
    assert_eq!(
        text.moved(CaretMotion::NextLine, item(), None),
        words(vec![
            format(TextFormat::Bullet(verbatim_model::BulletStyle::FilledRound)),
            UtteranceSegment::text("item"),
        ])
    );
    // A word says none.
    let word = formatted(
        unit(TextUnit::Word, "item", 201),
        vec![(0, 4, bulleted.clone())],
    );
    assert_eq!(
        text.moved(CaretMotion::NextWord, item(), Some(word)),
        words(vec![UtteranceSegment::text("item")])
    );
    // A line out of the list says nothing of it.
    let plain = formatted(
        line("after\n", 103, 0),
        vec![(
            0,
            6,
            verbatim_model::TextAttributes {
                bullet: Some(verbatim_model::BulletStyle::None),
                ..size("11.0 pt")
            },
        )],
    );
    assert_eq!(
        text.moved(CaretMotion::NextLine, plain, None),
        words(vec![UtteranceSegment::text("after")])
    );
}

#[test]
fn a_background_color_is_named_alone_or_after_the_color() {
    let mut text = editing("first\n", 0);
    let colored = |color: &str, background: &str| verbatim_model::TextAttributes {
        color: Some(color.to_owned()),
        background_color: Some(background.to_owned()),
        ..verbatim_model::TextAttributes::default()
    };
    let colored_line = formatted(
        line("one two three four\n", 101, 0),
        vec![
            (0, 4, colored("black", "white")),
            (4, 8, colored("dark red", "light grey")),
            (8, 14, colored("dark red", "white")),
            (14, 19, colored("black", "white")),
        ],
    );
    assert_eq!(
        text.moved(CaretMotion::NextLine, colored_line, None),
        words(vec![
            format(TextFormat::Color("black".to_owned())),
            format(TextFormat::OnBackgroundColor("white".to_owned())),
            UtteranceSegment::text("one"),
            format(TextFormat::Color("dark red".to_owned())),
            format(TextFormat::OnBackgroundColor("light grey".to_owned())),
            UtteranceSegment::text("two"),
            format(TextFormat::BackgroundColor("white".to_owned())),
            UtteranceSegment::text("three"),
            format(TextFormat::Color("black".to_owned())),
            UtteranceSegment::text("four"),
        ])
    );
}

#[test]
fn strikethrough_and_the_kind_of_underline_start_change_and_end() {
    use verbatim_model::LineStyle;
    let mut text = editing("first\n", 0);
    let drawn = |strikethrough, underline_style| verbatim_model::TextAttributes {
        strikethrough: Some(strikethrough),
        underline_style: Some(underline_style),
        // Read for the font attributes too; the kind is reported instead.
        underline: Some(underline_style != LineStyle::None),
        ..verbatim_model::TextAttributes::default()
    };
    let drawn_line = formatted(
        line("a b c d e\n", 101, 0),
        vec![
            (0, 2, drawn(LineStyle::None, LineStyle::None)),
            (2, 4, drawn(LineStyle::Single, LineStyle::Double)),
            (4, 6, drawn(LineStyle::Double, LineStyle::Wavy)),
            (6, 8, drawn(LineStyle::Double, LineStyle::Single)),
            (8, 10, drawn(LineStyle::None, LineStyle::None)),
        ],
    );
    assert_eq!(
        text.moved(CaretMotion::NextLine, drawn_line, None),
        words(vec![
            UtteranceSegment::text("a"),
            format(TextFormat::Strikethrough(LineStyle::Single)),
            format(TextFormat::UnderlineStyle(LineStyle::Double)),
            UtteranceSegment::text("b"),
            format(TextFormat::Strikethrough(LineStyle::Double)),
            format(TextFormat::UnderlineStyle(LineStyle::Wavy)),
            UtteranceSegment::text("c"),
            format(TextFormat::UnderlineStyle(LineStyle::Single)),
            UtteranceSegment::text("d"),
            format(TextFormat::Strikethrough(LineStyle::None)),
            format(TextFormat::UnderlineStyle(LineStyle::None)),
            UtteranceSegment::text("e"),
        ])
    );
}

#[test]
fn a_link_is_said_where_it_starts_and_where_it_ends() {
    let mut text = editing("first\n", 0);
    let linked = |link| verbatim_model::TextAttributes {
        link,
        ..verbatim_model::TextAttributes::default()
    };
    let linked_line = formatted(
        line("see the page here\n", 101, 0),
        vec![
            (0, 8, linked(false)),
            (8, 12, linked(true)),
            (12, 18, linked(false)),
        ],
    );
    assert_eq!(
        text.moved(CaretMotion::NextLine, linked_line, None),
        words(vec![
            UtteranceSegment::text("see the"),
            format(TextFormat::Link),
            UtteranceSegment::text("page"),
            format(TextFormat::NotLink),
            UtteranceSegment::text("here"),
        ])
    );
}

#[test]
fn a_focus_moving_while_the_before_read_runs_leaves_the_key_silent() {
    // A focus pending while the key's starting read is out withdraws the
    // read; the key is sent and says nothing, and the focus is taken.
    let mut text = editing("abc\n", 0);
    assert_eq!(
        text.key(CaretMotion::NextCharacter, false),
        [Heard::Asked(starting_read(
            CaretMotion::NextCharacter,
            TextProvider::Uia
        ))]
    );
    let focused = text
        .desk
        .focus(ONE, &node(6, Role::Button, StateSet::new()), &[]);
    assert_eq!(
        focused,
        desk::focus_heard(
            ONE,
            desk::focus_now_in(ONE, 6, &[]),
            vec![vec![
                UtteranceSegment::label("Body"),
                UtteranceSegment::new(SegmentContent::Role(Role::Button)),
            ]]
        )
    );
}

/// A chord in a text field says nothing for the line it changes, and a
/// paste's caret tell, with no change of selection, says nothing either
/// (`docs/design/caret-selection-rework.md`).
#[test]
fn a_paste_says_nothing_for_the_line_it_changes() {
    let mut text = editing("Nam\n", 3);
    assert_eq!(text.press(PressAction::EditingChord), []);
    assert_eq!(text.value_changed(FIELD), []);
    assert_eq!(
        text.selection(
            caret_on(line("Name\n", 100, 4), None),
            Some(differing(&[], None))
        ),
        []
    );
}

#[test]
fn speech_is_stamped_with_the_number_of_the_key_on_whose_behalf_it_is_spoken() {
    // A held Backspace, key 1, and "c" typed as key 2 behind it: the echo
    // is spoken at the typed key's turn, after Backspace's starting read,
    // and carries the letter's number.
    let mut text = editing("ab", 2);
    assert_eq!(
        text.key(CaretMotion::Backspace, false),
        [Heard::Asked(starting_read(
            CaretMotion::Backspace,
            TextProvider::Uia
        ))]
    );
    assert_eq!(text.typed("c"), []);
    let before = line("ab", 101, 2);
    let at = unit_at(&before, TextUnit::Character);
    assert_eq!(
        text.reply(started(before, None, at)),
        [Heard::Strokes(1), queued(vec![character("c")])]
    );
    assert_eq!(text.stamps, [Some(KeySeq::new(2))]);
}

#[test]
fn speech_of_an_unkeyed_input_or_a_forgotten_key_carries_no_number() {
    let mut text = editing("ab", 2);
    let copied = |text: &mut Text, trace_id| {
        let effects = verbatim_core::reduce(
            &mut text.desk.state,
            &mut text.desk.runtime,
            &Input::ClipboardCopied {
                trace_id,
                text: "ab".to_owned(),
                copied: true,
            },
        );
        text.stamps.clear();
        let _ = text.take(effects);
        text.stamps.clone()
    };
    // Typed "c" as key 1, its press traced `first`.
    let first = TraceId::from_raw(28);
    let typed = |text: &mut Text, trace_id, key| {
        let press = Press {
            key_seq: KeySeq::new(key),
            pressed_at_us: text.desk.at_us,
            action: PressAction::Typed {
                text: "c".to_owned(),
            },
        };
        let effects = verbatim_core::reduce(
            &mut text.desk.state,
            &mut text.desk.runtime,
            &Input::Queue {
                trace_id,
                at_us: text.desk.at_us,
                input: QueueInput::Press(Box::new(press)),
            },
        );
        let _ = text.take(effects);
    };
    typed(&mut text, first, 1);
    assert_eq!(copied(&mut text, first), [Some(KeySeq::new(1))]);
    assert_eq!(copied(&mut text, TraceId::from_raw(29)), [None]);
    // 1,024 newer keys push the first one's number out.
    for key in 2..=1_025 {
        typed(&mut text, TraceId::from_raw(30), key);
    }
    assert_eq!(copied(&mut text, first), [None]);
}

/// A console host's terminal, 20 cells wide and 4 rows high, showing
/// `text`, its caret then placed by `caret`, an escape sequence, as Core
/// reads it through its terminal model (`terminal_support`).
fn terminal(text: &str, caret: &str) -> terminal_support::Session {
    let mut outpost =
        terminal_support::Outpost::new(verbatim_term_sim::Flavor::ConsoleHost, 20, 4, 100);
    outpost.sim.write(text.as_bytes());
    outpost.sim.write(caret.as_bytes());
    terminal_support::Session::with(outpost)
}

/// Review command `command`, its first press, as the terminal session
/// takes it.
fn terminal_command(command: ReviewCommand) -> Input {
    Input::Command {
        pressed_at_ms: 0,
        key_seq: KeySeq::NONE_YET,
        trace_id: TraceId::from_raw(1),
        command,
        repeat: 0,
    }
}

/// Heard in a terminal with no key: each utterance's words.
fn terminal_heard(words: &[&str]) -> Vec<terminal_support::Said> {
    words
        .iter()
        .map(|words| terminal_support::said(None, words))
        .collect()
}

/// A terminal row's line ends at its last character: padding is never
/// reviewed (`books/dev/src/explanation/design/terminal-model.md`, "Features and settings"), so moving on
/// from the last character is at the edge, padded or not.
#[test]
fn a_terminal_rows_line_ends_at_its_last_character() {
    for row in ["ab", "ab        "] {
        // The caret on "b".
        let mut session = terminal(row, "\x1b[1;2H");
        session.input(&terminal_command(ReviewCommand::ReviewNextCharacter));
        assert_eq!(
            session.heard(),
            terminal_heard(&["Message(Right) b"]),
            "{row:?}"
        );
    }
}

/// In a terminal, bottom reads the review cursor's row again, as every
/// review command there does (`books/dev/src/explanation/design/terminal-model.md`, "The
/// review cursor"), then the text's last row.
#[test]
fn review_bottom_in_a_terminal_reads_the_last_row() {
    let mut session = terminal("$ ls\r\ndone", "\x1b[1;3H");
    session.input(&terminal_command(ReviewCommand::ReviewBottom));
    assert_eq!(session.heard(), terminal_heard(&["done"]));
}

/// In a terminal, start of line reads the review cursor's row again first
/// (`books/dev/src/explanation/design/terminal-model.md`, "The review cursor"), then says the
/// row's first cell.
#[test]
fn start_of_line_in_a_terminal_says_the_rows_first_cell() {
    let mut session = terminal("ab cd", "\x1b[1;4H");
    session.input(&terminal_command(ReviewCommand::ReviewStartOfLine));
    assert_eq!(session.heard(), terminal_heard(&["a"]));
}

#[test]
fn a_terminals_typing_waits_for_its_reading_rules_to_show_it() {
    // Its echo comes from the reading rules, once the terminal shows it
    // (`tests/terminal_reading.rs`); typing alone says nothing, nor does a
    // change that shows nothing new.
    let mut session = terminal("$ ", "");
    let typed = |text: &str| Input::CharacterTyped {
        key_seq: KeySeq::NONE_YET,
        trace_id: TraceId::from_raw(8),
        text: text.to_owned(),
        at_ms: 0,
    };
    session.input(&typed("l"));
    session.live(10);
    assert_eq!(session.heard(), []);

    // With terminal passwords spoken, typing is echoed at once.
    session.input(&Input::Settings(ReaderSettings {
        speak_terminal_passwords: true,
        ..ReaderSettings::default()
    }));
    session.input(&typed("x"));
    assert_eq!(
        session.heard(),
        [terminal_support::said(Some(TraceId::from_raw(8)), "x")]
    );
}

/// One part of a say-all utterance: an index mark, or text.
#[derive(Clone, Debug, PartialEq)]
enum Said {
    Mark(SpeechMark),
    Text(String),
}

/// Say-all's utterances among `heard`, each as its marks and texts in
/// order, checked to open with a mark, and every other effect, in order.
/// Any other part of say-all's speech fails the test, so none is skipped
/// unchecked.
fn said_all(heard: Vec<Heard>) -> (Vec<Vec<Said>>, Vec<Heard>) {
    let mut said = Vec::new();
    let mut rest = Vec::new();
    for heard in heard {
        let Heard::SayAll(segments) = heard else {
            rest.push(heard);
            continue;
        };
        let parts: Vec<Said> = segments
            .into_iter()
            .map(|segment| match segment.content {
                SegmentContent::Mark(mark) => Said::Mark(mark),
                SegmentContent::Text(text) => Said::Text(text),
                other => panic!("say-all speech is marks and text: {other:?}"),
            })
            .collect();
        assert!(
            matches!(parts.first(), Some(Said::Mark(_))),
            "an utterance opens with a mark: {parts:?}"
        );
        said.push(parts);
    }
    (said, rest)
}

/// The texts of each utterance, without their marks.
fn texts_of(said: &[Vec<Said>]) -> Vec<Vec<&str>> {
    said.iter()
        .map(|parts| {
            parts
                .iter()
                .filter_map(|part| match part {
                    Said::Text(text) => Some(text.as_str()),
                    Said::Mark(_) => None,
                })
                .collect()
        })
        .collect()
}

/// The marks of an utterance, in order.
fn marks_of(said: &[Said]) -> Vec<SpeechMark> {
    said.iter()
        .filter_map(|part| match part {
            Said::Mark(mark) => Some(*mark),
            Said::Text(_) => None,
        })
        .collect()
}

/// Say-all's read ahead from `at`, after `movement`, by `unit`.
fn read_ahead(at: TextPoint, movement: Option<(TextUnit, i32)>, unit: TextUnit) -> TextOp {
    TextOp::ReadAhead(verbatim_model::TextReadAhead {
        at,
        movement: movement.map(|(unit, count)| verbatim_model::TextMovement { unit, count }),
        unit,
        count: 20,
    })
}

/// The caret moved to the start of the line at `anchor`.
fn caret_to(anchor: u64) -> Heard {
    Heard::Asked(TextOp::MoveCaret(line_start(anchor)))
}

/// Say-all's first read, from the caret by sentence.
fn by_sentence_from_caret() -> Heard {
    Heard::Asked(read_ahead(TextPoint::Caret, None, TextUnit::Sentence))
}

impl Text {
    /// Say-all from the caret, its first read answered that there are no
    /// sentences and its read by line answered with `lines`: that answer's
    /// say-all utterances and other effects.
    fn by_line(&mut self, lines: Vec<TextChunk>) -> (Vec<Vec<Said>>, Vec<Heard>) {
        assert_eq!(
            self.command(ReviewCommand::SayAllFromCaret, 0),
            [Heard::Display(true), by_sentence_from_caret()]
        );
        assert_eq!(
            self.reply(TextReply::UnsupportedUnit(TextUnit::Sentence)),
            [Heard::Asked(read_ahead(
                TextPoint::Caret,
                None,
                TextUnit::Line
            ))]
        );
        said_all(self.chunks(lines))
    }

    /// Answers the one request out with `chunks`, moved by none.
    fn chunks(&mut self, chunks: Vec<TextChunk>) -> Vec<Heard> {
        self.reply(TextReply::Chunks { moved: 0, chunks })
    }

    /// Playback reached `mark`: say-all's utterances and other effects.
    fn mark(&mut self, mark: SpeechMark) -> (Vec<Vec<Said>>, Vec<Heard>) {
        said_all(self.step(QueueInput::MarkReached(mark)))
    }

    /// Answers the caret move out, minting the next baseline: every
    /// effect.
    fn placed(&mut self) -> Vec<Heard> {
        self.baseline += 1;
        let read_at_us = self.desk.at_us;
        self.reply(TextReply::CaretPlaced {
            read_at_us,
            baseline: TextAnchor(self.baseline),
        })
    }

    /// The speech side's report `input`, handled at once.
    fn speech(&mut self, input: QueueInput) -> Vec<Heard> {
        self.step(input)
    }

    /// A key that joins no queue, such as a modifier, pressed next and
    /// cutting speech, handled at once.
    fn cut(&mut self) -> Vec<Heard> {
        self.desk.keys += 1;
        let key = KeySeq::new(self.desk.keys);
        self.step(QueueInput::SpeechCut { key })
    }
}

/// Nothing said and nothing else done.
const NOTHING: (Vec<Vec<Said>>, Vec<Heard>) = (Vec::new(), Vec::new());

#[test]
fn say_all_reads_by_line_where_there_are_no_sentences_and_moves_the_caret() {
    let mut text = editing("First.\n", 0);
    let (said, rest) = text.by_line(vec![line("First.\n", 100, 0)]);
    assert_eq!(texts_of(&said), [["First."]]);
    // Little is left to speak, so the next batch is read at once, a line on
    // from the last line read.
    assert_eq!(
        rest,
        [Heard::Asked(read_ahead(
            line_start(100),
            Some((TextUnit::Line, 1)),
            TextUnit::Line
        ))]
    );
    // Playback reaches the first line while that read is out: its entry
    // waits its turn.
    assert_eq!(text.mark(marks_of(&said[0])[0]), NOTHING);
    // The document ends: at the mark's turn the caret moves to the line,
    // say-all is over, and the display may sleep.
    assert_eq!(
        text.chunks(vec![line("First.\n", 100, 0)]),
        [Heard::Display(false), caret_to(100)]
    );
}

#[test]
fn say_all_hands_out_a_batch_a_piece_at_a_time_and_ends_after_the_last() {
    let mut text = editing("x\n", 0);
    let last = TextChunk {
        last: true,
        ..line("Three.", 102, 0)
    };
    let (said, rest) = text.by_line(vec![line("One.\n", 100, 0), line("Two.\n", 101, 0), last]);
    // Two pieces with speech, the third waiting; the text's end was read,
    // so nothing more is asked for.
    assert_eq!(texts_of(&said), [["One."], ["Two."]]);
    assert_eq!(rest, []);
    // Reaching the first moves the caret there and hands on the third,
    // with its own mark.
    let (third, rest) = text.mark(marks_of(&said[0])[0]);
    assert_eq!(
        (texts_of(&third), rest),
        (vec![vec!["Three."]], vec![caret_to(100)])
    );
    assert_eq!(text.placed(), []);
    // Reaching the second moves the caret on.
    assert_eq!(
        text.mark(marks_of(&said[1])[0]),
        (Vec::new(), vec![caret_to(101)])
    );
    assert_eq!(text.placed(), []);
    // The last piece reached: the caret moves to it and say-all ends.
    assert_eq!(
        text.mark(marks_of(&third[0])[0]),
        (Vec::new(), vec![Heard::Display(false), caret_to(102)])
    );
}

#[test]
fn say_all_reads_the_next_batch_when_fewer_than_ten_pieces_are_left() {
    let mut text = editing("x\n", 0);
    // Twelve lines to speak: two with speech, ten buffered, which is not
    // yet fewer than ten.
    let lines: Vec<TextChunk> = (0..12)
        .map(|index| line("A b.\n", 100 + index, 0))
        .collect();
    let (said, rest) = text.by_line(lines);
    assert_eq!(rest, []);
    let mut marks: Vec<SpeechMark> = said.iter().map(|parts| marks_of(parts)[0]).collect();
    // Each mark reached moves the caret, holding the queue until the move
    // is answered, and hands on the next line; a read it wants joins
    // behind it.
    let mut reach = |text: &mut Text, index: usize| {
        let (said, rest) = text.mark(marks[index]);
        marks.extend(said.iter().map(|parts| marks_of(parts)[0]));
        assert_eq!(rest, [caret_to(100 + u64::try_from(index).expect("small"))]);
        text.placed()
    };
    // Each line reached leaves one fewer: eleven, then ten, then nine, when
    // the next batch is read, once.
    assert_eq!(reach(&mut text, 0), []);
    assert_eq!(reach(&mut text, 1), []);
    assert_eq!(
        reach(&mut text, 2),
        [Heard::Asked(read_ahead(
            line_start(111),
            Some((TextUnit::Line, 1)),
            TextUnit::Line
        ))]
    );
}

#[test]
fn say_all_splits_a_paragraph_into_sentences_and_stops_on_a_key() {
    let mut text = editing("x\n", 0);
    assert_eq!(
        text.command(ReviewCommand::SayAllFromCaret, 0),
        [Heard::Display(true), by_sentence_from_caret()]
    );
    let paragraph = TextChunk {
        unit: TextUnit::Paragraph,
        ..line("One. Two? Three!\r\n", 400, 0)
    };
    // Two sentences with speech, the third waiting; so little is left that
    // the next paragraph is read at once, a paragraph on.
    let (said, rest) = said_all(text.chunks(vec![paragraph]));
    assert_eq!(texts_of(&said), [["One. "], ["Two? "]]);
    assert_eq!(
        rest,
        [Heard::Asked(read_ahead(
            line_start(400),
            Some((TextUnit::Paragraph, 1)),
            TextUnit::Sentence
        ))]
    );
    assert_eq!(said_all(text.chunks(Vec::new())), NOTHING);
    // Reaching the second sentence moves the caret there and hands on the
    // third.
    let (third, rest) = text.mark(marks_of(&said[1])[0]);
    assert_eq!(texts_of(&third), [["Three!"]]);
    assert_eq!(
        rest,
        [Heard::Asked(TextOp::MoveCaret(TextPoint::At(
            TextPosition {
                anchor: TextAnchor(400),
                offset: 5
            }
        )))]
    );
    assert_eq!(text.placed(), []);
    // A key's cut stops it, dropping what was waiting; a late mark is
    // ignored.
    assert_eq!(text.cut(), [Heard::Display(false)]);
    assert_eq!(text.mark(marks_of(&third[0])[0]), NOTHING);
}

/// Speech the reducer did not cut off itself cuts say-all's utterances
/// off: once the pipeline reports them, say-all stops, as on a key, rather
/// than waiting for marks playback will never reach.
#[test]
fn say_all_stops_when_its_utterance_is_cut_off_elsewhere() {
    let mut text = editing("x\n", 0);
    let _ = text.command(ReviewCommand::SayAllFromCaret, 0);
    let (said, _) = said_all(text.chunks(vec![line("One.\n", 100, 0), line("Two.\n", 101, 0)]));
    assert_eq!(texts_of(&said), [["One."], ["Two."]]);
    let cut = |marks| QueueInput::UtteranceCancelled { marks, at_ms: 0 };
    // An utterance with no mark of say-all's changes nothing.
    assert_eq!(text.speech(cut(vec![SpeechMark(9_999)])), []);
    // Its read out is withdrawn as it stops.
    assert_eq!(
        text.speech(cut(vec![marks_of(&said[0])[0]])),
        [Heard::Display(false), Heard::Cancelled]
    );
    // Its other utterance's report, and a late mark, find nothing waiting.
    assert_eq!(text.speech(cut(vec![marks_of(&said[1])[0]])), []);
    assert_eq!(text.mark(marks_of(&said[1])[0]), NOTHING);
}

/// An utterance the synthesizer failed on counts as heard: say-all carries
/// on past it, as NVDA's speech manager does when a later index arrives,
/// leaving the caret where the last heard unit put it, and moves it again
/// when playback reaches the next utterance.
#[test]
fn say_all_carries_on_past_an_utterance_that_failed() {
    let mut text = editing("x\n", 0);
    let last = TextChunk {
        last: true,
        ..line("Three.\n", 102, 0)
    };
    let (said, _) = text.by_line(vec![line("One.\n", 100, 0), line("Two.\n", 101, 0), last]);
    assert_eq!(texts_of(&said), [["One."], ["Two."]]);
    // The first fails: nothing moves, and the third is handed on.
    let (third, rest) = said_all(text.speech(QueueInput::UtteranceFailed {
        marks: vec![marks_of(&said[0])[0]],
    }));
    assert_eq!((texts_of(&third), rest), (vec![vec!["Three."]], Vec::new()));
    // Playback reaches the second: the caret moves there.
    assert_eq!(
        text.mark(marks_of(&said[1])[0]),
        (Vec::new(), vec![caret_to(101)])
    );
}

#[test]
fn a_caret_key_after_say_all_starts_from_where_say_all_moved_the_caret() {
    // From `notepad_say_all`: say-all moves the caret to the second line as
    // playback reaches it, and a key stops say-all; Notepad's caret tell
    // of the move reaches Core after it. The key's starting read reads
    // the caret where say-all put it.
    let mut text = editing("One.\n", 0);
    let last = TextChunk {
        last: true,
        ..line("Two.\n", 101, 0)
    };
    let (said, _) = text.by_line(vec![line("One.\n", 100, 0), last]);
    assert_eq!(texts_of(&said), [["One."], ["Two."]]);
    let _ = text.mark(marks_of(&said[0])[0]);
    let _ = text.placed();
    assert_eq!(
        text.mark(marks_of(&said[1])[0]),
        (Vec::new(), vec![Heard::Display(false), caret_to(101)])
    );
    let _ = text.placed();
    assert_eq!(
        text.selection(caret_on(line("Two.\n", 101, 0), None), None),
        []
    );
    assert_eq!(
        text.key(CaretMotion::NextCharacter, false),
        [Heard::Asked(starting_read(
            CaretMotion::NextCharacter,
            TextProvider::Uia
        ))]
    );
}

#[test]
fn say_all_from_the_review_cursor_leaves_the_review_cursor_where_it_stopped() {
    let mut text = reviewing("one\n", 0);
    assert_eq!(
        text.command(ReviewCommand::SayAllFromReview, 0),
        [
            Heard::Display(true),
            Heard::Asked(read_ahead(line_start(100), None, TextUnit::Sentence))
        ]
    );
    let paragraph = TextChunk {
        unit: TextUnit::Paragraph,
        last: true,
        ..line("A b. C d.\n", 500, 0)
    };
    let (said, _) = said_all(text.chunks(vec![paragraph]));
    assert_eq!(texts_of(&said), [["A b. "], ["C d."]]);
    // Reaching the second sentence, the last, moves the review cursor,
    // asking nothing, and say-all ends.
    assert_eq!(
        text.mark(marks_of(&said[1])[0]),
        (Vec::new(), vec![Heard::Display(false)])
    );
    // The next review command reads the line where reading stopped.
    assert_eq!(
        text.command(ReviewCommand::ReviewCurrentLine, 0),
        [Heard::Asked(line_read(
            TextPoint::At(TextPosition {
                anchor: TextAnchor(500),
                offset: 5
            }),
            None
        ))]
    );
}

#[test]
fn say_all_splits_lines_at_their_last_sentence_end_and_joins_a_sentence_across_lines() {
    let mut text = editing("x\n", 0);
    let last = TextChunk {
        last: true,
        ..line("line.\n", 102, 0)
    };
    let (said, rest) = text.by_line(vec![
        line(
            "Verbatim is written in Rust. It is informed by NVDA\n",
            100,
            0,
        ),
        line("but not constrained by it. A last\n", 101, 0),
        last,
    ]);
    // The first line up to its sentence end, then the sentence that runs
    // on to the second line spoken whole, with the second line's mark
    // where its text starts; the text's end was read, so nothing more is
    // asked for.
    assert_eq!(
        texts_of(&said),
        [
            vec!["Verbatim is written in Rust. "],
            vec!["It is informed by NVDA ", "but not constrained by it. "]
        ]
    );
    assert_eq!(rest, []);
    let [first, second] = [marks_of(&said[0]), marks_of(&said[1])];
    assert_eq!((first.len(), second.len()), (1, 2));
    // The first line reached: the caret moves to its start, and the last
    // utterance is handed on, the text held back at the end spoken with
    // the last line.
    let (third, rest) = text.mark(first[0]);
    assert_eq!(
        (texts_of(&third), rest),
        (vec![vec!["A last ", "line."]], vec![caret_to(100)])
    );
    let third = marks_of(&third[0]);
    assert_eq!(text.placed(), []);
    // The rest of the first line: the caret stays on the first line.
    assert_eq!(text.mark(second[0]), NOTHING);
    // The second line's text starts: the caret moves to its start.
    assert_eq!(text.mark(second[1]), (Vec::new(), vec![caret_to(101)]));
    assert_eq!(text.placed(), []);
    // The last utterance starts with the rest of the second line: the
    // caret stays there, and a key stops say-all with it there.
    assert_eq!(text.mark(third[0]), NOTHING);
    assert_eq!(text.cut(), [Heard::Display(false)]);
    assert_eq!(text.mark(third[1]), NOTHING);
}

#[test]
fn say_all_speaks_ten_lines_with_no_sentence_end_together() {
    let mut text = editing("x\n", 0);
    // Twelve lines with no sentence end, the fifth blank, which says
    // nothing but counts among the ten.
    let mut lines: Vec<TextChunk> = (0..12)
        .map(|index| {
            let text = if index == 4 {
                "\n".to_owned()
            } else {
                format!("word {index}\n")
            };
            line(&text, 100 + index, 0)
        })
        .collect();
    lines[11].last = true;
    let (said, rest) = text.by_line(lines);
    assert_eq!(rest, []);
    // The first ten lines, the blank one silent, each with its mark; the
    // rest, spoken when the text ends.
    assert_eq!(
        texts_of(&said),
        [
            vec![
                "word 0 ", "word 1 ", "word 2 ", "word 3 ", "word 5 ", "word 6 ", "word 7 ",
                "word 8 ", "word 9",
            ],
            vec!["word 10 ", "word 11"],
        ]
    );
    assert_eq!((marks_of(&said[0]).len(), marks_of(&said[1]).len()), (9, 2));
    // Each line's mark moves the caret to that line's start.
    assert_eq!(
        text.mark(marks_of(&said[0])[4]),
        (Vec::new(), vec![caret_to(105)])
    );
}
impl Text {
    /// The foreground moved to `app`'s top-level window, told by the
    /// listener as a whole-window focus, and the outpost's run read node
    /// `number`, nameless editable text whose caret it did not read: every
    /// utterance spoken.
    fn whole_window(&mut self, app: App, number: u64) -> Vec<Vec<UtteranceSegment>> {
        let _ = self.desk.step(QueueInput::Foreground(
            verbatim_model::request::ForegroundReport {
                window: app.top_level(),
                root_owner: app.top_level(),
                process: app.pid,
                thread: 41,
                class: "Frame".to_owned(),
            },
        ));
        let tell = Event {
            object: EventObject::Msaa {
                window: app.top_level(),
                object: 0,
                child: 0,
            },
            window: TellWindow::Found(Box::new(AcceptanceFacts {
                window: app.facet(app.top_level(), "Frame"),
                window_to_foreground: Relation::Equal,
                window_to_active: Relation::Equal,
                ..app.facts()
            })),
            ..self.edit_tell(EventKind::Foreground, 0)
        };
        // The listener's first foreground report asks for the focus at
        // startup; the field's focus was told before it, so nothing is
        // there to read again.
        if let [(number, to, Wire::CurrentFocus { .. })] = self.desk.sent[..] {
            let _ = self.desk.answer(to, number, Outcome::NotThere);
        }
        let _ = self.desk.tell(desk::LISTENER, tell);
        let [(run, to, Wire::MsaaRun(_))] = self.desk.sent[..] else {
            panic!("one run asked: {:?}", self.desk.sent);
        };
        let nameless = NodeSnapshot {
            name: None,
            ..node(number, Role::EditableText, StateSet::new())
        };
        let mut read = FocusRead {
            caret: FocusCaret::NotRead,
            msaa: Some(verbatim_model::request::HeldIds {
                event: verbatim_model::request::MsaaIds {
                    window: app.top_level(),
                    object: 0,
                    child: 0,
                },
                held_child: 0,
            }),
            ..desk::focus_read_of(app, &nameless, &[])
        };
        read.focus.runtime_id = None;
        let tried = verbatim_model::request::CandidateAnswer::focused(read);
        self.desk
            .answer(to, run, Outcome::Result(Reply::Candidate(tried)))
            .into_iter()
            .filter_map(|heard| match heard {
                desk::Heard::Say(_, segments) => Some(segments),
                _ => None,
            })
            .collect()
    }
}

/// A nameless whole-window focus is a new focus object too: when the focus
/// returns to the text as one, its next line says its bold again, as
/// NVDA's per-object cache starts empty with each new object (Dickson,
/// 2026-11-08).
#[test]
fn a_nameless_whole_window_focus_reports_formatting_afresh() {
    let mut text = edit_control("first\n", 0);
    let bold = |said: &str| words(vec![format(TextFormat::Bold), UtteranceSegment::text(said)]);
    assert_eq!(
        text.moved(CaretMotion::NextLine, bold_line("first bold\n", 101), None),
        bold("first bold")
    );
    // Another window of the application, then the field's own again, each
    // a nameless whole-window focus, which says nothing. A classic edit
    // control, as an MSAA foreground tell of a UI Automation window is
    // dropped by its verdict.
    for (app, number) in [(ONE.in_window(7_000), 7), (ONE, FIELD)] {
        assert_eq!(
            text.whole_window(app, number),
            Vec::<Vec<UtteranceSegment>>::new()
        );
    }
    assert_eq!(
        text.moved(CaretMotion::NextLine, bold_line("second line\n", 102), None),
        bold("second line")
    );
}

/// A terminal keeps the review cursor's cell column: moving down to a
/// shorter row and back finds the same cell, and a cell past the row's
/// text is read as blank.
#[test]
fn a_terminal_keeps_a_cell_column_and_reads_past_the_text_as_blank() {
    // The caret on "d" at cell 3 of a padded row. The simulated terminal
    // gives every character one cell, so the column is kept by cell here
    // as for a wide character's two.
    let mut session = terminal("abcdef    \r\nxy", "\x1b[1;4H");
    session.input(&terminal_command(ReviewCommand::ReviewNextCharacter));
    session.input(&terminal_command(ReviewCommand::ReviewNextLine));
    // Cell 4 is past "xy": a blank cell, and the column is kept exactly.
    session.input(&terminal_command(ReviewCommand::ReviewCurrentCharacter));
    assert_eq!(
        session.heard(),
        terminal_heard(&["e", "xy", "Message(Blank)"])
    );
    session.input(&terminal_command(ReviewCommand::ReviewPreviousLine));
    assert_eq!(session.heard(), terminal_heard(&["abcdef"]));
    session.input(&terminal_command(ReviewCommand::ReviewCurrentCharacter));
    assert_eq!(session.heard(), terminal_heard(&["e"]));
}

/// A system caret event in a hung edit control holding attention: the
/// listener tells it, as NVDA tests no caret event for a hang
/// (`verbatim_listener_logic::tell::hang_tested`), and its read is refused
/// by the shell's live hang check, which plays the not-responding sound,
/// as Dickson decided (2026-11-19); NVDA's caret handling blocks on the
/// hung application until its watchdog cancels the call.
#[test]
fn a_caret_event_in_a_hung_window_holding_attention_plays_the_sound() {
    let mut text = edit_control("hello, world\n", 2);
    // The control's application is in front, holding attention.
    let fg = text.desk.feed(QueueInput::Foreground(
        verbatim_model::request::ForegroundReport {
            window: ONE.top_level(),
            root_owner: ONE.top_level(),
            process: ONE.pid,
            thread: 41,
            class: "Frame".to_owned(),
        },
    ));
    let [
        Effect::Queue(QueueEffect::Send { to, ask }),
        Effect::Queue(QueueEffect::WakeAt(_)),
    ] = &fg[..]
    else {
        panic!("the foreground's read asked: {fg:?}");
    };
    let _ = text.desk.answer(*to, ask.number.0, Outcome::NotThere);
    let tell = text.edit_tell(EventKind::SystemCaret, -8);
    let asked: Vec<verbatim_model::request::RequestNumber> = text
        .desk
        .feed(QueueInput::Tell {
            from: desk::LISTENER,
            tell: Box::new(tell),
        })
        .into_iter()
        .filter_map(|effect| match effect {
            Effect::Queue(QueueEffect::Send { ask, .. }) => Some(ask.number),
            _ => None,
        })
        .collect();
    let [read] = asked[..] else {
        panic!("one read asked: {asked:?}");
    };
    let refused = text.desk.feed(QueueInput::RefusedHung(read));
    let earcons: Vec<verbatim_model::Earcon> = refused
        .into_iter()
        .filter_map(|effect| match effect {
            Effect::PlayEarcon(earcon) => Some(earcon),
            _ => None,
        })
        .collect();
    assert_eq!(earcons, [verbatim_model::Earcon::AppNotResponding]);
}
