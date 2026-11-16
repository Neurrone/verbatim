//! The focused terminal's life and its reading in Core
//! (`docs/design/readers-tell-core-asks-build3.md`, Tables 1 to 4): the
//! focused terminal is one value ([`FocusedTerminal`]), its baseline read,
//! waiting for its window after a hang, then modelled with its reading
//! ([`Reading`]).
//!
//! The outpost tells every change signal of the terminal `Follow` names;
//! Core takes each tell at once in its inbox ([`told`]). A modelled
//! terminal with no read out and its flood policy live is read at once,
//! outside the event queue, as Core's own request awaited by its facts
//! (an output read); a change told while a read is out waits for that
//! read's answer and is read after it unless the read covered it; while
//! the policy holds, the release at a group's last line reads it. Every
//! full read's answer is taken by the terminal's model at once, in the
//! order the outpost's worker made the reads, and its speech joins the
//! event queue as a speech entry ([`SpeechEntry`]), spoken at its turn.
//! Core alone keeps "told, not yet read": the outpost keeps no flag
//! (Dickson, 2026-11-16).

use serde::{Deserialize, Serialize};
use verbatim_model::request::{
    CancelReason, Follow, FollowGroup, Followed, Reply, Request as Wire,
};
use verbatim_model::{
    Effect, NodeId, Role, TerminalRead, TerminalReadReply, TerminalScope, TextAnchor, TextOp,
    TextReply, TextUnit, TraceId,
};

use crate::entries::{Entries, Entry};
use crate::event_queue::{CoreRequest, Cx, OTHER_DUE_US, Outcome, Reader, Request, RequestWindow};
use crate::keys::entry::Fence;
use crate::state::SrState;
use crate::terminal::{Said, SaidItem, TerminalItem};
use crate::terminal_model::{self, Taken};

/// The focused terminal, as the committed focus names it: its node, the
/// window its requests name, and whether that window is a console host's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Terminal {
    /// The terminal's node.
    pub(crate) node: NodeId,
    /// The window its requests name, for the hang check.
    pub(crate) window: RequestWindow,
    /// Whether its window is a console host's.
    pub(crate) console: bool,
}

/// A `Follow` naming the focused terminal, out: its token, and whether it
/// was already sent again once after its due time (Table 1, column C).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FollowOut {
    token: u64,
    resent: bool,
}

/// The focused terminal's life (Table 1). `told` is the newest change time
/// not yet covered, in microseconds on the shared clock.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum FocusedTerminal {
    /// No terminal is the committed focus.
    #[default]
    None,
    /// Its baseline read, `read` the baseline's token while one is out.
    Baselining {
        terminal: Terminal,
        follow: Option<FollowOut>,
        read: Option<u64>,
        told: Option<u64>,
    },
    /// Its window hung before the baseline answered; it is read again when
    /// the window answers.
    WaitingForWindow {
        terminal: Terminal,
        told: Option<u64>,
    },
    /// Read, its model taken, with its reading.
    Modelled {
        terminal: Terminal,
        follow: Option<FollowOut>,
        reading: Reading,
    },
}

impl FocusedTerminal {
    /// The terminal, when there is one.
    pub(crate) fn terminal(&self) -> Option<Terminal> {
        match self {
            Self::None => None,
            Self::Baselining { terminal, .. }
            | Self::WaitingForWindow { terminal, .. }
            | Self::Modelled { terminal, .. } => Some(*terminal),
        }
    }
}

/// What an output read's speech does (Table 2): spoken; waited for Core's
/// speech queue, the release at a group's last line or not; or silent,
/// speaking only its answers to keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Mark {
    Spoken,
    Waited { release: bool },
    Silent,
}

/// A modelled terminal's reading (Table 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Reading {
    /// Live, nothing told unread.
    Quiet,
    /// Live, a change told while an entry's full read of it is out.
    Behind { told: u64 },
    /// An output read out: its token, its mark, the start it named, and
    /// the newest change told while it is out.
    Out {
        token: u64,
        mark: Mark,
        named: Option<TextAnchor>,
        again: Option<u64>,
    },
    /// Holding for the flood policy, with the newest change told.
    Holding { told: Option<u64> },
    /// Its window hung while a read was out; `holding` keeps whether the
    /// policy was holding.
    Stalled { holding: bool },
}

/// A request Core's terminal reading wants sent outside the queue: awaited
/// under its token, or by nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Wanted {
    token: Option<u64>,
    window: RequestWindow,
    request: Wire,
}

/// What Core keeps of the focused terminal's reading.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TerminalFocus {
    /// The focused terminal's life.
    pub(crate) focused: FocusedTerminal,
    /// Whether an entry's full read of the focused terminal is out (a
    /// caret key's or a review command's).
    entry_read: bool,
    /// The last token given to a request.
    tokens: u64,
    /// The time of the queue input being handled, in microseconds on the
    /// shared clock, at which the flood policy's requests are sent.
    pub(crate) now_us: u64,
    /// Keys pressed behind a held key's stroke, which reach the program at
    /// its pass, each with when the hook saw it, in milliseconds, and its
    /// trace (`crate::keys::terminal`).
    pub(crate) deferred: Vec<(crate::terminal::TerminalKeyKind, u64, TraceId)>,
    /// The requests wanted, taken as the queue settles.
    #[serde(skip)]
    wanted: Vec<Wanted>,
}

/// The modelled focused terminal, whose quiet-period timer counts.
pub(crate) fn modelled(state: &SrState) -> Option<NodeId> {
    match state.terminal_focus.focused {
        FocusedTerminal::Modelled { terminal, .. } => Some(terminal.node),
        _ => None,
    }
}

/// The focused terminal, whatever its life.
pub(crate) fn focused(state: &SrState) -> Option<Terminal> {
    state.terminal_focus.focused.terminal()
}

/// The requests wanted, taken.
pub(crate) fn wanted(state: &mut SrState) -> Vec<CoreRequest> {
    std::mem::take(&mut state.terminal_focus.wanted)
        .into_iter()
        .map(|wanted| CoreRequest {
            token: wanted.token,
            reader: Reader::Outpost(wanted.window.process),
            window: wanted.window,
            request: wanted.request,
            due_after_us: OTHER_DUE_US,
        })
        .collect()
}

/// Whether requests are wanted.
pub(crate) fn wants(state: &SrState) -> bool {
    !state.terminal_focus.wanted.is_empty()
}

/// A new token.
fn token(state: &mut SrState) -> u64 {
    state.terminal_focus.tokens = state.terminal_focus.tokens.saturating_add(1);
    state.terminal_focus.tokens
}

/// The `Follow` request naming `terminal`.
fn follow_request(terminal: &Terminal) -> Wire {
    Wire::Follow(Follow {
        node: Some(Followed {
            node: terminal.node,
            group: FollowGroup::Terminal {
                console: terminal.console,
            },
            extra: None,
        }),
    })
}

/// Wants a `Follow` naming `terminal`, sent once more after its due time
/// when `resent`.
fn want_follow(state: &mut SrState, terminal: &Terminal, resent: bool) -> FollowOut {
    let token = token(state);
    state.terminal_focus.wanted.push(Wanted {
        token: Some(token),
        window: terminal.window,
        request: follow_request(terminal),
    });
    FollowOut { token, resent }
}

/// The full read of `terminal` Core sends now, at `now_us`, with the facts
/// of its model: the start it names, the scope the rules owe, the screen's
/// shape and whether it takes the colours; a first read with none of
/// them before the terminal is modelled. The model counts it sent, and
/// its owed scope is the read's.
fn full_read(
    state: &mut SrState,
    terminal: &Terminal,
    (starting, output): (Option<TextUnit>, bool),
    now_us: u64,
) -> TerminalRead {
    let Some(model) = terminal_model::model_mut(state, terminal.node) else {
        return TerminalRead {
            start: None,
            scope: TerminalScope::Fresh,
            height: 0,
            width: 0,
            colours: false,
            console: terminal.console,
            starting,
            output,
        };
    };
    let colours = model.takes_colours(now_us / 1_000);
    let scope = model.owed.take().unwrap_or(TerminalScope::Fresh);
    model.sends = model.sends.saturating_add(1);
    TerminalRead {
        // A whole document or a first read again with its history block
        // names no start.
        start: match scope {
            TerminalScope::Fresh => model.start,
            TerminalScope::Whole | TerminalScope::HistoryBlock { .. } => None,
        },
        scope,
        height: model.height,
        width: u32::try_from(model.width).unwrap_or(u32::MAX),
        colours,
        console: terminal.console,
        starting,
        output,
    }
}

/// Wants an output read of `terminal` with `mark`, `again` the change told
/// that it carries over, and returns the reading it leaves.
fn want_output(
    state: &mut SrState,
    terminal: &Terminal,
    (mark, again): (Mark, Option<u64>),
    now_us: u64,
) -> Reading {
    let read = full_read(state, terminal, (None, true), now_us);
    let token = token(state);
    state.terminal_focus.wanted.push(Wanted {
        token: Some(token),
        window: terminal.window,
        request: Wire::Text {
            node: terminal.node,
            op: TextOp::ReadTerminal(read),
        },
    });
    if mark == (Mark::Waited { release: true }) {
        state.terminal.release_waiting = true;
    }
    Reading::Out {
        token,
        mark,
        named: read.start,
        again,
    }
}

/// Wants the baseline of `terminal`, a full read in `Follow`'s order
/// class, and returns its token.
fn want_baseline(state: &mut SrState, terminal: &Terminal, now_us: u64) -> u64 {
    let read = full_read(state, terminal, (None, false), now_us);
    let token = token(state);
    state.terminal_focus.wanted.push(Wanted {
        token: Some(token),
        window: terminal.window,
        request: Wire::Text {
            node: terminal.node,
            op: TextOp::ReadTerminal(read),
        },
    });
    token
}

/// The committed focus changed (Table 1, columns A and H): `to` is the
/// terminal now committed, if the new focus is one, at `now_us`. A
/// terminal committed, new or taken again, is baselining: its `Follow` and
/// baseline are wanted, the `Follow` among `follows`, the commit's, which
/// are all wanted in order first; a terminal left is told so.
pub(crate) fn committed(
    state: &mut SrState,
    to: Option<Terminal>,
    follows: Vec<(RequestWindow, Wire)>,
    now_us: u64,
) {
    let from = focused(state);
    if let Some(from) = from
        && to.is_none_or(|to| to.node != from.node)
        && let Some(model) = terminal_model::model_mut(state, from.node)
    {
        model.blurred();
    }
    let Some(terminal) = to else {
        for (window, request) in follows {
            state.terminal_focus.wanted.push(Wanted {
                token: None,
                window,
                request,
            });
        }
        state.terminal_focus.focused = FocusedTerminal::None;
        return;
    };
    let mut follow = None;
    for (window, request) in follows {
        let names_it = matches!(
            &request,
            Wire::Follow(Follow { node: Some(followed) }) if followed.node == terminal.node
        );
        let token = names_it.then(|| token(state));
        if let Some(token) = token {
            follow = Some(FollowOut {
                token,
                resent: false,
            });
        }
        state.terminal_focus.wanted.push(Wanted {
            token,
            window,
            request,
        });
    }
    if let Some(model) = terminal_model::model_mut(state, terminal.node) {
        model.focused(now_us / 1_000);
    }
    let read = want_baseline(state, &terminal, now_us);
    state.terminal_focus.focused = FocusedTerminal::Baselining {
        terminal,
        follow,
        read: Some(read),
        told: None,
    };
}

/// The terminal a focus of `role` read on `window`, `node`, is, when it is
/// one.
pub(crate) fn terminal_of(
    role: Role,
    node: NodeId,
    window: RequestWindow,
    console: bool,
) -> Option<Terminal> {
    (role == Role::Terminal).then_some(Terminal {
        node,
        window,
        console,
    })
}

/// A change of terminal `node` observed at `at_us`, told at once in the
/// inbox at `now_us` (Table 1, column D; Table 2, column A); a tell for
/// any other node is dropped.
pub(crate) fn told(state: &mut SrState, node: NodeId, at_us: u64, now_us: u64) {
    let focused = state.terminal_focus.focused;
    let changed = match focused {
        FocusedTerminal::Baselining {
            terminal,
            follow,
            read,
            ..
        } if terminal.node == node => {
            let read = read.or_else(|| Some(want_baseline(state, &terminal, now_us)));
            FocusedTerminal::Baselining {
                terminal,
                follow,
                read,
                told: Some(at_us),
            }
        }
        FocusedTerminal::WaitingForWindow { terminal, .. } if terminal.node == node => {
            FocusedTerminal::WaitingForWindow {
                terminal,
                told: Some(at_us),
            }
        }
        FocusedTerminal::Modelled {
            terminal,
            follow,
            reading,
        } if terminal.node == node => FocusedTerminal::Modelled {
            terminal,
            follow,
            reading: reading_told(state, &terminal, reading, (at_us, now_us)),
        },
        _ => return,
    };
    state.terminal_focus.focused = changed;
}

/// Table 2, column A: a change told at `at_us`.
fn reading_told(
    state: &mut SrState,
    terminal: &Terminal,
    reading: Reading,
    (at_us, now_us): (u64, u64),
) -> Reading {
    match reading {
        Reading::Quiet => {
            let covered = terminal_model::model_of(state, terminal.node)
                .is_some_and(|model| at_us < model.started_us);
            if covered {
                Reading::Quiet
            } else if state.terminal_focus.entry_read {
                Reading::Behind { told: at_us }
            } else {
                want_output(state, terminal, (Mark::Spoken, None), now_us)
            }
        }
        Reading::Behind { told } => Reading::Behind {
            told: told.max(at_us),
        },
        Reading::Out {
            token,
            mark,
            named,
            again,
        } => Reading::Out {
            token,
            mark,
            named,
            again: Some(again.map_or(at_us, |again| again.max(at_us))),
        },
        Reading::Holding { told } => Reading::Holding {
            told: Some(told.map_or(at_us, |told| told.max(at_us))),
        },
        Reading::Stalled { holding } => Reading::Stalled { holding },
    }
}

/// `told` unless the read begun at `started_us` covered it.
fn uncovered(told: Option<u64>, started_us: u64) -> Option<u64> {
    told.filter(|told| *told >= started_us)
}

/// After a read is taken, with nothing out: the reading the scope the
/// rules owe, or the colours owed, asks (Table 2, column L).
fn owed_read(state: &mut SrState, terminal: &Terminal, reading: Reading, now_us: u64) -> Reading {
    if reading != Reading::Quiet {
        return reading;
    }
    let owed = terminal_model::model_of(state, terminal.node)
        .is_some_and(|model| model.owed.is_some() || model.colours_owed());
    if owed {
        want_output(state, terminal, (Mark::Spoken, None), now_us)
    } else {
        reading
    }
}

/// A key Core took in for the focused terminal: its colours may be owed
/// now (Table 2, column L).
pub(crate) fn key_taken(state: &mut SrState, now_us: u64) {
    if let FocusedTerminal::Modelled {
        terminal,
        follow,
        reading,
    } = state.terminal_focus.focused
    {
        let reading = owed_read(state, &terminal, reading, now_us);
        state.terminal_focus.focused = FocusedTerminal::Modelled {
            terminal,
            follow,
            reading,
        };
    }
}

/// A speech entry: a full read's speech, joining the event queue to be
/// spoken at its turn (Table 3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SpeechEntry {
    /// The terminal read.
    pub(crate) node: NodeId,
    /// Its model's generation when the read was taken.
    generation: u64,
    /// What the rules said, in order.
    items: Vec<TerminalItem>,
    /// Whether the review cursor followed the caret.
    followed: bool,
    /// When the read began, in milliseconds on the shared clock.
    started_ms: u64,
    /// Whether it speaks only its answers to keys.
    silent: bool,
    /// Whether it is the release read's, whose turn the look at the
    /// backlog waits for.
    release: bool,
    /// The read's speech key.
    key: Option<TraceId>,
    /// The trace it is spoken under without one.
    trace: TraceId,
    /// Its speech fence, set when it joins the queue.
    pub(crate) fence: Fence,
}

/// The speech entry of `taken`, a read of `node` begun at `started_ms`
/// with `mark`, under `trace`: `None` when it says nothing and is no
/// release.
fn speech_entry(
    node: NodeId,
    taken: Taken,
    (started_ms, mark): (u64, Mark),
    trace: TraceId,
) -> Option<SpeechEntry> {
    let release = mark == (Mark::Waited { release: true });
    if taken.speech.is_none() && !release {
        return None;
    }
    let speech = taken.speech.unwrap_or_default();
    Some(SpeechEntry {
        node,
        generation: taken.generation,
        items: speech
            .items
            .into_iter()
            .map(|item| match item {
                SaidItem::Item(item) => item,
                SaidItem::Line(text) => TerminalItem::Output(text.to_string()),
            })
            .collect(),
        followed: speech.followed,
        started_ms,
        silent: mark == Mark::Silent,
        release,
        key: taken.key,
        trace,
        fence: Fence::default(),
    })
}

/// A speech entry at its turn (Table 3, column A or B): while its
/// terminal is still the focused terminal and its model the same, the
/// answers to keys are spoken and the output goes through the flood
/// policy, unless silent or read before the last cut; otherwise it ends
/// silent.
pub(crate) fn speak(state: &mut SrState, entry: SpeechEntry) -> Vec<Effect> {
    let current = modelled(state) == Some(entry.node)
        && terminal_model::model_of(state, entry.node)
            .is_some_and(|model| model.generation == entry.generation);
    if !current {
        if entry.release {
            return crate::terminal::release_ended(state);
        }
        return Vec::new();
    }
    let said = Said {
        items: entry.items.into_iter().map(SaidItem::Item).collect(),
        followed: entry.followed,
    };
    let effects = crate::terminal::read_spoken(
        state,
        entry.key.unwrap_or(entry.trace),
        (entry.node, entry.started_ms),
        said,
        (entry.silent, entry.release),
    );
    terminal_model::under(entry.key, effects)
}

/// A speech entry's function at its turn, which asks nothing.
pub(crate) fn speech_turn(cx: &mut Cx<Entries>, entry: SpeechEntry) {
    let effects = speak(&mut cx.facts_mut(), entry);
    for effect in effects {
        cx.emit(effect);
    }
}

/// The reply of a terminal's full read, decoded.
fn read_reply(reply: Reply) -> Option<Box<TerminalReadReply>> {
    match reply {
        Reply::Text(TextReply::TerminalRead(read)) => Some(read),
        _ => None,
    }
}

/// A request Core's terminal reading asked under `token` ended with
/// `outcome` at `now_us`: the `Follow`, the baseline or the output read it
/// is (Tables 1 and 2); one the focused terminal no longer awaits is
/// dropped. The speech entries returned join the back; what the flood
/// policy says goes into `said`.
pub(crate) fn answered(
    state: &mut SrState,
    token: u64,
    outcome: Outcome<Reply>,
    (now_us, trace): (u64, TraceId),
    said: &mut Vec<Effect>,
) -> Vec<Entry> {
    let focused = state.terminal_focus.focused;
    let (terminal, follow) = match focused {
        FocusedTerminal::Baselining {
            terminal, follow, ..
        }
        | FocusedTerminal::Modelled {
            terminal, follow, ..
        } => (terminal, follow),
        FocusedTerminal::None | FocusedTerminal::WaitingForWindow { .. } => return Vec::new(),
    };
    if follow.is_some_and(|follow| follow.token == token) {
        follow_ended(state, &terminal, follow, &outcome, now_us);
        return Vec::new();
    }
    match focused {
        FocusedTerminal::Baselining {
            read: Some(read),
            told,
            ..
        } if read == token => {
            baseline_ended(state, &terminal, (follow, told), outcome, (now_us, trace))
        }
        FocusedTerminal::Modelled {
            reading:
                Reading::Out {
                    token: out,
                    mark,
                    named,
                    again,
                },
            ..
        } if out == token => output_ended(
            state,
            (&terminal, follow),
            (mark, named, again),
            outcome,
            (now_us, trace),
            said,
        ),
        _ => Vec::new(),
    }
}

/// The focused terminal's `Follow` ended (Table 1, columns B, C and K).
fn follow_ended(
    state: &mut SrState,
    terminal: &Terminal,
    follow: Option<FollowOut>,
    outcome: &Outcome<Reply>,
    now_us: u64,
) {
    let focused = state.terminal_focus.focused;
    let resent = follow.is_some_and(|follow| follow.resent);
    let next = match outcome {
        Outcome::Refused(_) | Outcome::NotThere => FocusedTerminal::None,
        Outcome::NoLongerAwaited(CancelReason::DueTime) if !resent => {
            let follow = Some(want_follow(state, terminal, true));
            match focused {
                FocusedTerminal::Baselining { read, told, .. } => {
                    let read = read.or_else(|| Some(want_baseline(state, terminal, now_us)));
                    FocusedTerminal::Baselining {
                        terminal: *terminal,
                        follow,
                        read,
                        told,
                    }
                }
                FocusedTerminal::Modelled { reading, .. } => FocusedTerminal::Modelled {
                    terminal: *terminal,
                    follow,
                    reading,
                },
                other => other,
            }
        }
        Outcome::NotResponding | Outcome::NoLongerAwaited(CancelReason::Hung) => match focused {
            FocusedTerminal::Baselining { told, .. } => FocusedTerminal::WaitingForWindow {
                terminal: *terminal,
                told,
            },
            other => without_follow(other),
        },
        Outcome::Result(_) | Outcome::ReaderEnded | Outcome::NoLongerAwaited(_) => {
            without_follow(focused)
        }
    };
    state.terminal_focus.focused = next;
}

/// `focused` with no `Follow` out.
fn without_follow(focused: FocusedTerminal) -> FocusedTerminal {
    match focused {
        FocusedTerminal::Baselining {
            terminal,
            read,
            told,
            ..
        } => FocusedTerminal::Baselining {
            terminal,
            follow: None,
            read,
            told,
        },
        FocusedTerminal::Modelled {
            terminal, reading, ..
        } => FocusedTerminal::Modelled {
            terminal,
            follow: None,
            reading,
        },
        other => other,
    }
}

/// The baseline ended (Table 1, columns E, F and K).
fn baseline_ended(
    state: &mut SrState,
    terminal: &Terminal,
    (follow, told): (Option<FollowOut>, Option<u64>),
    outcome: Outcome<Reply>,
    (now_us, trace): (u64, TraceId),
) -> Vec<Entry> {
    let reply = match outcome {
        Outcome::Result(reply) => read_reply(reply),
        Outcome::NotThere => {
            state.terminal_focus.focused = FocusedTerminal::None;
            return Vec::new();
        }
        Outcome::NotResponding | Outcome::NoLongerAwaited(CancelReason::Hung) => {
            state.terminal_focus.focused = FocusedTerminal::WaitingForWindow {
                terminal: *terminal,
                told,
            };
            return Vec::new();
        }
        Outcome::ReaderEnded => return Vec::new(),
        Outcome::Refused(_) | Outcome::NoLongerAwaited(_) => None,
    };
    let Some(reply) = reply else {
        state.terminal_focus.focused = FocusedTerminal::Baselining {
            terminal: *terminal,
            follow,
            read: None,
            told,
        };
        return Vec::new();
    };
    modelled_by(
        state,
        terminal,
        (follow, told),
        (&reply, None),
        (now_us, trace),
    )
    .into_iter()
    .map(|entry| Entry::TerminalSpeech(Box::new(entry)))
    .collect()
}

/// `reply`, read naming `named`, taken as the terminal's baseline: the
/// terminal is modelled, and read again at once if a change was told after
/// the read began, or the rules owe a read (Table 1, column E).
fn modelled_by(
    state: &mut SrState,
    terminal: &Terminal,
    (follow, told): (Option<FollowOut>, Option<u64>),
    (reply, named): (&TerminalReadReply, Option<TextAnchor>),
    (now_us, trace): (u64, TraceId),
) -> Option<SpeechEntry> {
    let taken = terminal_model::take(state, terminal.node, reply, (named, false));
    let entry = speech_entry(
        terminal.node,
        taken,
        (reply.snapshot.started_ms, Mark::Spoken),
        trace,
    );
    let reading = match uncovered(told, reply.started_us) {
        Some(_) => want_output(state, terminal, (Mark::Spoken, None), now_us),
        None => owed_read(state, terminal, Reading::Quiet, now_us),
    };
    state.terminal_focus.focused = FocusedTerminal::Modelled {
        terminal: *terminal,
        follow,
        reading,
    };
    entry
}

/// An output read ended (Table 2, columns B to E).
fn output_ended(
    state: &mut SrState,
    (terminal, follow): (&Terminal, Option<FollowOut>),
    (mark, named, again): (Mark, Option<TextAnchor>, Option<u64>),
    outcome: Outcome<Reply>,
    (now_us, trace): (u64, TraceId),
    said: &mut Vec<Effect>,
) -> Vec<Entry> {
    let release = mark == (Mark::Waited { release: true });
    let set = |state: &mut SrState, reading: Reading| {
        state.terminal_focus.focused = FocusedTerminal::Modelled {
            terminal: *terminal,
            follow,
            reading,
        };
    };
    let reply = match outcome {
        Outcome::Result(reply) => read_reply(reply),
        Outcome::NotThere => {
            state.terminal_focus.focused = FocusedTerminal::None;
            state.drop_models(|model| model.node == terminal.node);
            if release {
                said.extend(crate::terminal::release_ended(state));
            }
            return Vec::new();
        }
        Outcome::NoLongerAwaited(CancelReason::DueTime) => {
            // One read sent again with the same mark (column D).
            let reading = want_output(state, terminal, (mark, again), now_us);
            set(state, reading);
            return Vec::new();
        }
        Outcome::NotResponding | Outcome::NoLongerAwaited(CancelReason::Hung) => {
            set(state, Reading::Stalled { holding: release });
            if release {
                said.extend(crate::terminal::release_ended(state));
            }
            return Vec::new();
        }
        Outcome::ReaderEnded => return Vec::new(),
        Outcome::Refused(_) | Outcome::NoLongerAwaited(_) => None,
    };
    let Some(reply) = reply else {
        // Refused or failed (column C): the change told stays unread
        // until the next one; one more read only for a change told while
        // it was out.
        if release {
            set(state, Reading::Holding { told: again });
            said.extend(crate::terminal::release_ended(state));
        } else {
            let reading = match again {
                Some(_) => want_output(state, terminal, (Mark::Spoken, None), now_us),
                None => Reading::Quiet,
            };
            set(state, reading);
        }
        return Vec::new();
    };
    let waited = matches!(mark, Mark::Waited { .. });
    let taken = terminal_model::take(state, terminal.node, &reply, (named, waited));
    output_taken(
        state,
        (terminal, follow),
        (mark, again),
        (taken, (reply.snapshot.started_ms, reply.started_us)),
        (now_us, trace),
    )
}

/// An output read `taken`, begun at `began` (milliseconds, and
/// microseconds): its speech entry joins, and the reading goes on by
/// Table 2, column B.
fn output_taken(
    state: &mut SrState,
    (terminal, follow): (&Terminal, Option<FollowOut>),
    (mark, again): (Mark, Option<u64>),
    (taken, began): (Taken, (u64, u64)),
    (now_us, trace): (u64, TraceId),
) -> Vec<Entry> {
    let release = mark == (Mark::Waited { release: true });
    let set = |state: &mut SrState, reading: Reading| {
        state.terminal_focus.focused = FocusedTerminal::Modelled {
            terminal: *terminal,
            follow,
            reading,
        };
    };
    let entry = speech_entry(terminal.node, taken, (began.0, mark), trace);
    let owed_scope =
        terminal_model::model_of(state, terminal.node).is_some_and(|model| model.owed.is_some());
    let again = uncovered(again, began.1);
    let reading = if owed_scope {
        want_output(state, terminal, (mark, again), now_us)
    } else if release {
        // The backlog look runs at the speech entry's turn.
        Reading::Holding { told: again }
    } else if crate::terminal::lines_hold(state) {
        Reading::Holding { told: again }
    } else if again.is_some() {
        want_output(state, terminal, (Mark::Spoken, None), now_us)
    } else {
        owed_read(state, terminal, Reading::Quiet, now_us)
    };
    set(state, reading);
    entry
        .map(|entry| Entry::TerminalSpeech(Box::new(entry)))
        .into_iter()
        .collect()
}

/// The lines waiting or handed to speech reached "Lines spoken in full"
/// (Table 2, column G): a live terminal holds.
pub(crate) fn lines_full(state: &mut SrState) {
    if let FocusedTerminal::Modelled {
        terminal,
        follow,
        reading,
    } = state.terminal_focus.focused
    {
        let reading = match reading {
            Reading::Quiet => Reading::Holding { told: None },
            Reading::Behind { told } => Reading::Holding { told: Some(told) },
            other => other,
        };
        state.terminal_focus.focused = FocusedTerminal::Modelled {
            terminal,
            follow,
            reading,
        };
    }
}

/// A group's last line was handed to speech (Table 2, column H): a
/// holding terminal with a change told is read, the release.
pub(crate) fn group_last_line(state: &mut SrState) {
    if let FocusedTerminal::Modelled {
        terminal,
        follow,
        reading: Reading::Holding { told: Some(_) },
    } = state.terminal_focus.focused
    {
        let now_us = state.terminal_focus.now_us;
        let reading = want_output(
            state,
            &terminal,
            (Mark::Waited { release: true }, None),
            now_us,
        );
        state.terminal_focus.focused = FocusedTerminal::Modelled {
            terminal,
            follow,
            reading,
        };
    }
}

/// The backlog was looked at (Table 2, column I), `nothing_waits` when no
/// line waits after it: a holding terminal goes live, a change told read
/// at once as a waited read.
pub(crate) fn looked(state: &mut SrState, nothing_waits: bool) {
    if !nothing_waits {
        return;
    }
    if let FocusedTerminal::Modelled {
        terminal,
        follow,
        reading: Reading::Holding { told },
    } = state.terminal_focus.focused
    {
        let now_us = state.terminal_focus.now_us;
        let reading = match told {
            Some(_) => want_output(
                state,
                &terminal,
                (Mark::Waited { release: false }, None),
                now_us,
            ),
            None => owed_read(state, &terminal, Reading::Quiet, now_us),
        };
        state.terminal_focus.focused = FocusedTerminal::Modelled {
            terminal,
            follow,
            reading,
        };
    }
}

/// A cut (Table 2, column J): changes held unread are read at once by a
/// silent read, and the policy goes live; a release read out becomes
/// silent.
pub(crate) fn cut(state: &mut SrState) {
    let FocusedTerminal::Modelled {
        terminal,
        follow,
        reading,
    } = state.terminal_focus.focused
    else {
        return;
    };
    state.terminal.release_waiting = false;
    let reading = match reading {
        Reading::Out {
            token,
            mark: Mark::Waited { release: true },
            named,
            again,
        } => Reading::Out {
            token,
            mark: Mark::Silent,
            named,
            again,
        },
        Reading::Holding { told: Some(_) } => {
            let now_us = state.terminal_focus.now_us;
            want_output(state, &terminal, (Mark::Silent, None), now_us)
        }
        Reading::Holding { told: None } => Reading::Quiet,
        Reading::Stalled { .. } => Reading::Stalled { holding: false },
        other => other,
    };
    state.terminal_focus.focused = FocusedTerminal::Modelled {
        terminal,
        follow,
        reading,
    };
}

/// A window in the recovery set, `window`, answers again at `now_us`
/// (Table 1, column J; Table 2, column K): the focused terminal in it is
/// followed and read again.
pub(crate) fn recovered(state: &mut SrState, window: &RequestWindow, now_us: u64) {
    let focused = state.terminal_focus.focused;
    let Some(terminal) = focused
        .terminal()
        .filter(|terminal| terminal.window.handle == window.handle)
    else {
        return;
    };
    let follow = Some(want_follow(state, &terminal, false));
    let next = match focused {
        FocusedTerminal::None => return,
        FocusedTerminal::Baselining { read, told, .. } => FocusedTerminal::Baselining {
            terminal,
            follow,
            read: read.or_else(|| Some(want_baseline(state, &terminal, now_us))),
            told,
        },
        FocusedTerminal::WaitingForWindow { told, .. } => {
            let read = Some(want_baseline(state, &terminal, now_us));
            FocusedTerminal::Baselining {
                terminal,
                follow,
                read,
                told,
            }
        }
        FocusedTerminal::Modelled { reading, .. } => {
            let reading = match reading {
                Reading::Quiet | Reading::Stalled { holding: false } => {
                    want_output(state, &terminal, (Mark::Spoken, None), now_us)
                }
                Reading::Behind { .. } => Reading::Behind { told: now_us },
                Reading::Out {
                    token, mark, named, ..
                } => Reading::Out {
                    token,
                    mark,
                    named,
                    again: Some(now_us),
                },
                Reading::Holding { .. } | Reading::Stalled { holding: true } => {
                    Reading::Holding { told: Some(now_us) }
                }
            };
            FocusedTerminal::Modelled {
                terminal,
                follow,
                reading,
            }
        }
    };
    state.terminal_focus.focused = next;
}

/// The outpost generation that holds the focused terminal's node ended
/// (Table 1, column I): its state goes, with every model of its terminals.
pub(crate) fn outpost_ended(state: &mut SrState, outpost: verbatim_model::OutpostId) {
    if focused(state).is_some_and(|terminal| terminal.node.outpost() == outpost) {
        state.terminal_focus.focused = FocusedTerminal::None;
        state.terminal_focus.entry_read = false;
    }
    terminal_model::outpost_ended(state, outpost);
}

/// A full read of the focused terminal an entry asks inside the queue: a
/// caret key's starting or final read, or a review command's fresh read.
#[derive(Clone, Copy, Debug)]
pub(crate) struct EntryRead {
    /// The terminal.
    pub(crate) terminal: Terminal,
    /// The request.
    pub(crate) read: TerminalRead,
}

impl Request for EntryRead {
    type Reply = Box<TerminalReadReply>;

    fn reader(&self) -> Reader {
        Reader::Outpost(self.terminal.window.process)
    }

    fn window(&self) -> RequestWindow {
        self.terminal.window
    }

    fn wire(&self) -> Wire {
        Wire::Text {
            node: self.terminal.node,
            op: TextOp::ReadTerminal(self.read),
        }
    }

    fn decode(reply: Reply) -> Option<Self::Reply> {
        read_reply(reply)
    }
}

/// The full read of terminal `node` an entry asks now, at `now_us`, with
/// `starting` the caret key's unit for a starting read: `None` when `node`
/// is not the focused terminal, baselining or modelled.
pub(crate) fn entry_read(
    state: &mut SrState,
    node: NodeId,
    starting: Option<TextUnit>,
    now_us: u64,
) -> Option<EntryRead> {
    let terminal = match state.terminal_focus.focused {
        FocusedTerminal::Baselining { terminal, .. }
        | FocusedTerminal::Modelled { terminal, .. }
            if terminal.node == node =>
        {
            terminal
        }
        _ => return None,
    };
    let read = full_read(state, &terminal, (starting, false), now_us);
    state.terminal_focus.entry_read = true;
    Some(EntryRead { terminal, read })
}

/// An entry's full read `asked` ended with `outcome` at `now_us` (Table 1,
/// column G; Table 2, column F): a result is taken by the model and its
/// speech returned, for the entry to speak after the terminal's speech
/// entries waiting ahead of it; a read ended otherwise reads a change told
/// meanwhile at once, since nothing else will.
pub(crate) fn entry_read_ended(
    state: &mut SrState,
    asked: &EntryRead,
    outcome: Option<&TerminalReadReply>,
    (now_us, trace): (u64, TraceId),
) -> Option<SpeechEntry> {
    state.terminal_focus.entry_read = false;
    let node = asked.terminal.node;
    let Some(reply) = outcome else {
        if let FocusedTerminal::Modelled {
            terminal,
            follow,
            reading: Reading::Behind { .. },
        } = state.terminal_focus.focused
            && terminal.node == node
        {
            let reading = want_output(state, &terminal, (Mark::Spoken, None), now_us);
            state.terminal_focus.focused = FocusedTerminal::Modelled {
                terminal,
                follow,
                reading,
            };
        }
        return None;
    };
    let named = asked.read.start;
    match state.terminal_focus.focused {
        FocusedTerminal::Baselining {
            terminal,
            follow,
            read: None,
            told,
        } if terminal.node == node => modelled_by(
            state,
            &terminal,
            (follow, told),
            (reply, named),
            (now_us, trace),
        ),
        FocusedTerminal::Baselining { terminal, .. } if terminal.node == node => {
            // The baseline answers first, in the worker's order; a read
            // answered before it is taken all the same.
            let taken = terminal_model::take(state, node, reply, (named, false));
            speech_entry(
                node,
                taken,
                (reply.snapshot.started_ms, Mark::Spoken),
                trace,
            )
        }
        FocusedTerminal::WaitingForWindow { terminal, told } if terminal.node == node => {
            // The window answered before its half-second check found it:
            // the read is the baseline, and `Follow` is sent again at once,
            // since the hung one may not have registered.
            let follow = Some(want_follow(state, &terminal, false));
            modelled_by(
                state,
                &terminal,
                (follow, told),
                (reply, named),
                (now_us, trace),
            )
        }
        FocusedTerminal::Modelled {
            terminal,
            follow,
            reading,
        } if terminal.node == node => {
            let taken = terminal_model::take(state, node, reply, (named, false));
            let entry = speech_entry(
                node,
                taken,
                (reply.snapshot.started_ms, Mark::Spoken),
                trace,
            );
            let started_us = reply.started_us;
            let reading = match reading {
                Reading::Behind { told } => match uncovered(Some(told), started_us) {
                    Some(_) => want_output(state, &terminal, (Mark::Spoken, None), now_us),
                    None => owed_read(state, &terminal, Reading::Quiet, now_us),
                },
                Reading::Out {
                    token,
                    mark,
                    named,
                    again,
                } => Reading::Out {
                    token,
                    mark,
                    named,
                    again: uncovered(again, started_us),
                },
                Reading::Holding { told } => Reading::Holding {
                    told: uncovered(told, started_us),
                },
                Reading::Quiet => owed_read(state, &terminal, Reading::Quiet, now_us),
                Reading::Stalled { holding } => Reading::Stalled { holding },
            };
            state.terminal_focus.focused = FocusedTerminal::Modelled {
                terminal,
                follow,
                reading,
            };
            entry
        }
        _ => None,
    }
}

/// Speaks, for an entry whose read of the focused terminal answered, the
/// terminal's speech entries waiting ahead of the first focus-kind entry,
/// in order, then `own`, the read's own speech (Table 3, column F).
pub(crate) fn speak_read(cx: &mut Cx<Entries>, own: Option<SpeechEntry>) {
    let node = own.as_ref().map(|entry| entry.node);
    let waiting = cx.take_waiting_before(|entry| {
        matches!(entry, Entry::TerminalSpeech(speech) if node.is_none_or(|node| speech.node == node))
    });
    let entries = waiting
        .into_iter()
        .filter_map(|entry| match entry {
            Entry::TerminalSpeech(speech) => Some(*speech),
            _ => None,
        })
        .chain(own);
    for entry in entries {
        let effects = speak(&mut cx.facts_mut(), entry);
        for effect in effects {
            cx.emit(effect);
        }
    }
}

/// Makes `node` the modelled focused terminal, its window `window`, with
/// its model, for the flood policy's tests, which give it what its reads
/// say ([`answered_with`]).
#[cfg(test)]
pub(crate) fn modelled_for_tests(state: &mut SrState, node: NodeId, window: RequestWindow) {
    if terminal_model::model_of(state, node).is_none() {
        state
            .terminals
            .add(node, verbatim_text::terminal_reading::Settings::default());
    }
    state.terminal_focus.focused = FocusedTerminal::Modelled {
        terminal: Terminal {
            node,
            window,
            console: false,
        },
        follow: None,
        reading: Reading::Quiet,
    };
}

/// The output read asked under `token` answered with a read that said
/// `speech`, begun at `started_us`, as the model takes it, for the flood
/// policy's tests: the reading goes on as for a read the model took, and
/// the read's speech entry is returned; `None` for a token not out.
#[cfg(test)]
pub(crate) fn answered_with(
    state: &mut SrState,
    token: u64,
    speech: Said,
    started_us: u64,
) -> Option<SpeechEntry> {
    let FocusedTerminal::Modelled {
        terminal,
        follow,
        reading:
            Reading::Out {
                token: out,
                mark,
                again,
                ..
            },
    } = state.terminal_focus.focused
    else {
        return None;
    };
    if out != token {
        return None;
    }
    let generation = terminal_model::model_of(state, terminal.node)?.generation;
    let read = Taken {
        speech: Some(speech),
        key: None,
        generation,
    };
    let at_us = state.terminal_focus.now_us;
    output_taken(
        state,
        (&terminal, follow),
        (mark, again),
        (read, (started_us / 1_000, started_us)),
        (at_us, TraceId::from_raw(5)),
    )
    .into_iter()
    .find_map(|entry| match entry {
        Entry::TerminalSpeech(entry) => Some(*entry),
        _ => None,
    })
}

/// The requests wanted, taken, each with its token and what it asks, for
/// the tests.
#[cfg(test)]
pub(crate) fn wanted_for_tests(state: &mut SrState) -> Vec<(Option<u64>, Wire)> {
    std::mem::take(&mut state.terminal_focus.wanted)
        .into_iter()
        .map(|wanted| (wanted.token, wanted.request))
        .collect()
}

#[cfg(test)]
#[path = "reading_tests.rs"]
mod tests;
