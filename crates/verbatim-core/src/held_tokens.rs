//! The listener's told elements Core holds, and the shell's record of the
//! views it sends the listener (design of the listener group, "Told
//! elements released by Core's held view"), as the outposts' held nodes
//! are sent: one function of Core's state computes the set
//! ([`crate::SrState::held_tokens`]), and the listener lets go of every
//! element it keeps that a view leaves out and whose tell Core has
//! handled, judging nothing itself.
//!
//! A token is held from its tell until Core decides: while its entry
//! waits or runs, and afterwards only while it is the readers' last
//! focus. An entry never made, dropped at arrival, merged or ended at the
//! front leaves the set with no release code on any path. The set never
//! takes back a token it dropped: a new focus always comes with a new
//! token, and no rule restores an older one.

use std::collections::BTreeSet;

use verbatim_model::tell::ElementToken;

/// The tokens Core holds of the listener generation `generation`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HeldTokens {
    /// The listener generation the tokens are of: the one running.
    pub generation: u64,
    /// The tokens.
    pub tokens: BTreeSet<ElementToken>,
}

/// A view of the tokens Core holds for the listener, with the position of
/// the last of its messages carrying tokens that Core has handled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct View {
    /// The listener generation it is for.
    pub generation: u64,
    /// The tokens Core holds.
    pub tokens: Vec<ElementToken>,
    /// The position of the last message carrying tokens Core handled.
    pub acknowledged: u64,
}

/// What the shell does with the view after a pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViewSend {
    /// Nothing: the listener's last view still says everything.
    Nothing,
    /// Write it at once: a token Core handled is not held, and no view
    /// written told the listener so.
    Now(View),
    /// Leave it for the next command written to the listener: only
    /// additions, or a position that moved with every handled token still
    /// held, which release nothing.
    Leave(View),
}

/// The shell's record of the views it sends the running listener: the
/// position of the last message carrying tokens it handled, the tokens of
/// those handled since the last view left or written, and that view.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListenerView {
    /// The listener generation counted, `None` before the first.
    generation: Option<u64>,
    /// How many messages carrying tokens of that generation were handled.
    position: u64,
    /// The tokens handled since the last view left or written.
    handled: BTreeSet<ElementToken>,
    /// The tokens and position of the last view left or written.
    last: (BTreeSet<ElementToken>, u64),
}

impl ListenerView {
    /// The owner thread reported the listener `generation` started: its
    /// count begins at 0, and nothing it was sent is known.
    pub fn started(&mut self, generation: u64) {
        *self = Self {
            generation: Some(generation),
            ..Self::default()
        };
    }

    /// A message of the listener `generation` carrying `token` was handed
    /// to Core: counted, when it is of the generation counted.
    pub fn handled(&mut self, generation: u64, token: ElementToken) {
        if self.generation == Some(generation) {
            self.position = self.position.saturating_add(1);
            self.handled.insert(token);
        }
    }

    /// The position of the last message carrying tokens handled.
    #[must_use]
    pub fn position(&self) -> u64 {
        self.position
    }

    /// What to send after a pass in which Core came to hold `held`: a view
    /// at once when a token handled since the last view, or one that view
    /// held, is not held now; otherwise a view left for the next command
    /// when the set or the position moved; otherwise nothing.
    pub fn after_pass(&mut self, held: Option<&HeldTokens>) -> ViewSend {
        let Some(generation) = self.generation else {
            return ViewSend::Nothing;
        };
        let empty = BTreeSet::new();
        let tokens = held
            .filter(|held| held.generation == generation)
            .map_or(&empty, |held| &held.tokens);
        let released = self
            .handled
            .iter()
            .chain(self.last.0.iter())
            .any(|token| !tokens.contains(token));
        if !released && self.last.0 == *tokens && self.last.1 == self.position {
            return ViewSend::Nothing;
        }
        self.last = (tokens.clone(), self.position);
        self.handled.clear();
        let view = View {
            generation,
            tokens: tokens.iter().copied().collect(),
            acknowledged: self.position,
        };
        if released {
            ViewSend::Now(view)
        } else {
            ViewSend::Leave(view)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held(tokens: &[u64]) -> HeldTokens {
        HeldTokens {
            generation: 1,
            tokens: tokens.iter().copied().map(ElementToken).collect(),
        }
    }

    fn view(tokens: &[u64], acknowledged: u64) -> View {
        View {
            generation: 1,
            tokens: tokens.iter().copied().map(ElementToken).collect(),
            acknowledged,
        }
    }

    /// A tell refused in the pass that handled it writes a view at once,
    /// though the set is what it was: the listener releases it within one
    /// pipe hop.
    #[test]
    fn a_tell_refused_in_its_own_pass_writes_a_view() {
        let mut record = ListenerView::default();
        record.started(1);
        record.handled(1, ElementToken(1));
        assert_eq!(
            record.after_pass(Some(&held(&[]))),
            ViewSend::Now(view(&[], 1))
        );
        assert_eq!(record.after_pass(Some(&held(&[]))), ViewSend::Nothing);
    }

    /// An addition alone writes nothing: it is left for the next command;
    /// the held token leaving the set later writes a view.
    #[test]
    fn an_addition_alone_is_left_and_its_leaving_written() {
        let mut record = ListenerView::default();
        record.started(1);
        record.handled(1, ElementToken(1));
        assert_eq!(
            record.after_pass(Some(&held(&[1]))),
            ViewSend::Leave(view(&[1], 1))
        );
        assert_eq!(record.after_pass(Some(&held(&[1]))), ViewSend::Nothing);
        record.handled(1, ElementToken(2));
        assert_eq!(
            record.after_pass(Some(&held(&[1, 2]))),
            ViewSend::Leave(view(&[1, 2], 2))
        );
        assert_eq!(
            record.after_pass(Some(&held(&[2]))),
            ViewSend::Now(view(&[2], 2))
        );
    }

    /// A token handled and held, in no view written, that leaves the set
    /// is written at once.
    #[test]
    fn a_held_token_leaving_before_any_view_is_written() {
        let mut record = ListenerView::default();
        record.started(1);
        record.handled(1, ElementToken(1));
        record.handled(1, ElementToken(2));
        assert_eq!(
            record.after_pass(Some(&held(&[1, 2]))),
            ViewSend::Leave(view(&[1, 2], 2))
        );
        record.handled(1, ElementToken(3));
        assert_eq!(
            record.after_pass(Some(&held(&[1, 3]))),
            ViewSend::Now(view(&[1, 3], 3))
        );
    }

    /// A new generation counts from 0 and forgets the last view; a message
    /// of another generation is not counted, and a set of another
    /// generation holds nothing of this one.
    #[test]
    fn a_new_generation_counts_from_zero() {
        let mut record = ListenerView::default();
        assert_eq!(record.after_pass(None), ViewSend::Nothing);
        record.started(1);
        record.handled(1, ElementToken(7));
        assert_eq!(
            record.after_pass(Some(&held(&[7]))),
            ViewSend::Leave(view(&[7], 1))
        );
        record.started(2);
        record.handled(1, ElementToken(8));
        record.handled(2, ElementToken(1));
        assert_eq!(record.position(), 1);
        assert_eq!(
            record.after_pass(Some(&held(&[7]))),
            ViewSend::Now(View {
                generation: 2,
                tokens: Vec::new(),
                acknowledged: 1,
            })
        );
    }
}
