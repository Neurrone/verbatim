//! Scripted-input tests for what a terminal says (`books/dev/src/explanation/design/terminal-model.md`,
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
    CaretKey, CaretMotion, Effect, Input, Message, NodeId, Phrase, ReaderSettings, ReviewCommand,
    Role, SegmentContent, SpeechMark, SpeechPriority, TextOp, TextReply, TraceId,
};

use super::{
    Said, SaidItem, Skipped, TerminalItem, TerminalKeyKind, TerminalUnit, caret_key_kind,
    typed_kind,
};
use crate::reduce::reduce;
use crate::state::SrState;

const TERMINAL: u64 = 2;

fn id(number: u64) -> NodeId {
    crate::entries::tests::node(number)
}

/// A state whose focus is the terminal, node 2, committed by the startup
/// read of its outpost through the event queue, which Core models, as
/// after its baseline: what its reads say is given as the model hands it
/// on ([`Playback`]).
fn terminal() -> SrState {
    use crate::entries::tests::{OUTPOST, answer, feed_at, focus_read, ready, values};
    let mut held = ready();
    let _ = feed_at(
        &mut held,
        500,
        crate::event_queue::Input::Foreground(crate::event_queue::Foreground {
            window: verbatim_model::WindowHandle(4),
            root_owner: verbatim_model::WindowHandle(4),
            process: crate::entries::tests::APP,
            thread: 41,
            by_class: None,
            shell: false,
        }),
    );
    let mut read = focus_read(None);
    read.focus.read.values = values("Terminal", Role::Terminal);
    let _ = feed_at(
        &mut held,
        600,
        answer(
            OUTPOST,
            1,
            verbatim_model::request::Reply::Focus(Box::new(read)),
        ),
    );
    assert_eq!(
        held.state.focused().map(|(_, node)| node.id),
        Some(id(TERMINAL))
    );
    let mut state = held.state;
    super::reading::modelled_for_tests(&mut state, id(TERMINAL), window());
    state
}

/// The terminal's window, which its requests name.
fn window() -> crate::event_queue::RequestWindow {
    crate::event_queue::RequestWindow {
        handle: verbatim_model::WindowHandle(5),
        top_level: verbatim_model::WindowHandle(4),
        root_owner: verbatim_model::WindowHandle(4),
        process: crate::entries::tests::APP,
        thread: 41,
        by_class: None,
    }
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

/// What terminal `node` wrote, which a read begun at `observed_at_ms` or
/// later says.
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
/// with a read's speech at its turn, and cuts speech off when that does,
/// as the reducer does after each input.
fn say(state: &mut SrState, out: &Out) -> Vec<Effect> {
    let mut effects = super::speech(
        state,
        TraceId::from_raw(2),
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
/// wrote.
trait Fed {
    /// Feeds it to `state`, `playback` keeping what a terminal wrote until
    /// Core reads it.
    fn fed(&self, state: &mut SrState, playback: &mut Playback) -> Vec<Effect>;
}

impl Fed for Input {
    fn fed(&self, state: &mut SrState, _: &mut Playback) -> Vec<Effect> {
        reduce(state, &mut crate::Runtime::default(), self)
    }
}

impl Fed for Out {
    /// The terminal wrote: its change is told to Core at once, and a read
    /// Core makes from then on says it.
    fn fed(&self, state: &mut SrState, playback: &mut Playback) -> Vec<Effect> {
        playback.clock_us = (playback.clock_us + 1).max(self.observed_at_ms * 1_000);
        if self.node == TERMINAL {
            playback.unread.push(self.speech.clone());
        }
        let now_us = playback.clock_us;
        super::reading::told(state, id(self.node), now_us, now_us);
        Vec::new()
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

/// What terminal `node` wrote, which a read begun at `observed_at_ms` or
/// later says.
fn said(node: u64, observed_at_ms: u64, speech: TerminalSpeech) -> Out {
    Out {
        node,
        observed_at_ms,
        speech,
    }
}

/// Speech reaching index mark `mark`, as the shell tells the queue.
/// `input` as the shell gives it to the queue.
fn queue_input(input: verbatim_model::request::QueueInput) -> Input {
    Input::Queue {
        trace_id: TraceId::from_raw(7),
        at_us: 0,
        input,
    }
}

fn mark_reached(mark: SpeechMark) -> Input {
    Input::Queue {
        trace_id: TraceId::from_raw(0),
        at_us: 0,
        input: verbatim_model::request::QueueInput::MarkReached(mark),
    }
}

fn typed(text: &str) -> Input {
    Input::CharacterTyped {
        key_seq: verbatim_model::KeySeq::NONE_YET,
        trace_id: TraceId::from_raw(3),
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

/// An output read, by its mark, as the tests name it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Read {
    /// Spoken.
    Spoken,
    /// Waited for Core's speech queue, at a group's last line.
    Release,
    /// Waited, not the release.
    Waited,
    /// Speaking only its answers to keys.
    Silent,
}

/// A speech queue that plays utterances in order, reaching each one's
/// index mark as it starts, and the terminal: what it writes is told to
/// Core at once, and each output read Core asks says everything written
/// since the last read, at once, its speech spoken at its turn, unless
/// `answer_late` holds a release read back until the test answers it.
#[derive(Default)]
struct Playback {
    queued: VecDeque<(Vec<SpeechMark>, String)>,
    heard: Vec<String>,
    /// What the terminal wrote that no read has said yet.
    unread: Vec<TerminalSpeech>,
    /// Every output read Core asked, in order.
    requests: Vec<Read>,
    /// Whether release reads wait for [`Playback::answer`].
    answer_late: bool,
    /// A release read held back, by its token.
    late: Option<u64>,
    /// Whether the front utterance has started playing.
    started: bool,
    /// The terminal's clock, in microseconds: each write and each read a
    /// moment later than the last.
    clock_us: u64,
    /// The output reads answered or held back, by their tokens.
    served: std::collections::BTreeSet<u64>,
}

impl Playback {
    /// Takes in a step's effects, then answers the reads Core wants.
    /// Terminal speech is queued, and starts with its index mark when it
    /// has one; any other effect fails the test, since none of these steps
    /// should produce one unasserted.
    fn take(&mut self, state: &mut SrState, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::Speak(utterance, _) => {
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
                Effect::ForKey(_, effect) => self.take(state, vec![*effect]),
                // The event queue's own effects as it settles: the
                // outpost's start and the next wake.
                Effect::Queue(_) => {}
                other => panic!("unexpected effect: {other:?}"),
            }
        }
        self.serve(state);
    }

    /// Answers the output read Core has out, as the outpost's worker
    /// makes it, until none is new.
    fn serve(&mut self, state: &mut SrState) {
        let _ = super::reading::wanted_for_tests(state);
        loop {
            let Some((token, read)) = out_read(state) else {
                return;
            };
            if !self.served.insert(token) {
                return;
            }
            self.requests.push(read);
            if self.answer_late && read == Read::Release {
                self.late = Some(token);
                return;
            }
            self.answer_with(state, token, None);
        }
    }

    /// What the terminal wrote since the last read, as one read finds it.
    fn take_unread(&mut self) -> TerminalSpeech {
        let mut all = TerminalSpeech::default();
        for speech in self.unread.drain(..) {
            all.items.extend(speech.items);
        }
        all
    }

    /// Answers the release read held back.
    fn answer(&mut self, state: &mut SrState) {
        let token = self.late.take().expect("a release read held back");
        self.answer_with(state, token, None);
    }

    /// Answers output read `token` with what the terminal wrote since the
    /// last read, or with `read` when given, its speech spoken at its turn
    /// at once.
    fn answer_with(&mut self, state: &mut SrState, token: u64, read: Option<TerminalSpeech>) {
        let output = read.unwrap_or_else(|| self.take_unread());
        self.clock_us += 1;
        let Some(entry) =
            super::reading::answered_with(state, token, said_of(output), self.clock_us)
        else {
            self.serve(state);
            return;
        };
        let mut effects = super::reading::speak(state, entry);
        if effects.iter().any(super::cuts_speech) {
            effects.extend(super::cut(state, 0));
        }
        self.take(state, effects);
    }

    /// Feeds `input` and takes in its effects.
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
            self.feed(state, &mark_reached(mark));
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
        self.feed(state, &mark_reached(opening));
    }

    /// Plays everything queued, and whatever that hands to speech.
    fn play_all(&mut self, state: &mut SrState) -> Vec<String> {
        while self.play_one(state) {}
        std::mem::take(&mut self.heard)
    }
}

/// The output read out, by its token and mark.
fn out_read(state: &SrState) -> Option<(u64, Read)> {
    use super::reading::{FocusedTerminal, Mark, Reading};
    match state.terminal_focus.focused {
        FocusedTerminal::Modelled {
            reading: Reading::Out { token, mark, .. },
            ..
        } => Some((
            token,
            match mark {
                Mark::Spoken => Read::Spoken,
                Mark::Waited { release: true } => Read::Release,
                Mark::Waited { release: false } => Read::Waited,
                Mark::Silent => Read::Silent,
            },
        )),
        _ => None,
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
    // A group's worth waits: Core holds, and what the terminal writes from
    // then on is only told.
    assert_eq!(playback.requests, [Read::Spoken]);
    playback.feed(
        &mut state,
        &output_from(TERMINAL, Some(Skipped::Count(1000)), lines(1031..=1060)),
    );
    // Lines 1 to 28 play; handing line 30 to speech asks for what is new.
    for _ in 0..28 {
        assert!(playback.play_one(&mut state));
    }
    assert_eq!(playback.requests, [Read::Spoken, Read::Release]);
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

/// A read Core asked of the focused console, answered after the focus
/// moved to another console, is not taken into the other's output.
#[test]
fn a_read_answered_after_the_focus_moved_to_another_console_is_not_taken() {
    let mut state = terminal();
    let mut playback = Playback {
        answer_late: true,
        ..Playback::default()
    };
    playback.feed(&mut state, &output(lines(1..=30)));
    playback.feed(&mut state, &output(lines(31..=31)));
    for _ in 0..28 {
        assert!(playback.play_one(&mut state));
    }
    let read = playback.late.take().expect("a read request held back");
    let other = super::reading::Terminal {
        node: id(TERMINAL + 1),
        window: window(),
        console: false,
    };
    super::reading::committed(&mut state, Some(other), Vec::new(), 0);
    let _ = super::reading::wanted_for_tests(&mut state);
    let before = state.clone();
    let mut said = Vec::new();
    let entries = super::reading::answered(
        &mut state,
        read,
        crate::event_queue::Outcome::Result(verbatim_model::request::Reply::Text(
            TextReply::TerminalRead(Box::new(read_reply("line 31"))),
        )),
        (1_000, TraceId::from_raw(4)),
        &mut said,
    );
    assert_eq!((entries, said), (Vec::new(), Vec::new()));
    assert_eq!(state, before);
}

/// A read of the terminal showing `text` on its one row.
fn read_reply(text: &str) -> verbatim_model::TerminalReadReply {
    let read = verbatim_text::terminal_rows::Read {
        text: verbatim_text::terminal_rows::ReadText::unpadded(text),
        screen: text.to_owned(),
        from_start: false,
        shrank: false,
        at_document_start: true,
        alternate: true,
        first_row: text.to_owned(),
        caret_rows_to_end: 1,
        caret_line: text.to_owned(),
        caret_column: 0,
        at_ms: 1,
        above: 0,
    };
    verbatim_model::TerminalReadReply {
        snapshot: crate::snapshot_of(&read, false, None),
        start: verbatim_model::TextAnchor(1),
        started_us: 1_000,
        point: None,
    }
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
    let _ = reduce(
        &mut state,
        &mut crate::Runtime::default(),
        &Input::Settings(settings),
    );
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
    assert_eq!(playback.requests, [Read::Spoken]);
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
    assert_eq!(playback.requests, [Read::Spoken]);
    playback.feed(&mut state, &output(lines(21..=120)));
    let mut expected = lines(21..=50);
    expected.push("skipped 40".to_owned());
    expected.extend(lines(91..=120));
    assert_eq!(playback.play_all(&mut state), expected);
    // Nothing more was written while Core held: no release read is
    // needed.
    assert_eq!(playback.requests, [Read::Spoken, Read::Spoken]);
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
    playback.feed(
        &mut state,
        &Input::Queue {
            trace_id: verbatim_model::TraceId::from_raw(0),
            at_us: 0,
            input: verbatim_model::request::QueueInput::SpeechCut,
        },
    );
    // The changes held unread are read by a silent read.
    assert_eq!(playback.requests, [Read::Spoken, Read::Silent]);
    assert!(playback.unread.is_empty(), "what was held unread is read");
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
    playback.feed(
        &mut state,
        &Input::Queue {
            trace_id: verbatim_model::TraceId::from_raw(0),
            at_us: 0,
            input: verbatim_model::request::QueueInput::SpeechCut,
        },
    );
    // The release read out at the cut becomes silent: nothing more is
    // asked.
    assert_eq!(playback.requests, [Read::Spoken, Read::Release]);
    let overtaken = playback
        .late
        .take()
        .expect("the release read out at the cut");
    let read = TerminalSpeech {
        items: vec![
            TerminalItem::Output("line 41".to_owned()),
            TerminalItem::Answer("> banana".to_owned()),
        ],
        followed: false,
    };
    playback.answer_with(&mut state, overtaken, Some(read));
    assert_eq!(playback.play_all(&mut state), ["> banana"]);
}

#[test]
fn output_read_before_a_cut_is_not_spoken() {
    let mut state = terminal();
    let mut playback = Playback::default();
    playback.feed(
        &mut state,
        &Input::Queue {
            trace_id: verbatim_model::TraceId::from_raw(0),
            at_us: 5_000_000,
            input: verbatim_model::request::QueueInput::SpeechCut,
        },
    );
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
        pressed_at_ms: 0,
        key_seq: verbatim_model::KeySeq::NONE_YET,
        trace_id: TraceId::from_raw(6),
        command: ReviewCommand::ToggleReportNewOutput,
        repeat: 0,
    };
    // Each press speaks its new value, then reports the settings, with the
    // toggle changed and nothing else, for the shell to save.
    let press = |state: &mut SrState, playback: &mut Playback, on: bool| {
        let mut effects = reduce(state, &mut crate::Runtime::default(), &toggle);
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
    playback.feed(
        &mut state,
        &Input::Queue {
            trace_id: verbatim_model::TraceId::from_raw(0),
            at_us: 0,
            input: verbatim_model::request::QueueInput::UtteranceFailed { marks },
        },
    );
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
    playback.feed(
        &mut state,
        &Input::Queue {
            trace_id: verbatim_model::TraceId::from_raw(0),
            at_us: 0,
            input: verbatim_model::request::QueueInput::SpeechCut,
        },
    );
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
            Effect::Speak(utterance, _) => words(&utterance.segments),
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
    let _ = reduce(
        &mut state,
        &mut crate::Runtime::default(),
        &Input::Settings(settings),
    );
    let effects = reduce(&mut state, &mut crate::Runtime::default(), &typed("p"));
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
    use verbatim_model::request::{
        Answer, Outcome, Press, PressAction, QueueEffect, QueueInput, Reply, Request as Wire,
    };
    let mut state = terminal();
    // A review command pressed, its fresh read of the terminal answered
    // showing "ready>": what it said.
    let review = |state: &mut SrState| -> Vec<String> {
        let effects = reduce(
            state,
            &mut crate::Runtime::default(),
            &Input::Queue {
                trace_id: TraceId::from_raw(7),
                at_us: 1_000,
                input: QueueInput::Press(Box::new(Press {
                    key_seq: verbatim_model::KeySeq::new(1),
                    pressed_at_us: 1_000,
                    action: PressAction::Command {
                        command: ReviewCommand::ReviewCurrentLine,
                        repeat: 0,
                    },
                })),
            },
        );
        let asked: Vec<_> = effects
            .into_iter()
            .filter_map(|effect| match effect {
                Effect::Queue(QueueEffect::Send { ask, .. }) => Some(ask),
                _ => None,
            })
            .collect();
        let [ask] = &asked[..] else {
            panic!("one read asked: {asked:?}");
        };
        assert!(
            matches!(
                ask.request,
                Wire::Text {
                    op: TextOp::ReadTerminal(_),
                    ..
                }
            ),
            "{ask:?}"
        );
        let effects = reduce(
            state,
            &mut crate::Runtime::default(),
            &Input::Queue {
                trace_id: TraceId::from_raw(8),
                at_us: 2_000,
                input: QueueInput::Answer {
                    from: crate::entries::tests::OUTPOST,
                    answer: Answer {
                        number: ask.number,
                        outcome: Outcome::Result(Reply::Text(TextReply::TerminalRead(Box::new(
                            read_reply("ready>"),
                        )))),
                        probes: Vec::new(),
                        detail: String::new(),
                    },
                },
            },
        );
        effects
            .into_iter()
            .map(|effect| match effect {
                Effect::ForKey(_, effect) => *effect,
                effect => effect,
            })
            .filter_map(|effect| match effect {
                Effect::Speak(utterance, _) => Some(words(&utterance.segments)),
                _ => None,
            })
            .collect()
    };
    let text = |state: &SrState| {
        state
            .navigator
            .as_ref()
            .map(|navigator| navigator.text.clone())
    };
    // The first review command reads where the caret is, and the review
    // cursor stays at that place.
    assert_eq!(review(&mut state), ["ready>"]);
    assert!(
        matches!(text(&state), Some(crate::state::ReviewText::At(_))),
        "{:?}",
        text(&state)
    );
    // The reading rules found that a key moved the caret, and the review
    // cursor followed it: the next command reads at the caret.
    let followed = TerminalSpeech {
        items: Vec::new(),
        followed: true,
    };
    let _ = say(&mut state, &said(TERMINAL, 0, followed));
    assert_eq!(text(&state), Some(crate::state::ReviewText::Caret));
}

#[test]
fn of_what_a_read_before_a_cut_said_only_the_answers_to_keys_are_spoken() {
    let mut state = terminal();
    let _ = reduce(
        &mut state,
        &mut crate::Runtime::default(),
        &Input::Queue {
            trace_id: verbatim_model::TraceId::from_raw(0),
            at_us: 5_000_000,
            input: verbatim_model::request::QueueInput::SpeechCut,
        },
    );
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
        playback.feed(
            &mut state,
            &Input::Queue {
                trace_id: verbatim_model::TraceId::from_raw(0),
                at_us: 0,
                input: verbatim_model::request::QueueInput::UtteranceCancelled { marks, at_ms: 0 },
            },
        );
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

/// When the terminal's outpost ends with output waiting, what Core read is
/// still spoken, as NVDA speaks the lines its monitor read, but nothing is
/// asked of the ended outpost: its hold goes with the terminal's model, and
/// once everything is heard nothing of the terminal is left. The index
/// marks of what was handed to speech wait in the event queue behind the
/// re-take, which the replacement answers (design, "Focus and foreground",
/// after a reader's replacement).
#[test]
fn output_read_before_the_outpost_ended_is_spoken_and_nothing_is_asked_of_it() {
    let mut state = terminal();
    let mut playback = Playback::default();
    playback.feed(&mut state, &output(lines(1..=40)));
    assert_eq!(playback.requests, [Read::Spoken]);
    for _ in 0..10 {
        assert!(playback.play_one(&mut state));
    }
    playback.requests.clear();
    playback.feed(
        &mut state,
        &Input::Queue {
            trace_id: TraceId::from_raw(6),
            at_us: 0,
            input: verbatim_model::request::QueueInput::ReaderEnded(crate::entries::tests::OUTPOST),
        },
    );
    // The replacement starts and answers the re-take with the same
    // terminal, taken silently; the index marks waiting behind the
    // re-take then run.
    let replacement = verbatim_model::request::Instance {
        reader: verbatim_model::request::Reader::Outpost(crate::entries::tests::APP),
        generation: crate::entries::tests::GENERATION + 1,
    };
    state.queue.start_for_tests(replacement);
    playback.feed(
        &mut state,
        &queue_input(verbatim_model::request::QueueInput::ReaderReady(
            replacement,
        )),
    );
    let mut read = crate::entries::tests::focus_read(None);
    read.focus.read.values = crate::entries::tests::values("Terminal", Role::Terminal);
    read.focus.read.node = Some(NodeId::in_outpost(
        verbatim_model::OutpostId(replacement.generation),
        TERMINAL,
    ));
    playback.feed(
        &mut state,
        &queue_input(verbatim_model::request::QueueInput::Answer {
            from: replacement,
            answer: verbatim_model::request::Answer {
                number: verbatim_model::request::RequestNumber(4),
                outcome: verbatim_model::request::Outcome::Result(
                    verbatim_model::request::Reply::Focus(Box::new(read)),
                ),
                probes: Vec::new(),
                detail: String::new(),
            },
        }),
    );
    let mut heard = std::mem::take(&mut playback.heard);
    heard.extend(playback.play_all(&mut state));
    assert_eq!(heard, lines(1..=40));
    assert_eq!(playback.requests, []);
    assert!(state.terminals.is_empty());
    assert_eq!(
        state.terminal,
        super::TerminalSpeechState {
            trace: Some(TraceId::from_raw(5)),
            ..super::TerminalSpeechState::default()
        }
    );
}
