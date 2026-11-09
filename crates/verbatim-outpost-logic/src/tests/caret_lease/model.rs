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
//! - A key pressed later silences an earlier key's words, and the
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
//!   is still running says nothing (2026-11-09).

use std::collections::BTreeMap;

use super::world::{Key, Text, Us, selection_changes};

/// The 100 ms bound, from when the key is passed on (2026-11-09).
pub const BOUND_US: Us = 100_000;
/// How long a held key's before read may run (2026-11-08).
pub const BEFORE_READ_US: Us = 150_000;

/// A key pressed, as the model follows it.
#[derive(Clone, Debug)]
struct Press {
    key: Key,
    held: bool,
    pressed: Us,
    passed: Option<Us>,
    /// The node's last read when the key was passed on: what it is
    /// compared with.
    baseline: Option<Text>,
    /// A read of the focused node committed between the press and the
    /// pass: its before read.
    before_read: bool,
    /// Heard, silenced or ended: its words are said or owed no more.
    done: bool,
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
    changes: Vec<(bool, String)>,
    /// The selection changes waiting for the words of the key that made
    /// them.
    held: Vec<(bool, String)>,
    expected: Vec<Heard>,
    /// The newest caret: when its read began, and the caret.
    newest: Option<(Us, usize)>,
    /// The field with the focus.
    focus: Option<usize>,
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

    /// Whether a focus moving to the other field is a case the rules
    /// decide: no key waits behind another, and no open key is past its
    /// bound with its final read not reported.
    #[must_use]
    pub fn focus_moves_decided(&self, now: Us) -> bool {
        let unpassed: Vec<usize> = (0..self.keys.len())
            .filter(|&id| self.keys[id].passed.is_none())
            .collect();
        if unpassed.len() > 1 {
            return false;
        }
        if let Some(&id) = unpassed.first()
            && id > 0
            && self.keys[id - 1]
                .passed
                .is_none_or(|at| now < at + BOUND_US)
        {
            return false;
        }
        self.open()
            .is_none_or(|open| self.keys[open].passed.is_none_or(|at| now <= at + BOUND_US))
    }

    /// Whether a selection no key made is decided: never over an open
    /// Backspace, for which the documents give no words when nothing was
    /// removed.
    #[must_use]
    pub fn selecting_decided(&self) -> bool {
        self.open()
            .is_none_or(|open| self.keys[open].key != Key::Backspace)
    }

    /// A key is pressed at `now`; any earlier key's words are silenced.
    pub fn press(&mut self, key: Key, held: bool, now: Us) {
        if let Some(open) = self.open() {
            self.keys[open].done = true;
        }
        self.keys.push(Press {
            key,
            held,
            pressed: now,
            passed: None,
            baseline: None,
            before_read: false,
            done: false,
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
        let press = &mut self.keys[id];
        press.passed = Some(now);
        press.baseline = baseline;
        Ok(())
    }

    /// A read of `field` committed, having seen `text`: the node's last
    /// read is replaced, and whether it shows a change is noted.
    pub fn committed(&mut self, field: usize, text: &Text) {
        let (shows, changes) = self.last_read.get(&field).map_or_else(
            || (false, Vec::new()),
            |last| {
                let shows = last.caret != text.caret
                    || last.selection != text.selection
                    || neighbours(last) != neighbours(text);
                (shows, selection_changes(last, text))
            },
        );
        self.shows_change = shows;
        self.changes = changes;
        if self.focus == Some(field) {
            for press in self.keys.iter_mut().filter(|press| press.passed.is_none()) {
                press.before_read = true;
            }
        }
        self.last_read.insert(field, text.clone());
    }

    /// A report of a read reached Core at `now`.
    pub fn report(&mut self, report: &Report, now: Us) {
        self.note_caret(report.began, report.text.caret);
        let changes = std::mem::take(&mut self.changes);
        let Some(open) = self.open() else {
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

    fn say_changes(&mut self, changes: &[(bool, String)]) {
        for (selected, letters) in changes {
            let word = if *selected { "selected" } else { "unselected" };
            self.expected
                .push(Heard::Words(format!("{letters} {word}")));
        }
    }

    /// A focus is reported at `now` on `field`, a read of `text` that
    /// began at `began`; `repeat` when it is the focused field again.
    pub fn focus(&mut self, field: usize, repeat: bool, text: &Text, began: Us, now: Us) {
        self.committed(field, text);
        if repeat {
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
            if press.before_read
                && press.passed.is_some_and(|at| now <= at + BOUND_US)
                && let Some(words) = focus_first_words(press.key, press.baseline.as_ref())
            {
                self.expected.push(Heard::Words(words));
            }
        }
        for press in self.keys.iter_mut().filter(|press| press.passed.is_none()) {
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
    }

    /// The outpost ended: its reads never report, and a key waiting on them
    /// ends with nothing said; a held key still passes on.
    pub fn outpost_ended(&mut self) {
        for press in self.keys.iter_mut().filter(|press| press.passed.is_some()) {
            press.done = true;
        }
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
                .caret
                .checked_sub(1)
                .map_or_else(|| "blank".to_owned(), |at| before.letters[at].to_string()),
        )
    } else {
        unit(key, before)
    }
}
