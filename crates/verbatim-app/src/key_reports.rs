//! The keyboard hook's two callbacks, which run on the hook thread
//! (`docs/design/ownership-input.md`, "One route, to the reducer's port"):
//! the speech effect of a key or injected gesture, and the reports of what
//! each caused, each turned into its commands for the reducer and posted
//! to the reducer's port, in the order the hook decided them, with nothing
//! between the hook and the port to reorder them.
//!
//! Nothing here blocks or takes a lock: the lookups are in tables made
//! when the hook starts and never changed, the posts are completion-port
//! posts, and `cargo xtask ci` checks this module as it checks the hook's
//! own (`xtask/src/ownership.rs`, the hook rule).

use std::collections::{HashMap, HashSet};
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

/// The hook's speech effect: a cancel names the key's number to the speech
/// queue, which fences off speech earlier keys caused, and tells the
/// reducer, whose say-all stops; Shift's pause toggles.
pub(crate) fn speech_effect(
    control: SpeechControl,
    commands: Poster<ShellCommand>,
) -> Box<dyn Fn(KeySpeechEffect, KeySeq) + Send> {
    Box::new(move |effect, key_seq| match effect {
        KeySpeechEffect::Cancel => {
            control.cancel_through(key_seq);
            let _ = commands.send(ShellCommand::Input(Box::new(Input::SpeechCancelled {
                at_ms: crate::latency::now_ms(),
            })));
        }
        KeySpeechEffect::TogglePause => control.toggle_pause(),
    })
}

/// The hook's report callback: notes each key the latency line counts
/// from, then posts the report's commands to the reducer.
pub(crate) fn reports(
    tables: Tables,
    ledger: Arc<LatencyLedger>,
    commands: Poster<ShellCommand>,
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
        commands_for(report, &tables, |command| {
            let _ = commands.send(command);
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
        KeyReport::Barrier(token) => post(ShellCommand::AwaitIdle(token)),
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

    /// What each command `report` is to the reducer says, in order.
    fn mapped(report: KeyReport) -> Vec<String> {
        let mut commands = Vec::new();
        commands_for(
            report,
            &Tables::for_layout(KeyboardLayout::Desktop),
            |command| commands.push(command),
        );
        commands
            .into_iter()
            .map(|command| match command {
                ShellCommand::Input(input) | ShellCommand::CaretKey { input, .. } => {
                    format!("{input:?}")
                }
                ShellCommand::HeldKey { input, .. } => format!("held {input:?}"),
                ShellCommand::DeferredKey { pressed, .. } => format!("deferred {pressed}"),
                ShellCommand::InputHandled(number) => format!("handled {number}"),
                ShellCommand::AwaitIdle(token) => format!("await idle {token}"),
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
        assert_eq!(mapped(KeyReport::Barrier(5)), ["await idle 5"]);
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
