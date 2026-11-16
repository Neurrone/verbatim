//! The world: what a real application and a real user do, written from the
//! recordings of 2026-11-16 (`docs/design/readers-tell-core-asks-build1.md`,
//! "Recordings") and none of the code's rules. A case is a [`Script`]: the
//! control, its text, the keys the user presses (at least 30 ms apart, as
//! tests pace them, Dickson, 2026-11-16) and how the application handles
//! each, how long each of Verbatim's reads of the application takes, and
//! whether the application moves its focus. [`App`] is the application's
//! state as strokes reach it.

use proptest::prelude::*;

/// Microseconds on the run's shared clock.
pub type Us = u64;

/// One millisecond.
pub const MS: Us = 1_000;

/// The kind of control the focus is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Windows 11 Notepad's document, read through UI Automation: every
    /// key it handles raises one `TextSelectionChanged` on the document,
    /// and a key that deletes text a `TextChanged` after it (R1).
    Notepad,
    /// The Windows Forms text box, read through its window messages: its
    /// caret events are the system caret's, which reach the listener and
    /// never an outpost (R1; design, "Events as the key's effect").
    TextBox,
}

/// A caret key the user presses: each one the hook holds while the focus
/// has text (ADR 0003, "Keys and commands"; NVDA's caret scripts,
/// `nvda/source/editableText.py` 388 to 412).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// Left Arrow: the character.
    Left,
    /// Right Arrow: the character.
    Right,
    /// Home: the character.
    Home,
    /// End: the character.
    End,
    /// Up Arrow: the line.
    Up,
    /// Down Arrow: the line.
    Down,
    /// Control+Right Arrow: the word.
    WordRight,
    /// Backspace: what it deletes.
    Backspace,
    /// Delete: the character it leaves at the caret.
    Delete,
}

/// Every key the world presses.
pub const KEYS: [Key; 9] = [
    Key::Left,
    Key::Right,
    Key::Home,
    Key::End,
    Key::Up,
    Key::Down,
    Key::WordRight,
    Key::Backspace,
    Key::Delete,
];

/// How the application handles a stroke that reaches it: it takes
/// strokes in the order they reach it, one at a time, and `change_us`
/// after it starts on this one its text and caret are as the key leaves
/// them; in Notepad it raises its events `event_us` after the change, and
/// never sooner than 3 ms after the stroke. Recorded applications change
/// within 1 to 13 ms of the key (R1); a loaded one later.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Handling {
    /// From starting on the stroke to the change.
    pub change_us: Us,
    /// From the change to the events.
    pub event_us: Us,
}

/// A key the user presses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Press {
    /// When.
    pub at: Us,
    /// Which key.
    pub key: Key,
    /// How the application handles its stroke, `None` when it does not
    /// handle the key at all.
    pub handling: Option<Handling>,
    /// How long the outpost's read of the caret before the key takes.
    pub read_us: Us,
}

/// The application moves its focus from the field to its "OK" button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FocusMove {
    /// When the application moves it: strokes reaching it later go to the
    /// button.
    pub at: Us,
    /// When the listener's tell of the button's focus reaches Core.
    pub told: Us,
    /// How long the listener's read of the element takes.
    pub listener_us: Us,
    /// How long the outpost's completion of the read takes.
    pub completion_us: Us,
}

/// One case.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Script {
    /// The control.
    pub kind: Kind,
    /// Its text: one line.
    pub text: String,
    /// The caret's offset in it.
    pub caret: usize,
    /// The keys, in press order.
    pub presses: Vec<Press>,
    /// Whether, and how, the application moves its focus.
    pub focus: Option<FocusMove>,
}

/// The application's field as strokes leave it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct App {
    /// The field's text.
    pub text: String,
    /// The caret's offset.
    pub caret: usize,
}

/// The application's input: when it is done with the strokes it has.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Input {
    busy_until: Us,
}

impl Input {
    /// A stroke reaches the field at `at`: when the application changes
    /// the field for it and, in Notepad, when it raises its events, or
    /// `None` when it does not handle the key.
    pub fn stroke(
        &mut self,
        at: Us,
        handling: Option<Handling>,
        kind: Kind,
    ) -> Option<(Us, Option<Us>)> {
        let handling = handling?;
        let change = at.max(self.busy_until) + handling.change_us;
        self.busy_until = change;
        let event = match kind {
            Kind::Notepad => Some((change + handling.event_us).max(at + 3 * MS)),
            Kind::TextBox => None,
        };
        Some((change, event))
    }
}

impl App {
    /// The field after `key`'s stroke is handled. The field is one line, so
    /// Up and Down Arrow move nothing.
    pub fn apply(&mut self, key: Key) {
        let len = self.text.len();
        match key {
            Key::Left => self.caret = self.caret.saturating_sub(1),
            Key::Right => self.caret = (self.caret + 1).min(len),
            Key::Home => self.caret = 0,
            Key::End => self.caret = len,
            Key::Up | Key::Down => {}
            Key::WordRight => {
                let bytes = self.text.as_bytes();
                let mut at = self.caret;
                while at < len && bytes[at] != b' ' {
                    at += 1;
                }
                while at < len && bytes[at] == b' ' {
                    at += 1;
                }
                self.caret = at;
            }
            Key::Backspace => {
                if self.caret > 0 {
                    self.text.remove(self.caret - 1);
                    self.caret -= 1;
                }
            }
            Key::Delete => {
                if self.caret < len {
                    self.text.remove(self.caret);
                }
            }
        }
    }

    /// The character at the caret, `None` at the text's end.
    pub fn character(&self) -> Option<char> {
        self.text[self.caret..].chars().next()
    }

    /// The character before the caret, `None` at the text's start.
    pub fn before(&self) -> Option<char> {
        self.text[..self.caret].chars().next_back()
    }

    /// The word the caret is in, with the spaces after it, as the
    /// application's text provider gives the word unit; empty at the
    /// text's end.
    pub fn word(&self) -> String {
        let bytes = self.text.as_bytes();
        if self.caret >= bytes.len() {
            return String::new();
        }
        let starts = |at: usize| bytes[at] != b' ' && (at == 0 || bytes[at - 1] == b' ');
        let start = (0..=self.caret).rev().find(|&at| starts(at)).unwrap_or(0);
        let end = (self.caret + 1..bytes.len())
            .find(|&at| starts(at))
            .unwrap_or(bytes.len());
        self.text[start..end].to_owned()
    }
}

/// The words a field's text is made of: two or three letters each, so no
/// word is spoken as a character.
const WORDS: [&str; 6] = ["ab", "cde", "fg", "hij", "kl", "mno"];

fn text() -> impl Strategy<Value = String> {
    proptest::collection::vec(0..WORDS.len(), 1..=3).prop_map(|words| {
        words
            .into_iter()
            .map(|w| WORDS[w])
            .collect::<Vec<_>>()
            .join(" ")
    })
}

/// A delay of `lo` to `hi` ms, in microseconds, at any microsecond.
fn delay(lo: Us, hi: Us) -> impl Strategy<Value = Us> {
    (lo * MS)..=(hi * MS)
}

fn handling() -> impl Strategy<Value = Option<Handling>> {
    prop_oneof![
        6 => (delay(1, 10), delay(0, 3)).prop_map(|delays| Some(handled(delays))),
        2 => (delay(20, 80), delay(0, 3)).prop_map(|delays| Some(handled(delays))),
        1 => (delay(120, 250), delay(0, 3)).prop_map(|delays| Some(handled(delays))),
        1 => Just(None),
    ]
}

fn handled((change_us, event_us): (Us, Us)) -> Handling {
    Handling {
        change_us,
        event_us,
    }
}

fn press() -> impl Strategy<Value = (Us, Key, Option<Handling>, Us)> {
    (
        prop_oneof![3 => delay(30, 60), 2 => delay(60, 200), 1 => delay(200, 400)],
        proptest::sample::select(KEYS.to_vec()),
        handling(),
        prop_oneof![5 => delay(1, 20), 2 => delay(20, 90), 1 => delay(90, 220)],
    )
}

prop_compose! {
    fn focus_move(first: Us)(
        after in delay(0, 400),
        told in delay(1, 4),
        listener in delay(1, 20),
        completion in delay(1, 30),
    ) -> FocusMove {
        FocusMove {
            at: first + after,
            told: first + after + told,
            listener_us: listener,
            completion_us: completion,
        }
    }
}

/// When the first key is pressed.
pub const FIRST: Us = 1_000 * MS;

/// A case: a control, its text and caret, one to four keys, and perhaps a
/// focus move.
pub fn script() -> impl Strategy<Value = Script> {
    prop_oneof![Just(Kind::Notepad), Just(Kind::TextBox)]
        .prop_flat_map(|kind| {
            (
                Just(kind),
                text(),
                proptest::collection::vec(press(), 1..=4),
                proptest::option::weighted(0.4, focus_move(FIRST)),
                any::<proptest::sample::Index>(),
            )
        })
        .prop_map(|(kind, text, presses, focus, caret)| {
            let caret = caret.index(text.len() + 1);
            let mut at = FIRST;
            let presses = presses
                .into_iter()
                .enumerate()
                .map(|(place, (gap, key, handling, read_us))| {
                    if place > 0 {
                        at += gap;
                    }
                    Press {
                        at,
                        key,
                        handling,
                        read_us,
                    }
                })
                .collect();
            Script {
                kind,
                text,
                caret,
                presses,
                focus,
            }
        })
}
