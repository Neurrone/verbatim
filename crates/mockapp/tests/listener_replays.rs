//! Replays of recorded runs across the process boundary: the flight
//! recorder's dump of Core's inputs, in the listener's order and with its
//! gaps, each UI Automation tell's check made again by a stand-in for the
//! listener that keeps its told elements as the listener does
//! (`verbatim_listener_logic::kept`), and lets them go by the held views
//! Core sends after each input, as verbatim.exe's shell sends them
//! (`verbatim_core::ListenerView`), each view taken at once or with the
//! next request to the listener.
//!
//! What a dump does not give is code-level: a check is made at its
//! event's time, the tell's `at_us`, with every request and view Core sent
//! before then taken and none sent later, as the check thread makes its
//! calls before the tell reaches Core; and a request's read starts as
//! Core sends it and ends with the recorded answer of its number. Which
//! kept elements a check reads is the listener's own rule, but their
//! keyboard-focus answers are made up, a limit: each is answered as the
//! tell's own element answered at its check, the same element at the same
//! moment, since the dump carries no answer for an element the run's
//! check did not read. The console replay's "terminal said once" rests on
//! that made-up answer; what it proves of the code is which elements each
//! check reads.
//!
//! The fixtures (`tests/fixtures/replays/`):
//!
//! - `conhost_editing_44344991044.jsonl`: `conhost_editing`'s dump in
//!   GitHub run 44344991044 to its 47th input, at commit 6cfc7f4e, each
//!   of its two foreground reports one input as Core takes them, where the console raised its terminal's focus again before
//!   Core handled the first, and Verbatim said "terminal" twice.
//! - `measure_focus_text_box_uia_focuses.jsonl`: the listener's start and
//!   its 46 UI Automation focus tells of `measure_focus_text_box`'s dump of
//!   2026-11-18, at commit ce3305a1: two Windows Forms text boxes, each
//!   focus refused by its window's verdict, the MSAA focus speaking for
//!   it.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;

use verbatim_core::{ListenerView, Runtime, View as HeldView, ViewSend, read_dump, reduce};
use verbatim_listener_logic::kept::{ListenerKept, Reported, Start, View, focus_kind};
use verbatim_model::event_kind::EventKind;
use verbatim_model::request::{ProcessFact, QueueEffect, QueueInput, Reader};
use verbatim_model::tell::{ElementToken, EventObject, Read};
use verbatim_model::{Effect, Input, Message, Pid, Role, SegmentContent, State};
use verbatim_process::held_slot::HeldSlot;

/// One utterance's contents, its index marks left out.
type Utterance = Vec<SegmentContent>;

/// What reaches the stand-in for the listener from Core.
enum Action {
    /// A request's read of a kept element starts.
    Read {
        token: ElementToken,
        number: u64,
        focus: bool,
    },
    /// A held view.
    View(HeldView),
}

/// The stand-in for the listener, its told elements carrying nothing,
/// judged by the listener's own `ListenerKept`.
#[derive(Default)]
struct Listener {
    kept: ListenerKept<()>,
    reported: Reported,
    /// The shell's record of the views it sends.
    shell: ListenerView,
    /// A view left for the next request, as the supervisor's writer leaves
    /// it in its slot for the next command.
    left: HeldSlot<HeldView>,
    /// The element each request out reads, by its number.
    reading: BTreeMap<u64, (Pid, ElementToken)>,
    /// What Core sent and the listener has not taken yet, each with the
    /// time Core sent it.
    pending: Vec<(u64, Action)>,
}

impl Listener {
    /// Takes everything Core sent at or before `at_us`, in order.
    fn reach(&mut self, at_us: u64) {
        let due = self
            .pending
            .iter()
            .take_while(|(sent, _)| *sent <= at_us)
            .count();
        let taken: Vec<(u64, Action)> = self.pending.drain(..due).collect();
        for (_, action) in taken {
            match action {
                Action::Read {
                    token,
                    number,
                    focus,
                } => {
                    if let Some(pid) = self.kept.owner(token)
                        && let Start::Read(()) = self.kept.start_read((pid, token), (number, focus))
                    {
                        self.reading.insert(number, (pid, token));
                    }
                }
                Action::View(view) => {
                    let view = View {
                        tokens: view.tokens.into_iter().collect(),
                        acknowledged: view.acknowledged,
                    };
                    let _ = self.kept.view(view, &mut self.reported);
                }
            }
        }
    }

    /// The tell of `kind` from `pid` naming `token` with `runtime_id`,
    /// checked at `at_us`: the kept elements its
    /// check reads, then the element kept and its tell written.
    fn told(
        &mut self,
        (kind, pid, at_us): (EventKind, Pid, u64),
        (token, runtime_id): (ElementToken, &verbatim_model::property::RuntimeId),
    ) -> Vec<ElementToken> {
        self.reach(at_us);
        let read = if focus_kind(kind) {
            self.kept
                .with_runtime_id(pid, runtime_id)
                .into_iter()
                .map(|(kept, ())| kept)
                .collect()
        } else {
            Vec::new()
        };
        self.kept.keep((pid, token), ((), runtime_id.clone()), kind);
        self.reported.queued(token);
        self.reported.written(true, [token]);
        self.shell.handled(token);
        read
    }

    /// The answer to request `number` reached Core at `at_us`: its read
    /// ended just before, its answer sent.
    fn answered(&mut self, number: u64, at_us: u64) {
        self.reach(at_us);
        if let Some(at) = self.reading.remove(&number) {
            let _ = self.kept.read_ended(at, (), true, &mut self.reported);
        }
    }
}

/// What a replay gave: the utterances Core said, and for each UI
/// Automation focus or menu opening, the kept elements its check read.
struct Replayed {
    said: Vec<Utterance>,
    checks: Vec<usize>,
}

/// Replays the fixture `name` through Core with the stand-in for the
/// listener.
fn replay(name: &str) -> Replayed {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/replays")
        .join(name);
    let file = File::open(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    let contents = read_dump(&mut BufReader::new(file)).expect("a dump");
    let mut state = contents.base.clone();
    let mut runtime = Runtime::default();
    let mut listener = Listener::default();
    let mut replayed = Replayed {
        said: Vec::new(),
        checks: Vec::new(),
    };
    let mut now_us = 0;
    for recorded in contents.inputs {
        let mut input = recorded.input;
        if let Input::Queue {
            input: queued,
            at_us,
            ..
        } = &mut input
        {
            now_us = *at_us;
            match queued {
                QueueInput::Process(ProcessFact::ListenerStarted(_)) => {
                    listener.shell.started();
                }
                QueueInput::Tell { from, tell } if from.reader == Reader::Listener => {
                    if let EventObject::Uia { token, runtime_id } = &tell.object {
                        let read = listener
                            .told((tell.kind, tell.process, tell.at_us), (*token, runtime_id));
                        if focus_kind(tell.kind)
                            && let Some(checked) = tell.checked.as_mut()
                        {
                            let own = checked.keyboard_focus.unwrap_or(Read::Failed);
                            replayed.checks.push(read.len());
                            checked.kept = read.into_iter().map(|kept| (kept, own)).collect();
                        }
                    }
                }
                QueueInput::Answer { from, answer } if from.reader == Reader::Listener => {
                    listener.answered(answer.number.0, now_us);
                }
                _ => {}
            }
        }
        for effect in reduce(&mut state, &mut runtime, &input) {
            match effect {
                Effect::Speak(utterance, _) => replayed.said.push(
                    utterance
                        .segments
                        .into_iter()
                        .map(|segment| segment.content)
                        .filter(|content| !matches!(content, SegmentContent::Mark(_)))
                        .collect(),
                ),
                Effect::Queue(QueueEffect::Send { to, ask }) if to.reader == Reader::Listener => {
                    let kept = match &ask.request {
                        verbatim_model::request::Question::Batch(batch) => {
                            match batch.steps.first().and_then(|step| step.op.subject()) {
                                Some(verbatim_model::batch::Subject::Kept {
                                    token, keep, ..
                                }) => Some((*token, *keep)),
                                _ => None,
                            }
                        }
                        verbatim_model::request::Question::Terminal(_) => None,
                    };
                    if let Some((token, keep)) = kept {
                        listener.pending.push((
                            now_us,
                            Action::Read {
                                token,
                                number: ask.number.0,
                                focus: keep,
                            },
                        ));
                    }
                    if let Some(view) = listener.left.carried(false) {
                        listener.pending.push((now_us, Action::View(view)));
                    }
                }
                _ => {}
            }
        }
        match listener.shell.after_pass(state.held_tokens().as_ref()) {
            ViewSend::Nothing => {}
            ViewSend::Now(view) => {
                listener.left.superseded();
                listener.pending.push((now_us, Action::View(view)));
            }
            ViewSend::Leave(view) => listener.left.leave(view, 0),
        }
    }
    replayed
}

/// The console's focuses on its terminal, the later ones checked while the
/// first's entry waited: each check reads the elements kept from the
/// tells before it, so Core takes the one it reads after the first as a
/// duplicate of the first, and says "terminal" once, after the startup's
/// desktop and the window's title. With only the elements Core had read
/// listed (ce3305a1), those checks read none, and "terminal" was said
/// twice.
#[test]
fn the_consoles_repeated_focus_says_terminal_once() {
    let replayed = replay("conhost_editing_44344991044.jsonl");
    assert_eq!(
        replayed.said,
        [
            vec![
                SegmentContent::Label("Desktop".to_owned()),
                SegmentContent::Role(Role::List)
            ],
            vec![
                SegmentContent::Label("Recycle Bin".to_owned()),
                SegmentContent::NegatedState(State::Selected),
                SegmentContent::Position {
                    position: 1,
                    set_size: Some(6)
                }
            ],
            vec![
                SegmentContent::Label("verbatim-e2e-console-terminal-editing-mwmbslsq".to_owned()),
                SegmentContent::Role(Role::Window)
            ],
            vec![SegmentContent::Role(Role::Terminal)],
            vec![SegmentContent::Message(Message::Blank)],
        ]
    );
    assert_eq!(replayed.checks, [0, 0, 1, 2, 2, 1]);
}

/// `measure_focus_text_box`'s returns to its two text boxes: each UI
/// Automation focus is refused by its window's verdict, and the view Core
/// sends after the input that refused it lets its element go, so each
/// check reads at most the one kept element whose view was not yet sent:
/// one or two keyboard-focus calls with its own. Before the held view, the
/// count rose from 1 to 22 over 20 returns (measurement of 2026-11-18).
#[test]
fn refused_text_box_focuses_are_let_go_so_each_check_reads_its_own_focus() {
    let replayed = replay("measure_focus_text_box_uia_focuses.jsonl");
    // The first return's second focus is checked before Core refused the
    // first, whose view is in flight; every later check reads none.
    let mut expected = vec![0; 46];
    expected[1] = 1;
    assert_eq!(replayed.checks, expected);
}
