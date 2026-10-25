//! Unit tests of Core's terminal model: what the reading rules say becomes
//! the flood policy's speech, and each key input becomes the rules' key.

use verbatim_model::{
    Backend, CaretKey, CaretMotion, Input, NodeDetails, NodeId, NodeSnapshot, NormalizedEvent,
    OutpostId, Pid, Role, Skipped, StateSet, TerminalItem, TerminalKeyKind, TerminalSpeech,
    TerminalUnit, TraceId,
};
use verbatim_text::terminal_reading::{Settings, Speech, Step};
use verbatim_text::terminal_rows::Lost;

use super::{TerminalModel, rules_key, speech_of};
use crate::state::SrState;

#[test]
fn what_the_rules_say_is_cores_speech_in_order() {
    let step = Step {
        speech: vec![
            Speech::Caret {
                row: 2,
                unit: verbatim_text::terminal_reading::Unit::Word,
                text: "word".to_owned(),
            },
            Speech::LostRead(Lost::About),
            Speech::Line {
                row: 3,
                text: "first".to_owned(),
            },
            Speech::Growth {
                row: 6,
                text: " grown".to_owned(),
            },
            Speech::Echo {
                row: 2,
                text: "x".to_owned(),
            },
        ],
        followed: true,
        ..Step::default()
    };
    assert_eq!(
        speech_of(&step),
        TerminalSpeech {
            items: vec![
                TerminalItem::Caret {
                    unit: TerminalUnit::Word,
                    text: "word".to_owned(),
                },
                // After a lost read, its lines follow an "about" count of
                // none; a line's growth is part of its line.
                TerminalItem::Skipped(Skipped::About(0)),
                TerminalItem::Output("first".to_owned()),
                TerminalItem::Growth("grown".to_owned()),
                TerminalItem::Echo("x".to_owned()),
            ],
            followed: true,
        }
    );
}

#[test]
fn each_key_input_is_the_rules_key_and_a_clearing_key_forget_then_other() {
    let node = NodeId::in_outpost(OutpostId(1), 12);
    let mut state = SrState::new();
    let _ = crate::reduce(
        &mut state,
        &Input::Event {
            trace_id: TraceId::mint(),
            observed_at_ms: 0,
            observed_at_us: 0,
            source: Pid(1),
            backend: Backend::Uia,
            window: None,
            event: NormalizedEvent::FocusChanged {
                node: NodeSnapshot {
                    id: node,
                    backend: Backend::Uia,
                    role: Role::Terminal,
                    name: None,
                    value: None,
                    states: StateSet::new(),
                    details: NodeDetails::default(),
                },
                foreground: false,
                ancestors: Vec::new(),
                ancestors_unknown: false,
                selected_child: None,
                text: None,
            },
        },
    );
    state
        .terminals
        .push(TerminalModel::new(node, Settings::default()));
    let caret = |motion, at_ms| Input::CaretKey {
        trace_id: TraceId::mint(),
        key: CaretKey {
            motion,
            select: false,
        },
        pressed_at_ms: at_ms,
    };
    let typed = |text: &str, at_ms| Input::CharacterTyped {
        trace_id: TraceId::mint(),
        text: text.to_owned(),
        at_ms,
    };
    for input in [
        caret(CaretMotion::NextWord, 1),
        caret(CaretMotion::PreviousLine, 2),
        caret(CaretMotion::Delete, 3),
        typed("a", 4),
        typed("\r", 5),
        typed("\t", 6),
        typed("\u{1}", 7),
        // Escape: the hook's clearing key, then the key passed.
        Input::ClearingKey {
            trace_id: TraceId::mint(),
            at_ms: 8,
        },
        caret(CaretMotion::Other, 8),
    ] {
        let _ = crate::reduce(&mut state, &input);
    }
    let kept: Vec<_> = state.terminals[0]
        .pending
        .iter()
        .map(|key| rules_key(&key.kind, key.at_ms))
        .collect();
    let expected: Vec<_> = [
        (TerminalKeyKind::Caret(TerminalUnit::Word), 1),
        (TerminalKeyKind::Caret(TerminalUnit::Line), 2),
        (TerminalKeyKind::Delete, 3),
        (TerminalKeyKind::Typed("a".to_owned()), 4),
        (TerminalKeyKind::Enter, 5),
        (TerminalKeyKind::Forget, 6),
        (TerminalKeyKind::Other, 7),
        (TerminalKeyKind::Forget, 8),
        (TerminalKeyKind::Other, 8),
    ]
    .iter()
    .map(|(kind, at_ms)| rules_key(kind, *at_ms))
    .collect();
    assert_eq!(kept, expected);
    assert_eq!(
        rules_key(&TerminalKeyKind::Caret(TerminalUnit::Line), 42),
        verbatim_text::terminal_reading::Key {
            kind: verbatim_text::terminal_reading::KeyKind::Caret(
                verbatim_text::terminal_reading::Unit::Line
            ),
            at_ms: 42,
        }
    );
}
