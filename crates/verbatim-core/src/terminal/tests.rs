//! Scripted-input tests for what a terminal says (`docs/design/terminal-model.md`,
//! "Core's flood policy"): the flood policy over the backlog of output not
//! yet spoken (a burst's first 30 lines in full, then the last 30 of what
//! waits), output arriving in several batches, the answers to keys spoken
//! at once and typing echoed by its settings, the review cursor following
//! the caret when the reading rules say it did, "Report new output"
//! (Verbatim+5), and speech being cut off. What the reading rules said of
//! each read is given to the flood policy as the terminal model gives it
//! ([`super::speech`]); the rest of the reducer is driven with its inputs.
//! Playback is simulated by reaching each utterance's index mark in turn.

use std::collections::VecDeque;

use verbatim_model::{
    Backend, CaretKey, CaretMotion, Effect, Input, Message, NodeDetails, NodeId, NodeSnapshot,
    NormalizedEvent, OutpostId, Phrase, Pid, QueryId, ReaderSettings, ReviewCommand, Role,
    SegmentContent, SpeechMark, SpeechPriority, StateSet, TextOp, TextPoint, TextReply,
    TextRequest, TraceId,
};

use super::{
    Said, SaidItem, Skipped, TerminalItem, TerminalKeyKind, TerminalUnit, caret_key_kind,
    typed_kind,
};
use crate::reduce::reduce;
use crate::state::SrState;

const OUTPOST: OutpostId = OutpostId(1);
const TERMINAL: u64 = 12;

fn id(number: u64) -> NodeId {
    NodeId::in_outpost(OUTPOST, number)
}

fn event(event: NormalizedEvent) -> Input {
    Input::Event {
        trace_id: TraceId::mint(),
        observed_at_us: 0,
        source: Pid(1),
        backend: Backend::Uia,
        window: None,
        event,
    }
}

/// A state whose focus is a terminal.
fn terminal() -> SrState {
    let mut state = SrState::new();
    let _ = reduce(&mut state, &terminal_focus(None));
    state
}

/// The focus on the terminal, with what its caret read found, `text`.
fn terminal_focus(text: Option<verbatim_model::FocusText>) -> Input {
    event(NormalizedEvent::FocusChanged {
        node: NodeSnapshot {
            id: id(TERMINAL),
            backend: Backend::Uia,
            role: Role::Terminal,
            name: Some("Terminal".to_owned()),
            value: None,
            states: StateSet::new(),
            details: NodeDetails::default(),
        },
        foreground: false,
        ancestors: Vec::new(),
        ancestors_unknown: false,
        selected_child: None,
        text: text.map(Box::new),
    })
}

fn lines(range: std::ops::RangeInclusive<u32>) -> Vec<String> {
    range.map(|n| format!("line {n}")).collect()
}

fn output(lines: Vec<String>) -> Out {
    output_from(TERMINAL, None, lines)
}

/// What the reading rules said of one read, as the terminal model hands
/// it to the flood policy.
#[derive(Clone, Debug, Default)]
struct TerminalSpeech {
    items: Vec<TerminalItem>,
    followed: bool,
}

/// What terminal `node` said in a read that began at `observed_at_ms`.
struct Out {
    node: u64,
    observed_at_ms: u64,
    speech: TerminalSpeech,
}

/// `speech` as the flood policy takes it.
fn said_of(speech: TerminalSpeech) -> Said {
    Said {
        items: speech.items.into_iter().map(SaidItem::Item).collect(),
        followed: speech.followed,
    }
}

/// Gives the flood policy what `out` said, as the terminal model does
/// with a read's speech, and cuts speech off when that does, as the
/// reducer does after each input.
fn say(state: &mut SrState, out: &Out) -> Vec<Effect> {
    let mut effects = super::speech(
        state,
        TraceId::mint(),
        out.observed_at_ms,
        id(out.node),
        said_of(out.speech.clone()),
    );
    if effects.iter().any(super::cuts_speech) {
        effects.extend(super::cut(state, 0));
    }
    effects
}

/// Something the test feeds: an input to the reducer, or what a terminal
/// said.
trait Fed {
    /// Feeds it to `state`, or keeps it in `playback` when the outpost
    /// holds what a terminal says.
    fn fed(&self, state: &mut SrState, playback: &mut Playback) -> Vec<Effect>;
}

impl Fed for Input {
    fn fed(&self, state: &mut SrState, _: &mut Playback) -> Vec<Effect> {
        reduce(state, self)
    }
}

impl Fed for Out {
    fn fed(&self, state: &mut SrState, playback: &mut Playback) -> Vec<Effect> {
        if playback.held {
            playback.unread.push(self.speech.clone());
            return Vec::new();
        }
        say(state, self)
    }
}

/// Speech of output: lines, after a count of lines skipped when given.
fn speech_of(skipped: Option<Skipped>, lines: Vec<String>) -> TerminalSpeech {
    TerminalSpeech {
        items: skipped
            .map(TerminalItem::Skipped)
            .into_iter()
            .chain(lines.into_iter().map(TerminalItem::Output))
            .collect(),
        followed: false,
    }
}

fn output_from(node: u64, skipped: Option<Skipped>, lines: Vec<String>) -> Out {
    said(node, 0, speech_of(skipped, lines))
}

/// What terminal `node` said in a read that began at `observed_at_ms`.
fn said(node: u64, observed_at_ms: u64, speech: TerminalSpeech) -> Out {
    Out {
        node,
        observed_at_ms,
        speech,
    }
}

fn typed(text: &str) -> Input {
    Input::CharacterTyped {
        trace_id: TraceId::mint(),
        text: text.to_owned(),
        at_ms: 1_000,
    }
}

/// What one utterance says, as text. Every segment kind terminal speech
/// may hold is written out; any other kind fails the test, so nothing
/// unexpected is spoken unseen.
fn words(segments: &[verbatim_model::UtteranceSegment]) -> String {
    segments
        .iter()
        .filter(|segment| !matches!(segment.content, SegmentContent::Mark(_)))
        .map(|segment| match &segment.content {
            SegmentContent::Text(text) | SegmentContent::Character(text) => text.clone(),
            SegmentContent::Phrase(Phrase::SkippedLines(count)) => format!("skipped {count}"),
            SegmentContent::Phrase(Phrase::SkippedUncountedLines) => "skipped".to_owned(),
            SegmentContent::Phrase(Phrase::SkippedMoreThanLines(count)) => {
                format!("skipped more than {count}")
            }
            SegmentContent::Phrase(Phrase::SkippedAboutLines(count)) => {
                format!("skipped about {count}")
            }
            SegmentContent::Message(Message::ReportNewOutputOn) => "on".to_owned(),
            SegmentContent::Message(Message::ReportNewOutputOff) => "off".to_owned(),
            SegmentContent::Message(Message::TerminalLineCut) => "line cut".to_owned(),
            SegmentContent::Message(Message::Blank) => "blank".to_owned(),
            other => panic!("unexpected segment in terminal speech: {other:?}"),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A speech queue that plays utterances in order, reaching each one's
/// index mark as it starts, and the terminal's outpost as on-demand reading
/// drives it: output the terminal writes reaches Core at once while the
/// outpost reads live, and waits in the outpost while it holds, until Core
/// asks for it. Requests are answered at once, unless `answer_late` holds
/// a read request back until the test answers it; a cancel sent while one
/// is held back waits behind it, as the outpost answers them in order.
#[derive(Default)]
struct Playback {
    queued: VecDeque<(Vec<SpeechMark>, String)>,
    heard: Vec<String>,
    /// Whether the outpost holds.
    held: bool,
    /// What the terminal said while the outpost held.
    unread: Vec<TerminalSpeech>,
    /// Every terminal request Core made, in order.
    requests: Vec<TextOp>,
    /// Whether read requests wait for [`Playback::answer`].
    answer_late: bool,
    /// A read request held back.
    late: Option<QueryId>,
    /// A cancel sent while a read request was held back.
    late_cancel: Option<QueryId>,
    /// Whether the front utterance has started playing.
    started: bool,
}

impl Playback {
    /// Takes in a step's effects. Terminal speech is queued, and starts
    /// with its index mark when it has one; terminal requests are answered
    /// as the outpost would; any other effect fails the test, since none of
    /// these steps should produce one unasserted.
    fn take(&mut self, state: &mut SrState, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::Speak(utterance) => {
                    assert_eq!(utterance.priority, SpeechPriority::Queued, "{utterance:?}");
                    let marks: Vec<SpeechMark> = utterance
                        .segments
                        .iter()
                        .filter_map(|segment| match segment.content {
                            SegmentContent::Mark(mark) => Some(mark),
                            _ => None,
                        })
                        .collect();
                    assert!(
                        !utterance.segments.is_empty(),
                        "an utterance with nothing in it: {utterance:?}"
                    );
                    self.queued.push_back((marks, words(&utterance.segments)));
                }
                Effect::StopSpeech => self.queued.clear(),
                Effect::Text(TextRequest { query_id, op, .. }) => {
                    self.requests.push(op.clone());
                    match op {
                        TextOp::TerminalHold => {
                            self.held = true;
                            self.reply(state, query_id, TextReply::Done);
                        }
                        TextOp::TerminalRead { hold } => {
                            self.held = hold;
                            if self.answer_late {
                                self.late = Some(query_id);
                            } else {
                                let output = self.take_unread();
                                self.answer_with(state, query_id, output);
                            }
                        }
                        TextOp::TerminalCancel => {
                            self.held = false;
                            self.unread.clear();
                            if self.late.is_some() {
                                self.late_cancel = Some(query_id);
                            } else {
                                self.answer_with(state, query_id, TerminalSpeech::default());
                            }
                        }
                        other => panic!("unexpected text request: {other:?}"),
                    }
                }
                other => panic!("unexpected effect: {other:?}"),
            }
        }
    }

    /// What the terminal wrote while the outpost held, as one read finds
    /// it.
    fn take_unread(&mut self) -> TerminalSpeech {
        let mut all = TerminalSpeech::default();
        for speech in self.unread.drain(..) {
            all.items.extend(speech.items);
        }
        all
    }

    /// Answers request `query_id` with `reply`.
    fn reply(&mut self, state: &mut SrState, query_id: QueryId, reply: TextReply) {
        let effects = reduce(
            state,
            &Input::TextCompleted {
                trace_id: TraceId::mint(),
                query_id,
                reply,
            },
        );
        self.take(state, effects);
    }

    /// Answers the read request held back.
    fn answer(&mut self, state: &mut SrState) {
        let query_id = self.late.take().expect("a read request held back");
        let output = self.take_unread();
        self.answer_with(state, query_id, output);
    }

    /// Answers the terminal read request `query_id` with what the read
    /// said, as the terminal model hands an answer's speech to the flood
    /// policy.
    fn answer_with(&mut self, state: &mut SrState, query_id: QueryId, output: TerminalSpeech) {
        let mut effects = super::answered(state, TraceId::mint(), query_id, said_of(output), None);
        if effects.iter().any(super::cuts_speech) {
            effects.extend(super::cut(state, 0));
        }
        self.take(state, effects);
    }

    /// Feeds `input` and takes in its effects. What a terminal says while
    /// the outpost holds waits in it instead.
    fn feed<T: Fed>(&mut self, state: &mut SrState, input: &T) {
        let effects = input.fed(state, self);
        self.take(state, effects);
    }

    /// Plays one utterance, or the rest of one already started.
    fn play_one(&mut self, state: &mut SrState) -> bool {
        let Some((marks, text)) = self.queued.pop_front() else {
            return false;
        };
        if self.started {
            self.started = false;
        } else {
            self.heard.push(text);
        }
        // Its opening mark as it starts, and its closing mark once heard.
        for mark in marks {
            self.feed(state, &Input::MarkReached { mark });
        }
        true
    }

    /// Starts the next utterance, reaching only its opening mark.
    fn start_one(&mut self, state: &mut SrState) {
        let (mut marks, text) = self.queued.pop_front().expect("an utterance to start");
        self.heard.push(text.clone());
        let opening = marks.remove(0);
        self.queued.push_front((marks, text));
        self.started = true;
        self.feed(state, &Input::MarkReached { mark: opening });
    }

    /// Plays everything queued, and whatever that hands to speech.
    fn play_all(&mut self, state: &mut SrState) -> Vec<String> {
        while self.play_one(state) {}
        std::mem::take(&mut self.heard)
    }
}

#[test]
fn output_under_the_limit_is_spoken_whole_across_batches() {
    let mut state = terminal();
    let mut playback = Playback::default();
    // Three batches arrive before anything is heard: 25 lines in all.
    playback.feed(&mut state, &output(lines(1..=10)));
    playback.feed(&mut state, &output(lines(11..=20)));
    playback.feed(&mut state, &output(lines(21..=25)));
    // Only two lines are handed to speech ahead of playback: one playing
    // and one ready behind it.
    let queued: Vec<&str> = playback
        .queued
        .iter()
        .map(|(_, text)| text.as_str())
        .collect();
    assert_eq!(queued, ["line 1", "line 2"]);
    assert_eq!(playback.play_all(&mut state), lines(1..=25));
}

#[test]
fn a_flood_speaks_its_first_lines_in_full_then_skips_to_the_last() {
    let mut state = terminal();
    let mut playback = Playback::default();
    playback.feed(&mut state, &output(lines(1..=100)));
    let heard = playback.play_all(&mut state);
    // The first 30 are spoken whole; once the 30th plays, the 70 waiting
    // are more than 30, and all but the last 30 are skipped.
    let mut expected = lines(1..=30);
    expected.push("skipped 40".to_owned());
    expected.extend(lines(71..=100));
    assert_eq!(heard, expected);
}

#[test]
fn a_burst_whose_first_line_plays_before_its_second_arrives_is_one_burst() {
    let mut state = terminal();
    let mut playback = Playback::default();
    playback.feed(&mut state, &output(lines(1..=1)));
    // The first line starts playing with nothing else waiting; the rest of
    // the flood arrives a moment later.
    playback.start_one(&mut state);
    playback.feed(&mut state, &output(lines(2..=100)));
    let mut expected = lines(1..=30);
    expected.push("skipped 40".to_owned());
    expected.extend(lines(71..=100));
    let mut heard = std::mem::take(&mut playback.heard);
    heard.extend(playback.play_all(&mut state));
    assert_eq!(heard, expected);
}

#[test]
fn a_held_flood_is_read_when_the_group_s_last_line_is_handed_to_speech() {
    let mut state = terminal();
    let mut playback = Playback {
        answer_late: true,
        ..Playback::default()
    };
    playback.feed(&mut state, &output(lines(1..=30)));
    // A group's worth waits: the outpost is told to hold, and what the
    // terminal writes from then on waits in it.
    assert_eq!(playback.requests, [TextOp::TerminalHold]);
    playback.feed(
        &mut state,
        &output_from(TERMINAL, Some(Skipped::Count(1000)), lines(1031..=1060)),
    );
    // Lines 1 to 28 play; handing line 30 to speech asks for what is new.
    for _ in 0..28 {
        assert!(playback.play_one(&mut state));
    }
    assert_eq!(
        playback.requests,
        [TextOp::TerminalHold, TextOp::TerminalRead { hold: true }]
    );
    // Lines 29 and 30 are heard before the answer comes: the backlog is
    // looked at only once it is in.
    assert!(playback.play_one(&mut state));
    assert!(playback.play_one(&mut state));
    assert!(playback.queued.is_empty());
    playback.answer(&mut state);
    let mut expected = lines(1..=30);
    expected.push("skipped 1000".to_owned());
    expected.extend(lines(1031..=1060));
    let mut heard = std::mem::take(&mut playback.heard);
    heard.extend(playback.play_all(&mut state));
    assert_eq!(heard, expected);
}

#[test]
fn lines_skipped_past_the_history_say_more_than_the_history_holds() {
    let mut state = terminal();
    let mut playback = Playback::default();
    playback.feed(&mut state, &output(lines(1..=40)));
    playback.feed(
        &mut state,
        &output_from(
            TERMINAL,
            Some(Skipped::MoreThan(9001)),
            lines(12001..=12030),
        ),
    );
    // Lines 31 to 40 went before the history's overflow, and are part of
    // it.
    let mut expected = lines(1..=30);
    expected.push("skipped more than 9001".to_owned());
    expected.extend(lines(12001..=12030));
    assert_eq!(playback.play_all(&mut state), expected);
}

/// Speech of a read past a full history: a "more than" count of none,
/// then the lines the history holds.
fn overflowed(lines: Vec<String>) -> TerminalSpeech {
    speech_of(Some(Skipped::MoreThan(0)), lines)
}

#[test]
fn history_lines_skipped_add_to_the_more_than_count_they_follow() {
    // A read past a full history of 100 rows gives its lines after a "more
    // than" count of none. Once the group of lines 1 to 30 is heard, line
    // 31 and the history's oldest 70 lines are skipped: 70 of the history's
    // lines are left unspoken, more than went before them.
    let mut state = terminal();
    let mut playback = Playback::default();
    playback.feed(&mut state, &output(lines(1..=31)));
    playback.feed(
        &mut state,
        &said(TERMINAL, 0, overflowed(lines(9001..=9100))),
    );
    let mut expected = lines(1..=30);
    expected.push("skipped more than 70".to_owned());
    expected.extend(lines(9071..=9100));
    assert_eq!(playback.play_all(&mut state), expected);
}

#[test]
fn lines_after_a_lost_read_found_by_text_are_counted_as_about() {
    // A lost read whose last rows were found in the whole document by
    // their text gives its lines after an "about" count of none. Once the
    // group of lines 1 to 30 is heard, line 31 and the read's oldest 70
    // lines are skipped: about 71, the count taken from text.
    let mut state = terminal();
    let mut playback = Playback::default();
    playback.feed(&mut state, &output(lines(1..=31)));
    playback.feed(
        &mut state,
        &said(
            TERMINAL,
            0,
            speech_of(Some(Skipped::About(0)), lines(9001..=9100)),
        ),
    );
    let mut expected = lines(1..=30);
    expected.push("skipped about 71".to_owned());
    expected.extend(lines(9071..=9100));
    assert_eq!(playback.play_all(&mut state), expected);
}

#[test]
fn a_flood_past_the_history_is_counted_as_more_than() {
    // A burst that starts past a full history of 100 rows: the history's
    // first 30 lines are spoken whole, and of the rest all but the last 30
    // are skipped, more than went by unread before them.
    let mut state = terminal();
    let mut playback = Playback::default();
    playback.feed(&mut state, &said(TERMINAL, 0, overflowed(lines(1..=100))));
    let mut expected = lines(1..=30);
    expected.push("skipped more than 40".to_owned());
    expected.extend(lines(71..=100));
    assert_eq!(playback.play_all(&mut state), expected);
}

#[test]
fn a_flood_that_starts_with_a_lost_read_is_counted_as_about() {
    // A lost read whose last rows were found by their text starts a burst
    // of 2,000 lines with nothing playing: the first 30 are spoken whole,
    // and the skip that follows carries the read's "about".
    let mut state = terminal();
    let mut playback = Playback::default();
    playback.feed(
        &mut state,
        &said(
            TERMINAL,
            0,
            speech_of(Some(Skipped::About(0)), lines(1..=2000)),
        ),
    );
    let mut expected = lines(1..=30);
    expected.push("skipped about 1940".to_owned());
    expected.extend(lines(1971..=2000));
    assert_eq!(playback.play_all(&mut state), expected);
}

/// A terminal state whose "Lines spoken in full" is `full`.
fn terminal_with_full_lines(full: u16) -> SrState {
    let mut state = terminal();
    let mut settings = state.settings();
    settings.terminal_full_lines = full;
    let _ = reduce(&mut state, &Input::Settings(settings));
    state
}

#[test]
fn a_later_read_that_skips_a_lost_read_s_lines_is_counted_as_about() {
    // With 100 lines spoken in full, a lost read found by text brings 150
    // lines with nothing playing, all but 50 of them the first group. When
    // line 100 is handed to speech Core asks for what is new, and a read
    // brings 400 more; once line 100 is heard, lines 101 to 150 of the lost
    // read and 370 of the next read are skipped, so the count is "about".
    let mut state = terminal_with_full_lines(100);
    let mut playback = Playback::default();
    playback.feed(
        &mut state,
        &said(
            TERMINAL,
            0,
            speech_of(Some(Skipped::About(0)), lines(1..=150)),
        ),
    );
    assert_eq!(playback.requests, [TextOp::TerminalHold]);
    playback.feed(&mut state, &output(lines(151..=550)));
    let mut expected = lines(1..=100);
    expected.push("skipped about 420".to_owned());
    expected.extend(lines(521..=550));
    assert_eq!(playback.play_all(&mut state), expected);
}

#[test]
fn a_later_read_that_skips_lines_past_the_history_is_counted_as_more_than() {
    // As above, but the 150 lines went by past a full history. When the
    // lost read is spoken none of its lines is to be skipped, so it says
    // "skipped" for the lines that went by; the read that follows makes the
    // flood policy skip lines 101 to 150, so the skip says "more than".
    let mut state = terminal_with_full_lines(100);
    let mut playback = Playback::default();
    playback.feed(&mut state, &said(TERMINAL, 0, overflowed(lines(1..=150))));
    playback.feed(&mut state, &output(lines(151..=550)));
    let mut expected = vec!["skipped".to_owned()];
    expected.extend(lines(1..=100));
    expected.push("skipped more than 420".to_owned());
    expected.extend(lines(521..=550));
    assert_eq!(playback.play_all(&mut state), expected);
}

#[test]
fn lines_past_the_history_that_the_last_lines_keep_say_skipped_first() {
    // With 10 lines spoken in full and the last 30 kept, a read past a full
    // history brings 25 lines: once the first 10 are heard, the 15 waiting
    // are more than 10 but all among the last 30, so none is skipped, and
    // the read's own count says that lines went by.
    let mut state = terminal_with_full_lines(10);
    let mut playback = Playback::default();
    playback.feed(&mut state, &said(TERMINAL, 0, overflowed(lines(1..=25))));
    let mut expected = vec!["skipped".to_owned()];
    expected.extend(lines(1..=25));
    assert_eq!(playback.play_all(&mut state), expected);
}

#[test]
fn a_lost_read_s_lines_past_ten_megabytes_are_skipped_as_about() {
    // Three lines of four megabytes each, read after a lost read found by
    // text, behind three lines: line 3 and the first big line join the
    // skipped count, which is "about" since one of them was counted from
    // text.
    let mut state = terminal();
    let mut playback = Playback::default();
    playback.feed(&mut state, &output(lines(1..=3)));
    let big = |c: char| c.to_string().repeat(4 * 1024 * 1024);
    playback.feed(
        &mut state,
        &said(
            TERMINAL,
            0,
            speech_of(Some(Skipped::About(0)), vec![big('a'), big('b'), big('c')]),
        ),
    );
    let mut expected = lines(1..=2);
    expected.extend(["skipped about 2".to_owned(), big('b'), big('c')]);
    assert_eq!(playback.play_all(&mut state), expected);
}

#[test]
fn a_lost_read_with_none_of_its_lines_skipped_says_only_its_lines() {
    // A lost read found by text whose 20 lines are all spoken: nothing is
    // skipped, so its "about" says nothing, reading goes live again once
    // they are heard, and the next burst is counted exactly.
    let mut state = terminal();
    let mut playback = Playback::default();
    playback.feed(
        &mut state,
        &said(
            TERMINAL,
            0,
            speech_of(Some(Skipped::About(0)), lines(1..=20)),
        ),
    );
    assert_eq!(playback.play_all(&mut state), lines(1..=20));
    assert_eq!(playback.requests, []);
    playback.feed(&mut state, &output(lines(21..=120)));
    let mut expected = lines(21..=50);
    expected.push("skipped 40".to_owned());
    expected.extend(lines(91..=120));
    assert_eq!(playback.play_all(&mut state), expected);
    assert_eq!(
        playback.requests,
        [
            TextOp::TerminalHold,
            TextOp::TerminalRead { hold: true },
            TextOp::TerminalRead { hold: true },
            TextOp::TerminalRead { hold: false }
        ]
    );
}

#[test]
fn a_flood_past_the_history_with_every_line_spoken_is_counted_as_unknown() {
    // A read past a full history of 40 rows: every line the history holds
    // is spoken, so how many went by is not known.
    let mut state = terminal();
    let mut playback = Playback::default();
    playback.feed(&mut state, &said(TERMINAL, 0, overflowed(lines(1..=40))));
    let mut expected = vec!["skipped".to_owned()];
    expected.extend(lines(1..=40));
    assert_eq!(playback.play_all(&mut state), expected);
}

#[test]
fn blank_lines_count_when_skipped() {
    let mut state = terminal();
    let mut playback = Playback::default();
    let mut flood = lines(1..=30);
    flood.extend((0..10).map(|_| String::new()));
    flood.extend(lines(41..=70));
    playback.feed(&mut state, &output(flood));
    let mut expected = lines(1..=30);
    expected.push("skipped 10".to_owned());
    expected.extend(lines(41..=70));
    assert_eq!(playback.play_all(&mut state), expected);
}

#[test]
fn a_lines_growth_skipped_is_not_counted_as_a_line() {
    // Line 40 was written in two parts, read apart: its second part is the
    // line's growth, and the 40 lines skipped count it once.
    let mut state = terminal();
    let mut playback = Playback::default();
    let mut items: Vec<TerminalItem> = lines(1..=39)
        .into_iter()
        .map(TerminalItem::Output)
        .collect();
    items.push(TerminalItem::Output("line 4".to_owned()));
    items.push(TerminalItem::Growth("0".to_owned()));
    items.extend(lines(41..=100).into_iter().map(TerminalItem::Output));
    playback.feed(
        &mut state,
        &said(
            TERMINAL,
            0,
            TerminalSpeech {
                items,
                followed: false,
            },
        ),
    );
    let mut expected = lines(1..=30);
    expected.push("skipped 40".to_owned());
    expected.extend(lines(71..=100));
    assert_eq!(playback.play_all(&mut state), expected);
}

#[test]
fn a_growth_at_the_boundary_of_the_lines_a_skip_keeps_stays_with_its_line() {
    // Line 70 was written in two parts, read apart, and is the oldest of the
    // last 30 lines: its growth counts with it, so the skip keeps the line
    // and its growth, and the lines kept never start with a bare growth.
    let mut state = terminal();
    let mut playback = Playback::default();
    let mut items: Vec<TerminalItem> = lines(1..=69)
        .into_iter()
        .map(TerminalItem::Output)
        .collect();
    items.push(TerminalItem::Output("line 7".to_owned()));
    items.push(TerminalItem::Growth("0".to_owned()));
    items.extend(lines(71..=99).into_iter().map(TerminalItem::Output));
    playback.feed(
        &mut state,
        &said(
            TERMINAL,
            0,
            TerminalSpeech {
                items,
                followed: false,
            },
        ),
    );
    let mut expected = lines(1..=30);
    expected.push("skipped 39".to_owned());
    expected.push("line 7".to_owned());
    expected.push("0".to_owned());
    expected.extend(lines(71..=99));
    assert_eq!(playback.play_all(&mut state), expected);
}

#[test]
fn a_cut_while_held_reads_to_the_end_without_speaking() {
    let mut state = terminal();
    let mut playback = Playback::default();
    playback.feed(&mut state, &output(lines(1..=40)));
    assert!(playback.play_one(&mut state));
    playback.feed(&mut state, &output(lines(41..=50)));
    playback.queued.clear();
    playback.feed(&mut state, &Input::SpeechCancelled { at_ms: 0 });
    assert_eq!(
        playback.requests,
        [TextOp::TerminalHold, TextOp::TerminalCancel]
    );
    assert!(playback.unread.is_empty(), "what the outpost held is gone");
    // Reading is live again: output after the cut is spoken as it comes.
    playback.feed(&mut state, &output(lines(51..=51)));
    assert_eq!(playback.play_all(&mut state), ["line 1", "line 51"]);
}

#[test]
fn a_read_a_cut_overtakes_speaks_only_its_answers_to_keys() {
    // A key cuts speech while Core's request for what is new is out: the
    // cancel overtakes it, and the outpost answers it first, from a read
    // made before it took the cancel. That read's output was read before
    // the cut and is not spoken; its answer to the key is.
    let mut state = terminal();
    let mut playback = Playback {
        answer_late: true,
        ..Playback::default()
    };
    playback.feed(&mut state, &output(lines(1..=30)));
    playback.feed(&mut state, &output(lines(31..=40)));
    for _ in 0..28 {
        assert!(playback.play_one(&mut state));
    }
    playback.queued.clear();
    playback.heard.clear();
    playback.feed(&mut state, &Input::SpeechCancelled { at_ms: 0 });
    assert_eq!(
        playback.requests,
        [
            TextOp::TerminalHold,
            TextOp::TerminalRead { hold: true },
            TextOp::TerminalCancel
        ]
    );
    let overtaken = playback
        .late
        .take()
        .expect("the request the cancel overtook");
    let read = TerminalSpeech {
        items: vec![
            TerminalItem::Output("line 41".to_owned()),
            TerminalItem::Answer("> banana".to_owned()),
        ],
        followed: false,
    };
    playback.answer_with(&mut state, overtaken, read);
    let cancel = playback
        .late_cancel
        .take()
        .expect("the cancel, answered after");
    playback.answer_with(&mut state, cancel, TerminalSpeech::default());
    assert_eq!(playback.play_all(&mut state), ["> banana"]);
}

#[test]
fn output_read_before_a_cut_is_not_spoken() {
    let mut state = terminal();
    let mut playback = Playback::default();
    playback.feed(&mut state, &Input::SpeechCancelled { at_ms: 5_000 });
    playback.feed(
        &mut state,
        &said(TERMINAL, 4_999, speech_of(None, lines(1..=2))),
    );
    assert_eq!(playback.play_all(&mut state), Vec::<String>::new());
}

#[test]
fn a_flood_read_with_its_start_apart_is_spoken_from_its_start() {
    let mut state = terminal();
    let mut playback = Playback::default();
    // The outpost read the flood's first and last lines, and counted the
    // lines between.
    let mut speech = speech_of(None, lines(1..=30));
    speech
        .items
        .extend(speech_of(Some(Skipped::Count(2940)), lines(2971..=3000)).items);
    playback.feed(&mut state, &said(TERMINAL, 0, speech));
    let mut expected = lines(1..=30);
    expected.push("skipped 2940".to_owned());
    expected.extend(lines(2971..=3000));
    assert_eq!(playback.play_all(&mut state), expected);
}

#[test]
fn output_arriving_while_the_last_lines_play_is_skipped_again() {
    let mut state = terminal();
    let mut playback = Playback::default();
    playback.feed(&mut state, &output(lines(1..=100)));
    // The first group and the skipped count play, and the next group's
    // first line starts.
    for _ in 0..32 {
        assert!(playback.play_one(&mut state));
    }
    // The output goes on: once the next group (the last 30 of the first
    // batch) has played, what waits is skipped again down to its last 30.
    playback.feed(&mut state, &output(lines(101..=200)));
    let mut expected = lines(1..=30);
    expected.push("skipped 40".to_owned());
    expected.extend(lines(71..=100));
    expected.push("skipped 70".to_owned());
    expected.extend(lines(171..=200));
    let mut heard = std::mem::take(&mut playback.heard);
    heard.extend(playback.play_all(&mut state));
    assert_eq!(heard, expected);
}

#[test]
fn a_backlog_from_many_batches_is_counted_exactly() {
    let mut state = terminal();
    let mut playback = Playback::default();
    playback.feed(&mut state, &output(lines(1..=10)));
    // Ten batches of a hundred lines arrive before anything is heard.
    for batch in 0..10 {
        let first = 11 + batch * 100;
        playback.feed(&mut state, &output(lines(first..=first + 99)));
    }
    let mut expected = lines(1..=30);
    expected.push("skipped 950".to_owned());
    expected.extend(lines(981..=1010));
    assert_eq!(playback.play_all(&mut state), expected);
}

#[test]
fn newer_output_never_cancels_older_and_skipped_counts_add_up() {
    let mut state = terminal();
    let mut playback = Playback::default();
    playback.feed(&mut state, &output(lines(1..=20)));
    // The first line plays; then a second batch arrives with lines the
    // outpost could not read, counted.
    assert!(playback.play_one(&mut state));
    playback.feed(
        &mut state,
        &output_from(TERMINAL, Some(Skipped::Count(50)), lines(71..=100)),
    );
    let heard = playback.play_all(&mut state);
    // The first group is the first 30 lines that arrived, the 20 of the
    // first batch and 10 of the second, with the count of the 50 unread
    // (21 to 70) where they went by; the 20 left are no more than 30, and
    // all are spoken.
    let mut expected = lines(1..=20);
    expected.push("skipped 50".to_owned());
    expected.extend(lines(71..=100));
    assert_eq!(heard, expected);
}

#[test]
fn lines_skipped_uncounted_say_so_without_a_number() {
    let mut state = terminal();
    let mut playback = Playback::default();
    playback.feed(
        &mut state,
        &output_from(TERMINAL, Some(Skipped::Uncounted), lines(1..=3)),
    );
    let mut expected = vec!["skipped".to_owned()];
    expected.extend(lines(1..=3));
    assert_eq!(playback.play_all(&mut state), expected);
}

#[test]
fn blank_lines_are_dropped() {
    let mut state = terminal();
    let mut playback = Playback::default();
    playback.feed(
        &mut state,
        &output(vec![
            "one".to_owned(),
            String::new(),
            "  ".to_owned(),
            "two".to_owned(),
        ]),
    );
    assert_eq!(playback.play_all(&mut state), ["one", "two"]);
}

#[test]
fn report_new_output_toggles_with_verbatim_5() {
    let mut state = terminal();
    let mut playback = Playback::default();
    let toggle = Input::Command {
        trace_id: TraceId::mint(),
        command: ReviewCommand::ToggleReportNewOutput,
        repeat: 0,
    };
    // Each press speaks its new value, then reports the settings, with the
    // toggle changed and nothing else, for the shell to save.
    let press = |state: &mut SrState, playback: &mut Playback, on: bool| {
        let mut effects = reduce(state, &toggle);
        assert_eq!(
            effects.pop(),
            Some(Effect::SettingsChanged(ReaderSettings {
                report_terminal_output: on,
                ..ReaderSettings::default()
            }))
        );
        assert_eq!(effects.len(), 1, "{effects:?}");
        playback.take(state, effects);
    };
    press(&mut state, &mut playback, false);
    playback.feed(&mut state, &output(lines(1..=3)));
    assert_eq!(playback.play_all(&mut state), ["off"]);
    press(&mut state, &mut playback, true);
    playback.feed(&mut state, &output(lines(4..=4)));
    assert_eq!(playback.play_all(&mut state), ["on", "line 4"]);
}

#[test]
fn turning_report_new_output_off_in_the_settings_drops_what_is_waiting() {
    let mut state = terminal();
    let mut playback = Playback::default();
    playback.feed(&mut state, &output(lines(1..=10)));
    assert!(playback.play_one(&mut state));
    let mut settings = state.settings();
    settings.report_terminal_output = false;
    playback.feed(&mut state, &Input::Settings(settings));
    // Only what speech already had plays; the rest of the output is gone,
    // as when Verbatim+5 turns reporting off. "line 1" played before the
    // change, and speech held the next two.
    assert_eq!(
        playback.play_all(&mut state),
        ["line 1", "line 2", "line 3"]
    );
}

/// An utterance of output the synthesizer failed on counts as heard:
/// output carries on past it, nothing waiting is dropped.
#[test]
fn output_carries_on_past_an_utterance_that_failed() {
    let mut state = terminal();
    let mut playback = Playback::default();
    playback.feed(&mut state, &output(lines(1..=4)));
    // The first line fails, reported with its marks.
    let (marks, _) = playback.queued.pop_front().expect("line 1 handed on");
    playback.feed(&mut state, &Input::UtteranceFailed { marks });
    assert_eq!(
        playback.play_all(&mut state),
        ["line 2", "line 3", "line 4"]
    );
}

#[test]
fn speech_cut_off_drops_the_output_still_waiting() {
    let mut state = terminal();
    let mut playback = Playback::default();
    playback.feed(&mut state, &output(lines(1..=10)));
    assert!(playback.play_one(&mut state));
    // A key press cuts speech off: what was handed to speech is gone, and
    // so is what was waiting.
    playback.queued.clear();
    playback.feed(&mut state, &Input::SpeechCancelled { at_ms: 0 });
    assert_eq!(playback.play_all(&mut state), ["line 1"]);
    // Output after it is spoken as usual.
    playback.feed(&mut state, &output(lines(11..=11)));
    assert_eq!(playback.play_all(&mut state), ["line 11"]);
}

#[test]
fn output_from_a_terminal_without_the_focus_is_not_spoken() {
    let mut state = terminal();
    let mut playback = Playback::default();
    playback.feed(&mut state, &output_from(99, None, lines(1..=2)));
    assert_eq!(playback.play_all(&mut state), Vec::<String>::new());
}

#[test]
fn a_line_larger_than_everything_kept_waiting_is_cut_and_says_so() {
    let mut state = terminal();
    let mut playback = Playback::default();
    // Eleven megabytes of two-byte characters: cut on a character boundary
    // at ten.
    let line = "é".repeat(11 * 1024 * 1024 / 2);
    playback.feed(&mut state, &output(vec![line]));
    let heard = playback.play_all(&mut state);
    assert_eq!(heard.len(), 2);
    assert_eq!(heard[0], "é".repeat(10 * 1024 * 1024 / 2));
    assert_eq!(heard[1], "line cut");
}

#[test]
fn waiting_output_past_ten_megabytes_skips_its_oldest_lines() {
    let mut state = terminal();
    let mut playback = Playback::default();
    playback.feed(&mut state, &output(lines(1..=3)));
    // Three lines of four megabytes each behind them: the oldest lines
    // still waiting join the skipped count until what waits fits ten
    // megabytes: line 3 (lines 1 and 2 are with speech) and the first big
    // line.
    let big = |c: char| c.to_string().repeat(4 * 1024 * 1024);
    playback.feed(&mut state, &output(vec![big('a'), big('b'), big('c')]));
    let heard = playback.play_all(&mut state);
    let mut expected = lines(1..=2);
    expected.extend(["skipped 2".to_owned(), big('b'), big('c')]);
    assert_eq!(heard, expected);
}

#[test]
fn the_blank_lines_a_burst_starts_with_count_like_any_other() {
    let mut state = terminal();
    let mut playback = Playback::default();
    // Every line counts, blank ones included (coherence review, 2026-10-09):
    // two blank lines first use up two of the first group's thirty, which
    // are not spoken, and the backlog's count includes the blank ones.
    let mut drawn: Vec<String> = vec![String::new(), String::new()];
    drawn.extend(lines(1..=40));
    drawn.extend((0..10).map(|_| String::new()));
    drawn.extend(lines(41..=70));
    playback.feed(&mut state, &output(drawn));
    let mut expected = lines(1..=28);
    expected.push("skipped 22".to_owned());
    expected.extend(lines(41..=70));
    assert_eq!(playback.play_all(&mut state), expected);
}

/// The utterances `effects` speak, each as text, and any other effect
/// spelled out.
fn spoken(effects: &[Effect]) -> Vec<String> {
    effects
        .iter()
        .map(|effect| match effect {
            Effect::Speak(utterance) => words(&utterance.segments),
            other => format!("{other:?}"),
        })
        .collect()
}

#[test]
fn the_answers_to_keys_are_spoken_at_once_ahead_of_output() {
    let mut state = terminal();
    let speech = TerminalSpeech {
        items: vec![
            TerminalItem::Caret {
                unit: TerminalUnit::Word,
                text: "echo".to_owned(),
            },
            TerminalItem::Caret {
                unit: TerminalUnit::Character,
                text: String::new(),
            },
            TerminalItem::Lost("x".to_owned()),
            TerminalItem::Gained("completion".to_owned()),
            TerminalItem::Answer("> banana".to_owned()),
            TerminalItem::Suggestion("ls -la".to_owned()),
            TerminalItem::Output("output".to_owned()),
        ],
        followed: false,
    };
    let effects = say(&mut state, &said(TERMINAL, 0, speech));
    // The answers, in order, then the output's first line handed to
    // speech with its marks.
    assert_eq!(
        spoken(&effects),
        [
            "echo",
            "blank",
            "x",
            "completion",
            "> banana",
            "ls -la",
            "output"
        ]
    );
}

#[test]
fn typing_the_terminal_showed_is_echoed_by_the_typing_echo_settings() {
    let mut state = terminal();
    let echo = TerminalSpeech {
        items: vec![TerminalItem::Echo("ls".to_owned())],
        followed: false,
    };
    let effects = say(&mut state, &said(TERMINAL, 0, echo.clone()));
    assert_eq!(spoken(&effects), ["l", "s"]);
    // With passwords spoken, typing is echoed as it is typed, and the
    // terminal showing it says nothing more.
    let mut settings = state.settings();
    settings.speak_terminal_passwords = true;
    let _ = reduce(&mut state, &Input::Settings(settings));
    let effects = reduce(&mut state, &typed("p"));
    assert_eq!(
        spoken(&effects[..1]),
        ["p"],
        "the character is echoed as it is typed"
    );
    let effects = say(&mut state, &said(TERMINAL, 0, echo));
    assert_eq!(spoken(&effects), Vec::<String>::new());
}

#[test]
fn the_review_cursor_reads_at_the_caret_once_it_followed_it() {
    let mut state = terminal();
    let review = |state: &mut SrState| {
        reduce(
            state,
            &Input::Command {
                trace_id: TraceId::mint(),
                command: ReviewCommand::ReviewCurrentLine,
                repeat: 0,
            },
        )
    };
    let at = |effects: &[Effect]| -> Vec<TextPoint> {
        effects
            .iter()
            .filter_map(|effect| match effect {
                Effect::Text(TextRequest {
                    op: TextOp::Read(read),
                    ..
                }) => Some(read.at),
                _ => None,
            })
            .collect()
    };
    // The first review command reads where the caret is.
    let effects = review(&mut state);
    assert_eq!(at(&effects), [TextPoint::Caret]);
    let Some(Effect::Text(request)) = effects.into_iter().next() else {
        panic!("a read of the caret's line");
    };
    let chunk = verbatim_model::TextChunk {
        unit: verbatim_model::TextUnit::Line,
        text: "ready>".to_owned(),
        start: verbatim_model::TextAnchor(7),
        offset: 0,
        languages: Vec::new(),
        first: false,
        last: true,
        truncated: false,
        formats: Vec::new(),
    };
    let effects = reduce(
        &mut state,
        &Input::TextCompleted {
            trace_id: TraceId::mint(),
            query_id: request.query_id,
            reply: TextReply::Read { moved: 0, chunk },
        },
    );
    assert_eq!(spoken(&effects), ["ready>"]);
    // In a terminal every review command reads the line again first, at
    // the review cursor's place.
    let effects = review(&mut state);
    assert_eq!(
        at(&effects),
        [TextPoint::At(verbatim_model::TextPosition {
            anchor: verbatim_model::TextAnchor(7),
            offset: 0,
        })]
    );
    // The reading rules found that a key moved the caret, and the review
    // cursor followed it: the next command reads at the caret.
    let followed = TerminalSpeech {
        items: Vec::new(),
        followed: true,
    };
    let _ = say(&mut state, &said(TERMINAL, 0, followed));
    assert_eq!(at(&review(&mut state)), [TextPoint::Caret]);
}

#[test]
fn of_what_a_read_before_a_cut_said_only_the_answers_to_keys_are_spoken() {
    let mut state = terminal();
    let _ = reduce(&mut state, &Input::SpeechCancelled { at_ms: 5_000 });
    let speech = TerminalSpeech {
        items: vec![
            TerminalItem::Echo("a".to_owned()),
            TerminalItem::Output("before the cut".to_owned()),
        ],
        followed: false,
    };
    let effects = say(&mut state, &said(TERMINAL, 4_999, speech));
    assert_eq!(spoken(&effects), ["a"]);
}

/// Speech the reducer did not cut off itself (a synthesizer switch, focus
/// speech whose focus moved on, or, before every utterance came from the
/// reducer, the clipboard's confirmation spoken interrupting) cuts off the
/// output handed to speech. Without the pipeline's report of each cut
/// utterance, the reducer kept waiting for its marks and spoke no output
/// until the next key; with it, what waited is dropped, as on any cut, and
/// output after it is spoken.
#[test]
fn output_cut_off_outside_the_reducer_is_released_by_the_pipelines_report() {
    let mut state = terminal();
    let mut playback = Playback::default();
    playback.feed(&mut state, &output(lines(1..=10)));
    assert!(playback.play_one(&mut state));
    // Both utterances handed on are cut off, each reported with its marks.
    let cut: Vec<(Vec<SpeechMark>, String)> = playback.queued.drain(..).collect();
    assert_eq!(cut.len(), 2, "{cut:?}");
    for (marks, _) in cut {
        playback.feed(&mut state, &Input::UtteranceCancelled { marks, at_ms: 0 });
    }
    assert_eq!(playback.play_all(&mut state), ["line 1"]);
    playback.feed(&mut state, &output(lines(11..=11)));
    assert_eq!(playback.play_all(&mut state), ["line 11"]);
}

#[test]
fn keys_are_told_to_the_reading_rules_by_their_unit() {
    let key = |motion| CaretKey {
        motion,
        select: false,
    };
    assert_eq!(
        caret_key_kind(key(CaretMotion::NextCharacter)),
        TerminalKeyKind::Caret(TerminalUnit::Character)
    );
    assert_eq!(
        caret_key_kind(key(CaretMotion::PreviousWord)),
        TerminalKeyKind::Caret(TerminalUnit::Word)
    );
    assert_eq!(
        caret_key_kind(key(CaretMotion::PreviousLine)),
        TerminalKeyKind::Caret(TerminalUnit::Line)
    );
    assert_eq!(
        caret_key_kind(key(CaretMotion::Backspace)),
        TerminalKeyKind::Other
    );
    assert_eq!(typed_kind("a"), TerminalKeyKind::Typed("a".to_owned()));
    assert_eq!(typed_kind("\r"), TerminalKeyKind::Enter);
    assert_eq!(typed_kind("\t"), TerminalKeyKind::Forget);
    assert_eq!(typed_kind("\u{1}"), TerminalKeyKind::Other);
}
