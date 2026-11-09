//! The reference model of the caret generated test (`super`), written from
//! `docs/parity/text.md`, `docs/nvda/editable-text-and-terminals.md` and
//! Dickson's dated decisions: a record per key press and the speech those
//! rules require, in terms of keys and speech. It knows no state of Core's
//! or the outpost's; every read it hears of is the world's.
//!
//! The rules, each from those documents:
//!
//! - A read counts as the open key's evidence when it ended at or after
//!   the key was passed on and shows a change against the node's one last
//!   read, which every read replaces (2026-10-25, 2026-11-09).
//! - A key with evidence speaks its unit from that read; Backspace speaks
//!   what the read shows removed against the read before the key.
//! - A key with none by its bound speaks as one that moved nothing, once
//!   Core's final read reports: a movement key and Delete their unit,
//!   Backspace and a Shift key nothing (2026-10-29). An effect later than
//!   that is not spoken (2026-11-08).
//! - A key pressed later silences an earlier key's words, however early
//!   it was pressed, even before the earlier key was passed on, and the
//!   selection changes held for it wait for the later key's words
//!   (`docs/parity/text.md`, "Holding caret keys" and "Selection over
//!   changed text in a classic edit control").
//! - Every selection change is heard once, after the words of the key that
//!   made it and before a focus that moves away (2026-11-09).
//! - A repeated focus's selection changes are spoken; a new focus's are
//!   not (2026-10-25, amended 2026-10-29).
//! - A read that fails or is abandoned ends its key silently: an abandoned
//!   before read passes the key on and the key says nothing
//!   (`nvda/source/editableText.py` 181 to 186), an abandoned final read
//!   says nothing for the key (168 to 172), and no later evidence is that
//!   key's (the coordinator's ruling, 2026-11-09).
//! - A final read that shows no change answers only the key that asked
//!   it, never one that replaced its asker, held or not (the coordinator's
//!   ruling, 2026-11-09). One that shows a change is an ordinary report,
//!   so that key's evidence when it ended after that key's pass
//!   (2026-11-09).
//! - The caret is the focus object's, the one whose read began most
//!   recently (2026-10-25), and ends with that focus.
//! - A report of a node no longer the focus says nothing and is not a
//!   key's evidence, since a caret event is taken only for the focus
//!   (`docs/parity/text.md`).
//! - A later key silences only reads begun at or after its press; a read
//!   begun before it still speaks for the key it silenced while that key
//!   is still current (`docs/parity/text.md`, "Caret keys and what each
//!   speaks"). Once the later key has replaced it, its before read asked
//!   or its pass made, the key speaks no words whatever its read's timing,
//!   and such a read is an ordinary report whose selection changes are
//!   heard (the coordinator's ruling, 2026-11-09).
//! - A focus moving first within the 100 ms speaks the key's unit from the
//!   read before it, Backspace what that read shows it removes, with no
//!   before read nothing (2026-11-08, 2026-11-09); a key whose before read
//!   is still running says nothing (2026-11-09). After the 100 ms and
//!   before the key has spoken, the key says nothing, its held selection
//!   changes still coming first (the coordinator's ruling, 2026-11-09).
//! - A key waiting behind another when the focus moves still reaches the
//!   application in press order, and says nothing for itself if its node
//!   is no longer the focus when it is judged, at its pass (Dickson's
//!   answer, 2026-11-09).
//! - A selection change arriving while a key is between its release and
//!   its words is held and said after them, whether or not another key
//!   waits behind it; before a key has been released it is said at once
//!   (the coordinator's ruling, 2026-11-09).
//! - Control+Backspace over text speaks as Backspace does, with the word
//!   before the caret or the selection's start in place of its character
//!   (`docs/parity/text.md`, "Selection over changed text in a classic edit
//!   control").
//! - A key pressed behind a focus report that waits for its live re-read
//!   runs after that report is the focus, and is judged on it, as a script
//!   runs after the focus event ahead of it (ADR 0001, "One ordered focus
//!   queue"); its bound starts when the report stops waiting. A caret event
//!   of another node while a report waits is a report of no focus.
//! - A key pressed on a focus with no text has nothing to read and says
//!   nothing (`docs/parity/text.md`, "Holding caret keys").
//! - A focus in another top-level window cuts speech just before it
//!   (`docs/parity/focus-events.md`), after a key's words.
//! - A key that ends with the key after it queued behind a waiting focus
//!   report says its held selection changes at that ending, and a before
//!   read abandoned with no key left says what was held (the coordinator's
//!   ruling, 2026-11-09).
//! - What Backspace removed is the difference between its before line and
//!   its after line, carets included (`docs/design/caret-selection-rework.md`,
//!   `removal`), so an older baseline gives what that difference gives.
//! - A read of a key's node counts for that key when it was dispatched or
//!   committed while the key was within its window, from Core's asking
//!   its before read or its pass, however late the read ends; outside the
//!   window it is an ordinary report (the coordinator's ruling,
//!   2026-11-09). The key's final read ends its window as it commits
//!   (2026-11-09).
//! - At exit nothing further is heard: held keys are delivered, held
//!   selection changes are not said (the coordinator's ruling, 2026-11-09).
//! - Backspace speaks only when its caret moved
//!   (`nvda/source/editableText.py` 313 to 320): with the caret not moved
//!   it says nothing for the key and the change is heard; with the caret
//!   moved and nothing removed it says "blank" (the coordinator's ruling, 2026-11-09).
//! - Backspace over a selection says what comes before the selection's
//!   start, Delete its unit, and the change follows, in general terms
//!   when the text changed: "selection removed" when nothing is selected
//!   now (`docs/parity/text.md`, "Selection over changed text in a
//!   classic edit control").

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use super::world::{Key, Text, Us, selection_changes};

/// The words for the selection's change from `last` to `text`, in general
/// terms when the text changed between them.
fn change_words(last: &Text, text: &Text) -> Vec<String> {
    let changes = selection_changes(last, text);
    if last.letters == text.letters {
        return changes
            .into_iter()
            .map(|(selected, letters)| {
                let word = if selected { "selected" } else { "unselected" };
                format!("{letters} {word}")
            })
            .collect();
    }
    let whole: Option<String> = text
        .selection
        .map(|(start, end)| text.letters[start..end].iter().collect());
    let mut words = Vec::new();
    if changes.iter().any(|(selected, _)| *selected)
        && let Some(whole) = &whole
    {
        words.push(format!("{whole} selected"));
    }
    if changes.iter().any(|(selected, _)| !*selected) {
        words.push(whole.map_or_else(
            || "selection removed".to_owned(),
            |whole| format!("{whole} selected instead"),
        ));
    }
    words
}

/// The number of the button, among the fields a focus can be on.
const BUTTON: usize = 2;
/// The 100 ms bound, from when the key is passed on (2026-11-09).
pub const BOUND_US: Us = 100_000;
/// How long a key's reads count for it, from its before read's asking
/// or its pass (the coordinator's ruling, 2026-11-09).
const KEY_WINDOW_US: Us = 300_000;
/// How long a held key's before read may run (2026-11-08).
pub const BEFORE_READ_US: Us = 150_000;

/// How far a key's before read has gone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BeforeRead {
    /// Core has not asked it.
    NotAsked,
    /// Core has asked it, and it has not committed.
    Asked,
    /// It committed.
    Committed,
}

/// A key pressed, as the model follows it.
#[derive(Clone, Debug)]
struct Press {
    key: Key,
    held: bool,
    /// The field with the focus at its press; `None` when there was none
    /// or its outpost has ended.
    node: Option<usize>,
    /// Its before read.
    before: BeforeRead,
    passed: Option<Us>,
    /// The node's last read when the key was passed on: what it is
    /// compared with.
    baseline: Option<Text>,
    /// Its window: from when Core asked its before read, or its pass with
    /// none, to the end of that period.
    window: Option<(Us, Us)>,
    /// Heard, silenced or ended: its words are said or owed no more.
    done: bool,
    /// It waits behind a focus report and has not been run.
    queued: bool,
    /// No bound runs before this: the press, or the end of the wait it
    /// was queued behind.
    floor: Us,
    /// A later key, pressed at this time, silenced it: its selection
    /// changes wait for that key's words, and only a read begun before
    /// this time can still speak for it.
    silenced_at: Option<Us>,
}

/// A final read asked: the key open when it was asked, if any, and the
/// key it was asked for, the newest passed on.
#[derive(Clone, Copy, Debug)]
struct FinalAsk {
    open: Option<usize>,
    key: Option<usize>,
}

/// What a report told Core, for [`Model::report`].
#[derive(Clone, Debug)]
pub struct Report {
    /// What it saw.
    pub text: Text,
    /// When its read began.
    pub began: Us,
    /// When its read ended.
    pub ended: Us,
    /// Whether it is Core's final read of a key.
    pub final_read: bool,
    /// Whether its read was dispatched while a key of its node was within
    /// its window.
    pub dispatched_in_window: bool,
}

/// What a read that ended without a result was for a key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadRole {
    /// A held key's before read.
    Before,
    /// Core's final read of a key.
    Final,
    /// Any other read.
    Other,
}

/// Something heard, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Heard {
    /// Words.
    Words(String),
    /// A focus's own speech.
    Focus,
    /// Speech was cut.
    Cut,
}

/// The reference model.
#[derive(Clone, Debug, Default)]
#[expect(clippy::struct_excessive_bools, reason = "each is a fact of the case")]
pub struct Model {
    keys: Vec<Press>,
    /// Each node's one last read, replaced by every read.
    last_read: BTreeMap<usize, Text>,
    /// The field of the newest read committed.
    committed_field: Option<usize>,
    /// Whether the newest read differed from the one before it.
    shows_change: bool,
    /// The selection changes the newest read shows.
    changes: Vec<String>,
    /// The selection changes waiting for the words of the key that made
    /// them.
    held: Vec<String>,
    expected: Vec<Heard>,
    /// The newest caret: when its read began, and the caret.
    newest: Option<(Us, usize)>,
    /// The field with the focus, or [`BUTTON`].
    focus: Option<usize>,
    /// The field a focus report waiting for its live re-read is for.
    waiting: Option<usize>,
    /// The focus is in another top-level window.
    other_window: bool,
    /// The shapes this case reached.
    pub reached: BTreeSet<&'static str>,
    /// Its outpost has not ended since it was reported. A new generation's
    /// report of the same field is that focus again.
    alive: bool,
    /// The keys whose before reads are asked and not ended, oldest first.
    before_reads: VecDeque<usize>,
    /// The final reads asked and not ended, oldest first.
    final_reads: VecDeque<FinalAsk>,
    /// Whether a key of the node was within its window when the read in
    /// flight was dispatched.
    read_dispatched_in_window: bool,
    exited: bool,
}

impl Model {
    /// The caret Core must hold.
    #[must_use]
    pub fn caret(&self) -> Option<usize> {
        self.newest.map(|(_, caret)| caret)
    }

    /// A read of `field` was dispatched at `at`: whether a key of its node
    /// is within its window then is a fact of that read.
    pub fn read_dispatched(&mut self, field: usize, at: Us) {
        self.read_dispatched_in_window = self.in_window_at(field, at);
    }

    /// Whether the read in flight was dispatched in a key's window.
    #[must_use]
    pub const fn read_in_flight_in_window(&self) -> bool {
        self.read_dispatched_in_window
    }

    /// Whether a key of `field` is within its window at `at`.
    fn in_window_at(&self, field: usize, at: Us) -> bool {
        self.keys.iter().any(|press| {
            press.node == Some(field)
                && press
                    .window
                    .is_some_and(|(from, until)| from <= at && at < until)
        })
    }

    /// Whether a focus report waits for its live re-read.
    #[must_use]
    pub const fn report_waits(&self) -> bool {
        self.waiting.is_some()
    }

    /// Whether the focus's outpost has not ended.
    #[must_use]
    pub const fn outpost_alive(&self) -> bool {
        self.alive
    }

    /// Whether the focus is field `field`.
    #[must_use]
    pub fn focus_is(&self, field: usize) -> bool {
        self.focus == Some(field)
    }

    /// The field a waiting focus report is for.
    #[must_use]
    pub const fn waiting_field(&self) -> Option<usize> {
        self.waiting
    }

    /// Whether the focus is in another top-level window.
    #[must_use]
    pub const fn in_other_window(&self) -> bool {
        self.other_window
    }

    /// The key still open: passed on and not done.
    fn open(&self) -> Option<usize> {
        self.keys
            .iter()
            .position(|press| press.passed.is_some() && !press.done)
    }

    /// Whether a key has not yet reached the application: a key pressed
    /// now waits behind it, as the hook defers it
    /// (`docs/parity/text.md`, "Holding caret keys").
    #[must_use]
    pub fn key_waits(&self) -> bool {
        self.keys.iter().any(|press| press.passed.is_none())
    }

    /// A key is pressed at `now`; any earlier key's words are silenced.
    pub fn press(&mut self, key: Key, held: bool, now: Us) {
        // Passed on or not: NVDA's helper returns while a script waits
        // (`nvda/source/editableText.py` 166).
        for press in self.keys.iter_mut().filter(|press| !press.done) {
            press.done = true;
            press.silenced_at = Some(now);
        }
        let queued = self.waiting.is_some();
        if queued {
            self.reached.insert("a key behind a focus report");
        }
        let node = self
            .waiting
            .or(self.focus)
            .filter(|field| self.alive && *field != BUTTON);
        if node.is_none() {
            self.reached.insert("a key on a focus with no text");
        }
        self.keys.push(Press {
            key,
            held,
            node,
            before: BeforeRead::NotAsked,
            passed: None,
            baseline: None,
            window: None,
            done: false,
            queued,
            floor: now,
            silenced_at: None,
        });
    }

    /// Key `id` reached the application at `now`.
    ///
    /// # Errors
    ///
    /// When it was not the oldest key not yet passed on.
    pub fn passed(&mut self, id: usize, now: Us) -> Result<(), String> {
        let oldest = self.keys.iter().position(|press| press.passed.is_none());
        if oldest != Some(id) {
            return Err(format!(
                "key {id} passed on, but the oldest not yet passed is {oldest:?}"
            ));
        }
        let baseline = self
            .focus
            .and_then(|field| self.last_read.get(&field).cloned());
        let focus = self.focus;
        // A before read given up leaves the reads a commit is credited to.
        self.before_reads.retain(|&key| key != id);
        let press = &mut self.keys[id];
        press.passed = Some(now);
        press.baseline = baseline;
        press.window.get_or_insert((now, now + KEY_WINDOW_US));
        if press.node.is_none() || press.node != focus {
            press.done = true;
            self.say_held_when_no_key_is_out();
        }
        Ok(())
    }

    /// Core asked a before read, of the oldest key not yet passed on.
    pub fn before_read_asked(&mut self, now: Us) {
        if let Some(id) = self.keys.iter().position(|press| press.passed.is_none()) {
            self.keys[id].before = BeforeRead::Asked;
            self.keys[id].window = Some((now, now + KEY_WINDOW_US));
            self.before_reads.push_back(id);
        }
    }

    /// Core asked a final read, for the key open now.
    pub fn final_read_asked(&mut self) {
        self.reached.insert("a key's bound passed");
        let open = self.open();
        let key = self.keys.iter().rposition(|press| press.passed.is_some());
        self.final_reads.push_back(FinalAsk { open, key });
    }

    /// The read in flight ended without a result, for `role`: the key it
    /// served ends silently.
    pub fn abandoned(&mut self, role: ReadRole) {
        match role {
            ReadRole::Before => {
                if let Some(id) = self.before_reads.pop_front()
                    && self.keys[id].passed.is_none()
                {
                    self.keys[id].done = true;
                }
                if self.keys.iter().all(|press| press.done) {
                    self.say_held_when_no_key_is_out();
                }
            }
            ReadRole::Final => {
                let asker = self.final_reads.pop_front().and_then(|ask| ask.open);
                if let Some(asker) = asker
                    && self.open() == Some(asker)
                {
                    self.keys[asker].done = true;
                    self.say_held_when_no_key_is_out();
                } else if let Some(id) = self.silenced_before_a_queued_key() {
                    self.keys[id].silenced_at = None;
                    self.say_held_when_no_key_is_out();
                }
            }
            ReadRole::Other => {}
        }
    }

    /// Whether a key is between its release and its words: released with
    /// its words owed, or silenced and not yet replaced.
    fn key_is_out(&self) -> bool {
        self.keys.iter().enumerate().any(|(id, press)| {
            press.passed.is_some()
                && (!press.done || (press.silenced_at.is_some() && !self.replaced(id)))
        })
    }

    /// Whether the key after key `id` has become current: its before read
    /// has been asked, or it has been passed on.
    fn replaced(&self, id: usize) -> bool {
        self.keys
            .get(id + 1)
            .is_none_or(|later| later.before != BeforeRead::NotAsked || later.passed.is_some())
    }

    /// With no key out, the held selection changes are said.
    fn say_held_when_no_key_is_out(&mut self) {
        if !self.key_is_out() {
            let held = std::mem::take(&mut self.held);
            self.say_changes(&held);
        }
    }

    /// A read of `field` committed, having seen `text`: the node's last
    /// read is replaced, and whether it shows a change is noted.
    /// `before_read` when it is a held key's before read.
    pub fn committed(&mut self, field: usize, text: &Text, before_read: bool) {
        let (shows, changes) = self.last_read.get(&field).map_or_else(
            || (false, Vec::new()),
            |last| {
                let shows = last.caret != text.caret
                    || last.selection != text.selection
                    || neighbours(last) != neighbours(text);
                (shows, change_words(last, text))
            },
        );
        self.committed_field = Some(field);
        self.shows_change = shows;
        self.changes = changes;
        if before_read
            && let Some(id) = self.before_reads.pop_front()
            && self.keys[id].passed.is_none()
        {
            self.keys[id].before = BeforeRead::Committed;
        }
        self.last_read.insert(field, text.clone());
    }

    /// A report of a read reached Core at `now`.
    pub fn report(&mut self, report: &Report, now: Us) {
        let ask = report
            .final_read
            .then(|| self.final_reads.pop_front())
            .flatten();
        let asker = ask.and_then(|ask| ask.open);
        // The final read ends the window of the key it was asked for, and
        // no later key's.
        if let Some(key) = ask.and_then(|ask| ask.key) {
            if self.keys[key].silenced_at.is_some() {
                self.reached
                    .insert("an answer to a key superseded by a later one");
            }
            self.keys[key].window = None;
        }
        let changes = std::mem::take(&mut self.changes);
        if self.focus != self.committed_field {
            self.reached.insert("a report behind the focus");
            return;
        }
        self.note_caret(report.began, report.text.caret);
        let Some(open) = self.open() else {
            if let Some(id) = self.silenced_evidence(report) {
                self.reached.insert("a silenced key's words");
                let press = &self.keys[id];
                let words = evidence_words(press.key, press.baseline.as_ref(), &report.text);
                self.keys[id].silenced_at = None;
                if let Some(words) = words {
                    self.expected.push(Heard::Words(words));
                }
                let held = std::mem::take(&mut self.held);
                self.say_changes(&held);
                self.say_changes(&changes);
                return;
            }
            if let Some(id) = self.silenced_before_a_queued_key()
                && self.ends_silenced_key(id, report)
            {
                self.keys[id].silenced_at = None;
                let held = std::mem::take(&mut self.held);
                self.say_changes(&held);
                self.say_changes(&changes);
                return;
            }
            if self.key_is_out() {
                self.held.extend(changes);
                return;
            }
            let held = std::mem::take(&mut self.held);
            self.say_changes(&held);
            self.say_changes(&changes);
            return;
        };
        let press = &self.keys[open];
        let passed = press.passed.unwrap_or(now);
        let evidence = report.ended >= passed
            && self.shows_change
            && (report.final_read || counts_for(press, report));
        if !evidence && asker != Some(open) {
            self.held.extend(changes);
            return;
        }
        let words = if evidence {
            evidence_words(press.key, press.baseline.as_ref(), &report.text)
        } else {
            unit(press.key, &report.text)
        };
        self.keys[open].done = true;
        if let Some(words) = words {
            self.expected.push(Heard::Words(words));
        }
        let held = std::mem::take(&mut self.held);
        self.say_changes(&held);
        self.say_changes(&changes);
    }

    /// The newest silenced key `report` is still evidence for: not yet
    /// replaced, its read began before the later key was pressed and ended
    /// after this key's pass, and it shows a change.
    fn silenced_evidence(&self, report: &Report) -> Option<usize> {
        if !self.shows_change {
            return None;
        }
        (0..self.keys.len()).rev().find(|&id| {
            let press = &self.keys[id];
            !self.replaced(id)
                && press.node.is_some()
                && press.node == self.focus
                && press.silenced_at.is_some_and(|at| report.began < at)
                && press.passed.is_some_and(|at| report.ended >= at)
                && counts_for(press, report)
        })
    }

    /// The silenced key, passed on and still current, whose later key is
    /// queued behind a waiting focus report, so nothing in the line follows
    /// it.
    fn silenced_before_a_queued_key(&self) -> Option<usize> {
        (0..self.keys.len()).rev().find(|&id| {
            let press = &self.keys[id];
            press.passed.is_some()
                && press.silenced_at.is_some()
                && self.keys.get(id + 1).is_some_and(|later| later.queued)
        })
    }

    /// Whether `report` ends silenced key `id`: its final read, or its
    /// evidence.
    fn ends_silenced_key(&self, id: usize, report: &Report) -> bool {
        let press = &self.keys[id];
        report.final_read
            || (self.shows_change
                && press.passed.is_some_and(|at| report.ended >= at)
                && counts_for(press, report))
    }

    fn note_caret(&mut self, began: Us, caret: usize) {
        if self.newest.is_none_or(|(at, _)| began >= at) {
            self.newest = Some((began, caret));
        }
    }

    fn say_changes(&mut self, changes: &[String]) {
        for words in changes {
            self.expected.push(Heard::Words(words.clone()));
        }
    }

    /// A focus is reported at `now` on `field`, a read of `text` that
    /// began at `began`: a repeat when it is the focused field again.
    pub fn focus(&mut self, field: usize, text: &Text, began: Us, now: Us) {
        self.committed(field, text, false);
        if !self.alive {
            self.newest = None;
        }
        self.alive = true;
        self.waiting_ends(now);
        if self.focus == Some(field) {
            self.report(
                &Report {
                    text: text.clone(),
                    began,
                    ended: now,
                    final_read: false,
                    dispatched_in_window: self.in_window_at(field, began),
                },
                now,
            );
            return;
        }
        self.changes.clear();
        self.newest = Some((began, text.caret));
        let cut = std::mem::take(&mut self.other_window);
        self.focus_moves(field, now, cut);
    }

    /// The focus moves at `now` to a focus reported on `to`, `cut` when
    /// into another top-level window: what is owed of the old one.
    fn focus_moves(&mut self, to: usize, now: Us, cut: bool) {
        if let Some(open) = self.open() {
            self.keys[open].done = true;
            let press = self.keys[open].clone();
            if press.before == BeforeRead::Committed
                && press.passed.is_some_and(|at| now < at + BOUND_US)
                && let Some(words) = focus_first_words(press.key, press.baseline.as_ref())
            {
                self.reached
                    .insert("a focus moving first says a key's words");
                self.expected.push(Heard::Words(words));
            }
        }
        // A key waiting behind another is judged at its pass.
        for press in self
            .keys
            .iter_mut()
            .filter(|press| press.passed.is_none() && press.before != BeforeRead::NotAsked)
        {
            press.done = true;
        }
        let held = std::mem::take(&mut self.held);
        self.say_changes(&held);
        if cut {
            self.expected.push(Heard::Cut);
        }
        self.expected.push(Heard::Focus);
        self.focus = Some(to);
    }

    /// A report on the button is the focus at `now`, `new_window` when in
    /// another top-level window.
    pub fn focus_button(&mut self, new_window: bool, now: Us) {
        self.changes.clear();
        self.newest = None;
        if new_window {
            self.other_window = true;
        }
        self.focus_moves(BUTTON, now, new_window);
    }

    /// A focus report on `field` waits for its live re-read: its read is
    /// the node's last read, and nothing else has happened.
    pub fn focus_waits(&mut self, field: usize, text: &Text) {
        self.reached.insert("a focus report waiting");
        self.committed(field, text, false);
        self.changes.clear();
        self.waiting = Some(field);
    }

    /// The wait for a live re-read ended at `now`: the keys behind it run.
    fn waiting_ends(&mut self, now: Us) {
        if self.waiting.take().is_some() {
            for press in self.keys.iter_mut().filter(|press| press.queued) {
                press.queued = false;
                press.floor = now;
            }
        }
    }

    /// The first focus of a case: nothing compared and no key open.
    pub fn first_focus(&mut self, field: usize, text: &Text, began: Us) {
        self.last_read.insert(field, text.clone());
        self.note_caret(began, text.caret);
        self.expected.push(Heard::Focus);
        self.focus = Some(field);
        self.alive = true;
    }

    /// The outpost ended: its nodes are gone with their last reads, its
    /// reads never report, and a key waiting on them ends with nothing
    /// said; a held key still passes on, its node gone.
    pub fn outpost_ended(&mut self, now: Us) {
        self.reached.insert("an outpost ended");
        self.waiting_ends(now);
        self.other_window = false;
        for press in &mut self.keys {
            if press.passed.is_some() {
                press.done = true;
            }
            press.node = None;
        }
        self.alive = false;
        self.last_read.clear();
        self.before_reads.clear();
        self.final_reads.clear();
    }

    /// The clock reached `now`.
    ///
    /// # Errors
    ///
    /// When an open key's bound has passed with no final read asked.
    pub fn clock(&self, now: Us, final_read_asked: bool) -> Result<(), String> {
        if let Some(open) = self.open()
            && let Some(passed) = self.keys[open].passed
            && now >= passed + BOUND_US
            && !final_read_asked
        {
            return Err(format!(
                "key {open}'s 100 ms bound passed at {now} with no final read asked"
            ));
        }
        Ok(())
    }

    /// Verbatim exited.
    pub fn exit(&mut self) {
        if self.keys.iter().any(|press| press.passed.is_none()) {
            self.reached.insert("exit with a key in line");
        }
        self.held.clear();
        self.exited = true;
    }

    /// Checks the facts at `now`: speech heard so far, keys delivered, and
    /// the caret, when the seam can read it.
    ///
    /// # Errors
    ///
    /// The first thing that differs.
    pub fn check(&self, heard: &[Heard], now: Us, caret: Option<usize>) -> Result<(), String> {
        if heard != self.expected.as_slice() {
            return Err(format!(
                "speech differs at {now}: heard {heard:?}, the rules require {:?}",
                self.expected
            ));
        }
        if let Some(caret) = caret
            && Some(caret) != self.caret()
        {
            return Err(format!(
                "Core's caret is {caret} at {now}, the newest read's is {:?}",
                self.caret()
            ));
        }
        for (id, press) in self.keys.iter().enumerate() {
            if press.passed.is_some() {
                continue;
            }
            if self.exited {
                return Err(format!("key {id} withheld after exit"));
            }
            if press.held
                && !press.queued
                && let Some(latest) = self.latest_pass(id)
                && now >= latest
            {
                return Err(format!(
                    "key {id} not passed on by its bound {latest}, at {now}"
                ));
            }
        }
        Ok(())
    }

    /// The latest key `id` may be passed on: its before read's limit after
    /// the key ahead's 100 ms (`docs/parity/text.md`, "Holding caret keys";
    /// 2026-11-08); `None` while the key ahead has no pass to count from.
    fn latest_pass(&self, id: usize) -> Option<Us> {
        let press = &self.keys[id];
        let ahead = match id.checked_sub(1) {
            Some(ahead) => Some(self.keys[ahead].passed?),
            None => None,
        };
        let from = ahead.map_or(press.floor, |at| press.floor.max(at + BOUND_US));
        Some(from + BEFORE_READ_US)
    }
}

/// The characters either side of the caret.
fn neighbours(text: &Text) -> (Option<char>, Option<char>) {
    let before = text
        .caret
        .checked_sub(1)
        .and_then(|at| text.letters.get(at).copied());
    (before, text.at_caret())
}

/// The unit a key speaks at `text`.
fn unit(key: Key, text: &Text) -> Option<String> {
    match key {
        Key::Right | Key::Left | Key::Delete => Some(
            text.at_caret()
                .map_or_else(|| "blank".to_owned(), |letter| letter.to_string()),
        ),
        Key::Down => Some(if text.letters.is_empty() {
            "blank".to_owned()
        } else {
            text.line()
        }),
        Key::ShiftRight | Key::Backspace | Key::WordBackspace => None,
    }
}

/// Whether `report`'s read counts for `press`: dispatched in a key's
/// window on its node, or committed by the end of `press`'s window.
fn counts_for(press: &Press, report: &Report) -> bool {
    report.dispatched_in_window
        || press
            .window
            .is_some_and(|(from, until)| from <= report.ended && report.ended <= until)
}

/// What the difference from `before` to `after` shows removed at their
/// carets: the text between them when the rest is kept, the text after the
/// caret when it stayed; "blank" for a caret moved forward with nothing
/// removed; `None` for anything else.
fn removed_between(before: &Text, after: &Text) -> Option<String> {
    let (was, now) = (&before.letters, &after.letters);
    let (from, to) = (after.caret, before.caret);
    if from > to {
        // Text gained before the caret is a joined line, whose break this
        // one-line world has no name for; otherwise nothing was removed.
        let joined = now.len().checked_sub(was.len()) == Some(from) && now.ends_with(was);
        return (!joined).then(|| "blank".to_owned());
    }
    let kept_before = was.get(..from) == now.get(..from);
    if from == to {
        let gone = was.len().checked_sub(now.len()).filter(|gone| *gone > 0)?;
        return (kept_before && was.get(to + gone..) == now.get(from..))
            .then(|| was[to..to + gone].iter().collect());
    }
    (kept_before && was.get(to..) == now.get(from..)).then(|| was[from..to].iter().collect())
}

/// A key's words when `text` is its evidence: its unit, or for Backspace
/// what `text` shows removed against `baseline`.
fn evidence_words(key: Key, baseline: Option<&Text>, text: &Text) -> Option<String> {
    if matches!(key, Key::Backspace | Key::WordBackspace) {
        let baseline = baseline?;
        if baseline.letters == text.letters {
            return (baseline.caret != text.caret).then(|| "blank".to_owned());
        }
        if let Some((start, _)) = baseline.selection {
            return Some(before_start(key, baseline, start));
        }
        removed_between(baseline, text)
    } else {
        unit(key, text)
    }
}

/// What Backspace or Control+Backspace removes before `start` of `text`:
/// the character there, or for Control+Backspace all before it, "blank" at
/// the text's start.
fn before_start(key: Key, text: &Text, start: usize) -> String {
    if start == 0 {
        return "blank".to_owned();
    }
    if key == Key::WordBackspace {
        return text.letters[..start].iter().collect();
    }
    text.letters[start - 1].to_string()
}

/// A key's words when the focus moves first: the unit at its before read,
/// Backspace the letter before the caret, "blank" at the text's start.
fn focus_first_words(key: Key, before: Option<&Text>) -> Option<String> {
    let before = before?;
    if matches!(key, Key::Backspace | Key::WordBackspace) {
        Some(before_start(key, before, before.ends().0))
    } else {
        unit(key, before)
    }
}
