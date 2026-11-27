//! A key reaching the focused terminal, taken by its model
//! (`docs/design/readers-tell-core-asks-build3.md`, "Keys and ordering"):
//! a key the hook passed at once is taken when it reaches Core's inbox; a
//! held caret key, and a key pressed behind one, join Core's stroke line
//! with the terminal key each is, and are taken as the line sends them, in
//! the order they reached the program ([`reached`]); each is given to the
//! reading rules before a read that ended at or after it ("Keys" in
//! `books/dev/src/explanation/design/terminal-model.md`).

use serde::{Deserialize, Serialize};
use verbatim_model::request::PressAction;
use verbatim_model::{KeySeq, NodeId, TraceId};

use crate::keys::entry::PressEntry;
use crate::now::Now;
use crate::state::SrState;
use crate::terminal::TerminalKeyKind;

/// A key the focused terminal's model takes when it reaches the program:
/// the terminal it was bound to at its arrival, what it is to the reading
/// rules, and its trace and number.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TerminalKey {
    pub(crate) node: NodeId,
    pub(crate) kind: TerminalKeyKind,
    pub(crate) trace: TraceId,
    pub(crate) key: KeySeq,
}

/// What the press `press` is to the focused terminal's reading rules, at
/// its arrival, whatever the terminal's life, so that a key held while a
/// baseline is out, as after a silent re-take, is taken if the terminal is
/// modelled by its send: text typed by what it types, a key that clears
/// the line as `Forget`, an editing chord as any other key, and a caret key
/// bound to the focused terminal, held or not, by its unit.
fn kinds_at_arrival(state: &SrState, press: &PressEntry) -> Vec<TerminalKey> {
    let Some(terminal) = crate::terminal::reading::focused(state).map(|terminal| terminal.node)
    else {
        return Vec::new();
    };
    let kinds = match &press.press.action {
        PressAction::Typed { text } => vec![crate::terminal::typed_kind(text)],
        PressAction::Clearing => vec![TerminalKeyKind::Forget],
        PressAction::EditingChord => vec![TerminalKeyKind::Other],
        PressAction::CaretKey { key, bound, .. }
            if bound.as_ref().is_some_and(|bound| bound.node == terminal) =>
        {
            vec![crate::terminal::caret_key_kind(*key)]
        }
        PressAction::CaretKey { .. }
        | PressAction::Command { .. }
        | PressAction::Script { .. }
        // The key's other report, a clearing key's or a chord's, is what
        // it is to the terminal.
        | PressAction::Control => Vec::new(),
    };
    kinds
        .into_iter()
        .map(|kind| TerminalKey {
            node: terminal,
            kind,
            trace: press.trace_id,
            key: press.press.key_seq,
        })
        .collect()
}

/// `press` reached Core's inbox at `now`. A held key's stroke joins the
/// stroke line with the terminal key it is; a key pressed while a held
/// stroke holds the line joins it behind, reaching the program when what
/// is ahead of it goes; any other key reaches the program now, at the time
/// the hook saw it.
pub(crate) fn arrived(state: &mut SrState, press: &PressEntry, now: Now) {
    let keys = kinds_at_arrival(state, press);
    if let PressAction::CaretKey {
        held: Some(stroke), ..
    } = &press.press.action
    {
        state.strokes.hold(
            press.press.key_seq,
            stroke.clone(),
            (Some(press.press.pressed_at_us), keys.into_iter().next()),
        );
    } else if state.strokes.holds() {
        state.strokes.behind(keys);
    } else {
        take(state, keys, press.press.pressed_at_us / 1_000, now);
    }
}

/// The terminal keys `reached` reached the program as the stroke line sent
/// them at `now`.
pub(crate) fn reached(state: &mut SrState, reached: Vec<TerminalKey>, now: Now) {
    take(state, reached, now.us().div_ceil(1_000), now);
}

/// `keys` reached the program at `at_ms`: the focused terminal's model
/// takes each, in order, and its reading the key, at `now`, while the
/// terminal each was bound to is the modelled focus; a key reaching a
/// terminal that left the focus, or whose baseline is still out, is not
/// taken, as the rules start from the baseline's read.
fn take(state: &mut SrState, keys: Vec<TerminalKey>, at_ms: u64, now: Now) {
    for key in keys {
        if crate::terminal::reading::modelled(state) == Some(key.node) {
            crate::terminal_model::key(state, key.kind, at_ms, (key.trace, key.key));
            crate::terminal::reading::key_taken(state, now);
        }
    }
}
