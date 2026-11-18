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
//!   today's 'had an effect' test"), ends the wait. A later key or a
//!   pending focus ends the wait silently.
//! - Dickson's ruling of 2026-11-17, which replaces his 2026-11-16
//!   approval of NVDA's event rule: "An event counts only if the caret is
//!   no longer where it was when the key was pressed; otherwise the watch
//!   keeps checking until the caret moves or time runs out." So in a UI
//!   Automation field (every one but a terminal) a caret or text-change
//!   event heard within 60 ms of the key's pass only brings a check
//!   forward, to run at once; that check, as every other, ends the wait
//!   only when it finds the key's effect against the read before the key.
//!   An event heard before the pass, while the read before the key is out,
//!   is nothing special, and one heard later than 60 ms does nothing. A key
//!   that moves nothing says its unit at 100 ms, as without events.
//!   Because Notepad raised a key's event about 7 ms late, inside the next
//!   key's watch, and NVDA's rule spoke the line before the next key moved.
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
//! - Dickson's decision of 2026-11-18 (the focus validity design, "The
//!   caret key"), after NVDA's caret wait, which pumps events at each try,
//!   its pump reading each MSAA focus's object and queuing it only when
//!   focused (`nvda/source/editableText.py` 93 to 99;
//!   `IAccessibleHandler/__init__.py` 762 to 782;
//!   `NVDAObjects/IAccessible/__init__.py` 874 to 892): an MSAA focus not
//!   yet read is not pending. It does not stop a key's read before it;
//!   at the key's pass, or told while the key watches, it is confirmed by
//!   one outpost read of its focused state, the watch stopped meanwhile.
//!   Focused, it is pending, and is read in full at its own turn; refused,
//!   the key watches on for the rest of its bound, checking at once, and
//!   the focus's turn says nothing. A UI Automation focus is pending when
//!   told. A later key withdraws the confirmation, leaving the focus
//!   unread.
//!
//! - NVDA's caret wait read from its source, as the coordinator gave it
//!   (`nvda/source/editableText.py`, `_hasCaretMoved`, 70 to 163): a
//!   change first seen at the bound counts as the effect (124 to 152).
//!
//! The model is a simulation of its own over the script's times. Two
//! things at one microsecond leave their order open, so the model refuses
//! the case ([`Refused`]) and the world never runs it.

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
/// How long after the key's pass a UI Automation event brings a check
/// forward.
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
    /// Key `0`, sent at `pass` with its field as `before`, waits on the
    /// confirmation of the MSAA focus told.
    Confirming { key: usize, pass: Us, before: App },
    /// The focus is being read.
    Focusing,
}

/// The focus told, until its turn ends it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Told {
    /// None waits.
    None,
    /// An MSAA focus not yet read: not pending.
    Unread,
    /// A focus that counts as pending: a UI Automation focus, or an MSAA
    /// focus confirmed focused.
    Pending,
    /// An MSAA focus whose confirmation found it not focused.
    Refused,
}

/// A read the outpost's worker runs, one at a time in the order asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Work {
    /// The read before key `0`.
    Read(usize),
    /// The focus's read, or its completion.
    Focus,
    /// The read of an MSAA focus's focused state, for a caret key.
    Confirm,
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
    /// The focus told and not yet taken.
    told: Told,
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
            told: Told::None,
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
            self.happen(event);
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

    fn happen(&mut self, event: Event) {
        match event {
            Event::Press(key) => {
                self.pressed(key);
            }
            Event::Release(key) => {
                self.release(key);
            }
            Event::Change(key) => {
                self.app.apply(self.press(key).key);
            }
            Event::Raised(_) => {
                self.raised();
            }
            Event::FocusMoved => {
                self.field_focused = false;
            }
            Event::FocusTold => {
                self.focus_told();
            }
            Event::ListenerDone => {
                self.ask(Work::Focus);
            }
            Event::WorkerDone => {
                self.worker_done();
            }
            Event::Check(pass) => {
                self.check(pass);
            }
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
            Front::Confirming { key: ahead, .. } => {
                // A later key withdraws the confirmation, the focus unread.
                self.marked[ahead] = true;
                self.withdraw(Work::Confirm);
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
        self.queue.push_back(Entry::Focus);
        if self.script.kind == Kind::TextBox {
            // An MSAA focus: unread, so a key's read before it reads on and
            // a watch is stopped for its confirmation. Every focus told
            // during a key's wait was raised before its bound.
            self.told = Told::Unread;
            if let Front::Watching {
                key, pass, before, ..
            } = self.front.clone()
            {
                self.confirm(key, pass, before);
            }
            return;
        }
        self.told = Told::Pending;
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
            Front::Idle | Front::Reading(_) | Front::Confirming { .. } | Front::Focusing => {}
        }
    }

    /// Key `key`, sent at `pass`, stops its watch while the outpost reads
    /// the MSAA focus's focused state.
    fn confirm(&mut self, key: usize, pass: Us, before: App) {
        self.end();
        self.front = Front::Confirming { key, pass, before };
        self.ask(Work::Confirm);
    }

    /// The confirmation answers: focused, the focus is pending and the key
    /// ends silently, Backspace saying what it deleted; refused, the key
    /// watches the rest of its bound, checking at once.
    fn confirmed(&mut self) {
        let Front::Confirming { key, pass, before } = self.front.clone() else {
            unreachable!("a confirmation no longer wanted is withdrawn");
        };
        // The application's focus is on the button once it moved there.
        if !self.field_focused {
            self.told = Told::Pending;
            if self.press(key).key == Key::Backspace {
                self.say(deleted(&before));
            }
            self.end();
            return;
        }
        self.told = Told::Refused;
        self.front = Front::Watching {
            key,
            pass,
            before,
            events: self.trusts_events(),
        };
        self.at(self.now, Event::Check(pass));
        let mut check = pass;
        while check <= pass + BOUND {
            if check > self.now {
                self.at(check, Event::Check(pass));
            }
            check += POLL;
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
        if self.told == Told::Pending {
            if self.press(key).key == Key::Backspace {
                self.say(deleted(&before));
            }
            return;
        }
        let pass = self.now;
        if self.told == Told::Unread {
            self.confirm(key, pass, before);
            return;
        }
        self.front = Front::Watching {
            key,
            pass,
            before,
            // A UI Automation field's events bring checks forward, and the
            // outpost hears the committed focus's alone.
            events: self.trusts_events(),
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
    fn check(&mut self, began: Us) {
        let Front::Watching {
            key, pass, before, ..
        } = self.front.clone()
        else {
            return;
        };
        if pass != began {
            return;
        }
        let changed = had_effect(self.press(key).key, &before, &self.app);
        if self.now < pass + BOUND {
            // A check before the bound, polled or brought forward by an
            // event, ends the wait only with the key's effect (Dickson,
            // 2026-11-17).
            if changed {
                self.effect(key, &before);
            }
            return;
        }
        if changed {
            // The check at the bound counts: NVDA's loop reads the caret and
            // compares it with the bookmark before it computes the elapsed
            // time and breaks (`nvda/source/editableText.py` 124 to 152), so
            // its last try, at or past the timeout, finds the effect.
            self.effect(key, &before);
            return;
        }
        if self.press(key).key != Key::Backspace {
            self.say(unit(self.press(key).key, &self.app));
        }
        self.end();
    }

    /// The application raised its events: within 60 ms of the key's pass,
    /// in a field whose events are heard, they bring a check forward to
    /// now, which ends the wait only with the key's effect (Dickson,
    /// 2026-11-17); heard at any other time they do nothing.
    fn raised(&mut self) {
        if let Front::Watching { pass, events, .. } = self.front
            && events
            && self.now <= pass + EVENTS
        {
            self.check(pass);
        }
    }

    /// Whether NVDA trusts the field's events for a caret key's effect
    /// (`caretMovementDetectionUsesEvents`), and the outpost hears them: a
    /// UI Automation field while it is the committed focus.
    fn trusts_events(&self) -> bool {
        self.script.kind == Kind::Notepad && self.field_committed
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
                Entry::Focus if self.told == Told::Refused => {
                    // A refused confirmation serves the focus's turn,
                    // which asks nothing and says nothing.
                    self.told = Told::None;
                }
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
        if self.marked[key] || (self.told == Told::Pending && self.press(key).key != Key::Backspace)
        {
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
            // The world gives every outpost read of the button one
            // latency.
            Work::Focus | Work::Confirm => self.script.focus.expect("a focus told").completion_us,
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
            Work::Confirm => self.confirmed(),
            Work::Focus => {
                self.field_committed = false;
                self.told = Told::None;
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
