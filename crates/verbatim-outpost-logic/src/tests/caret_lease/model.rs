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
//! - The caret is the one whose read began most recently (2026-10-25).
//! - A focus moving first within the 100 ms speaks the key's unit from the
//!   read before it, Backspace what that read shows it removes, with no
//!   before read nothing (2026-11-08, 2026-11-09); a key whose before read
//!   is still running says nothing (2026-11-09). After the 100 ms and
//!   before the key has spoken, the key says nothing, its held selection
//!   changes still coming first (Dickson's answer, 2026-11-09).
//! - A key waiting behind another when the focus moves still reaches the
//!   application in press order, and says nothing for itself if its node
//!   is no longer the focus when it is judged, at its pass (Dickson's
//!   answer, 2026-11-09).
//! - A selection change arriving while a key is between its release and
//!   its words is held and said after them, whether or not another key
//!   waits behind it; before a key has been released it is said at once
//!   (Dickson's answer, 2026-11-09).
//! - Backspace speaks only when its caret moved
//!   (`nvda/source/editableText.py` 313 to 320): with the caret not moved
//!   it says nothing for the key and the change is heard; with the caret
//!   moved and nothing removed it says "blank" (Dickson's answer,
//!   2026-11-09).
//! - Backspace over a selection says what comes before the selection's
//!   start, Delete its unit, and the change follows, in general terms
//!   when the text changed: "selection removed" when nothing is selected
//!   now (`docs/parity/text.md`, "Selection over changed text in a
//!   classic edit control").

use std::collections::BTreeMap;

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

/// The 100 ms bound, from when the key is passed on (2026-11-09).
pub const BOUND_US: Us = 100_000;
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
    pressed: Us,
    /// The field with the focus at its press; `None` when there was none
    /// or its outpost has ended.
    node: Option<usize>,
    /// Its before read.
    before: BeforeRead,
    passed: Option<Us>,
    /// The node's last read when the key was passed on: what it is
    /// compared with.
    baseline: Option<Text>,
    /// Heard, silenced or ended: its words are said or owed no more.
    done: bool,
    /// A later key silenced it, whose words its selection changes wait for.
    silenced: bool,
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
}

/// Something heard, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Heard {
    /// Words.
    Words(String),
    /// A focus's own speech.
    Focus,
}

/// The reference model.
#[derive(Clone, Debug, Default)]
pub struct Model {
    keys: Vec<Press>,
    /// Each node's one last read, replaced by every read.
    last_read: BTreeMap<usize, Text>,
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
    /// The field with the focus.
    focus: Option<usize>,
    /// Its outpost has not ended since it was reported. A new generation's
    /// report of the same field is that focus again.
    alive: bool,
    exited: bool,
}

impl Model {
    /// The caret Core must hold.
    #[must_use]
    pub fn caret(&self) -> Option<usize> {
        self.newest.map(|(_, caret)| caret)
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
            press.silenced = true;
        }
        self.keys.push(Press {
            key,
            held,
            pressed: now,
            node: self.focus.filter(|_| self.alive),
            before: BeforeRead::NotAsked,
            passed: None,
            baseline: None,
            done: false,
            silenced: false,
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
        let press = &mut self.keys[id];
        press.passed = Some(now);
        press.baseline = baseline;
        if press.node.is_none() || press.node != focus {
            press.done = true;
            self.say_held_when_no_key_is_out();
        }
        Ok(())
    }

    /// Core asked a before read, of the oldest key not yet passed on.
    pub fn before_read_asked(&mut self) {
        if let Some(press) = self.keys.iter_mut().find(|press| press.passed.is_none()) {
            press.before = BeforeRead::Asked;
        }
    }

    /// Whether a key is between its release and its words: released with
    /// its words owed, or silenced with the key that silenced it, or one
    /// after that, not yet done.
    fn key_is_out(&self) -> bool {
        self.keys.iter().enumerate().any(|(id, press)| {
            press.passed.is_some()
                && (!press.done
                    || (press.silenced && self.keys[id + 1..].iter().any(|later| !later.done)))
        })
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
        self.shows_change = shows;
        self.changes = changes;
        if before_read
            && let Some(press) = self.keys.iter_mut().find(|press| press.passed.is_none())
        {
            press.before = BeforeRead::Committed;
        }
        self.last_read.insert(field, text.clone());
    }

    /// A report of a read reached Core at `now`.
    pub fn report(&mut self, report: &Report, now: Us) {
        self.note_caret(report.began, report.text.caret);
        let changes = std::mem::take(&mut self.changes);
        let Some(open) = self.open() else {
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
        let evidence = report.ended >= passed && self.shows_change;
        if !evidence && !report.final_read {
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
        self.alive = true;
        if self.focus == Some(field) {
            self.report(
                &Report {
                    text: text.clone(),
                    began,
                    ended: now,
                    final_read: false,
                },
                now,
            );
            return;
        }
        self.changes.clear();
        self.note_caret(began, text.caret);
        if let Some(open) = self.open() {
            self.keys[open].done = true;
            let press = self.keys[open].clone();
            if press.before == BeforeRead::Committed
                && press.passed.is_some_and(|at| now <= at + BOUND_US)
                && let Some(words) = focus_first_words(press.key, press.baseline.as_ref())
            {
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
        self.expected.push(Heard::Focus);
        self.focus = Some(field);
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
    pub fn outpost_ended(&mut self) {
        for press in &mut self.keys {
            if press.passed.is_some() {
                press.done = true;
            }
            press.node = None;
        }
        self.alive = false;
        self.last_read.clear();
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
            if press.held && now >= self.latest_pass(id) {
                return Err(format!(
                    "key {id} not passed on by its bound {}, at {now}",
                    self.latest_pass(id)
                ));
            }
        }
        Ok(())
    }

    /// The latest key `id` may be passed on: its before read's limit after
    /// the key ahead's 100 ms (`docs/parity/text.md`, "Holding caret keys";
    /// 2026-11-08).
    fn latest_pass(&self, id: usize) -> Us {
        let press = &self.keys[id];
        let ahead = id.checked_sub(1).and_then(|ahead| self.keys[ahead].passed);
        let from = ahead.map_or(press.pressed, |at| press.pressed.max(at + BOUND_US));
        from + BEFORE_READ_US
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
        Key::ShiftRight | Key::Backspace => None,
    }
}

/// A key's words when `text` is its evidence: its unit, or for Backspace
/// what `text` shows removed against `baseline`.
fn evidence_words(key: Key, baseline: Option<&Text>, text: &Text) -> Option<String> {
    if key == Key::Backspace {
        let baseline = baseline?;
        if baseline.letters.len() == text.letters.len() {
            return (baseline.caret != text.caret).then(|| "blank".to_owned());
        }
        if let Some((start, _)) = baseline.selection {
            return Some(
                start
                    .checked_sub(1)
                    .map_or_else(|| "blank".to_owned(), |at| baseline.letters[at].to_string()),
            );
        }
        let gone = baseline.letters.len().checked_sub(text.letters.len())?;
        (gone > 0).then(|| {
            baseline.letters[text.caret..text.caret + gone]
                .iter()
                .collect()
        })
    } else {
        unit(key, text)
    }
}

/// A key's words when the focus moves first: the unit at its before read,
/// Backspace the letter before the caret, "blank" at the text's start.
fn focus_first_words(key: Key, before: Option<&Text>) -> Option<String> {
    let before = before?;
    if key == Key::Backspace {
        Some(
            before
                .ends()
                .0
                .checked_sub(1)
                .map_or_else(|| "blank".to_owned(), |at| before.letters[at].to_string()),
        )
    } else {
        unit(key, before)
    }
}
