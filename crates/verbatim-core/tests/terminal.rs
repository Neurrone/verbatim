//! Scripted-input tests for terminal output in the reducer (milestone M4
//! item 9): the flood policy over the backlog of output not yet spoken
//! ("30 and 30"), output arriving in several batches, lines rewritten in
//! place, typing the terminal shows, "Report new output" (Verbatim+5), and
//! speech being cut off. Playback is simulated by reaching each utterance's
//! index mark in turn.

use std::collections::VecDeque;

use verbatim_core::{SrState, reduce};
use verbatim_model::{
    Backend, Effect, Input, LineChange, Message, NodeDetails, NodeId, NodeSnapshot,
    NormalizedEvent, OutpostId, Phrase, Pid, ReaderSettings, ReviewCommand, Role, SegmentContent,
    Skipped, SpeechMark, StateSet, TerminalOutput, TraceId,
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
            changed,
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
    }
}

fn typed(text: &str) -> Input {
    Input::CharacterTyped {
        trace_id: TraceId::mint(),
        text: text.to_owned(),
    }
}

/// What one utterance says, as text.
fn words(segments: &[verbatim_model::UtteranceSegment]) -> String {
    segments
        .iter()
        .filter_map(|segment| match &segment.content {
            SegmentContent::Text(text) | SegmentContent::Character(text) => Some(text.clone()),
            SegmentContent::Phrase(Phrase::SkippedLines(count)) => Some(format!("skipped {count}")),
            SegmentContent::Phrase(Phrase::SkippedUncountedLines) => Some("skipped".to_owned()),
            SegmentContent::Message(Message::ReportNewOutputOn) => Some("on".to_owned()),
            SegmentContent::Message(Message::ReportNewOutputOff) => Some("off".to_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A speech queue that plays utterances in order, reaching each one's
/// index mark as it starts.
#[derive(Default)]
struct Playback {
    queued: VecDeque<(Option<SpeechMark>, String)>,
    heard: Vec<String>,
}

impl Playback {
    /// Takes in a step's effects.
    fn take(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::Speak(utterance) => {
                    let mark =
                        utterance
                            .segments
                            .iter()
                            .find_map(|segment| match segment.content {
                                SegmentContent::Mark(mark) => Some(mark),
                                _ => None,
                            });
                    self.queued.push_back((mark, words(&utterance.segments)));
                }
                Effect::StopSpeech => self.queued.clear(),
                _ => {}
            }
        }
    }

    /// Feeds `input` and takes in its effects.
    fn feed(&mut self, state: &mut SrState, input: &Input) {
        self.take(reduce(state, input));
    }

    /// Plays one utterance.
    fn play_one(&mut self, state: &mut SrState) -> bool {
        let Some((mark, text)) = self.queued.pop_front() else {
            return false;
        };
        self.heard.push(text);
        if let Some(mark) = mark {
            self.feed(state, &Input::MarkReached { mark });
        }
        true
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
    // Only a little is handed to speech ahead of playback.
    assert!(playback.queued.len() <= 2, "{:?}", playback.queued);
    assert_eq!(playback.play_all(&mut state), lines(1..=25));
}

#[test]
fn a_flood_skips_all_but_the_last_lines_and_says_how_many() {
    let mut state = terminal();
    let mut playback = Playback::default();
    playback.feed(&mut state, &output(lines(1..=100)));
    let heard = playback.play_all(&mut state);
    // All but the last 30 were skipped, before any was spoken.
    let mut expected = vec!["skipped 70".to_owned()];
    expected.extend(lines(71..=100));
    assert_eq!(heard, expected);
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
    // Lines 2 and 3 were already handed to speech; lines 4 to 20 and the
    // 50 unread (21 to 70) make 67 skipped, so every line is accounted for.
    let mut expected = lines(1..=3);
    expected.push("skipped 67".to_owned());
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
    let effects = reduce(&mut state, &toggle);
    assert!(effects.iter().any(|effect| matches!(
        effect,
        Effect::SettingsChanged(settings) if !settings.report_terminal_output
    )));
    playback.take(effects);
    playback.feed(&mut state, &output(lines(1..=3)));
    assert_eq!(playback.play_all(&mut state), ["off"]);
    playback.feed(&mut state, &toggle);
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
    // as when Verbatim+5 turns reporting off.
    // "line 1" played before the change, and speech held the next two.
    let heard = playback.play_all(&mut state);
    for waiting in 4..=10 {
        let line = format!("line {waiting}");
        assert!(!heard.contains(&line), "{line} was spoken: {heard:?}");
    }
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
    playback.feed(&mut state, &Input::SpeechCancelled);
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
