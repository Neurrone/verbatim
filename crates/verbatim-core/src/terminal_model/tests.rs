//! Unit tests of Core's terminal model: what the reading rules say becomes
//! the flood policy's speech, and each key input becomes the rules' key.

use verbatim_model::{
    Backend, CaretKey, CaretMotion, CaretReport, EarlierElement, FocusText, Input, NodeDetails,
    NodeId, NodeSnapshot, NormalizedEvent, OutpostId, Pid, Role, StateSet, TextAnchor, TextChunk,
    TextUnit, TraceId,
};
use verbatim_text::terminal_reading::{Settings, Speech, Step};
use verbatim_text::terminal_rows::Lost;

use super::{Said, SaidItem, TerminalModel, rules_key, speech_of};
use crate::state::SrState;
use crate::terminal::{Skipped, TerminalItem, TerminalKeyKind, TerminalUnit};

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
                text: "first".into(),
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
        speech_of(step),
        Said {
            items: vec![
                SaidItem::Item(TerminalItem::Caret {
                    unit: TerminalUnit::Word,
                    text: "word".to_owned(),
                }),
                // After a lost read, its lines follow an "about" count of
                // none; a line's growth is part of its line.
                SaidItem::Item(TerminalItem::Skipped(Skipped::About(0))),
                SaidItem::Line("first".into()),
                SaidItem::Item(TerminalItem::Growth("grown".to_owned())),
                SaidItem::Item(TerminalItem::Echo("x".to_owned())),
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
                selected_child: None,
                text: None,
                earlier: None,
                group: None,
            },
        },
    );
    state
        .terminals
        .push(TerminalModel::new(node, Settings::default()));
    let caret = |motion, at_ms| Input::CaretKey {
        key_seq: verbatim_model::KeySeq::NONE_YET,
        trace_id: TraceId::mint(),
        key: CaretKey {
            motion,
            select: false,
        },
        pressed_at_ms: at_ms,
    };
    let typed = |text: &str, at_ms| Input::CharacterTyped {
        key_seq: verbatim_model::KeySeq::NONE_YET,
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
            key_seq: verbatim_model::KeySeq::NONE_YET,
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

/// A focus on terminal `node`, carrying `text` and what its reader read of
/// the element it held before under the node's runtime id, `earlier`.
fn terminal_focus(node: NodeId, text: Option<FocusText>, earlier: Option<EarlierElement>) -> Input {
    Input::Event {
        trace_id: TraceId::mint(),
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
            selected_child: None,
            text: text.map(Box::new),
            earlier,
            group: None,
        },
    }
}

/// A terminal focused with its caret and modelled, then focused again under
/// the same node, whose report says the element its reader held under that
/// runtime id is `earlier`: Core forgets the node's caret and its model, so
/// the new element's text is taken afresh, as a new object's is
/// (`docs/parity/focus-events.md`, "Duplicate focus suppression").
fn a_replaced_element_forgets_its_nodes_caret_and_model(earlier: EarlierElement) {
    let node = NodeId::in_outpost(OutpostId(1), 12);
    let mut state = SrState::new();
    let caret = CaretReport {
        line: TextChunk {
            unit: TextUnit::Line,
            text: "ready> ".to_owned(),
            start: TextAnchor(1),
            offset: 7,
            languages: Vec::new(),
            first: false,
            last: true,
            truncated: false,
            formats: Vec::new(),
        },
        selection: None,
        selection_line: None,
    };
    let _ = crate::reduce(
        &mut state,
        &terminal_focus(
            node,
            Some(FocusText::Caret {
                caret,
                selected_text: None,
            }),
            None,
        ),
    );
    state
        .terminals
        .push(TerminalModel::new(node, Settings::default()));
    assert_eq!(
        (
            state.caret.as_ref().map(|caret| caret.node),
            state.terminals.len()
        ),
        (Some(node), 1),
        "the first element's caret and model are kept"
    );

    let _ = crate::reduce(&mut state, &terminal_focus(node, None, Some(earlier)));
    assert_eq!(
        (
            state.caret.as_ref().map(|caret| caret.node),
            state.terminals.len()
        ),
        (None, 0)
    );
}

#[test]
fn an_element_that_lost_the_focus_under_the_same_node_forgets_its_caret_and_model() {
    a_replaced_element_forgets_its_nodes_caret_and_model(EarlierElement::NotFocused);
}

#[test]
fn an_element_gone_under_the_same_node_forgets_its_caret_and_model() {
    a_replaced_element_forgets_its_nodes_caret_and_model(EarlierElement::Gone);
}
