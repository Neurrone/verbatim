//! The focused terminal (`books/dev/src/explanation/design/terminal-model.md`;
//! build 9's design, section 5): Core watches the terminal the committed
//! focus is, as NVDA's monitor loop does, and decides what is said; the
//! outpost reads the window, two screens up to the document's end, when
//! Core asks, by the request list's generic steps, of whose outcomes Core
//! makes the window ([`crate::terminal_window`]).
//!
//! - The watch ([`Watch`]) is the record of the focused terminal's reads:
//!   unread, its baseline out, live with the last window and its read idle
//!   or out, or gone. A change told marks it dirty; at settle a dirty live
//!   watch with no read out sends one, unless the pacing holds it. Every
//!   outcome of a read is matched by name.
//! - Each answer is judged against the last window by `verbatim-text`'s
//!   [`Reading`] (the echo, the keys' answers, the output), and its count
//!   of rows written past the end mark by the terminal's places
//!   ([`places`]); what it says joins the event queue as one entry
//!   ([`SpeechEntry`]) and is spoken at its turn, its output through the
//!   pacing ([`pacing`]).
//! - A key's or command's read is a read of the watch's too: it takes over
//!   any read out, and its answer is judged as the watch's.
//!
//! The record lives in the committed focus's context, so every commit
//! replaces it; for the same node not left the pacing and the last window
//! read are carried over, and the new watch reads on from that window. The
//! places outlive it, one record per terminal node, dropped with
//! the outpost generation that minted the node.

pub(crate) mod commands;
pub(crate) mod pacing;
pub(crate) mod places;

use std::collections::VecDeque;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use verbatim_model::batch::StepOutcome;
use verbatim_model::request::{CancelReason, Followed, PressAction, Reply};
use verbatim_model::{
    CaretKey, CaretMotion, Effect, FocusValidity, Message, NodeId, OutpostId, SegmentContent,
    SpeechPriority, TerminalSnapshot, TerminalText, TraceId, Utterance, UtteranceSegment,
};
use verbatim_text::terminal_window::{
    Caret, ColourRun, KeyKind, Read as Parsed, Reading, Unit, Window,
};

use crate::batch::{Builder, Got};
use crate::entries::{Entries, Entry};
use crate::event_queue::{Cx, Item, Outcome, Reader, RequestWindow};
use crate::keys::entry::{Fence, PressEntry};
use crate::state::SrState;
use crate::terminal::pacing::{Pacing, Under, Voice, Waiting};
use crate::terminal::places::TerminalPlaces;
use crate::terminal_window::{
    TerminalWindow, Unread, WindowAsk, WindowCaret, add_steps, window_of,
};
use crate::{editing, text};

/// The focused terminal, as the committed focus names it: its node, the
/// window its requests name, and whether that window is a console host's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Terminal {
    /// The terminal's node.
    pub(crate) node: NodeId,
    /// The window its requests name.
    pub(crate) window: RequestWindow,
    /// Whether its window is a console host's.
    pub(crate) console: bool,
}

/// The watch: the focused terminal's reads (build 9's design, 5.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Watch {
    /// No baseline: its read was refused, or its window hung; `dirty` when
    /// a change was told since.
    Unread { dirty: bool },
    /// Its baseline out under `token`.
    Baselining { token: u64, dirty: bool },
    /// Read: the last window, and its read.
    Live { old: Arc<Window>, read: Read },
    /// Its node is gone.
    Gone,
}

/// A live watch's read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Read {
    /// None out; `dirty` when a change was told since the last was sent.
    Idle { dirty: bool },
    /// One out under `token`.
    Out {
        token: u64,
        dirty: bool,
        speaks: Speaks,
        /// Why it was sent, by which its answer is judged.
        sent: Sent,
    },
}

/// Why a read of the watch's was sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Sent {
    /// A change told.
    Change,
    /// Once the pacing's group had handed on its last line: a read that
    /// waited for speech (TM 17).
    Waited,
    /// For a key or a command: its answer is the key's, in the key's
    /// window however long the read waited for the caret (TM 18).
    Key,
}

impl Sent {
    /// A read the watch sends, `released` when the pacing's group has
    /// handed on its last line.
    fn of(released: bool) -> Self {
        if released { Self::Waited } else { Self::Change }
    }
}

/// What a read out says when it answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Speaks {
    /// Its output, the echo and the keys' answers.
    Output,
    /// Only the echo and the keys' answers: a cut came while the pacing
    /// held it (TM, "Core's flood policy").
    KeysOnly,
}

/// What Core keeps of the focused terminal, in the committed focus's
/// context.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TerminalFocus {
    /// The terminal.
    pub(crate) terminal: Terminal,
    /// Its reads.
    pub(crate) watch: Watch,
    /// The reading beside the watch's last window: typing, held rows and
    /// the keys' window.
    pub(crate) reading: Reading,
    /// The output being handed to speech.
    pub(crate) pacing: Pacing,
    /// The newest key passed to the terminal: its trace and when it reached
    /// the program, by which a read is the key's answer.
    key: Option<(TraceId, u64)>,
    /// The command entered last, while its program may print it elsewhere
    /// (F12).
    reprint: Option<Reprint>,
}

/// A command entered at Enter, marked on its line, until the quiet period
/// after the key ends: a read that shows the line gone from the mark's
/// place and printed on a new row, as Pi and Claude Code print the input,
/// marks that row instead (TM, "Moving by command").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Reprint {
    /// The line's text.
    text: String,
    /// The mark's place when it was set, in rows above the document's end.
    place: u32,
    /// The rows every place had moved by then ([`places::TerminalPlaces::written`]).
    written: u64,
    /// When the quiet period after the Enter ends.
    until_us: u64,
}

impl TerminalFocus {
    /// What a commit of the same node, not left, carries over: the pacing,
    /// and the last window the watch read, from which the commit's watch
    /// reads on (T1), in a record whose watch the commit's terminal
    /// replaces.
    pub(crate) fn carried(&self) -> Self {
        Self {
            terminal: self.terminal,
            watch: match &self.watch {
                Watch::Live { old, .. } => Watch::Live {
                    old: Arc::clone(old),
                    read: Read::Idle { dirty: true },
                },
                Watch::Unread { .. } | Watch::Baselining { .. } | Watch::Gone => Watch::Gone,
            },
            reading: Reading::new(),
            pacing: self.pacing.clone(),
            key: None,
            reprint: None,
        }
    }
}

/// How a read of the watch's ended, every outcome by name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Ended {
    /// Column B: the window.
    Read(Box<TerminalWindow>),
    /// Column C1: refused or no longer awaited, `due` past its due time.
    Abandoned { due: bool },
    /// Column C2: a call failed, or the node has no text.
    Failed,
    /// Column C3: the window hung.
    Hung,
    /// Column C4: the node is gone.
    NotThere,
    /// Column C5: the outpost ended.
    ReaderGone,
}

impl Ended {
    /// A request's `outcome`, its result's step judged by `step`.
    fn of<T>(outcome: Outcome<T>, step: impl FnOnce(T) -> Self) -> Self {
        match outcome {
            Outcome::Result(result) => step(result),
            Outcome::Refused(_)
            | Outcome::NoLongerAwaited(CancelReason::Withdrawn | CancelReason::Foreground) => {
                Self::Abandoned { due: false }
            }
            Outcome::NoLongerAwaited(CancelReason::DueTime) => Self::Abandoned { due: true },
            Outcome::NotResponding | Outcome::NoLongerAwaited(CancelReason::Hung) => Self::Hung,
            Outcome::NotThere => Self::NotThere,
            Outcome::ReaderEnded => Self::ReaderGone,
        }
    }

    /// The window a read's step outcomes made, or why they made none.
    fn of_window(window: Result<TerminalWindow, Unread>) -> Self {
        match window {
            Ok(window) => Self::Read(Box::new(window)),
            Err(Unread::NotThere) => Self::NotThere,
            // A text that answers no read, as a call that failed, leaves
            // an empty baseline, as NVDA's monitor starts from empty text.
            Err(Unread::Failed) => Self::Failed,
        }
    }

    /// The window read's `outcomes`, from its first step on, as asked by
    /// `ask`, answered at `at_us`.
    fn of_steps(
        outcomes: Option<&[StepOutcome]>,
        (ask, first): (&WindowAsk, usize),
        at_us: u64,
    ) -> Self {
        match outcomes.and_then(|outcomes| outcomes.get(first..)) {
            Some(steps) => Self::of_window(window_of(steps, ask, at_us)),
            None => Self::Failed,
        }
    }
}

/// A key's answer, as its entry carries it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct KeyAnswer {
    unit: Unit,
    text: String,
}

/// What a read or the quiet timer said, joining the event queue as one
/// entry and spoken at its turn (build 9's design, row T2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SpeechEntry {
    /// The terminal read.
    pub(crate) node: NodeId,
    echo: Vec<String>,
    keys: Vec<KeyAnswer>,
    output: Vec<Waiting>,
    /// The key it answers, whose speech it is.
    key: Option<TraceId>,
    /// The trace it is spoken under.
    trace: TraceId,
    /// Its speech fence, set when it joins the queue.
    pub(crate) fence: Fence,
}

/// The terminal the committed focus of `role` read on `window`, `node`, is,
/// when it is one.
pub(crate) fn terminal_of(
    role: verbatim_model::Role,
    node: NodeId,
    window: RequestWindow,
    console: bool,
) -> Option<Terminal> {
    (role == verbatim_model::Role::Terminal).then_some(Terminal {
        node,
        window,
        console,
    })
}

/// The focused terminal's record.
fn focus_of(state: &SrState) -> Option<&TerminalFocus> {
    state.versions.focus()?.context.terminal.as_deref()
}

/// The focused terminal, its watch read or not.
pub(crate) fn focused(state: &SrState) -> Option<Terminal> {
    focus_of(state).map(|focus| focus.terminal)
}

/// Runs `then` on the focused terminal's record, taken out of the focus's
/// context while it runs, with the state beside it.
fn with_focus<R>(
    state: &mut SrState,
    then: impl FnOnce(&mut SrState, &mut TerminalFocus) -> R,
) -> Option<R> {
    let mut focus = state.versions.focus_mut()?.context.terminal.take()?;
    let result = then(state, &mut focus);
    if let Some(record) = state.versions.focus_mut() {
        record.context.terminal = Some(focus);
    }
    Some(result)
}

/// The places of terminal `node`, made the first time, moved to the back
/// as the most recently used.
fn places_mut(state: &mut SrState, node: NodeId) -> &mut TerminalPlaces {
    let places = &mut state.terminal_places;
    let entry = match places.iter().position(|(of, _)| *of == node) {
        Some(at) => places.remove(at),
        None => (node, TerminalPlaces::default()),
    };
    &mut places.push_mut(entry).1
}

/// The places of terminal `node`, when it has any.
pub(crate) fn places_of(state: &SrState, node: NodeId) -> Option<&TerminalPlaces> {
    state
        .terminal_places
        .iter()
        .find_map(|(of, places)| (*of == node).then_some(places))
}

/// Every anchor the terminals' places hold, with its node.
pub(crate) fn held(
    state: &SrState,
) -> impl Iterator<Item = (NodeId, verbatim_model::TextAnchor)> + '_ {
    state
        .terminal_places
        .iter()
        .flat_map(|(node, places)| places.anchors().map(move |anchor| (*node, anchor)))
}

/// Terminal `node`'s element is gone, its runtime id reused: its places go.
pub(crate) fn forget(state: &mut SrState, node: NodeId) {
    state.terminal_places.retain(|(of, _)| *of != node);
}

/// The outpost generation `outpost` ended: its terminals' places go.
pub(crate) fn outpost_ended(state: &mut SrState, outpost: OutpostId) {
    state
        .terminal_places
        .retain(|(node, _)| node.outpost() != Some(outpost));
}

/// Whether the keys' window is open at `now_us`: the newest key passed
/// reached the program within [`verbatim_text::terminal_window::KEY_WINDOW_US`].
fn in_key_window(focus: &TerminalFocus, now_us: u64) -> bool {
    focus.key.is_some_and(|(_, at_us)| {
        now_us <= at_us.saturating_add(verbatim_text::terminal_window::KEY_WINDOW_US)
    })
}

/// The window read of `focus`'s terminal as Core names it for an input at
/// `now_us`, `held` when the pacing held it; `None` for a baseline, judged
/// silently, whose read needs no key's colours.
fn ask_now(
    state: &mut SrState,
    focus: &TerminalFocus,
    held: bool,
    now_us: Option<u64>,
) -> WindowAsk {
    let colours = now_us.is_some_and(|now_us| in_key_window(focus, now_us));
    let opening = opening_of(state);
    places_mut(state, focus.terminal.node).ask(held, opening, colours)
}

/// "Lines spoken in full", the opening group of a read past more rows than
/// it and the window together.
fn opening_of(state: &SrState) -> u32 {
    u32::try_from(state.settings.full_lines()).unwrap_or(u32::MAX)
}

/// Wants a read of `focus`'s terminal for an input at `now_us`, `held`
/// when the pacing held it, `None` for a baseline, and returns its token.
fn want_read(state: &mut SrState, focus: &TerminalFocus, held: bool, now_us: Option<u64>) -> u64 {
    let ask = ask_now(state, focus, held, now_us);
    let mut read = Builder::<WatchRead>::named();
    add_steps(&mut read, focus.terminal.node, &ask);
    let token = state.wants.token();
    let window = focus.terminal.window;
    state.wants.want(
        Some(token),
        (Reader::Outpost(window.process), window),
        read.finish().question(),
    );
    token
}

/// The brand of the watch's own reads.
enum WatchRead {}

/// The committed focus changed (column F): `to` is the terminal now
/// committed, if the new focus is one, at `now_us`. Every `Follow` the
/// commit wants is wanted first, in order; a terminal's record is made,
/// carrying the pacing and the last window a commit of the same node kept,
/// and its read, an output read on from that window or else its baseline,
/// is wanted after the `Follow`.
pub(crate) fn committed(
    state: &mut SrState,
    to: Option<Terminal>,
    follows: Vec<(RequestWindow, Option<Followed>)>,
) {
    for (window, node) in follows {
        state.wants.want(
            None,
            (Reader::Outpost(window.process), window),
            crate::focus::follow::batch(node.as_ref()).question(),
        );
    }
    let Some(record) = state.versions.focus_mut() else {
        return;
    };
    let carried = record.context.terminal.take();
    let Some(terminal) = to else {
        return;
    };
    let (pacing, old) = carried
        .filter(|carried| carried.terminal.node == terminal.node)
        .map(|carried| {
            let old = match carried.watch {
                Watch::Live { old, .. } => Some(old),
                Watch::Unread { .. } | Watch::Baselining { .. } | Watch::Gone => None,
            };
            (carried.pacing, old)
        })
        .unwrap_or_default();
    let mut focus = TerminalFocus {
        terminal,
        watch: Watch::Unread { dirty: false },
        reading: Reading::new(),
        pacing,
        key: None,
        reprint: None,
    };
    focus.watch = match old {
        // The same node not left reads on from the window last spoken, so
        // output written meanwhile, a dropped read's included, is spoken
        // (T1; Claude, 2026-11-22).
        Some(old) => {
            let waited = focus.pacing.released();
            let token = want_read(state, &focus, waited, None);
            Watch::Live {
                old,
                read: Read::Out {
                    token,
                    dirty: false,
                    speaks: Speaks::Output,
                    sent: Sent::of(waited),
                },
            }
        }
        None => Watch::Baselining {
            token: want_read(state, &focus, false, None),
            dirty: false,
        },
    };
    if let Some(record) = state.versions.focus_mut() {
        record.context.terminal = Some(Box::new(focus));
    }
}

/// Column A: a change of terminal `node` told at `now_us`; a tell for any
/// other node is dropped.
pub(crate) fn told(state: &mut SrState, node: NodeId, now_us: u64) {
    let _ = with_focus(state, |state, focus| {
        if focus.terminal.node == node {
            mark_dirty(&mut focus.watch);
            settle(state, focus, now_us);
        }
    });
}

/// Columns A and R: the watch has a change to read.
fn mark_dirty(watch: &mut Watch) {
    match watch {
        Watch::Unread { dirty }
        | Watch::Baselining { dirty, .. }
        | Watch::Live {
            read: Read::Idle { dirty } | Read::Out { dirty, .. },
            ..
        } => *dirty = true,
        Watch::Gone => {}
    }
}

/// Column R: a window in the recovery set, `window`, answers again at
/// `now_us`: the focused terminal in it is read again.
pub(crate) fn recovered(state: &mut SrState, window: &RequestWindow, now_us: u64) {
    let _ = with_focus(state, |state, focus| {
        if focus.terminal.window.handle == window.handle {
            mark_dirty(&mut focus.watch);
            settle(state, focus, now_us);
        }
    });
}

/// Column E, after every input that changes the watch or the pacing: a
/// dirty unread watch sends its baseline, and a dirty live one with no
/// read out sends one unless the pacing holds it; `now_us` is the input's
/// time. The look at what a pacing due holds is taken only at a turn of
/// the queue's ([`look`]).
fn settle(state: &mut SrState, focus: &mut TerminalFocus, now_us: u64) {
    let full = state.settings.full_lines();
    match focus.watch {
        Watch::Unread { dirty: true } => {
            let token = want_read(state, focus, false, None);
            focus.watch = Watch::Baselining {
                token,
                dirty: false,
            };
        }
        Watch::Live {
            read: Read::Idle { dirty: true },
            ..
        } if !focus.pacing.holds(full) => {
            let waited = focus.pacing.released();
            let token = want_read(state, focus, waited, Some(now_us));
            if let Watch::Live { read, .. } = &mut focus.watch {
                *read = Read::Out {
                    token,
                    dirty: false,
                    speaks: Speaks::Output,
                    sent: Sent::of(waited),
                };
            }
        }
        Watch::Unread { dirty: false }
        | Watch::Baselining { .. }
        | Watch::Live { .. }
        | Watch::Gone => {}
    }
}

/// At a turn of the queue's, a mark's or a speech entry's: a pacing due
/// looks at what waits (F1) unless more output is yet to reach it, an
/// output read of the watch's out or, `waits`, a speech entry of the
/// terminal's with output waiting for its turn, so that the count is taken
/// after the lines of the read at the group's last line are applied (Q9;
/// Claude, 2026-11-22).
fn look(state: &mut SrState, focus: &mut TerminalFocus, waits: bool) -> Vec<Effect> {
    let reading = waits
        || matches!(
            focus.watch,
            Watch::Live {
                read: Read::Out {
                    speaks: Speaks::Output,
                    ..
                },
                ..
            }
        );
    speaking(state, |voice| focus.pacing.look(reading, voice))
}

/// Whether a speech entry of the focused terminal with output waits in the
/// queue, `waiting`, for its turn ([`look`]).
pub(crate) fn output_waits(state: &SrState, waiting: &VecDeque<Item<Entry>>) -> bool {
    let Some(node) = focused(state).map(|terminal| terminal.node) else {
        return false;
    };
    waiting
        .iter()
        .flat_map(|item| match item {
            Item::One(entry) | Item::Menu { entry, .. } => std::slice::from_ref(entry),
            Item::Run(entries) => entries.as_slice(),
        })
        .any(|entry| {
            matches!(entry, Entry::TerminalSpeech(speech)
                if speech.node == node && !speech.output.is_empty())
        })
}

/// The speech entry an ended read of the watch's joins: `entry`, the read's
/// own, or, when it made none while the pacing is due, an empty one, whose
/// turn takes the look its own turn would have ([`look`]).
fn joins(focus: &TerminalFocus, entry: Option<SpeechEntry>, trace: TraceId) -> Option<SpeechEntry> {
    entry.or_else(|| {
        matches!(focus.pacing, Pacing::Due { .. }).then(|| SpeechEntry {
            node: focus.terminal.node,
            echo: Vec::new(),
            keys: Vec::new(),
            output: Vec::new(),
            key: None,
            trace,
            fence: Fence::default(),
        })
    })
}

/// Runs `then` with the pacing's voice: the limits, the marks and the
/// focus's utterance.
fn speaking<R>(state: &mut SrState, then: impl FnOnce(&mut Voice<'_>) -> R) -> R {
    let (full, last) = (state.settings.full_lines(), state.settings.last_lines());
    let focus = state.versions.focus().map(|focus| &focus.context);
    let source = focus.map(|focus| crate::reduce::source_of(&focus.snapshot));
    let validity = focus.map(|focus| FocusValidity {
        node: focus.snapshot.id,
        window: verbatim_model::WindowIdentity::of(&focus.snapshot),
    });
    let utterance = move |trace_id, segments| Utterance {
        trace_id,
        priority: SpeechPriority::Queued,
        segments,
        source,
        say_all: false,
        validity,
    };
    let mut voice = Voice {
        full,
        last,
        next_mark: &mut state.next_mark,
        utterance: &utterance,
    };
    then(&mut voice)
}

/// A read of the watch's under `token` ended as `ended`: the watch moves
/// by its column, and a window read is judged, its speech returned to join
/// the queue; an answer the watch does not await is dropped (column D).
fn answered(
    state: &mut SrState,
    focus: &mut TerminalFocus,
    (token, ended): (u64, Ended),
    trace: TraceId,
) -> Option<SpeechEntry> {
    let node = focus.terminal.node;
    match focus.watch.clone() {
        Watch::Baselining { token: out, dirty } if out == token => {
            focus.watch = match ended {
                Ended::Read(read) => {
                    let window = Arc::new(parse(&read));
                    take_caret(state, node, &read);
                    let opening = opening_of(state);
                    let _ = places_mut(state, node).took(&read, &window, (true, 0, opening));
                    Watch::Live {
                        old: window,
                        read: Read::Idle { dirty },
                    }
                }
                Ended::Abandoned { .. } => Watch::Unread { dirty },
                Ended::Failed => Watch::Live {
                    old: Arc::new(Window::empty()),
                    read: Read::Idle { dirty },
                },
                Ended::Hung => Watch::Unread { dirty: false },
                Ended::NotThere => Watch::Gone,
                // The re-take replaces the record.
                Ended::ReaderGone => return None,
            };
            None
        }
        Watch::Live {
            old,
            read:
                Read::Out {
                    token: out,
                    dirty,
                    speaks,
                    sent,
                },
        } if out == token => {
            let (old, read, entry) = match ended {
                Ended::Read(read) => {
                    let (old, entry) = judged(state, focus, (&old, &read), (speaks, sent), trace);
                    (old, Read::Idle { dirty }, Some(entry))
                }
                Ended::Abandoned { due } => (
                    old,
                    Read::Idle {
                        dirty: dirty || due,
                    },
                    None,
                ),
                Ended::Failed => (old, Read::Idle { dirty }, None),
                Ended::Hung => (old, Read::Idle { dirty: false }, None),
                Ended::NotThere => {
                    focus.watch = Watch::Gone;
                    return None;
                }
                Ended::ReaderGone => return None,
            };
            focus.watch = Watch::Live { old, read };
            entry
        }
        Watch::Unread { .. } | Watch::Baselining { .. } | Watch::Live { .. } | Watch::Gone => None,
    }
}

/// The window `read` answered, parsed.
fn parse(read: &TerminalWindow) -> Window {
    let caret = read
        .caret
        .as_ref()
        .zip(read.caret_rows_to_end)
        .map(|(caret, rows)| Caret {
            rows_to_end: rows,
            column: {
                let line = &caret.report.line;
                let offset = crate::text::byte_offset(line.offset).min(line.text.len());
                line.text
                    .get(..offset)
                    .map_or(0, |before| before.chars().count())
            },
        });
    let colours = read.colours.as_ref().map(|rows| {
        rows.iter()
            .map(|runs| {
                runs.iter()
                    .map(|run| ColourRun {
                        column: run.column,
                        length: run.length,
                        foreground: run.foreground,
                        background: run.background,
                    })
                    .collect()
            })
            .collect()
    });
    Window::new(Parsed {
        text: &read.rows.text,
        lengths: &read.rows.lengths,
        width: read.width,
        screen_top: read.screen_top,
        screen_rows: read.screen_rows,
        caret,
        colours,
        ended_us: read.ended_us,
    })
}

/// The caret `read` found, taken as the focus's.
fn take_caret(state: &mut SrState, node: NodeId, read: &TerminalWindow) {
    if let Some(caret) = &read.caret {
        editing::take_caret(state, node, caret.report.clone());
    }
}

/// Judges the window `read` against `old` (column B of a live watch): the
/// caret taken, the diff, the count of rows written past the mark, and a
/// new screen; returns the window the watch keeps and the read's speech.
fn judged(
    state: &mut SrState,
    focus: &mut TerminalFocus,
    (old, read): (&Arc<Window>, &TerminalWindow),
    (speaks, sent): (Speaks, Sent),
    trace: TraceId,
) -> (Arc<Window>, SpeechEntry) {
    let node = focus.terminal.node;
    let window = Arc::new(parse(read));
    take_caret(state, node, read);
    let answer = match sent {
        Sent::Change => focus.reading.answer(old, &window),
        Sent::Waited => focus.reading.answer_waited(old, &window),
        Sent::Key => focus.reading.answer_key(old, &window),
    };
    let mut output = Vec::new();
    let kept = if answer.dropped {
        Arc::clone(old)
    } else {
        let opening = opening_of(state);
        let placed = places_mut(state, node).took(
            read,
            &window,
            (answer.shares_line, answer.output.len(), opening),
        );
        let speaks_output = speaks == Speaks::Output && state.settings.report_terminal_output;
        if let Some(past) = placed.past {
            output = past;
        } else if placed.within && !answer.shares_line {
            // A new screen (F14): its rows, and the review cursor at the
            // caret.
            output = window
                .screen_lines()
                .into_iter()
                .map(Waiting::Line)
                .collect();
            if let Some(navigator) = state.navigator.as_mut()
                && navigator.object.id == node
            {
                navigator.text = crate::state::ReviewText::Caret;
            }
            places_mut(state, node).review = None;
        } else {
            output = answer.output.into_iter().map(Waiting::Line).collect();
        }
        if !speaks_output {
            output.clear();
        }
        reprinted(state, focus, (old, &window));
        Arc::clone(&window)
    };
    let key = focus
        .key
        .filter(|(_, at_us)| {
            read.ended_us <= at_us.saturating_add(verbatim_text::terminal_window::KEY_WINDOW_US)
        })
        .map(|(trace, _)| trace);
    let entry = SpeechEntry {
        node,
        echo: answer.echo,
        keys: answer
            .keys
            .into_iter()
            .map(|said| KeyAnswer {
                unit: said.unit,
                text: said.text,
            })
            .collect(),
        output,
        key,
        trace,
        fence: Fence::default(),
    };
    (kept, entry)
}

/// After a read of `focus`'s terminal, `new` against `old`, the command
/// entered last (F12): forgotten once the read ends past the quiet period
/// after its Enter, or once the read shows its line still at the mark's
/// place; else, when a new row of the read holds the line, the nearest the
/// caret, the mark moves there, as where the program printed the input.
fn reprinted(state: &mut SrState, focus: &mut TerminalFocus, (old, new): (&Window, &Window)) {
    let Some(reprint) = focus.reprint.take() else {
        return;
    };
    if new.ended_us() > reprint.until_us {
        return;
    }
    let node = focus.terminal.node;
    let moved = places_mut(state, node)
        .written()
        .saturating_sub(reprint.written);
    let Ok(moved) = u32::try_from(moved) else {
        return;
    };
    let place = reprint.place.saturating_add(moved);
    let at = new.places_of(&reprint.text);
    if at.contains(&place) {
        return;
    }
    // A row the last read had at its place, moved with the places, is no
    // new row: an older line with the same text does not count.
    let before = old.places_of(&reprint.text);
    let caret = new.caret_line().map_or(0, |(_, above)| above);
    let Some(found) = at
        .into_iter()
        .filter(|&at| {
            at.checked_sub(moved)
                .is_none_or(|was| !before.contains(&was))
        })
        .min_by_key(|at| at.abs_diff(caret))
    else {
        focus.reprint = Some(reprint);
        return;
    };
    let places = places_mut(state, node);
    places
        .marks
        .retain(|mark| !(mark.above == place && mark.text == reprint.text));
    places.mark(places::RowPlace {
        above: found,
        column: 0,
        text: reprint.text,
    });
}

/// The read `read` as the control plane's event stream carries it, for the
/// end-to-end suite's waits on a prompt: its rows, the caret and the
/// caret's rows to the end.
fn observed(terminal: &Terminal, read: &TerminalWindow, trace: TraceId) -> Effect {
    let snapshot = TerminalSnapshot {
        text: read.rows.clone(),
        screen: TerminalText::default(),
        from_start: true,
        shrank: false,
        at_document_start: false,
        alternate: read.screen_at_start,
        first_row: read.first_row.clone(),
        caret_rows_to_end: read.caret_rows_to_end.unwrap_or(0),
        caret_line: TerminalText::default(),
        caret_column: 0,
        above: read.above,
        started_ms: read.began_us / 1_000,
        ended_ms: read.ended_us / 1_000,
        waited: false,
        colours: None,
        caret: read.caret.as_ref().map(|caret| caret.report.clone()),
    };
    verbatim_model::Observed::effect(
        trace,
        (terminal.window.process, verbatim_model::Backend::Uia),
        None,
        verbatim_model::NormalizedEvent::TerminalSnapshot {
            node_id: terminal.node,
            snapshot: Box::new(snapshot),
        },
    )
}

/// A request Core's watch asked under `token` ended with `outcome` at
/// `at_us`, when its answer reached Core and the read's time: the watch
/// moves by its column, the read's speech joins the back, and what settle
/// says goes into `said`.
pub(crate) fn facts_answered(
    state: &mut SrState,
    token: u64,
    outcome: Outcome<Reply>,
    (at_us, trace, said): (u64, TraceId, &mut Vec<Effect>),
) -> Vec<Entry> {
    with_focus(state, |state, focus| {
        // The read's steps follow from what Core names now: the marks and
        // the height change only when a read of the watch's is taken, so
        // for the read the watch awaits they are those it was sent with;
        // the search and the colours, last, say themselves by their kinds.
        let ask = ask_now(state, focus, false, Some(at_us));
        let ended = Ended::of(outcome, |reply| match reply {
            Reply::Steps(steps) => Ended::of_steps(Some(&steps), (&ask, 0), at_us),
        });
        if let Ended::Read(read) = &ended
            && watch_token(&focus.watch) == Some(token)
        {
            said.push(observed(&focus.terminal, read, trace));
        }
        let entry = answered(state, focus, (token, ended), trace);
        settle(state, focus, at_us);
        joins(focus, entry, trace)
            .map(|entry| Entry::TerminalSpeech(Box::new(entry)))
            .into_iter()
            .collect()
    })
    .unwrap_or_default()
}

/// The token of the read the watch has out.
fn watch_token(watch: &Watch) -> Option<u64> {
    match watch {
        Watch::Baselining { token, .. }
        | Watch::Live {
            read: Read::Out { token, .. },
            ..
        } => Some(*token),
        Watch::Unread { .. }
        | Watch::Live {
            read: Read::Idle { .. },
            ..
        }
        | Watch::Gone => None,
    }
}

/// Column K: a key's or command's read of terminal `node` sent under a new
/// token, which takes over the read out, its answer stale. `None` when
/// `node` is not the focused terminal, or its watch is gone.
fn key_read_sent(state: &mut SrState, node: NodeId, now_us: u64) -> Option<KeySent> {
    with_focus(state, |state, focus| {
        if focus.terminal.node != node {
            return None;
        }
        let token = state.wants.token();
        focus.watch = match &focus.watch {
            Watch::Unread { dirty } | Watch::Baselining { dirty, .. } => Watch::Baselining {
                token,
                dirty: *dirty,
            },
            Watch::Live {
                old,
                read: Read::Idle { dirty } | Read::Out { dirty, .. },
            } => Watch::Live {
                old: Arc::clone(old),
                read: Read::Out {
                    token,
                    dirty: *dirty,
                    speaks: Speaks::Output,
                    sent: Sent::Key,
                },
            },
            Watch::Gone => return None,
        };
        Some(KeySent {
            token,
            ask: ask_now(state, focus, false, Some(now_us)),
        })
    })
    .flatten()
}

/// A key's or command's read as sent: its token and what Core named to
/// it.
#[derive(Debug, PartialEq, Eq)]
struct KeySent {
    token: u64,
    ask: WindowAsk,
}

/// What a key's or command's read brought.
#[derive(Clone, Debug)]
pub(crate) struct KeyRead {
    /// Whether it answered with the window.
    pub(crate) answered: bool,
    /// The caret it found, when it read one: a caret key's starting point.
    pub(crate) caret: Option<WindowCaret>,
}

/// Where a key's or command's read's speech goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReadSpeech {
    /// It joins the back of the queue as its own entry (row T2): a key's,
    /// which says nothing itself.
    Joins,
    /// It is spoken at once in the entry's own turn, under its fence: a
    /// command's, whose own answer follows it, so that output the read
    /// found is said before the answer, as it was written before the
    /// press.
    Now,
}

/// A key's or command's read under `token` ended as `ended`, inside its
/// entry `cx`: judged as the watch's, its speech going as `speech` says.
/// What it brought, whatever the watch made of it.
fn key_read_ended(
    cx: &mut Cx<Entries>,
    token: u64,
    ended: Ended,
    (trace, speech): (TraceId, ReadSpeech),
) -> KeyRead {
    let now_us = cx.now_us();
    let read = match &ended {
        Ended::Read(read) => KeyRead {
            answered: true,
            caret: read.caret.clone(),
        },
        Ended::Abandoned { .. }
        | Ended::Failed
        | Ended::Hung
        | Ended::NotThere
        | Ended::ReaderGone => KeyRead {
            answered: false,
            caret: None,
        },
    };
    let (entry, effects) = {
        let mut facts = cx.facts_mut();
        with_focus(&mut facts, |state, focus| {
            let mut effects = Vec::new();
            if let Ended::Read(read) = &ended
                && watch_token(&focus.watch) == Some(token)
            {
                effects.push(observed(&focus.terminal, read, trace));
            }
            let entry = answered(state, focus, (token, ended), trace);
            settle(state, focus, now_us);
            (joins(focus, entry, trace), effects)
        })
        .unwrap_or_default()
    };
    for effect in effects {
        cx.emit(effect);
    }
    if let Some(mut entry) = entry {
        match speech {
            ReadSpeech::Joins => cx.join(Entry::TerminalSpeech(Box::new(entry))),
            ReadSpeech::Now => {
                entry.fence = crate::entries::front_fence(cx);
                speech_turn(cx, entry);
            }
        }
    }
    read
}

/// A key's or command's read of the focused terminal `node`, under its
/// key's `trace`, its speech going as `speech` says: `None` when it is not
/// the focused terminal or its watch is gone; else what it brought.
pub(crate) async fn key_read(
    cx: &mut Cx<Entries>,
    (node, window): (NodeId, RequestWindow),
    (trace, speech): (TraceId, ReadSpeech),
    due_us: u64,
) -> Option<KeyRead> {
    let now_us = cx.now_us();
    let KeySent { token, ask } = key_read_sent(&mut cx.facts_mut(), node, now_us)?;
    let mut batch = cx.batch(|| ());
    add_steps(&mut batch, node, &ask);
    let request = batch.finish().of(Reader::Outpost(window.process), window);
    let outcome = cx.ask(request, due_us).await;
    let at_us = cx.now_us();
    let ended = Ended::of(outcome, |answers| {
        Ended::of_steps(answers.outcomes(), (&ask, 0), at_us)
    });
    Some(key_read_ended(cx, token, ended, (trace, speech)))
}

/// How a caret key's wait in a terminal ended.
pub(crate) enum KeyWait {
    /// It answered; the read after it is the watch's.
    Done,
    /// An arrival withdrew it.
    Withdrawn,
    /// It failed or was not answered.
    Ended,
}

/// A caret key's wait in the focused terminal `node`, then its final read,
/// in one request: the read is a key's read of the watch's (column K).
pub(crate) async fn key_wait(
    cx: &mut Cx<Entries>,
    (node, window): (NodeId, RequestWindow),
    wait: verbatim_model::batch::CaretWait,
    (trace, due_us): (TraceId, u64),
) -> KeyWait {
    let now_us = cx.now_us();
    let Some(KeySent { token, ask }) = key_read_sent(&mut cx.facts_mut(), node, now_us) else {
        return KeyWait::Ended;
    };
    let mut batch = cx.batch(|| ());
    let waited = batch.wait_for_caret(node, wait);
    add_steps(&mut batch, node, &ask);
    let request = batch.finish().of(Reader::Outpost(window.process), window);
    let outcome = cx.ask(request, due_us).await;
    let at_us = cx.now_us();
    let (ended, end) = match outcome.split::<std::convert::Infallible>() {
        Ok(answers) => (
            Ended::of_steps(answers.outcomes(), (&ask, 1), at_us),
            if matches!(answers.get(waited), Got::Done(_)) {
                KeyWait::Done
            } else {
                KeyWait::Ended
            },
        ),
        Err(Outcome::NoLongerAwaited(CancelReason::Withdrawn)) => {
            (Ended::Abandoned { due: false }, KeyWait::Withdrawn)
        }
        Err(unanswered) => (
            Ended::of(unanswered, |never| match never {}),
            KeyWait::Ended,
        ),
    };
    let _ = key_read_ended(cx, token, ended, (trace, ReadSpeech::Joins));
    end
}

/// A speech entry at its turn: while its terminal is still the focused
/// terminal, the echo and the keys' answers are spoken, and the output is
/// handed to the pacing unless a cut passed its fence (column G).
pub(crate) fn speech_turn(cx: &mut Cx<Entries>, entry: SpeechEntry) {
    let now_us = cx.now_us();
    let waits = output_waits(&cx.facts(), &cx.waiting());
    let effects = speak(&mut cx.facts_mut(), entry, (now_us, waits));
    for effect in effects {
        cx.emit(effect);
    }
}

/// [`speech_turn`]'s speech, at `now_us`, `waits` when another speech
/// entry of the terminal's with output waits behind it ([`look`]).
fn speak(state: &mut SrState, entry: SpeechEntry, (now_us, waits): (u64, bool)) -> Vec<Effect> {
    let cut = state.arrivals.cut_since(entry.fence);
    let trace = entry.key.unwrap_or(entry.trace);
    let effects = with_focus(state, |state, focus| {
        if focus.terminal.node != entry.node {
            return Vec::new();
        }
        let mut effects = Vec::new();
        for typed in &entry.echo {
            // With passwords spoken, typing was echoed as it was typed.
            if !state.settings.speak_terminal_passwords {
                effects.extend(editing::echo(state, trace, typed));
            }
        }
        for answer in &entry.keys {
            let segments = match answer.unit {
                Unit::Character => text::character_segments(
                    Some(answer.text.as_str()).filter(|text| !text.is_empty()),
                    None,
                ),
                Unit::Word => text::word_segments(&answer.text, None),
                Unit::Line => text::text_segments(&answer.text, None),
            };
            effects.push(editing::speak(trace, segments));
        }
        if !cut && !entry.output.is_empty() && state.settings.report_terminal_output {
            let under = Under {
                trace,
                fence: entry.fence.key(),
            };
            effects.extend(speaking(state, |voice| {
                focus.pacing.take(entry.output, under, voice)
            }));
        }
        settle(state, focus, now_us);
        effects.extend(look(state, focus, waits));
        effects
    })
    .unwrap_or_default();
    match entry.key {
        Some(key) => effects
            .into_iter()
            .map(|effect| Effect::ForKey(key, Box::new(effect)))
            .collect(),
        None => effects,
    }
}

/// Columns H and I: playback reached `mark`, at its turn at `now_us`,
/// `waits` when a speech entry of the terminal's with output waits for its
/// turn ([`look`]).
pub(crate) fn mark_reached(
    state: &mut SrState,
    mark: verbatim_model::SpeechMark,
    (now_us, waits): (u64, bool),
) -> Vec<Effect> {
    with_focus(state, |state, focus| {
        let mut effects = speaking(state, |voice| focus.pacing.mark_reached(mark, voice));
        settle(state, focus, now_us);
        effects.extend(look(state, focus, waits));
        effects
    })
    .unwrap_or_default()
}

/// Column J: speech was cut off by an input at `now_us`. A read out while
/// the pacing holds speaks only keys, and a change held unread is read at
/// once, speaking only keys; what waits is dropped.
pub(crate) fn cut(state: &mut SrState, now_us: u64) {
    let _ = with_focus(state, |state, focus| {
        let held = focus.pacing.holds(state.settings.full_lines());
        focus.pacing = Pacing::Idle;
        if held {
            match &mut focus.watch {
                Watch::Live {
                    read: Read::Out { speaks, .. },
                    ..
                } => *speaks = Speaks::KeysOnly,
                Watch::Live {
                    read: Read::Idle { dirty: true },
                    ..
                } => {
                    let token = want_read(state, focus, true, Some(now_us));
                    if let Watch::Live { read, .. } = &mut focus.watch {
                        *read = Read::Out {
                            token,
                            dirty: false,
                            speaks: Speaks::KeysOnly,
                            sent: Sent::Waited,
                        };
                    }
                }
                Watch::Live { .. }
                | Watch::Unread { .. }
                | Watch::Baselining { .. }
                | Watch::Gone => {}
            }
        }
        settle(state, focus, now_us);
    });
}

/// An utterance cut off before playback reached `marks`, an input at
/// `now_us`: when output handed on waits for one of them, speech was cut
/// off (column J).
pub(crate) fn utterance_cancelled(
    state: &mut SrState,
    marks: &[verbatim_model::SpeechMark],
    now_us: u64,
) {
    if focus_of(state).is_some_and(|focus| focus.pacing.cancelled(marks)) {
        cut(state, now_us);
    }
}

/// Whether an effect cuts speech off, dropping output handed to speech.
pub(crate) fn cuts_speech(effect: &Effect) -> bool {
    match effect {
        Effect::StopSpeech => true,
        Effect::Speak(utterance, _) => utterance.priority == SpeechPriority::Interrupt,
        Effect::ForKey(_, effect) => cuts_speech(effect),
        _ => false,
    }
}

/// Column K2: "report new output" turned off, by its key or the settings,
/// at `now_us`, drops what waits.
pub(crate) fn drop_waiting(state: &mut SrState, now_us: u64) {
    let _ = with_focus(state, |state, focus| {
        focus.pacing.drop_waiting();
        settle(state, focus, now_us);
    });
}

/// Verbatim+5, pressed at `now_us`: toggles "Report new output", says its
/// new value, and reports the settings for the shell to save.
pub(crate) fn toggle(state: &mut SrState, trace_id: TraceId, now_us: u64) -> Vec<Effect> {
    let on = !state.settings.report_terminal_output;
    state.settings.report_terminal_output = on;
    if !on {
        drop_waiting(state, now_us);
    }
    vec![
        editing::speak(
            trace_id,
            vec![UtteranceSegment::new(SegmentContent::Message(if on {
                Message::ReportNewOutputOn
            } else {
                Message::ReportNewOutputOff
            }))],
        ),
        Effect::SettingsChanged(state.settings),
    ]
}

/// What a caret key is to the reading: its unit, Delete, Enter, or any
/// other key.
pub(crate) fn caret_key_kind(key: CaretKey) -> KeyKind {
    match key.motion {
        CaretMotion::PreviousCharacter
        | CaretMotion::NextCharacter
        | CaretMotion::StartOfLine
        | CaretMotion::EndOfLine => KeyKind::Caret(Unit::Character),
        CaretMotion::PreviousWord | CaretMotion::NextWord => KeyKind::Caret(Unit::Word),
        CaretMotion::PreviousLine
        | CaretMotion::NextLine
        | CaretMotion::PreviousPage
        | CaretMotion::NextPage
        | CaretMotion::PreviousParagraph
        | CaretMotion::NextParagraph
        | CaretMotion::Top
        | CaretMotion::Bottom => KeyKind::Caret(Unit::Line),
        CaretMotion::Delete => KeyKind::Delete,
        // Enter held for a terminal, by a hook view behind the focus, is
        // the carriage return it types there.
        CaretMotion::NewLine => KeyKind::Enter,
        _ => KeyKind::Other,
    }
}

/// What text typed is to the reading: Enter, which forgets the typing not
/// yet shown; Tab, which forgets it too; a control character, typing
/// nothing shown as itself; and anything else, typed.
fn typed_kind(typed: &str) -> KeyKind {
    if typed.chars().any(|c| matches!(c, '\r' | '\n')) {
        KeyKind::Enter
    } else if typed.contains('\t') {
        KeyKind::Forget
    } else if typed.chars().any(char::is_control) {
        KeyKind::Other
    } else {
        KeyKind::Typed
    }
}

/// `press` reached Core's inbox: a key the focused terminal takes at its
/// arrival is passed to its reading, with when it reached the program,
/// which for a key behind a held stroke is no later than that stroke's
/// release (`state.strokes`). A held caret key is passed at its pass
/// ([`key_passed`]).
pub(crate) fn key_arrived(state: &mut SrState, press: &PressEntry) {
    let Some(terminal) = focused(state) else {
        return;
    };
    let (kind, typed) = match &press.press.action {
        PressAction::Typed { text } => (typed_kind(text), Some(text.as_str())),
        PressAction::Clearing => (KeyKind::Forget, None),
        PressAction::EditingChord => (KeyKind::Other, None),
        PressAction::CaretKey {
            key,
            held: None,
            bound,
        } if bound
            .as_ref()
            .is_some_and(|bound| bound.node == terminal.node) =>
        {
            (caret_key_kind(*key), None)
        }
        PressAction::CaretKey { .. }
        | PressAction::Command { .. }
        | PressAction::Script { .. }
        | PressAction::Control => return,
    };
    let pressed_us = press.press.pressed_at_us;
    let at_us = if state.strokes.holds() {
        state
            .strokes
            .next_release()
            .map_or(pressed_us, |release| release.max(pressed_us))
    } else {
        pressed_us
    };
    let typed = typed.filter(|_| kind == KeyKind::Typed).map(str::to_owned);
    key_passed(state, (kind, typed.as_deref()), at_us, press.trace_id);
}

/// A key of `kind` reached the focused terminal's program at `at_us` under
/// `trace`, `typed` its text when it is typed: held for its echo while
/// "speak passwords" is off, and opening or extending the keys' window;
/// Enter on a caret line with text marks the line for the command marks
/// (F12).
pub(crate) fn key_passed(
    state: &mut SrState,
    (kind, typed): (KeyKind, Option<&str>),
    at_us: u64,
    trace: TraceId,
) {
    let speak_passwords = state.settings.speak_terminal_passwords;
    let _ = with_focus(state, |state, focus| {
        if let Some(typed) = typed
            && !speak_passwords
        {
            focus.reading.hold_typed(typed, at_us);
        }
        if let Watch::Live { old, .. } = &focus.watch {
            if kind == KeyKind::Enter
                && let Some((line, above)) = old.caret_line()
                && !line.trim().is_empty()
            {
                let places = places_mut(state, focus.terminal.node);
                places.mark(places::RowPlace {
                    above,
                    column: 0,
                    text: line.to_owned(),
                });
                focus.reprint = Some(Reprint {
                    text: line.to_owned(),
                    place: above,
                    written: places.written(),
                    until_us: at_us.saturating_add(verbatim_text::terminal_window::QUIET_US),
                });
            }
            focus.reading.key(kind, at_us, old);
        }
        focus.key = Some((trace, at_us));
    });
}

/// The one time the focused terminal's held rows wait for, for
/// `Logic::due`.
pub(crate) fn due(state: &SrState) -> Option<u64> {
    focus_of(state)?.reading.due()
}

/// The quiet timer at `now_us`: what comes due joins the back as a speech
/// entry, under `trace`.
pub(crate) fn tick(state: &mut SrState, now_us: u64, trace: TraceId) -> Option<Entry> {
    if due(state).is_none_or(|due| due > now_us) {
        return None;
    }
    with_focus(state, |_, focus| {
        let answer = focus.reading.timer(now_us);
        Entry::TerminalSpeech(Box::new(SpeechEntry {
            node: focus.terminal.node,
            echo: answer.echo,
            keys: answer
                .keys
                .into_iter()
                .map(|said| KeyAnswer {
                    unit: said.unit,
                    text: said.text,
                })
                .collect(),
            output: answer.output.into_iter().map(Waiting::Line).collect(),
            key: None,
            trace,
            fence: Fence::default(),
        }))
    })
}

/// Makes the committed focus's record watch terminal `node` on `window`,
/// live on an empty window with nothing out, for tests of what passes to
/// it.
#[cfg(test)]
pub(crate) fn live_for_tests(state: &mut SrState, node: NodeId, window: RequestWindow) {
    let record = state
        .versions
        .focus_mut()
        .expect("a committed focus for the terminal's record");
    record.context.terminal = Some(Box::new(TerminalFocus {
        terminal: Terminal {
            node,
            window,
            console: false,
        },
        watch: Watch::Live {
            old: Arc::new(Window::empty()),
            read: Read::Idle { dirty: false },
        },
        reading: Reading::new(),
        pacing: Pacing::Idle,
        key: None,
        reprint: None,
    }));
}

/// When the newest key passed to the focused terminal reached its program,
/// for tests.
#[cfg(test)]
pub(crate) fn passed_for_tests(state: &SrState) -> Option<u64> {
    focus_of(state)?.key.map(|(_, at_us)| at_us)
}

#[cfg(test)]
mod tests;
