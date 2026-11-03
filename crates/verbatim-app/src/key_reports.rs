//! The keyboard hook's two callbacks, which run on the hook thread
//! (`docs/design/ownership.md`):
//! the speech effect of a key, and the reports of what
//! each caused, each turned into its commands for the reducer and posted
//! to the reducer's port, in the order the hook decided them, with nothing
//! between the hook and the port to reorder them.
//!
//! Nothing here blocks or takes a lock: the lookups are in tables made
//! when the hook starts and never changed, the posts are completion-port
//! posts, and `cargo xtask ci` checks this module as it checks the hook's
//! own (`xtask/src/ownership.rs`, the hook rule).
//!
//! A post a port refuses, which only a system out of memory does, is kept
//! on the hook thread and posted again ahead of the next key's posts
//! ([`Ports`]), with no lock and no timer.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use verbatim_input::{EmittedGesture, KeySpeechEffect, KeyboardLayout, ScriptAction, ToggleKey};
use verbatim_model::{CaretKey, CaretMotion, GestureId, Input, KeySeq, LockKey, Script};
use verbatim_process::port_queue::Poster;
use verbatim_speech::SpeechControl;

use crate::latency::LatencyLedger;
use verbatim_input_windows::KeyReport;

use crate::{SHOW_MENU_GESTURE, ShellCommand};

/// What a gesture or an observed key is to the reducer, looked up on the
/// hook thread: fixed when the hook starts.
pub(crate) struct Tables {
    caret_keys: HashMap<GestureId, CaretKey>,
    clearing_keys: HashSet<GestureId>,
    show_menu: GestureId,
    scripts: HashMap<GestureId, ScriptAction>,
    toggles: HashMap<GestureId, LockKey>,
}

impl Tables {
    /// The tables for keyboard layout `layout`.
    pub(crate) fn for_layout(layout: KeyboardLayout) -> Self {
        Self {
            caret_keys: verbatim_input::caret_bindings().into_iter().collect(),
            clearing_keys: verbatim_input::clearing_keys().into_iter().collect(),
            show_menu: GestureId::parse(SHOW_MENU_GESTURE).expect("valid binding"),
            scripts: verbatim_input::bindings_for(layout).into_iter().collect(),
            toggles: [
                ToggleKey::CapsLock,
                ToggleKey::NumLock,
                ToggleKey::ScrollLock,
            ]
            .into_iter()
            .map(|toggle| (toggle.gesture(), toggle.lock_key()))
            .collect(),
        }
    }
}

/// One of the hook's posts: a command for the reducer's port, or a key's
/// cancel or pause toggle for the speech queue's.
enum Post {
    Command(ShellCommand),
    Cancel(KeySeq),
    TogglePause,
}

/// What the hook's fault point is told of a post: which port it is for,
/// and the key it is of, when it has one.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PostSubject {
    speech: bool,
    key_seq: Option<KeySeq>,
}

thread_local! {
    /// The hook thread's posts a port refused while it was still taken,
    /// with the key each is of, in the hook's order: empty but while a
    /// system out of memory refuses packets.
    static KEPT: RefCell<VecDeque<(Post, Option<KeySeq>)>> = const { RefCell::new(VecDeque::new()) };
}

/// The two ports the hook's callbacks post to, through one function on the
/// hook thread ([`Ports::post`]): a post a port refuses is kept and posted
/// again, ahead of the next one, so no command or cancel overtakes a
/// refused one, and a key's cancel and the reducer's `SpeechCancelled`
/// are both kept or both made. One whose port has closed is dropped, its
/// taker having gone.
#[derive(Clone)]
pub(crate) struct Ports {
    commands: Poster<ShellCommand>,
    control: SpeechControl,
}

impl Ports {
    /// The reducer's port, `commands`, and the speech queue's, `control`.
    pub(crate) fn new(commands: Poster<ShellCommand>, control: SpeechControl) -> Self {
        Self { commands, control }
    }

    /// Posts `post`, of key `key_seq` if any: at once while nothing is
    /// kept; otherwise behind what is kept, which is posted from its front
    /// until a port refuses again.
    fn post(&self, post: Post, key_seq: Option<KeySeq>) {
        KEPT.with_borrow_mut(|kept| {
            if kept.is_empty() {
                if let Some(refused) = self.try_post(post, key_seq) {
                    kept.push_back((refused, key_seq));
                }
                return;
            }
            kept.push_back((post, key_seq));
            while let Some((post, key_seq)) = kept.pop_front() {
                if let Some(refused) = self.try_post(post, key_seq) {
                    kept.push_front((refused, key_seq));
                    return;
                }
            }
        });
    }

    /// Posts `post`, and gives it back when its port refused it and is
    /// still taken; one taken, or whose port has closed, is gone.
    fn try_post(&self, post: Post, key_seq: Option<KeySeq>) -> Option<Post> {
        #[cfg(test)]
        let refused = verbatim_faults::hit(
            verbatim_faults::Fault::KeyPostRefused,
            &PostSubject {
                speech: !matches!(post, Post::Command(_)),
                key_seq,
            },
        );
        #[cfg(not(test))]
        let refused = {
            let _ = key_seq;
            false
        };
        match post {
            Post::Command(command) => {
                let given_back = if refused {
                    Err(command)
                } else {
                    self.commands.send(command)
                };
                match given_back {
                    Ok(()) => None,
                    Err(command) => self.commands.is_open().then_some(Post::Command(command)),
                }
            }
            Post::Cancel(key) => {
                if !refused && self.control.cancel_through(key) {
                    return None;
                }
                self.control.is_open().then_some(Post::Cancel(key))
            }
            Post::TogglePause => {
                if !refused && self.control.toggle_pause() {
                    return None;
                }
                self.control.is_open().then_some(Post::TogglePause)
            }
        }
    }
}

/// The hook's speech effect: a cancel names the key's number to the speech
/// queue, which fences off speech earlier keys caused, and tells the
/// reducer, whose say-all stops; Shift's pause toggles.
pub(crate) fn speech_effect(ports: Ports) -> Box<dyn Fn(KeySpeechEffect, KeySeq) + Send> {
    Box::new(move |effect, key_seq| match effect {
        KeySpeechEffect::Cancel => {
            ports.post(Post::Cancel(key_seq), Some(key_seq));
            ports.post(
                Post::Command(ShellCommand::Input(Box::new(Input::SpeechCancelled {
                    at_ms: crate::latency::now_ms(),
                }))),
                Some(key_seq),
            );
        }
        KeySpeechEffect::TogglePause => ports.post(Post::TogglePause, Some(key_seq)),
    })
}

/// The key a report is of, when it names one.
fn key_of(report: &KeyReport) -> Option<KeySeq> {
    match report {
        KeyReport::Observed { key_seq, .. }
        | KeyReport::Held { key_seq, .. }
        | KeyReport::Passed { key_seq, .. }
        | KeyReport::Typed { key_seq, .. }
        | KeyReport::Gesture { key_seq, .. } => Some(*key_seq),
        KeyReport::Deferred { .. } | KeyReport::Handled(_) => None,
    }
}

/// The hook's report callback: notes each key the latency line counts
/// from, then posts the report's commands to the reducer.
pub(crate) fn reports(
    tables: Tables,
    ledger: Arc<LatencyLedger>,
    ports: Ports,
) -> Box<dyn Fn(KeyReport) + Send> {
    Box::new(move |report| {
        // Every key the hook saw, for the latency line's time from the
        // last key to speech.
        match &report {
            KeyReport::Typed { pressed_at_us, .. } => ledger.key_seen(*pressed_at_us, true),
            KeyReport::Observed { pressed_at_us, .. }
            | KeyReport::Held { pressed_at_us, .. }
            | KeyReport::Passed { pressed_at_us, .. } => ledger.key_seen(*pressed_at_us, false),
            _ => {}
        }
        let key_seq = key_of(&report);
        commands_for(report, &tables, |command| {
            ports.post(Post::Command(command), key_seq);
        });
    })
}

/// The commands `report` is to the reducer, in order, each given to
/// `post`: none for a gesture nothing is bound to.
fn commands_for(report: KeyReport, tables: &Tables, mut post: impl FnMut(ShellCommand)) {
    let other_key = CaretKey {
        motion: CaretMotion::Other,
        select: false,
    };
    match report {
        KeyReport::Observed {
            gesture,
            key_seq,
            pressed_at_us,
        } => {
            let caret_key = |caret| ShellCommand::CaretKey {
                input: Box::new(Input::CaretKey {
                    trace_id: gesture.trace_id,
                    key_seq,
                    key: caret,
                    pressed_at_ms: pressed_at_us / 1_000,
                }),
                pressed_at_us,
            };
            if let Some(&caret) = tables.caret_keys.get(&gesture.gesture) {
                post(caret_key(caret));
            } else if tables.clearing_keys.contains(&gesture.gesture) {
                // The typing held is forgotten first; then the key is
                // judged like any other (Escape clearing a line speaks what
                // it removed).
                post(ShellCommand::Input(Box::new(Input::ClearingKey {
                    trace_id: gesture.trace_id,
                    key_seq,
                    at_ms: pressed_at_us / 1_000,
                })));
                post(caret_key(other_key));
            }
        }
        // A held key always reaches the reducer thread, which sends it on;
        // the hook holds only caret keys, so it is one.
        KeyReport::Held {
            gesture,
            key_seq,
            pressed_at_us,
            stroke,
        } => post(ShellCommand::HeldKey {
            input: tables.caret_keys.get(&gesture.gesture).map(|&caret| {
                Box::new(Input::HeldKey {
                    trace_id: gesture.trace_id,
                    key_seq,
                    key: caret,
                    pressed_at_ms: pressed_at_us / 1_000,
                })
            }),
            trace_id: gesture.trace_id,
            gesture: gesture.gesture,
            pressed_at_us,
            stroke,
        }),
        KeyReport::Deferred { stroke, pressed } => {
            post(ShellCommand::DeferredKey { stroke, pressed });
        }
        KeyReport::Passed {
            trace_id,
            key_seq,
            pressed_at_us,
        } => post(ShellCommand::CaretKey {
            input: Box::new(Input::CaretKey {
                trace_id,
                key_seq,
                key: other_key,
                pressed_at_ms: pressed_at_us / 1_000,
            }),
            pressed_at_us,
        }),
        KeyReport::Typed {
            trace_id,
            key_seq,
            text,
            pressed_at_us,
        } => post(ShellCommand::Input(Box::new(Input::CharacterTyped {
            trace_id,
            key_seq,
            text,
            at_ms: pressed_at_us / 1_000,
        }))),
        KeyReport::Gesture {
            gesture,
            key_seq,
            pressed_at_us,
        } => {
            if let Some(input) = gesture_input(&gesture, key_seq, pressed_at_us, tables) {
                post(ShellCommand::Input(Box::new(input)));
            }
        }
        KeyReport::Handled(number) => post(ShellCommand::InputHandled(number)),
    }
}

/// The input a gesture is to the reducer: the menu's, the time's or the
/// shell list's script, a lock key's press, or a review command; `None`
/// for a gesture nothing is bound to.
fn gesture_input(
    gesture: &EmittedGesture,
    key_seq: KeySeq,
    pressed_at_us: u64,
    tables: &Tables,
) -> Option<Input> {
    let trace_id = gesture.trace_id;
    let repeat = gesture.repeat;
    let at_ms = pressed_at_us / 1_000;
    let script = |script| Input::Script {
        trace_id,
        key_seq,
        repeat,
        script,
        pressed_at_ms: at_ms,
    };
    if gesture.gesture == tables.show_menu {
        return Some(script(Script::ShowMenu));
    }
    if let Some(&toggle) = tables.toggles.get(&gesture.gesture) {
        return Some(Input::ToggleKeyPressed {
            trace_id,
            key_seq,
            toggle,
            pressed_at_ms: at_ms,
        });
    }
    match tables.scripts.get(&gesture.gesture)? {
        ScriptAction::SpeakTime => Some(script(Script::ReportTime)),
        ScriptAction::ShowTrayList => Some(script(Script::ShellList)),
        action => action.review_command().map(|command| Input::Command {
            trace_id,
            key_seq,
            command,
            repeat,
            pressed_at_ms: at_ms,
        }),
    }
}

#[cfg(test)]
mod tests {
    use verbatim_input_windows::HeldStroke;
    use verbatim_model::{ReviewCommand, TraceId};

    use super::*;

    /// What each command `report` is to the reducer says, in order, with
    /// the desktop layout's tables.
    fn mapped(report: KeyReport) -> Vec<String> {
        mapped_in(KeyboardLayout::Desktop, report)
    }

    /// What each command `report` is to the reducer says, in order, with
    /// `layout`'s tables.
    fn mapped_in(layout: KeyboardLayout, report: KeyReport) -> Vec<String> {
        let mut commands = Vec::new();
        commands_for(report, &Tables::for_layout(layout), |command| {
            commands.push(command);
        });
        commands
            .into_iter()
            .map(|command| match command {
                ShellCommand::Input(input) | ShellCommand::CaretKey { input, .. } => {
                    format!("{input:?}")
                }
                ShellCommand::HeldKey { input, .. } => format!("held {input:?}"),
                ShellCommand::DeferredKey { pressed, .. } => format!("deferred {pressed}"),
                ShellCommand::InputHandled(number) => format!("handled {number}"),
                _ => "something else".to_owned(),
            })
            .collect()
    }

    fn emitted(trace_id: TraceId, gesture: &str, repeat: u8) -> EmittedGesture {
        EmittedGesture {
            trace_id,
            gesture: GestureId::parse(gesture).expect("valid id"),
            repeat,
        }
    }

    const PRESSED_AT_US: u64 = 1_700_000_000_123_000;

    fn gesture(trace_id: TraceId, name: &str, repeat: u8) -> KeyReport {
        KeyReport::Gesture {
            gesture: emitted(trace_id, name, repeat),
            key_seq: KeySeq::new(4),
            pressed_at_us: PRESSED_AT_US,
        }
    }

    #[test]
    fn each_report_maps_to_its_commands() {
        let trace_id = TraceId::mint();
        let key_seq = KeySeq::new(4);
        let caret = |motion| Input::CaretKey {
            trace_id,
            key_seq,
            key: CaretKey {
                motion,
                select: false,
            },
            pressed_at_ms: PRESSED_AT_US / 1_000,
        };
        let observed = |name| KeyReport::Observed {
            gesture: emitted(trace_id, name, 0),
            key_seq,
            pressed_at_us: PRESSED_AT_US,
        };
        assert_eq!(
            mapped(observed("kb:downarrow")),
            [format!("{:?}", caret(CaretMotion::NextLine))]
        );
        assert_eq!(
            mapped(observed("kb:escape")),
            [
                format!(
                    "{:?}",
                    Input::ClearingKey {
                        trace_id,
                        key_seq,
                        at_ms: PRESSED_AT_US / 1_000
                    }
                ),
                format!("{:?}", caret(CaretMotion::Other)),
            ]
        );
        assert_eq!(mapped(KeyReport::Handled(3)), ["handled 3"]);
        assert_eq!(
            mapped(KeyReport::Typed {
                trace_id,
                key_seq,
                text: "a".to_owned(),
                pressed_at_us: PRESSED_AT_US
            }),
            [format!(
                "{:?}",
                Input::CharacterTyped {
                    trace_id,
                    key_seq,
                    text: "a".to_owned(),
                    at_ms: PRESSED_AT_US / 1_000
                }
            )]
        );
    }

    #[test]
    fn each_gesture_maps_to_its_script_command_or_lock_key() {
        let trace_id = TraceId::mint();
        let key_seq = KeySeq::new(4);
        let script = |script, repeat| {
            format!(
                "{:?}",
                Input::Script {
                    trace_id,
                    key_seq,
                    repeat,
                    script,
                    pressed_at_ms: PRESSED_AT_US / 1_000
                }
            )
        };
        assert_eq!(
            mapped(gesture(trace_id, "kb:verbatim+v", 0)),
            [script(Script::ShowMenu, 0)]
        );
        assert_eq!(
            mapped(gesture(trace_id, "kb:verbatim+f12", 1)),
            [script(Script::ReportTime, 1)]
        );
        assert_eq!(
            mapped(gesture(trace_id, "kb:verbatim+f11", 0)),
            [script(Script::ShellList, 0)]
        );
        assert_eq!(
            mapped(gesture(trace_id, "kb:numpad8", 2)),
            [format!(
                "{:?}",
                Input::Command {
                    trace_id,
                    key_seq,
                    command: ReviewCommand::ReviewCurrentLine,
                    repeat: 2,
                    pressed_at_ms: PRESSED_AT_US / 1_000
                }
            )]
        );
        assert_eq!(
            mapped(gesture(trace_id, "kb:scrolllock", 0)),
            [format!(
                "{:?}",
                Input::ToggleKeyPressed {
                    trace_id,
                    key_seq,
                    toggle: LockKey::ScrollLock,
                    pressed_at_ms: PRESSED_AT_US / 1_000
                }
            )]
        );
        assert_eq!(
            mapped(gesture(trace_id, "kb:verbatim+q", 0)),
            Vec::<String>::new()
        );
    }

    /// A laptop gesture maps through the laptop layout's table, and the
    /// desktop layout, which does not bind it, maps it to nothing.
    #[test]
    fn a_laptop_gesture_maps_to_its_command_only_in_the_laptop_layout() {
        let trace_id = TraceId::mint();
        let report = || gesture(trace_id, "kb:verbatim+shift+period", 1);
        assert_eq!(
            mapped_in(KeyboardLayout::Laptop, report()),
            [format!(
                "{:?}",
                Input::Command {
                    trace_id,
                    key_seq: KeySeq::new(4),
                    command: ReviewCommand::ReviewCurrentLine,
                    repeat: 1,
                    pressed_at_ms: PRESSED_AT_US / 1_000
                }
            )]
        );
        assert_eq!(
            mapped_in(KeyboardLayout::Desktop, report()),
            Vec::<String>::new()
        );
    }

    /// The reducer's port and the speech queue's, real completion-port
    /// queues, as the hook posts to them, with their taking ends.
    fn ports() -> (
        Ports,
        verbatim_process::port_queue::PortQueue<ShellCommand>,
        verbatim_process::port_queue::PortQueue<verbatim_speech::QueueItem>,
    ) {
        let (commands, commands_taken) =
            verbatim_process::port_queue::port_queue().expect("the reducer's port");
        let (speech, speech_taken) =
            verbatim_process::port_queue::port_queue().expect("the speech queue's port");
        let control = SpeechControl::with_inbox(Arc::new(crate::PortInbox(speech)));
        (Ports::new(commands, control), commands_taken, speech_taken)
    }

    /// The key each command waiting on `port` is of, typed text being the
    /// only command these tests post, and `None` for a cancel's
    /// `SpeechCancelled`.
    fn typed_keys(
        port: &mut verbatim_process::port_queue::PortQueue<ShellCommand>,
    ) -> Vec<Option<KeySeq>> {
        std::iter::from_fn(|| port.try_recv())
            .map(|command| match command {
                ShellCommand::Input(input) => match *input {
                    Input::CharacterTyped { key_seq, .. } => Some(key_seq),
                    Input::SpeechCancelled { .. } => None,
                    other => panic!("the hook posted {other:?}"),
                },
                _ => panic!("the hook posted something other than an input"),
            })
            .collect()
    }

    /// The key each cancel waiting on `port` names, `None` for any other
    /// item, a pause toggle being the only other these tests post.
    fn cancelled_keys(
        port: &mut verbatim_process::port_queue::PortQueue<verbatim_speech::QueueItem>,
    ) -> Vec<Option<KeySeq>> {
        std::iter::from_fn(|| port.try_recv())
            .map(|item| item.cancels_through())
            .collect()
    }

    fn typed(key_seq: KeySeq) -> KeyReport {
        KeyReport::Typed {
            trace_id: TraceId::mint(),
            key_seq,
            text: "a".to_owned(),
            pressed_at_us: PRESSED_AT_US,
        }
    }

    /// A key's command the reducer's port refuses is kept on the hook
    /// thread and posted ahead of the next key's: each is taken once, in
    /// the hook's order.
    #[test]
    fn a_refused_key_command_is_posted_before_the_next_keys_commands() {
        // Keys no other test posts, so the selector names this test's post.
        let (first, second) = (KeySeq::new(91_001), KeySeq::new(91_002));
        let refusal = verbatim_faults::arm(
            verbatim_faults::Fault::KeyPostRefused,
            move |post: &PostSubject| {
                *post
                    == PostSubject {
                        speech: false,
                        key_seq: Some(first),
                    }
            },
            1,
        );
        let (ports, mut commands, _speech) = ports();
        let report = reports(
            Tables::for_layout(KeyboardLayout::Desktop),
            Arc::new(LatencyLedger::new(1, 1, Arc::default())),
            ports,
        );

        report(typed(first));
        assert_eq!(typed_keys(&mut commands), [], "the refused command is kept");
        report(typed(second));
        assert_eq!(
            typed_keys(&mut commands),
            [Some(first), Some(second)],
            "the kept command first, then the next key's"
        );
        drop(refusal);
    }

    /// A key's cancel the speech queue's port refuses is kept, and the
    /// `SpeechCancelled` the key posts after it waits behind it; both are
    /// posted ahead of the next key's posts, each taken once.
    #[test]
    fn a_refused_speech_cancel_is_posted_before_the_next_keys_posts() {
        // Keys no other test posts, so the selector names this test's post.
        let (first, second) = (KeySeq::new(92_001), KeySeq::new(92_002));
        // Refused as the key posts it, and again as the key's own
        // `SpeechCancelled` is posted behind it.
        let refusal = verbatim_faults::arm(
            verbatim_faults::Fault::KeyPostRefused,
            move |post: &PostSubject| {
                *post
                    == PostSubject {
                        speech: true,
                        key_seq: Some(first),
                    }
            },
            2,
        );
        let (ports, mut commands, mut speech) = ports();
        let effect = speech_effect(ports);

        effect(KeySpeechEffect::Cancel, first);
        assert_eq!(
            cancelled_keys(&mut speech),
            [],
            "the refused cancel is kept"
        );
        assert_eq!(
            typed_keys(&mut commands),
            [],
            "the key's SpeechCancelled waits behind its cancel"
        );
        effect(KeySpeechEffect::Cancel, second);
        assert_eq!(
            cancelled_keys(&mut speech),
            [Some(first), Some(second)],
            "the kept cancel first, then the next key's"
        );
        assert_eq!(
            typed_keys(&mut commands),
            [None, None],
            "each key's SpeechCancelled, once"
        );
        drop(refusal);
    }

    /// Shift's pause toggle waits behind a key's kept cancel rather than
    /// overtaking it.
    #[test]
    fn a_pause_toggle_is_posted_behind_a_kept_speech_cancel() {
        // A key no other test posts, so the selector names this test's post.
        let key = KeySeq::new(93_001);
        // Refused as the key posts it, and again as the key's own
        // `SpeechCancelled` is posted behind it.
        let refusal = verbatim_faults::arm(
            verbatim_faults::Fault::KeyPostRefused,
            move |post: &PostSubject| {
                *post
                    == PostSubject {
                        speech: true,
                        key_seq: Some(key),
                    }
            },
            2,
        );
        let (ports, _commands, mut speech) = ports();
        let effect = speech_effect(ports);

        effect(KeySpeechEffect::Cancel, key);
        assert_eq!(
            cancelled_keys(&mut speech),
            [],
            "the refused cancel is kept"
        );
        effect(KeySpeechEffect::TogglePause, key);
        assert_eq!(
            cancelled_keys(&mut speech),
            [Some(key), None],
            "the kept cancel first, then the toggle"
        );
        drop(refusal);
    }

    #[test]
    fn a_held_caret_key_maps_to_its_held_key() {
        let trace_id = TraceId::mint();
        let key_seq = KeySeq::new(4);
        assert_eq!(
            mapped(KeyReport::Held {
                gesture: emitted(trace_id, "kb:downarrow", 0),
                key_seq,
                pressed_at_us: PRESSED_AT_US,
                stroke: HeldStroke {
                    vk: 0x28,
                    scan_code: 0x50,
                    extended: true,
                    sequence: 1
                },
            }),
            [format!(
                "held {:?}",
                Some(Box::new(Input::HeldKey {
                    trace_id,
                    key_seq,
                    key: CaretKey {
                        motion: CaretMotion::NextLine,
                        select: false
                    },
                    pressed_at_ms: PRESSED_AT_US / 1_000
                }))
            )]
        );
    }
}
