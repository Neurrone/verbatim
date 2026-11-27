//! A key reaching the focused terminal, taken by its model
//! (`docs/design/readers-tell-core-asks-build3.md`, "Keys and ordering"):
//! each key's time is recorded when it reaches Core's inbox, a held caret
//! key's at its pass, and given to the reading rules before a read that
//! ended at or after it ("Keys" in
//! `books/dev/src/explanation/design/terminal-model.md`).

use verbatim_model::request::PressAction;
use verbatim_model::{KeySeq, TraceId};

use crate::keys::entry::PressEntry;
use crate::now::Now;
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
        // The key's other report, a clearing key's or a chord's, is what
        // it is to the terminal.
        | PressAction::Control => Vec::new(),
    }
}

/// `press` reached Core's inbox at `now`: a key the focused terminal's
/// model takes at its arrival is taken, with the time it reached the
/// program; one pressed while a held key's stroke holds the line reaches
/// the program at that key's pass, and is taken then ([`passed`]).
pub(crate) fn arrived(state: &mut SrState, press: &PressEntry, now: Now) {
    let at_ms = press.press.pressed_at_us / 1_000;
    let deferred = state.strokes.holds();
    let named = (press.trace_id, press.press.key_seq);
    for kind in kinds_at_arrival(state, press) {
        if deferred {
            state.terminal_focus.deferred.push((kind, at_ms, named));
        } else {
            taken(state, kind, at_ms, named, now);
        }
    }
}

/// A held caret key of `kind` passed to the focused terminal at
/// `passed_at_us`, named by its trace and number, in the step of the input
/// at `now`: the model takes it, and then the keys deferred behind it,
/// which reached the program with it.
pub(crate) fn passed(
    state: &mut SrState,
    kind: TerminalKeyKind,
    (passed_at_us, now): (u64, Now),
    named: (TraceId, KeySeq),
) {
    let at_ms = passed_at_us.div_ceil(1_000);
    taken(state, kind, at_ms, named, now);
    released(state, at_ms, now);
}

/// The line no longer holds at `at_ms`, in the step of the input at
/// `now`: the keys deferred behind a held stroke reached the program, no
/// earlier than they were pressed.
pub(crate) fn released(state: &mut SrState, at_ms: u64, now: Now) {
    if state.strokes.holds() {
        return;
    }
    for (kind, pressed_ms, named) in std::mem::take(&mut state.terminal_focus.deferred) {
        taken(state, kind, pressed_ms.max(at_ms), named, now);
    }
}

/// The focused terminal's model takes a key of `kind` that reached the
/// program at `at_ms`, and its reading the key, at `now`.
fn taken(
    state: &mut SrState,
    kind: TerminalKeyKind,
    at_ms: u64,
    named: (TraceId, KeySeq),
    now: Now,
) {
    crate::terminal_model::key(state, kind, at_ms, named);
    crate::terminal::reading::key_taken(state, now);
}
