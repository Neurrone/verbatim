//! A key reaching the focused terminal, taken by its model
//! (`docs/design/readers-tell-core-asks-build3.md`, "Keys and ordering"):
//! each key's time is recorded when it reaches Core's inbox, a held caret
//! key's at its pass, and given to the reading rules before a read that
//! ended at or after it ("Keys" in
//! `books/dev/src/explanation/design/terminal-model.md`).

use verbatim_model::TraceId;
use verbatim_model::request::PressAction;

use crate::keys::entry::PressEntry;
use crate::state::SrState;
use crate::terminal::TerminalKeyKind;

/// What the press `press` is to the focused terminal's reading rules, when
/// it is taken at its arrival: text typed by what it types, a key that
/// clears the line as `Forget`, an editing chord as any other key, and a
/// caret key the hook passed at once, bound to the focused terminal, by
/// its unit. A held caret key is taken at its pass ([`passed`]).
fn kinds_at_arrival(state: &SrState, press: &PressEntry) -> Vec<TerminalKeyKind> {
    let Some(terminal) = crate::terminal::reading::modelled(state) else {
        return Vec::new();
    };
    match &press.press.action {
        PressAction::Typed { text } => vec![crate::terminal::typed_kind(text)],
        PressAction::Clearing => vec![TerminalKeyKind::Forget],
        PressAction::EditingChord => vec![TerminalKeyKind::Other],
        PressAction::CaretKey {
            key,
            held: None,
            bound,
        } if bound.as_ref().is_some_and(|bound| bound.node == terminal) => {
            vec![crate::terminal::caret_key_kind(*key)]
        }
        PressAction::CaretKey { .. }
        | PressAction::Command { .. }
        | PressAction::Script { .. }
        | PressAction::Passed => Vec::new(),
    }
}

/// `press` reached Core's inbox: a key the focused terminal's model takes
/// at its arrival is taken, with the time it reached the program; one
/// pressed while a held key's stroke holds the line reaches the program
/// at that key's pass, and is taken then ([`passed`]).
pub(crate) fn arrived(state: &mut SrState, press: &PressEntry) {
    let at_ms = press.press.pressed_at_us / 1_000;
    let deferred = state.strokes.holds();
    for kind in kinds_at_arrival(state, press) {
        if deferred {
            state
                .terminal_focus
                .deferred
                .push((kind, at_ms, press.trace_id));
        } else {
            taken(state, kind, at_ms, press.trace_id);
        }
    }
}

/// A held caret key of `kind` passed to the focused terminal at
/// `passed_at_us`, under `trace`: the model takes it, and then the keys
/// deferred behind it, which reached the program with it.
pub(crate) fn passed(
    state: &mut SrState,
    kind: TerminalKeyKind,
    passed_at_us: u64,
    trace: TraceId,
) {
    let at_ms = passed_at_us.div_ceil(1_000);
    taken(state, kind, at_ms, trace);
    released(state, at_ms);
}

/// The line no longer holds at `at_ms`: the keys deferred behind a held
/// stroke reached the program, no earlier than they were pressed.
pub(crate) fn released(state: &mut SrState, at_ms: u64) {
    if state.strokes.holds() {
        return;
    }
    for (kind, pressed_ms, trace) in std::mem::take(&mut state.terminal_focus.deferred) {
        taken(state, kind, pressed_ms.max(at_ms), trace);
    }
}

fn taken(state: &mut SrState, kind: TerminalKeyKind, at_ms: u64, trace: TraceId) {
    crate::terminal_model::key(state, kind, at_ms, trace);
    let now_us = state.terminal_focus.now_us;
    crate::terminal::reading::key_taken(state, now_us);
}
