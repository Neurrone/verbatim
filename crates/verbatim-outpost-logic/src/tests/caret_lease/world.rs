//! The world of the caret generated test (`super`): an application as a
//! screen reader meets one. Two text fields, each one line of letters with
//! a caret and a selection; keys that take effect when the application
//! gets to them, or never; events raised whenever the application pleases;
//! reads that see the application as it is when they end. Nothing here
//! knows how Verbatim reads or holds a key.

use std::collections::VecDeque;

/// Microseconds on the run's one clock.
pub type Us = u64;

/// A key the test presses, as the user would.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// Right Arrow.
    Right,
    /// Left Arrow.
    Left,
    /// Down Arrow, which moves by line.
    Down,
    /// Shift+Right Arrow.
    ShiftRight,
    /// Delete.
    Delete,
    /// Backspace.
    Backspace,
}

/// The keys in the order a generated action names them.
pub const KEYS: [Key; 6] = [
    Key::Right,
    Key::Left,
    Key::Down,
    Key::ShiftRight,
    Key::Delete,
    Key::Backspace,
];

/// A text field's content, as a read sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Text {
    /// The letters of its one line.
    pub letters: Vec<char>,
    /// The caret, in letters.
    pub caret: usize,
    /// The selection, when something is selected: its start and end, the
    /// caret at one of them.
    pub selection: Option<(usize, usize)>,
}

impl Text {
    fn new(letters: &str) -> Self {
        Self {
            letters: letters.chars().collect(),
            caret: 0,
            selection: None,
        }
    }

    /// The selection's ends, the caret twice when nothing is selected.
    #[must_use]
    pub fn ends(&self) -> (usize, usize) {
        self.selection.unwrap_or((self.caret, self.caret))
    }

    /// The letter at the caret, when there is one.
    #[must_use]
    pub fn at_caret(&self) -> Option<char> {
        self.letters.get(self.caret).copied()
    }

    /// The line's letters.
    #[must_use]
    pub fn line(&self) -> String {
        self.letters.iter().collect()
    }
}

/// How the selection changed from `old` to `new`, each change whether it
/// was selected and its letters, in the order `docs/parity/text.md` gives
/// them: when the two do not touch, the old is unselected and the new
/// selected; otherwise the start's change, then the end's.
#[must_use]
pub fn selection_changes(old: &Text, new: &Text) -> Vec<(bool, String)> {
    let (old_start, old_end) = old.ends();
    let (new_start, new_end) = new.ends();
    // Where the text changed an end can lie past the other text's end.
    let piece = |text: &Text, from: usize, to: usize| -> String {
        let to = to.min(text.letters.len());
        text.letters[from.min(to)..to].iter().collect()
    };
    let mut changes = Vec::new();
    if new_end < old_start || new_start > old_end {
        if old_start != old_end {
            changes.push((false, piece(old, old_start, old_end)));
        }
        if new_start != new_end {
            changes.push((true, piece(new, new_start, new_end)));
        }
        return changes;
    }
    if new_start < old_start {
        changes.push((true, piece(new, new_start, old_start)));
    } else if new_start > old_start {
        changes.push((false, piece(old, old_start, new_start)));
    }
    if new_end > old_end {
        changes.push((true, piece(new, old_end, new_end)));
    } else if new_end < old_end {
        changes.push((false, piece(old, new_end, old_end)));
    }
    changes
}

/// One step a case takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// A key is pressed, `KEYS[key]`, held by the hook or passed as
    /// pressed.
    Press { key: u8, held: bool },
    /// The application gets to the oldest key it has been given and acts
    /// on it, or finds it has nothing to do (`effect` false).
    Apply { effect: bool },
    /// The application or the mouse changes the selection with no key.
    Select,
    /// The application raises a caret event for the focused field.
    Event,
    /// The read in flight ends, seeing the application as it is now.
    Finish,
    /// The read in flight is abandoned, never ending.
    Abandon,
    /// A focus is reported, on the other field or again on this one, read
    /// `ago` tens of milliseconds ago.
    Focus { other: bool, ago: u8 },
    /// The clock moves on `ms` milliseconds.
    Tick(u8),
    /// The outpost's process ends.
    OutpostEnd,
    /// Verbatim exits.
    Exit,
}

/// The application's two fields and which has the focus, the keys it has
/// been given and not yet acted on, and each field's past, so a focus can
/// report a read made earlier.
#[derive(Clone, Debug)]
pub struct World {
    /// The two fields.
    pub fields: [Text; 2],
    /// The field with the focus.
    pub focus: usize,
    given: VecDeque<Key>,
    past: Vec<(Us, usize, Text)>,
}

impl World {
    /// The application as a case finds it.
    #[must_use]
    pub fn new() -> Self {
        Self {
            fields: [Text::new("abcdef"), Text::new("uvwxyz")],
            focus: 0,
            given: VecDeque::new(),
            past: Vec::new(),
        }
    }

    /// Records the fields as they are at `now`, for [`Self::text_at`].
    pub fn note(&mut self, now: Us) {
        for field in 0..2 {
            let text = self.fields[field].clone();
            let same = self
                .past
                .iter()
                .rev()
                .find(|(_, seen, _)| *seen == field)
                .is_some_and(|(_, _, last)| *last == text);
            if !same {
                self.past.push((now, field, text));
            }
        }
    }

    /// Field `field` as it was at `at`: the last noted at or before it.
    #[must_use]
    pub fn text_at(&self, field: usize, at: Us) -> Text {
        self.past
            .iter()
            .rev()
            .find(|(when, seen, _)| *seen == field && *when <= at)
            .map_or_else(|| self.fields[field].clone(), |(_, _, text)| text.clone())
    }

    /// Whether a key has been given to the application and not acted on.
    #[must_use]
    pub fn has_given(&self) -> bool {
        !self.given.is_empty()
    }

    /// Gives the application `key`, passed on to it.
    pub fn give(&mut self, key: Key) {
        self.given.push_back(key);
    }

    /// The application acts on the oldest key it was given, on the focused
    /// field; with `effect` false, a key with nothing to do.
    pub fn apply(&mut self, effect: bool) {
        let Some(key) = self.given.pop_front() else {
            return;
        };
        if effect {
            self.act(key);
        }
    }

    fn act(&mut self, key: Key) {
        let field = &mut self.fields[self.focus];
        let last = field.letters.len();
        match key {
            Key::Right if field.selection.is_some() => {
                field.caret = field.ends().1;
                field.selection = None;
            }
            Key::Left if field.selection.is_some() => {
                field.caret = field.ends().0;
                field.selection = None;
            }
            Key::Right if field.caret < last => field.caret += 1,
            Key::Left if field.caret > 0 => field.caret -= 1,
            Key::Down => {
                field.caret = last;
                field.selection = None;
            }
            Key::ShiftRight if field.caret < last => {
                let start = field.selection.map_or(field.caret, |(start, _)| start);
                field.caret += 1;
                field.selection = Some((start, field.caret));
            }
            Key::Delete | Key::Backspace if field.selection.is_some() => {
                let (start, end) = field.ends();
                field.letters.drain(start..end);
                field.caret = start;
                field.selection = None;
            }
            Key::Delete if field.caret < last => {
                field.letters.remove(field.caret);
            }
            Key::Backspace if field.caret > 0 => {
                field.caret -= 1;
                field.letters.remove(field.caret);
            }
            _ => {}
        }
    }

    /// The application or the mouse selects the letter before the caret,
    /// which leaves the caret where it is, or at the text's start the
    /// letter after it, which moves the caret, or collapses the selection,
    /// with no key.
    pub fn select(&mut self) {
        let field = &mut self.fields[self.focus];
        if field.selection.take().is_some() {
            return;
        }
        if field.caret > 0 {
            field.selection = Some((field.caret - 1, field.caret));
        } else if !field.letters.is_empty() {
            field.selection = Some((0, 1));
            field.caret = 1;
        }
    }
}
