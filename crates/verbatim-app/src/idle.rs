//! The harness's idle barrier on the reducer's side: a round through every
//! child (`docs/design/communication.md`, section 6.7).
//!
//! The control plane's `AwaitIdle` waiters are posted straight to the
//! reducer's port, each naming the harness's last numbered input, whose
//! end the keyboard hook posts to the same port behind everything the
//! input caused. Once a waiter's input has been handled, the reducer
//! starts a round: the supervisor's relay asks
//! the listener, then every outpost, whether anything that came before the
//! question is still in progress, each child answering behind every
//! message it sent before. The relay says which outposts it asked
//! ([`verbatim_outpost::OutpostMessage::IdleQueried`]); each answers, or
//! ends, which counts as its answer, except a spare that failed before its
//! `Ready`, whose place goes to the generation holding its facts. When
//! every one has, the reducer takes
//! in what is already queued for it and checks its own conditions (no
//! request to an outpost outstanding, no outpost starting, no focus
//! wanted). If they hold, the round has passed and its waiters are
//! answered; if not, the round waits for them to clear and runs again,
//! numbered anew, so a waiter is answered only after a round in which
//! every party was idle.
//!
//! [`IdleBarrier`] is that state, pure, so its rules are tested on their
//! own; the reducer thread drives it and does the asking and the answering.
//! [`HeldMarks`] is the harness's marks of handled input held behind a lock
//! key's announcement still to come, which an idle round also waits for.

use std::collections::HashSet;

use verbatim_model::OutpostId;

/// The barrier's waiters and the round under way. `W` is what answers a
/// waiter: the reducer keeps the control plane's reply channel.
pub(crate) struct IdleBarrier<W> {
    /// Waiters no round has taken yet: their input is still to be handled,
    /// or they came while a round was asking.
    waiting: Vec<(Option<u64>, W)>,
    /// The round under way, if any.
    round: Option<Round<W>>,
    /// The number the next round takes; rounds are numbered from 1.
    next: u64,
}

/// One round and the waiters it answers.
struct Round<W> {
    number: u64,
    /// The outposts asked that have neither answered nor ended; `None`
    /// until the relay says which it asked.
    unanswered: Option<HashSet<OutpostId>>,
    /// Whether every answer came and the reducer's own conditions did not
    /// hold: the next round runs once they do.
    clearing: bool,
    waiters: Vec<W>,
}

/// Where the barrier stands, for the error a waiter that timed out
/// reports.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Standing {
    /// No round is under way.
    NoRound,
    /// Round `n` waits for the relay: the listener's answer, or an
    /// outpost's launch.
    AwaitingRelay(u64),
    /// Round `n` waits for these outposts' answers.
    AwaitingOutposts(u64, Vec<OutpostId>),
    /// Round `n` was answered by every child and waits for the reducer's
    /// conditions to clear before the next round.
    Clearing(u64),
}

impl<W> Default for IdleBarrier<W> {
    fn default() -> Self {
        Self {
            waiting: Vec::new(),
            round: None,
            next: 1,
        }
    }
}

impl<W> IdleBarrier<W> {
    /// Puts a waiter in, to be answered once the input numbered
    /// `after_input`, if any, has been handled and a round started after
    /// that has passed.
    pub(crate) fn push(&mut self, after_input: Option<u64>, waiter: W) {
        self.waiting.push((after_input, waiter));
    }

    /// The relay asked `outposts` for round `round`.
    pub(crate) fn queried(&mut self, round: u64, outposts: Vec<OutpostId>) {
        if let Some(current) = self.asking(round)
            && current.unanswered.is_none()
        {
            current.unanswered = Some(outposts.into_iter().collect());
        }
    }

    /// `outpost` answered round `round`.
    pub(crate) fn answered(&mut self, round: u64, outpost: OutpostId) {
        if let Some(unanswered) = self
            .asking(round)
            .and_then(|current| current.unanswered.as_mut())
        {
            unanswered.remove(&outpost);
        }
    }

    /// `outpost` ended: in a round asking it, its end is its answer, since
    /// everything it sent is ahead of its end.
    pub(crate) fn ended(&mut self, outpost: OutpostId) {
        if let Some(unanswered) = self
            .round
            .as_mut()
            .filter(|current| !current.clearing)
            .and_then(|current| current.unanswered.as_mut())
        {
            unanswered.remove(&outpost);
        }
    }

    /// `outpost`, a spare given to an application, failed before its
    /// `Ready`, and the generation `next` holds what was held for it
    /// ([`verbatim_outpost::OutpostMessage::Regenerated`]): in a round
    /// asking `outpost` that it has not answered, `next` answers in its
    /// place, so `outpost`'s end, which follows, does not end the round
    /// before the facts held for it are handled.
    pub(crate) fn handed_on(&mut self, outpost: OutpostId, next: OutpostId) {
        if let Some(unanswered) = self
            .round
            .as_mut()
            .filter(|current| !current.clearing)
            .and_then(|current| current.unanswered.as_mut())
            && unanswered.remove(&outpost)
        {
            unanswered.insert(next);
        }
    }

    /// Whether every outpost asked in the round under way has answered or
    /// ended: the reducer takes in what is already queued for it, then
    /// [`finish`](Self::finish)es the round.
    pub(crate) fn all_answered(&self) -> bool {
        self.round.as_ref().is_some_and(|current| {
            !current.clearing && current.unanswered.as_ref().is_some_and(HashSet::is_empty)
        })
    }

    /// Ends the round every child has answered: with the reducer's
    /// conditions `clear`, it has passed, and its waiters are returned to
    /// be answered; otherwise it waits for them to clear
    /// ([`start`](Self::start)).
    pub(crate) fn finish(&mut self, clear: bool) -> Vec<W> {
        if !self.all_answered() {
            return Vec::new();
        }
        if !clear {
            if let Some(current) = self.round.as_mut() {
                current.clearing = true;
            }
            return Vec::new();
        }
        self.round
            .take()
            .map(|current| current.waiters)
            .unwrap_or_default()
    }

    /// Starts a round when one is due, returning its number for the relay:
    /// when no round is under way and a waiter's input has been handled
    /// (`handled_input` is the highest handled), or when the round under
    /// way waits for the reducer's conditions and they are now `clear`.
    /// The new round takes every waiter whose input has been handled.
    pub(crate) fn start(&mut self, handled_input: u64, clear: bool) -> Option<u64> {
        if self
            .round
            .as_ref()
            .is_some_and(|current| !current.clearing || !clear)
        {
            return None;
        }
        let (ready, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.waiting)
            .into_iter()
            .partition(|(after_input, _)| after_input.is_none_or(|input| input <= handled_input));
        self.waiting = waiting;
        let ready = ready.into_iter().map(|(_, waiter)| waiter);
        let number = self.next;
        if let Some(current) = self.round.as_mut() {
            current.waiters.extend(ready);
            current.number = number;
            current.unanswered = None;
            current.clearing = false;
        } else {
            let waiters: Vec<W> = ready.collect();
            if waiters.is_empty() {
                return None;
            }
            self.round = Some(Round {
                number,
                unanswered: None,
                clearing: false,
                waiters,
            });
        }
        self.next += 1;
        Some(number)
    }

    /// Where the barrier stands.
    pub(crate) fn standing(&self) -> Standing {
        match &self.round {
            None => Standing::NoRound,
            Some(current) if current.clearing => Standing::Clearing(current.number),
            Some(current) => match &current.unanswered {
                None => Standing::AwaitingRelay(current.number),
                Some(unanswered) => {
                    let mut outposts: Vec<OutpostId> = unanswered.iter().copied().collect();
                    outposts.sort_unstable();
                    Standing::AwaitingOutposts(current.number, outposts)
                }
            },
        }
    }

    /// Whether any waiter is in, in a round or not.
    pub(crate) fn has_waiters(&self) -> bool {
        !self.waiting.is_empty() || self.round.is_some()
    }

    /// The round under way, while it is still asking, if it is `round`.
    fn asking(&mut self, round: u64) -> Option<&mut Round<W>> {
        self.round
            .as_mut()
            .filter(|current| current.number == round && !current.clearing)
    }
}

/// The harness's marks of handled input that came while a lock key's
/// announcement was still to come, which the reducer thread hands on, in
/// order, once it has been made, so the harness reads no speech before it
/// (`docs/design/ownership-input.md`, "The router's work as effects").
#[derive(Debug, Default)]
pub(crate) struct HeldMarks(Vec<u64>);

impl HeldMarks {
    /// The mark `mark` was taken: the marks to hand on now, none while an
    /// announcement is `pending`.
    pub(crate) fn take(&mut self, mark: u64, pending: bool) -> Vec<u64> {
        self.0.push(mark);
        self.release(pending)
    }

    /// An input was handled: the marks held, to hand on now that no
    /// announcement is `pending`, or none.
    pub(crate) fn release(&mut self, pending: bool) -> Vec<u64> {
        if pending || self.0.is_empty() {
            Vec::new()
        } else {
            std::mem::take(&mut self.0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mark_taken_while_an_announcement_is_pending_is_handed_on_after_it() {
        let mut marks = HeldMarks::default();
        assert_eq!(marks.take(3, true), Vec::<u64>::new());
        assert_eq!(marks.take(4, true), Vec::<u64>::new());
        // An input that leaves the announcement pending hands on nothing.
        assert_eq!(marks.release(true), Vec::<u64>::new());
        // The input that made it hands both on, in order.
        assert_eq!(marks.release(false), [3, 4]);
        assert_eq!(marks.release(false), Vec::<u64>::new());
        // With nothing pending, a mark goes at once.
        assert_eq!(marks.take(5, false), [5]);
    }

    const A: OutpostId = OutpostId(1);
    const B: OutpostId = OutpostId(2);

    /// A barrier with waiter `"w"` in, after no input, and its first round
    /// started, asking `outposts`.
    fn asking(outposts: &[OutpostId]) -> IdleBarrier<&'static str> {
        let mut barrier = IdleBarrier::default();
        barrier.push(None, "w");
        assert_eq!(barrier.start(0, true), Some(1));
        barrier.queried(1, outposts.to_vec());
        barrier
    }

    #[test]
    fn a_round_passes_once_every_outpost_asked_has_answered() {
        let mut barrier = asking(&[A, B]);
        assert_eq!(
            barrier.standing(),
            Standing::AwaitingOutposts(1, vec![A, B])
        );
        // An answer to another round does not count.
        barrier.answered(2, A);
        barrier.answered(1, B);
        assert!(!barrier.all_answered());
        assert_eq!(barrier.finish(true), Vec::<&str>::new());
        barrier.answered(1, A);
        assert!(barrier.all_answered());
        assert_eq!(barrier.finish(true), ["w"]);
        assert_eq!(barrier.standing(), Standing::NoRound);
        assert!(!barrier.has_waiters());
    }

    #[test]
    fn no_round_passes_before_the_relay_says_which_outposts_it_asked() {
        let mut barrier = IdleBarrier::default();
        barrier.push(None, "w");
        assert_eq!(barrier.start(0, true), Some(1));
        assert_eq!(barrier.standing(), Standing::AwaitingRelay(1));
        assert!(!barrier.all_answered());
        barrier.queried(1, Vec::new());
        assert_eq!(barrier.finish(true), ["w"]);
    }

    #[test]
    fn an_outpost_ending_during_a_round_counts_as_answered() {
        let mut barrier = asking(&[A, B]);
        barrier.answered(1, A);
        barrier.ended(B);
        assert!(barrier.all_answered());
        assert_eq!(barrier.finish(true), ["w"]);
    }

    /// A spare that failed before its `Ready` hands its place in the round
    /// to the generation that holds its facts: its end, which follows,
    /// does not end the round, the next generation's answer does.
    #[test]
    fn a_failed_spares_place_in_a_round_goes_to_its_next_generation() {
        const NEXT: OutpostId = OutpostId(3);
        let mut barrier = asking(&[A, B]);
        barrier.answered(1, A);
        barrier.handed_on(B, NEXT);
        barrier.ended(B);
        assert!(!barrier.all_answered());
        assert_eq!(
            barrier.standing(),
            Standing::AwaitingOutposts(1, vec![NEXT])
        );
        barrier.answered(1, NEXT);
        assert_eq!(barrier.finish(true), ["w"]);
    }

    /// An outpost that starts during a round, which the round did not ask,
    /// leaves the reducer's conditions unmet when every answer is in: the
    /// round does not pass, and once the outpost is ready the next round
    /// runs and answers the waiter.
    #[test]
    fn an_outpost_starting_during_a_round_makes_the_next_round_run() {
        let mut barrier = asking(&[A]);
        barrier.answered(1, A);
        // The outpost started meanwhile is still starting.
        assert_eq!(barrier.finish(false), Vec::<&str>::new());
        assert_eq!(barrier.standing(), Standing::Clearing(1));
        assert_eq!(barrier.start(0, false), None, "not while it starts");
        // Now it is ready: round 2 asks both.
        assert_eq!(barrier.start(0, true), Some(2));
        // A late answer to round 1 counts for nothing.
        barrier.answered(1, A);
        barrier.queried(2, vec![A, B]);
        barrier.answered(2, A);
        assert_eq!(barrier.finish(true), Vec::<&str>::new());
        barrier.answered(2, B);
        assert_eq!(barrier.finish(true), ["w"]);
    }

    #[test]
    fn a_waiter_starts_no_round_before_its_input_is_handled() {
        let mut barrier = IdleBarrier::default();
        barrier.push(Some(5), "w");
        assert_eq!(barrier.start(4, true), None);
        assert_eq!(barrier.start(5, true), Some(1));
    }

    /// A waiter that comes while a round is asking is answered only by a
    /// round started after it, which starts once the first has passed.
    #[test]
    fn a_waiter_that_comes_during_a_round_waits_for_the_next() {
        let mut barrier = asking(&[A]);
        barrier.push(None, "later");
        assert_eq!(barrier.start(0, true), None, "round 1 is still asking");
        barrier.answered(1, A);
        assert_eq!(barrier.finish(true), ["w"]);
        assert_eq!(barrier.start(0, true), Some(2));
        barrier.queried(2, vec![A]);
        barrier.answered(2, A);
        assert_eq!(barrier.finish(true), ["later"]);
    }
}
