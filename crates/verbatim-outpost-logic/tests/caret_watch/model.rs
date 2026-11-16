//! The reference model: what a script's keys say, and when, written from
//! NVDA's caret scripts through the parity documents and Dickson's
//! recorded decisions, never from the design's or the code's rules.
//!
//! - NVDA's caret wait (`docs/parity/text.md`, "Caret keys and what each
//!   speaks", "A caret key that moves nothing", "Holding caret keys";
//!   `docs/nvda/editable-text-and-terminals.md`): the caret is read before
//!   the key is sent on; after it, the caret is checked at once and then
//!   every 10 ms (reads here take no time) until 100 ms; the first check
//!   that finds the key's effect, by the test ADR 0003 keeps ("ends on
//!   today's 'had an effect' test"), ends the wait. In a UI
//!   Automation field, whose caret events NVDA trusts (every one but a
//!   terminal), a caret or text-change event within 60 ms of the key is
//!   its effect, and the caret is read then. A later key or a pending focus
//!   ends the wait silently.
//! - What a key says (`docs/parity/text.md`): with its effect, or at
//!   100 ms, a movement key and Delete its unit at the caret then, read
//!   once more; Backspace, with its effect, what it deleted, from the read
//!   before it, and at 100 ms nothing.
//! - Dickson's decisions (ADR 0003, "Keys and commands";
//!   `docs/design/readers-tell-core-asks-build1.md`, "What users hear"): a
//!   key and a focus are handled one at a time in arrival order; a held
//!   caret key is sent to the application 150 ms after its press if it has
//!   not been, and then says nothing; a key the hook did not hold says
//!   nothing; Backspace with a focus pending says what it deleted, then the
//!   focus; a key whose field lost the focus before its turn runs on that
//!   field, after the new focus; no settle: the next key reads at its turn;
//!   the outpost runs one read at a time, and a read no longer wanted is
//!   left to finish (ADR 0003, "A frozen window").
//!
//! The model is a simulation of its own over the script's times. Where
//! the sources leave the outcome open, the model refuses the case
//! ([`Refused`]) so the world never generates it; each such place names
//! the question it waits on.

use std::collections::VecDeque;

use super::world::{App, Input, Key, Kind, MS, Press, Script, Us};

/// What is heard, as the model names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Heard {
    /// One character, spoken on its own.
    Char(char),
    /// Text: a word or a line.
    Text(String),
    /// "blank".
    Blank,
    /// A control's focus: its name and its role, a button.
    Focus(String),
    /// Anything else, as the seam saw it: never expected.
    Other(String),
}

/// A case whose outcome the sources leave open, with the question.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refused(pub &'static str);

/// What the model expects: each utterance with the time it is said.
pub type Expected = Vec<(Us, Heard)>;

/// The model's outcome of a script.
pub struct Prediction {
    /// What is said, and when.
    pub expected: Expected,
    /// Whether the hook held each key: the committed focus had text at its
    /// press.
    pub held: Vec<bool>,
}

/// The bound: NVDA's `caretMoveTimeoutMs`.
const BOUND: Us = 100 * MS;
/// The check interval: NVDA's sleep between tries.
const POLL: Us = 10 * MS;
/// How long after the key a UI Automation event counts as its effect.
const EVENTS: Us = 60 * MS;
/// The hook's release of a held key.
const RELEASE: Us = 150 * MS;

/// A queue entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Entry {
    Key(usize),
    Focus,
}

/// What the front entry is doing.
#[derive(Clone, Debug)]
enum Front {
    /// Nothing: the next entry is taken.
    Idle,
    /// Key `0`'s read before it is out.
    Reading(usize),
    /// Key `0` was sent at `pass`, its field as the read before it found
    /// it, watched until its effect or `pass + BOUND`.
    Watching {
        key: usize,
        pass: Us,
        before: App,
        events: bool,
    },
    /// The focus is being read.
    Focusing,
}

/// A read the outpost's worker runs, one at a time in the order asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Work {
    /// The read before key `0`.
    Read(usize),
    /// The focus's read, or its completion.
    Focus,
}

/// Something that happens at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Event {
    /// The user presses key `0`.
    Press(usize),
    /// Key `0`'s hook release.
    Release(usize),
    /// The application changes the field for key `0`'s stroke.
    Change(usize),
    /// The application raises its events for key `0`'s stroke.
    Raised(usize),
    /// The application moves its focus.
    FocusMoved,
    /// The listener tells Core of the new focus.
    FocusTold,
    /// The listener's read of the focus is done.
    ListenerDone,
    /// The outpost's worker ends what it runs.
    WorkerDone,
    /// A check of the watch that began at the pass given, or its bound.
    Check(Us),
}

/// The model's run of one script.
pub struct Model<'s> {
    script: &'s Script,
    now: Us,
    events: Vec<(Us, Event)>,
    app: App,
    input: Input,
    /// Whether the application's focus is on the field.
    field_focused: bool,
    /// Whether Core's committed focus is the field.
    field_committed: bool,
    /// Whether a focus tell waits, counting as pending.
    pending: bool,
    queue: VecDeque<Entry>,
    front: Front,
    /// Per key: held by the hook, sent to the application, marked by a
    /// later key, released by the hook's timer.
    held: Vec<bool>,
    sent: Vec<bool>,
    marked: Vec<bool>,
    released: Vec<bool>,
    /// The outpost's worker: what it runs, whether that is still wanted,
    /// and what waits.
    running: Option<(Work, bool)>,
    waiting: VecDeque<Work>,
    expected: Expected,
}

impl<'s> Model<'s> {
    /// The model of `script`, from the field committed with the caret where
    /// the script puts it.
    pub fn new(script: &'s Script) -> Self {
        let keys = script.presses.len();
        let mut events: Vec<(Us, Event)> = script
            .presses
            .iter()
            .enumerate()
            .map(|(key, press)| (press.at, Event::Press(key)))
            .collect();
        if let Some(focus) = script.focus {
            events.push((focus.at, Event::FocusMoved));
            events.push((focus.told, Event::FocusTold));
        }
        Self {
            script,
            now: 0,
            events,
            app: App {
                text: script.text.clone(),
                caret: script.caret,
            },
            input: Input::default(),
            field_focused: true,
            field_committed: true,
            pending: false,
            queue: VecDeque::new(),
            front: Front::Idle,
            held: vec![false; keys],
            sent: vec![false; keys],
            marked: vec![false; keys],
            released: vec![false; keys],
            running: None,
            waiting: VecDeque::new(),
            expected: Vec::new(),
        }
    }

    /// What the script's keys say, or the question that leaves it open.
    pub fn run(mut self) -> Result<Prediction, Refused> {
        while let Some((at, event)) = self.next()? {
            self.now = at;
            self.happen(event)?;
            self.take_next();
        }
        Ok(Prediction {
            expected: self.expected,
            held: self.held,
        })
    }

    /// The earliest event, removed; two at one time leave their order open.
    fn next(&mut self) -> Result<Option<(Us, Event)>, Refused> {
        self.events.sort_unstable();
        if self.events.len() > 1 && self.events[0].0 == self.events[1].0 {
            return Err(Refused("two things at one microsecond"));
        }
        if self.events.is_empty() {
            return Ok(None);
        }
        Ok(Some(self.events.remove(0)))
    }

    fn at(&mut self, at: Us, event: Event) {
        self.events.push((at, event));
    }

    fn press(&self, key: usize) -> &Press {
        &self.script.presses[key]
    }

    fn say(&mut self, heard: Heard) {
        self.expected.push((self.now, heard));
    }

    /// Key `key`'s stroke is sent to the application now.
    fn send(&mut self, key: usize) {
        if self.sent[key] {
            return;
        }
        self.sent[key] = true;
        if !self.field_focused {
            // The stroke reaches the button, which no key changes.
            return;
        }
        self.events
            .retain(|&(_, event)| event != Event::Release(key));
        if let Some((change, event)) =
            self.input
                .stroke(self.now, self.press(key).handling, self.script.kind)
        {
            self.at(change, Event::Change(key));
            if let Some(event) = event {
                self.at(event, Event::Raised(key));
            }
        }
    }

    /// The front entry ends now.
    fn end(&mut self) {
        if let Front::Watching { pass, .. } = self.front {
            self.events
                .retain(|&(_, event)| event != Event::Check(pass));
        }
        self.front = Front::Idle;
    }

    fn happen(&mut self, event: Event) -> Result<(), Refused> {
        match event {
            Event::Press(key) => {
                self.pressed(key);
                Ok(())
            }
            Event::Release(key) => {
                self.release(key);
                Ok(())
            }
            Event::Change(key) => {
                self.app.apply(self.press(key).key);
                Ok(())
            }
            Event::Raised(_) => self.raised(),
            Event::FocusMoved => {
                self.field_focused = false;
                Ok(())
            }
            Event::FocusTold => {
                self.focus_told();
                Ok(())
            }
            Event::ListenerDone => {
                self.ask(Work::Focus);
                Ok(())
            }
            Event::WorkerDone => {
                self.worker_done();
                Ok(())
            }
            Event::Check(pass) => self.check(pass),
        }
    }

    /// The hook holds a caret key while the committed focus has text, and
    /// passes it at once otherwise; any key marks every key ahead of it.
    fn pressed(&mut self, key: usize) {
        for entry in &self.queue {
            if let Entry::Key(ahead) = *entry {
                self.marked[ahead] = true;
            }
        }
        match self.front {
            Front::Reading(ahead) => self.marked[ahead] = true,
            Front::Watching { key: ahead, .. } => {
                // A later key ends the wait silently.
                self.marked[ahead] = true;
                self.end();
            }
            Front::Idle | Front::Focusing => {}
        }
        self.held[key] = self.field_committed;
        if self.held[key] {
            let release = self.press(key).at + RELEASE;
            self.at(release, Event::Release(key));
        } else {
            self.send(key);
        }
        self.queue.push_back(Entry::Key(key));
    }

    /// The hook's release: the stroke goes, and the key says nothing; its
    /// read, if out, is no longer wanted but runs on in the worker.
    fn release(&mut self, key: usize) {
        if self.sent[key] {
            return;
        }
        self.released[key] = true;
        if let Front::Reading(reading) = self.front
            && reading == key
        {
            self.withdraw(Work::Read(key));
            self.end();
        }
        self.send(key);
    }

    /// The listener's focus tell: a focus pending ends a key's wait
    /// silently, Backspace saying what it deleted first; a key reading
    /// before it is sent stops reading and is sent, but Backspace reads on.
    fn focus_told(&mut self) {
        self.pending = true;
        self.queue.push_back(Entry::Focus);
        match self.front.clone() {
            Front::Reading(key) if self.press(key).key != Key::Backspace => {
                self.withdraw(Work::Read(key));
                self.end();
                self.send(key);
            }
            Front::Watching { key, before, .. } => {
                if self.press(key).key == Key::Backspace {
                    self.say(deleted(&before));
                }
                self.end();
            }
            Front::Idle | Front::Reading(_) | Front::Focusing => {}
        }
    }

    /// The read before key `key` answers: the key is sent, then watched
    /// unless a later key marked it or a focus is pending.
    fn read_done(&mut self, key: usize) {
        let before = self.app.clone();
        self.end();
        self.send(key);
        if self.marked[key] {
            return;
        }
        if self.pending {
            if self.press(key).key == Key::Backspace {
                self.say(deleted(&before));
            }
            return;
        }
        let pass = self.now;
        self.front = Front::Watching {
            key,
            pass,
            before,
            // NVDA trusts a UI Automation field's events, and the outpost
            // hears the committed focus's alone.
            events: self.script.kind == Kind::Notepad && self.field_committed,
        };
        let mut check = pass;
        while check <= pass + BOUND {
            self.at(check, Event::Check(pass));
            check += POLL;
        }
    }

    /// A check of the watch that began at `began`: the key's effect, by
    /// the test ADR 0003 keeps ("Keys and commands": today's "had an
    /// effect"; `docs/parity/text.md`, "What counts as a caret key's
    /// evidence"); at the bound, the key's words with none.
    fn check(&mut self, began: Us) -> Result<(), Refused> {
        let Front::Watching {
            key, pass, before, ..
        } = self.front.clone()
        else {
            return Ok(());
        };
        if pass != began {
            return Ok(());
        }
        let changed = had_effect(self.press(key).key, &before, &self.app);
        if self.now < pass + BOUND {
            if changed {
                self.effect(key, &before);
            }
            return Ok(());
        }
        if changed {
            // A change after the last check before the bound: NVDA's try at
            // about 100 ms finds it and counts it as the effect.
            return Err(Refused(
                "does a change first seen at the bound count as the key's effect?",
            ));
        }
        if self.press(key).key != Key::Backspace {
            self.say(unit(self.press(key).key, &self.app));
        }
        self.end();
        Ok(())
    }

    /// The application raised its events: within 60 ms of the key they
    /// are its effect. Heard while a key's read before it is out, they are
    /// open: NVDA's wait would find them pending at its first try, and
    /// whether the watch counts what came before it is not recorded.
    fn raised(&mut self) -> Result<(), Refused> {
        match self.front.clone() {
            Front::Watching {
                key,
                pass,
                before,
                events,
            } => {
                if events && self.now <= pass + EVENTS {
                    self.effect(key, &before);
                }
                Ok(())
            }
            Front::Reading(_) if self.field_committed => Err(Refused(
                "does a caret event heard while a key's read before it is out count as that key's effect?",
            )),
            Front::Idle | Front::Reading(_) | Front::Focusing => Ok(()),
        }
    }

    /// Key `key`'s effect, now: its words from the caret read now.
    fn effect(&mut self, key: usize, before: &App) {
        let words = match self.press(key).key {
            Key::Backspace => deleted(before),
            other => unit(other, &self.app),
        };
        self.say(words);
        self.end();
    }

    /// While nothing runs, the next entry is taken at its turn.
    fn take_next(&mut self) {
        while matches!(self.front, Front::Idle) {
            let Some(entry) = self.queue.pop_front() else {
                return;
            };
            match entry {
                Entry::Focus => {
                    // A UI Automation focus is read in two requests, the
                    // listener's then the outpost's; an MSAA focus in one,
                    // the outpost's (ADR 0003, "A UI Automation focus
                    // read").
                    self.front = Front::Focusing;
                    let focus = self.script.focus.expect("a focus told");
                    match self.script.kind {
                        Kind::Notepad => self.at(self.now + focus.listener_us, Event::ListenerDone),
                        Kind::TextBox => self.ask(Work::Focus),
                    }
                }
                Entry::Key(key) => self.turn(key),
            }
        }
    }

    /// Key `key` at its turn.
    fn turn(&mut self, key: usize) {
        if !self.held[key] || self.released[key] {
            return;
        }
        if self.marked[key] || (self.pending && self.press(key).key != Key::Backspace) {
            self.send(key);
            return;
        }
        self.front = Front::Reading(key);
        self.ask(Work::Read(key));
    }

    /// The outpost is asked for `work`: run at once while nothing runs,
    /// else after what waits.
    fn ask(&mut self, work: Work) {
        self.waiting.push_back(work);
        self.dispatch();
    }

    fn dispatch(&mut self) {
        if self.running.is_some() {
            return;
        }
        let Some(work) = self.waiting.pop_front() else {
            return;
        };
        let takes = match work {
            Work::Read(key) => self.press(key).read_us,
            Work::Focus => self.script.focus.expect("a focus told").completion_us,
        };
        self.running = Some((work, true));
        self.at(self.now + takes, Event::WorkerDone);
    }

    /// `work` is no longer wanted: dropped while it waits, left to finish
    /// while it runs (ADR 0003, "A frozen window").
    fn withdraw(&mut self, work: Work) {
        self.waiting.retain(|&waiting| waiting != work);
        if let Some((running, wanted)) = self.running.as_mut()
            && *running == work
        {
            *wanted = false;
        }
    }

    /// The worker ends what it runs, and takes the next.
    fn worker_done(&mut self) {
        let (work, wanted) = self.running.take().expect("the worker runs");
        self.dispatch();
        if !wanted {
            return;
        }
        match work {
            Work::Read(key) => self.read_done(key),
            Work::Focus => {
                self.field_committed = false;
                self.pending = false;
                self.say(Heard::Focus("OK".to_owned()));
                self.end();
            }
        }
    }
}

/// Whether the field now shows key `key`'s effect beside the field as the
/// read before it found it: the caret left where it was, the characters
/// beside it changed, or, after a key that deletes, its line changed.
fn had_effect(key: Key, before: &App, now: &App) -> bool {
    now.caret != before.caret
        || now.before() != before.before()
        || now.character() != before.character()
        || (matches!(key, Key::Backspace | Key::Delete) && now.text != before.text)
}

/// A key's unit at the caret, as NVDA speaks a unit
/// (`nvda/source/speech/speech.py`, `getTextInfoSpeech` and `isBlank`): a
/// character or a word of one character spelled, a space among them;
/// text of spaces alone, or none, "blank"; spaces around text unheard.
fn unit(key: Key, app: &App) -> Heard {
    match key {
        Key::Left | Key::Right | Key::Home | Key::End | Key::Delete => {
            app.character().map_or(Heard::Blank, Heard::Char)
        }
        Key::Up | Key::Down => match app.text.trim() {
            "" => Heard::Blank,
            line => Heard::Text(line.to_owned()),
        },
        Key::WordRight => {
            let word = app.word();
            let first = if word.trim().is_empty() {
                word.as_str()
            } else {
                word.trim()
            };
            let mut chars = first.chars();
            match (chars.next(), chars.next()) {
                (None, _) => Heard::Blank,
                (Some(only), None) => Heard::Char(only),
                _ if first.trim().is_empty() => Heard::Blank,
                _ => Heard::Text(first.to_owned()),
            }
        }
        Key::Backspace => unreachable!("Backspace says what it deleted"),
    }
}

/// What Backspace deletes from the field as the read before it found it:
/// the character before the caret, "blank" at the text's start.
fn deleted(before: &App) -> Heard {
    before.text[..before.caret]
        .chars()
        .next_back()
        .map_or(Heard::Blank, Heard::Char)
}
