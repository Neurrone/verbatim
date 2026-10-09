//! Scripted-input tests for terminal output in the reducer (milestone M4
//! item 9): the flood policy over the backlog of output not yet spoken (a
//! burst's first 30 lines in full, then the last 30 of what waits), output arriving in several batches, lines rewritten in
//! place, typing the terminal shows, "Report new output" (Verbatim+5), and
//! speech being cut off. Playback is simulated by reaching each utterance's
//! index mark in turn.

use std::collections::VecDeque;

use verbatim_core::{SrState, reduce};
use verbatim_model::{
    Backend, Effect, Input, LineChange, Message, NodeDetails, NodeId, NodeSnapshot,
    NormalizedEvent, OutpostId, Phrase, Pid, QueryId, ReaderSettings, ReviewCommand, Role,
    SegmentContent, Skipped, SpeechMark, SpeechPriority, StateSet, TerminalOutput, TextOp,
    TextReply, TextRequest, TraceId,
};

const OUTPOST: OutpostId = OutpostId(1);
const TERMINAL: u64 = 12;

fn id(number: u64) -> NodeId {
    NodeId::in_outpost(OUTPOST, number)
}

fn event(event: NormalizedEvent) -> Input {
    Input::Event {
        trace_id: TraceId::mint(),
        observed_at_ms: 0,
        source: Pid(1),
        backend: Backend::Uia,
        window: None,
        event,
    }
}

/// A state whose focus is a terminal.
fn terminal() -> SrState {
    let mut state = SrState::new();
    let _ = reduce(
        &mut state,
        &event(NormalizedEvent::FocusChanged {
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
        }),
    );
    state
}

fn lines(range: std::ops::RangeInclusive<u32>) -> Vec<String> {
    range.map(|n| format!("line {n}")).collect()
}

fn output(lines: Vec<String>) -> Input {
    output_from(TERMINAL, None, None, lines)
}

fn output_from(
    node: u64,
    changed: Option<LineChange>,
    skipped: Option<Skipped>,
    lines: Vec<String>,
) -> Input {
    event(NormalizedEvent::TerminalOutput {
        node_id: id(node),
        output: TerminalOutput {
            above: Vec::new(),
            changed,
            head: Vec::new(),
            skipped,
            lines,
        },
    })
}

fn appended(text: &str, line: &str) -> LineChange {
    LineChange {
        text: text.to_owned(),
        line: line.to_owned(),
        appended: true,
        uncertain: 0,
        inserted: text.to_owned(),
        since_read: None,
    }
}

fn typed(text: &str) -> Input {
    Input::CharacterTyped {
        trace_id: TraceId::mint(),
        text: text.to_owned(),
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
            SegmentContent::Message(Message::ReportNewOutputOn) => "on".to_owned(),
            SegmentContent::Message(Message::ReportNewOutputOff) => "off".to_owned(),
            SegmentContent::Message(Message::TerminalLineCut) => "line cut".to_owned(),
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
/// a read request back until the test answers it.
#[derive(Default)]
struct Playback {
    queued: VecDeque<(Vec<SpeechMark>, String)>,
    heard: Vec<String>,
    /// Whether the outpost holds.
    held: bool,
    /// What the terminal wrote while the outpost held.
    unread: Vec<TerminalOutput>,
    /// Every terminal request Core made, in order.
    requests: Vec<TextOp>,
    /// Whether read requests wait for [`Playback::answer`].
    answer_late: bool,
    /// A read request held back.
    late: Option<QueryId>,
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
                                self.reply(state, query_id, TextReply::Terminal(Box::new(output)));
                            }
                        }
                        TextOp::TerminalCancel => {
                            self.held = false;
                            self.unread.clear();
                            self.reply(state, query_id, TextReply::Terminal(Box::default()));
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
    fn take_unread(&mut self) -> TerminalOutput {
        let mut all = TerminalOutput::default();
        for output in self.unread.drain(..) {
            all.head.extend(output.head);
            all.lines.extend(output.lines);
            all.skipped = match (all.skipped, output.skipped) {
                (Some(a), Some(b)) => Some(a.plus(b)),
                (a, b) => a.or(b),
            };
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
        self.reply(state, query_id, TextReply::Terminal(Box::new(output)));
    }

    /// Feeds `input` and takes in its effects. Terminal output the outpost
    /// holds waits in it instead.
    fn feed(&mut self, state: &mut SrState, input: &Input) {
        if self.held
            && let Input::Event {
                event: NormalizedEvent::TerminalOutput { output, .. },
                ..
            } = input
        {
            self.unread.push(output.clone());
            return;
        }
        let effects = reduce(state, input);
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
        &output_from(
            TERMINAL,
            None,
            Some(Skipped::Count(1000)),
            lines(1031..=1060),
        ),
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
            None,
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
fn output_read_before_a_cut_is_not_spoken() {
    let mut state = terminal();
    let mut playback = Playback::default();
    playback.feed(&mut state, &Input::SpeechCancelled { at_ms: 5_000 });
    let read_before = Input::Event {
        trace_id: TraceId::mint(),
        observed_at_ms: 4_999,
        source: Pid(1),
        backend: Backend::Uia,
        window: None,
        event: NormalizedEvent::TerminalOutput {
            node_id: id(TERMINAL),
            output: TerminalOutput {
                lines: lines(1..=2),
                ..TerminalOutput::default()
            },
        },
    };
    playback.feed(&mut state, &read_before);
    assert_eq!(playback.play_all(&mut state), Vec::<String>::new());
}

#[test]
fn a_flood_read_with_its_start_apart_is_spoken_from_its_start() {
    let mut state = terminal();
    let mut playback = Playback::default();
    // The outpost read the flood's first and last lines, and counted the
    // lines between.
    playback.feed(
        &mut state,
        &event(NormalizedEvent::TerminalOutput {
            node_id: id(TERMINAL),
            output: TerminalOutput {
                above: Vec::new(),
                changed: None,
                head: lines(1..=30),
                skipped: Some(Skipped::Count(2940)),
                lines: lines(2971..=3000),
            },
        }),
    );
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
        &output_from(TERMINAL, None, Some(Skipped::Count(50)), lines(71..=100)),
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
        &output_from(TERMINAL, None, Some(Skipped::Uncounted), lines(1..=3)),
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
fn a_line_rewritten_while_waiting_is_spoken_once_as_it_now_is() {
    let mut state = terminal();
    let mut playback = Playback::default();
    playback.feed(&mut state, &output(lines(1..=3)));
    // The last line, a progress bar, is rewritten twice before it is
    // spoken: only its newest version is heard.
    for percent in [40, 70] {
        playback.feed(
            &mut state,
            &output_from(
                TERMINAL,
                Some(LineChange {
                    text: format!("{percent}%"),
                    line: format!("progress {percent}%"),
                    appended: false,
                    uncertain: 0,
                    inserted: String::new(),
                    since_read: None,
                }),
                None,
                Vec::new(),
            ),
        );
    }
    assert_eq!(
        playback.play_all(&mut state),
        ["line 1", "line 2", "progress 70%"]
    );
    // Once spoken, a further rewrite speaks what changed.
    playback.feed(
        &mut state,
        &output_from(
            TERMINAL,
            Some(LineChange {
                text: "100%".to_owned(),
                line: "progress 100%".to_owned(),
                appended: false,
                uncertain: 0,
                inserted: String::new(),
                since_read: None,
            }),
            None,
            Vec::new(),
        ),
    );
    assert_eq!(playback.play_all(&mut state), ["100%"]);
}

#[test]
fn typing_is_echoed_when_the_terminal_shows_it_and_not_spoken_again() {
    let mut state = terminal();
    let mut playback = Playback::default();
    playback.feed(&mut state, &typed("l"));
    playback.feed(&mut state, &typed("s"));
    assert_eq!(playback.play_all(&mut state), Vec::<String>::new());
    // The terminal shows the typing at the end of its line: echoed as
    // characters, and not read again as output.
    playback.feed(
        &mut state,
        &output_from(
            TERMINAL,
            Some(appended("ls", "ready> ls")),
            None,
            Vec::new(),
        ),
    );
    assert_eq!(playback.play_all(&mut state), ["l", "s"]);
    // Tab completion: what the terminal adds beyond the typing is output.
    playback.feed(&mut state, &typed("\t"));
    playback.feed(
        &mut state,
        &output_from(
            TERMINAL,
            Some(appended("ass", "ready> lsass")),
            None,
            Vec::new(),
        ),
    );
    assert_eq!(playback.play_all(&mut state), ["ass"]);
}

#[test]
fn typing_after_a_prompt_s_trailing_space_is_echoed() {
    let mut state = terminal();
    let mut playback = Playback::default();
    // The prompt "ready> " was read as "ready>": its space comes back with
    // the typing, uncertain, and is not taken for something else shown.
    playback.feed(&mut state, &typed("e"));
    let mut change = appended(" e", "ready> e");
    change.uncertain = 1;
    playback.feed(
        &mut state,
        &output_from(TERMINAL, Some(change), None, Vec::new()),
    );
    assert_eq!(playback.play_all(&mut state), ["e"]);
    // Typed white space is matched as typed, padding or not.
    playback.feed(&mut state, &typed(" "));
    playback.feed(&mut state, &typed("x"));
    let mut change = appended(" x", "ready> e x");
    change.uncertain = 1;
    playback.feed(
        &mut state,
        &output_from(TERMINAL, Some(change), None, Vec::new()),
    );
    assert_eq!(playback.play_all(&mut state), [" ", "x"]);
}

#[test]
fn typing_the_terminal_does_not_show_is_never_spoken() {
    let mut state = terminal();
    let mut playback = Playback::default();
    // A password prompt shows an asterisk for each character typed.
    for character in ["s", "e"] {
        playback.feed(&mut state, &typed(character));
        playback.feed(
            &mut state,
            &output_from(
                TERMINAL,
                Some(appended("*", "Password: *")),
                None,
                Vec::new(),
            ),
        );
    }
    playback.feed(&mut state, &typed("\r"));
    playback.feed(&mut state, &output(vec!["done".to_owned()]));
    assert_eq!(playback.play_all(&mut state), ["*", "*", "done"]);
}

#[test]
fn with_passwords_spoken_typing_is_echoed_at_once_and_once_only() {
    let mut state = terminal();
    let mut playback = Playback::default();
    playback.feed(
        &mut state,
        &Input::Settings(ReaderSettings {
            speak_terminal_passwords: true,
            ..ReaderSettings::default()
        }),
    );
    playback.feed(&mut state, &typed("x"));
    playback.feed(
        &mut state,
        &output_from(TERMINAL, Some(appended("x", "ready> x")), None, Vec::new()),
    );
    assert_eq!(playback.play_all(&mut state), ["x"]);
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
    playback.feed(&mut state, &output_from(99, None, None, lines(1..=2)));
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

/// Presses Up in the terminal, and returns the query its caret watch
/// is answered under.
fn press_up(state: &mut SrState, at_ms: u64) -> QueryId {
    let _ = reduce(state, &Input::SpeechCancelled { at_ms });
    let effects = reduce(
        state,
        &Input::CaretKey {
            trace_id: TraceId::mint(),
            key: verbatim_model::CaretKey {
                motion: verbatim_model::CaretMotion::PreviousLine,
                select: false,
            },
            pressed_at_ms: at_ms,
        },
    );
    let queries: Vec<QueryId> = effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Text(TextRequest {
                query_id,
                op: TextOp::AwaitCaret(_),
                ..
            }) => Some(*query_id),
            _ => None,
        })
        .collect();
    assert_eq!(queries.len(), 1, "one caret watch in {effects:?}");
    queries[0]
}

/// The answer to a caret key whose caret moved onto `line`.
fn moved_onto(line: &str) -> TextReply {
    TextReply::Caret(Box::new(verbatim_model::CaretReply {
        moved: true,
        caret: verbatim_model::CaretReport {
            line: verbatim_model::TextChunk {
                unit: verbatim_model::TextUnit::Line,
                text: line.to_owned(),
                start: verbatim_model::TextAnchor(100),
                offset: u32::try_from(line.len()).expect("a short line"),
                languages: Vec::new(),
                first: false,
                last: false,
                truncated: false,
                formats: Vec::new(),
            },
            selection: None,
        },
        read_at_ms: 11,
        unit: None,
        selection_changes: Vec::new(),
        same_line: None,
        removed: None,
    }))
}

#[test]
fn the_line_a_caret_key_redraws_is_the_keys_once_its_answer_says_it() {
    let mut state = terminal();
    let mut playback = Playback::default();
    // Up recalls a command from the shell's history: the key's own answer
    // speaks the line, so the line's change is not spoken again.
    let query_id = press_up(&mut state, 10);
    playback.reply(&mut state, query_id, moved_onto("ready> echo one"));
    let mut recalled = appended("echo one", "ready> echo one");
    recalled.uncertain = 0;
    playback.feed(
        &mut state,
        &output_from(TERMINAL, Some(recalled), None, Vec::new()),
    );
    assert_eq!(playback.play_all(&mut state), ["ready> echo one"]);
    // Enter, a key of its own, and the command's output is spoken.
    let _ = reduce(&mut state, &Input::SpeechCancelled { at_ms: 20 });
    playback.feed(&mut state, &output(vec!["one".to_owned()]));
    assert_eq!(playback.play_all(&mut state), ["one"]);
}

#[test]
fn the_line_a_caret_key_changes_without_moving_the_caret_is_output() {
    let mut state = terminal();
    let mut playback = Playback::default();
    // Up in a menu a program redraws on the caret's line, the caret staying
    // where it was: no answer says what the key did, so the line's change
    // is spoken as output, and the watch ends saying nothing.
    let query_id = press_up(&mut state, 10);
    let change = LineChange {
        text: "CertificateAutoEnrollmentPolicy".to_owned(),
        line: "ready> Get-CertificateAutoEnrollmentPolicy".to_owned(),
        appended: false,
        uncertain: 0,
        inserted: "AutoEnrollmentPolicy".to_owned(),
        since_read: None,
    };
    playback.feed(
        &mut state,
        &output_from(TERMINAL, Some(change), None, Vec::new()),
    );
    playback.reply(&mut state, query_id, TextReply::WatchEnded);
    assert_eq!(
        playback.play_all(&mut state),
        ["CertificateAutoEnrollmentPolicy"]
    );
}

#[test]
fn a_rewrite_that_does_not_show_the_typing_never_echoes_it() {
    let mut state = terminal();
    let mut playback = Playback::default();
    // A password typed while a clock on the prompt's line ticks: the clock's
    // rewrite is spoken, the typing held is not.
    for character in ["s", "e"] {
        playback.feed(&mut state, &typed(character));
    }
    let tick = LineChange {
        text: "05".to_owned(),
        line: "Password: 12:00:05".to_owned(),
        appended: false,
        uncertain: 0,
        inserted: "5".to_owned(),
        since_read: None,
    };
    playback.feed(
        &mut state,
        &output_from(TERMINAL, Some(tick), None, Vec::new()),
    );
    assert_eq!(playback.play_all(&mut state), ["05"]);
    // A character typed in the middle of a command is echoed when the line
    // shows it there.
    playback.feed(&mut state, &typed("\r"));
    playback.feed(&mut state, &typed("x"));
    let inserted = LineChange {
        text: "abxcd".to_owned(),
        line: "ready> abxcd".to_owned(),
        appended: false,
        uncertain: 0,
        inserted: "x".to_owned(),
        since_read: None,
    };
    playback.feed(
        &mut state,
        &output_from(TERMINAL, Some(inserted), None, Vec::new()),
    );
    assert_eq!(playback.play_all(&mut state), ["x"]);
}

#[test]
fn a_clearing_key_forgets_the_typing_held() {
    // Escape, Control+C and the others come as one input.
    {
        let mut state = terminal();
        let mut playback = Playback::default();
        playback.feed(&mut state, &typed("a"));
        playback.feed(&mut state, &typed("b"));
        playback.feed(
            &mut state,
            &Input::ClearingKey {
                trace_id: TraceId::mint(),
            },
        );
        // The line then grows by what was typed: it is output, one line,
        // the typing having been forgotten (echoed, it would be two
        // characters).
        playback.feed(
            &mut state,
            &output_from(
                TERMINAL,
                Some(appended("ab", "ready> ab")),
                None,
                Vec::new(),
            ),
        );
        assert_eq!(playback.play_all(&mut state), ["ab"]);
    }
}

/// The terminal's caret on `text` at byte `offset`, as the outpost reports
/// it.
fn caret_at(text: &str, offset: u32) -> Input {
    event(NormalizedEvent::CaretMoved {
        node_id: id(TERMINAL),
        caret: verbatim_model::CaretReport {
            line: verbatim_model::TextChunk {
                unit: verbatim_model::TextUnit::Line,
                text: text.to_owned(),
                start: verbatim_model::TextAnchor(1),
                offset,
                languages: Vec::new(),
                first: false,
                last: false,
                truncated: false,
                formats: Vec::new(),
            },
            selection: None,
        },
    })
}

#[test]
fn a_typed_space_padding_hides_is_echoed_when_the_caret_moves_past_it() {
    let mut state = terminal();
    let mut playback = Playback::default();
    // The focus's first caret report reads its line, as a focus does.
    playback.feed(&mut state, &caret_at("ready> echo          ", 11));
    assert_eq!(playback.play_all(&mut state), ["ready> echo"]);
    playback.feed(&mut state, &typed(" "));
    // The line's text reads the same, but the caret moved one cell on.
    playback.feed(&mut state, &caret_at("ready> echo          ", 12));
    assert_eq!(playback.play_all(&mut state), [" "]);
    // A caret that moved by something else echoes nothing.
    playback.feed(&mut state, &typed(" "));
    playback.feed(&mut state, &caret_at("ready> echo          ", 14));
    assert_eq!(playback.play_all(&mut state), Vec::<String>::new());
}

#[test]
fn typing_is_matched_with_the_lines_change_since_it_was_read() {
    let mut state = terminal();
    let mut playback = Playback::default();
    let appended = |text: &str, line: &str| LineChange {
        text: text.to_owned(),
        line: line.to_owned(),
        appended: true,
        uncertain: text.len() - text.trim_start().len(),
        inserted: text.to_owned(),
        since_read: None,
    };
    // A letter typed again after Backspace: nothing to say from what the
    // line said, the letter since it was read.
    let back = LineChange {
        text: String::new(),
        line: "ready> ls".to_owned(),
        appended: false,
        uncertain: 0,
        inserted: String::new(),
        since_read: Some(Box::new(appended("s", "ready> ls"))),
    };
    playback.feed(&mut state, &typed("s"));
    playback.feed(
        &mut state,
        &output_from(TERMINAL, Some(back.clone()), None, Vec::new()),
    );
    assert_eq!(playback.play_all(&mut state), ["s"]);
    // " o" typed on a line Escape cleared: a rewrite of what it said, " o"
    // added since it was read.
    let retyped = LineChange {
        text: "o".to_owned(),
        line: "ready> echo o".to_owned(),
        appended: false,
        uncertain: 0,
        inserted: "o".to_owned(),
        since_read: Some(Box::new(appended(" o", "ready> echo o"))),
    };
    playback.feed(&mut state, &typed(" "));
    playback.feed(&mut state, &typed("o"));
    playback.feed(
        &mut state,
        &output_from(TERMINAL, Some(retyped), None, Vec::new()),
    );
    assert_eq!(playback.play_all(&mut state), [" ", "o"]);
    // With no typing, a line that came back to what it said is silent.
    playback.feed(
        &mut state,
        &output_from(TERMINAL, Some(back), None, Vec::new()),
    );
    assert_eq!(playback.play_all(&mut state), Vec::<String>::new());
}

#[test]
fn typing_past_the_right_margin_is_echoed_not_spoken_as_output() {
    let mut state = terminal();
    let mut playback = Playback::default();
    for character in ["a", "b", "c", "d"] {
        playback.feed(&mut state, &typed(character));
    }
    // The row filled with "ab"; "cd" went onto a row of its own, echoed
    // character by character, not spoken as one line of output.
    playback.feed(
        &mut state,
        &output_from(
            TERMINAL,
            Some(appended("ab", "ready> ab")),
            None,
            vec!["cd".to_owned()],
        ),
    );
    assert_eq!(playback.play_all(&mut state), ["a", "b", "c", "d"]);
}

#[test]
fn typing_over_ghost_text_that_showed_it_already_is_echoed() {
    let mut state = terminal();
    let mut playback = Playback::default();
    playback.feed(&mut state, &caret_at("ready> git status", 8));
    assert_eq!(playback.play_all(&mut state), ["ready> git status"]);
    // The prediction showed "it status"; typing "i" changes no text.
    playback.feed(&mut state, &typed("i"));
    playback.feed(&mut state, &caret_at("ready> git status", 9));
    assert_eq!(playback.play_all(&mut state), ["i"]);
    // A character the prediction did not show waits for the terminal.
    playback.feed(&mut state, &typed("x"));
    playback.feed(&mut state, &caret_at("ready> git status", 10));
    assert_eq!(playback.play_all(&mut state), Vec::<String>::new());
}

#[test]
fn typing_the_caret_shows_before_the_screen_read_is_echoed_once() {
    let mut state = terminal();
    let mut playback = Playback::default();
    playback.feed(&mut state, &caret_at("ready>          ", 7));
    assert_eq!(playback.play_all(&mut state), ["ready>"]);
    playback.feed(&mut state, &typed("."));
    // The console host's caret reports, read before the screen: the line
    // already shows the "." and the caret then moves past it.
    playback.feed(&mut state, &caret_at("ready> .        ", 7));
    playback.feed(&mut state, &caret_at("ready> .        ", 8));
    let shown = LineChange {
        text: " .".to_owned(),
        line: "ready> .".to_owned(),
        appended: true,
        uncertain: 1,
        inserted: " .".to_owned(),
        since_read: None,
    };
    playback.feed(
        &mut state,
        &output_from(TERMINAL, Some(shown), None, Vec::new()),
    );
    assert_eq!(playback.play_all(&mut state), ["."]);
}

#[test]
fn a_change_since_read_that_is_not_the_typing_is_never_spoken() {
    let mut state = terminal();
    let mut playback = Playback::default();
    // A key a full-screen program reads without showing it, then a row it
    // scrolled in: from what the line said, nothing; since it was read,
    // "row 0", which is output already reported above it, not typing.
    playback.feed(&mut state, &typed("n"));
    let scrolled = LineChange {
        text: String::new(),
        line: "row 0".to_owned(),
        appended: false,
        uncertain: 0,
        inserted: String::new(),
        since_read: Some(Box::new(LineChange {
            text: "row 0".to_owned(),
            line: "row 0".to_owned(),
            appended: true,
            uncertain: 0,
            inserted: "row 0".to_owned(),
            since_read: None,
        })),
    };
    let output = TerminalOutput {
        above: vec!["row 0".to_owned()],
        changed: Some(scrolled),
        ..TerminalOutput::default()
    };
    playback.feed(
        &mut state,
        &event(NormalizedEvent::TerminalOutput {
            node_id: id(TERMINAL),
            output,
        }),
    );
    assert_eq!(playback.play_all(&mut state), ["row 0"]);
}

#[test]
fn the_blank_lines_a_burst_starts_with_are_not_counted() {
    let mut state = terminal();
    let mut playback = Playback::default();
    // A footer drawn on the last row of an empty screen: the rows above it
    // are blank, and not output.
    let mut drawn: Vec<String> = (0..28).map(|_| String::new()).collect();
    drawn.extend(lines(1..=30));
    drawn.extend((0..10).map(|_| String::new()));
    drawn.extend(lines(41..=70));
    playback.feed(&mut state, &output(drawn));
    let mut expected = lines(1..=30);
    expected.push("skipped 10".to_owned());
    expected.extend(lines(41..=70));
    assert_eq!(playback.play_all(&mut state), expected);
}
