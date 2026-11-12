//! A review or object-navigation press (`docs/design/m42-parity.md`,
//! "Build two"), written as one function per kind of press, run by Core's
//! driver (`crate::driver`).
//!
//! Every review command reads live (Dickson, 2026-10-08): a press asks the
//! application, waits, and goes on with the answer, sometimes with a
//! second read. From its first read the press is the key line's current
//! key (`crate::key_line`), so whatever is pressed behind it waits, and it
//! ends, the line moving on, when its function does. A press gives way,
//! saying nothing, when its bound passes with something behind it or a
//! focus moves the navigator while it reads; its outpost's end, or the end
//! of its terminal's model, ends it the same way. Each of those ends the
//! read the function waits on with `None`, so the function has one way
//! out, its own last line.

use verbatim_model::{
    Effect, NodeId, NodeSnapshot, QueryId, QueryKind, ReviewCommand, TextPoint, TraceId,
};

use crate::driver::{self, Core, Cx, Signal, Wait, Woke};
use crate::editing::{self, TextStep};
use crate::key_line::{self, ReviewPress};
use crate::queries::{Asking, Ending, Then};
use crate::reduce::{self, Navigated, Reason};
use crate::review_text::{self, After, Plan, Seeded, Start};
use crate::state::{NavigatorRead, PendingText};

/// Starts `press` on the navigator object: it runs at once to its first
/// read, and with no read to make it is over at once.
pub(crate) fn start(core: &mut Core, press: ReviewPress) -> Vec<Effect> {
    let Some(node) = core.navigator.as_ref().map(|navigator| navigator.object.id) else {
        return Vec::new();
    };
    driver::spawn(core, move |cx| async move {
        let reads = Reads {
            cx,
            key: press.trace_id,
        };
        // `None` is a press that gave way or lost its read, with nothing
        // more to say: either way the line moves on.
        let _ = run(&reads, press, node).await;
        reads
            .cx
            .run(|core| key_line::press_ended(core, press.trace_id));
    })
}

/// The press, by its command.
async fn run(reads: &Reads, press: ReviewPress, node: NodeId) -> Option<()> {
    match press.command {
        ReviewCommand::Parent => navigate(reads, node, QueryKind::Parent).await,
        ReviewCommand::NextSibling => navigate(reads, node, QueryKind::NextSibling).await,
        ReviewCommand::PreviousSibling => navigate(reads, node, QueryKind::PreviousSibling).await,
        ReviewCommand::FirstChild => navigate(reads, node, QueryKind::FirstChild).await,
        ReviewCommand::ReportObject => object(reads, press, node).await,
        ReviewCommand::PreviousCommandMark | ReviewCommand::NextCommandMark => {
            command_mark(reads, press, node).await
        }
        command => {
            let start = reads.cx.with(|core| review_text::start_of(core, command))?;
            review(reads, press, node, start).await
        }
    }
}

/// A review-cursor command over the text of `node`: the review cursor is
/// placed, by a read of its line unless Core holds the line and the
/// command only moves, and the command is run there, with a second read
/// when it moves to another line, selects, copies or asks where it is.
async fn review(
    reads: &Reads,
    mut press: ReviewPress,
    node: NodeId,
    mut start: Start,
) -> Option<()> {
    let mut trace = press.trace_id;
    loop {
        let position = match start {
            Start::At(position) => position,
            Start::Flat => return object(reads, press, node).await,
            Start::Line(at) => {
                let line = Asking::Text(node, review_text::line_at(at));
                let (ending, answer) = reads.read(node, line).await?;
                trace = answer;
                let reply = ending.reply();
                match reads
                    .cx
                    .with(|core| review_text::seeded(core, trace, press, node, reply))
                {
                    Seeded::At(position) => position,
                    Seeded::Flat => return object(reads, press, node).await,
                    Seeded::Lost => {
                        (press.repeat, start) = (0, Start::Line(TextPoint::Caret));
                        continue;
                    }
                    Seeded::Said(effects) => return reads.say(effects),
                }
            }
        };
        let plan = reads
            .cx
            .with(|core| review_text::execute(core, trace, press, node, &position));
        let (op, after) = match plan {
            Plan::Say(effects) => return reads.say(effects),
            Plan::Read(op, after) => (op, after),
        };
        let (ending, answer) = reads.read(node, Asking::Text(node, op)).await?;
        trace = answer;
        let reply = ending.reply();
        if let Some(effects) = reads
            .cx
            .with(|core| review_text::after_read(core, trace, press, node, after, reply))
        {
            return reads.say(effects);
        }
        // The position it moved from is gone: again, from the caret.
        (press.repeat, start) = (0, Start::Line(TextPoint::Caret));
    }
}

/// A flat review command or Report Current Object: the navigator object is
/// read live, and the command run on its text, or the object reported: on
/// the first press its full announcement, on the second its text spelled,
/// on the third its name and value copied (NVDA's multi-press semantics).
async fn object(reads: &Reads, press: ReviewPress, node: NodeId) -> Option<()> {
    let (ending, trace) = reads.read(node, Asking::ReadNode(node)).await?;
    let flat = reads
        .cx
        .with(|core| review_text::object_read(core, press.command, ending))?;
    if press.command != ReviewCommand::ReportObject {
        return reads.say(reads.cx.with(|core| {
            reduce::flat_review_command(core, trace, press.command, press.repeat, &flat)
        }));
    }
    let object = reads.cx.with(|core| {
        core.navigator
            .as_ref()
            .map(|navigator| navigator.object.clone())
    })?;
    match press.repeat {
        0 => {
            reads.cx.run(|core| {
                vec![reduce::announce_focus_object(
                    core,
                    trace,
                    &object,
                    Reason::Query,
                )]
            });
            navigator_text(reads, trace, &object, NavigatorRead::Announce).await
        }
        // An object with text spells or copies its name and the text it
        // would announce, the selection or the caret's line, as NVDA does
        // (`docs/nvda/speech.md`, "What an object with text says").
        1 if editing::may_have_text(object.role) => {
            navigator_text(reads, trace, &object, NavigatorRead::Spell).await
        }
        2 if editing::may_have_text(object.role) => {
            navigator_text(reads, trace, &object, NavigatorRead::Copy).await
        }
        repeat => reads.say(reduce::report_object_again(trace, &object, repeat)),
    }
}

/// An object-navigation press from `from`: the neighbour is fetched, the
/// navigator moves to it, and it is announced, its text after the rest.
async fn navigate(reads: &Reads, from: NodeId, kind: QueryKind) -> Option<()> {
    let (ending, trace) = reads.read(from, Asking::Navigate(from, kind)).await?;
    let result = ending.navigated();
    match reads
        .cx
        .with(|core| reduce::navigated(core, trace, kind, result))
    {
        Navigated::Said(effects) => reads.say(effects),
        Navigated::To(object) => {
            reads.cx.run(|core| {
                vec![reduce::announce_navigator(
                    core,
                    trace,
                    &object,
                    Reason::Focus,
                )]
            });
            navigator_text(reads, trace, &object, NavigatorRead::Announce).await
        }
    }
}

/// A move to the previous or next command mark of the terminal `node`,
/// which Core's model of it answers from its memory as the line move it
/// is; nothing when Core does not model the navigator.
async fn command_mark(reads: &Reads, press: ReviewPress, node: NodeId) -> Option<()> {
    let (from, landing) = reads
        .cx
        .with(|core| review_text::mark_move(core, press.command))?;
    let forward = press.command == ReviewCommand::NextCommandMark;
    let (query, effects) = reads.cx.with(|core| {
        crate::terminal_model::command_mark(core, node, press.trace_id, (forward, from))
    })?;
    reads.cx.emit(effects);
    let (ending, trace) = reads.answer(query).await?;
    let reply = ending.reply();
    let after = After::Land(landing);
    if let Some(effects) = reads
        .cx
        .with(|core| review_text::after_read(core, trace, press, node, after, reply))
    {
        return reads.say(effects);
    }
    // The mark it moved from is gone: the press goes on from the caret.
    let press = ReviewPress { repeat: 0, ..press };
    review(reads, press, node, Start::Line(TextPoint::Caret)).await
}

/// The text of `object` for `read`, as the navigator's announcement reads
/// it (`editing::navigator_text`), each of its reads one of the press's.
async fn navigator_text(
    reads: &Reads,
    mut trace: TraceId,
    object: &NodeSnapshot,
    read: NavigatorRead,
) -> Option<()> {
    let mut step = reads
        .cx
        .with(|core| editing::object_text(core, trace, object, read));
    loop {
        let (op, then) = match step {
            TextStep::Said(effects) => return reads.say(effects),
            TextStep::Read(op, then) => (op, then),
        };
        let (ending, answer) = reads.read(object.id, Asking::Text(object.id, op)).await?;
        trace = answer;
        let pending = PendingText {
            node: object.id,
            then,
            about: None,
        };
        let reply = ending.reply();
        step = reads
            .cx
            .with(|core| editing::text_reply(core, trace, &pending, reply));
    }
}

/// A press's reads.
struct Reads {
    cx: Cx,
    /// The press's key, which names it to the key line.
    key: TraceId,
}

impl Reads {
    /// Asks `asking` of `node` and waits for its answer, the press the
    /// current key from here: the answer with its trace, or `None` when
    /// the press gave way or its read was lost.
    async fn read(&self, node: NodeId, asking: Asking) -> Option<(Ending, TraceId)> {
        let (query, effects) = self.cx.with(|core| {
            let (query, effect) = core.queries.ask(asking, Then::Task);
            let mut effects = key_line::press_reads(core, node, self.key);
            effects.push(effect);
            (query, effects)
        });
        self.cx.emit(effects);
        self.answer(query).await
    }

    /// Waits for the answer to `query`, a read already asked.
    async fn answer(&self, query: QueryId) -> Option<(Ending, TraceId)> {
        let wait = Wait {
            query: Some(query),
            signals: &[Signal::PressGivesWay],
            due_us: None,
        };
        match self.cx.wait(wait).await {
            Woke::Answered {
                ending, trace_id, ..
            } => Some((ending, trace_id)),
            Woke::Signalled(..) => {
                self.cx
                    .run(|core| key_line::withdraw_press_read(core, self.key, query));
                None
            }
            Woke::Due | Woke::Lost => None,
        }
    }

    /// Says `effects`, the press's last words.
    #[expect(
        clippy::unnecessary_wraps,
        reason = "a press's way out, like its reads"
    )]
    fn say(&self, effects: Vec<Effect>) -> Option<()> {
        self.cx.emit(effects);
        Some(())
    }
}
