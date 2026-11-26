//! The worker generation in charge and the program it runs: one program at
//! a time, numbered within its generation, so that a dead generation's
//! and an earlier program's results are refused. A program runs until it
//! ends; nothing here times it. A generation is abandoned only when Core
//! cancels the request its program runs because the foreground moved away
//! (`books/dev/src/reference/crates/verbatim-outpost-logic.md`, "Core's requests").

/// One request of one worker generation: what a worker's results name, so
/// that a dead generation's and an earlier request's are refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Request {
    /// The worker generation the request belongs to.
    pub generation: u64,
    /// The request's number within its generation.
    pub number: u64,
}

/// An abandonment: the generation in charge died, and the next took
/// charge.
#[derive(Debug)]
pub struct Abandoned {
    /// The generation that died.
    pub dead: u64,
    /// The generation now in charge.
    pub replacement: u64,
}

/// The generation in charge and its program, if one runs.
#[derive(Clone, Debug)]
pub struct InCharge<T> {
    generation: u64,
    requests: u64,
    running: Option<(Request, T)>,
}

impl<T> Default for InCharge<T> {
    fn default() -> Self {
        Self {
            generation: 0,
            requests: 0,
            running: None,
        }
    }
}

impl<T> InCharge<T> {
    /// Starts a program, keeping `item` with it, and returns its request.
    pub fn start(&mut self, item: T) -> Request {
        debug_assert!(self.running.is_none(), "a program started over another");
        let request = self.next_request();
        self.running = Some((request, item));
        request
    }

    /// Ends `request`'s program, returning its item; `None` when it is not
    /// the one running.
    pub fn end(&mut self, request: Request) -> Option<T> {
        let (_, item) = self.running.take_if(|(running, _)| *running == request)?;
        Some(item)
    }

    /// Abandons the generation in charge with its program: the next
    /// generation takes charge. `None` with no program running.
    pub fn abandon(&mut self) -> Option<Abandoned> {
        self.running.take()?;
        let dead = self.generation;
        self.generation = self.generation.saturating_add(1);
        Some(Abandoned {
            dead,
            replacement: self.generation,
        })
    }

    /// Whether a result of `request` is taken: it is the program running.
    #[must_use]
    pub fn accepts(&self, request: Request) -> bool {
        self.running
            .as_ref()
            .is_some_and(|(running, _)| *running == request)
    }

    /// The generation in charge.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The program running and its item.
    #[must_use]
    pub fn running(&self) -> Option<(Request, &T)> {
        self.running
            .as_ref()
            .map(|(request, item)| (*request, item))
    }

    /// The program running and its item, to change.
    pub fn running_mut(&mut self) -> Option<(Request, &mut T)> {
        self.running
            .as_mut()
            .map(|(request, item)| (*request, item))
    }

    /// A new request of the generation in charge, for a program that is
    /// not kept running.
    pub fn next_request(&mut self) -> Request {
        self.requests = self.requests.saturating_add(1);
        Request {
            generation: self.generation,
            number: self.requests,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only the program running's results are taken; an abandonment moves
    /// charge to the next generation, whose requests a dead one's never
    /// equal.
    #[test]
    fn only_the_running_programs_results_are_taken_and_abandonment_moves_charge() {
        let mut charge = InCharge::default();
        let first = charge.start("first");
        assert!(charge.accepts(first));
        let abandoned = charge.abandon().expect("a program runs");
        assert_eq!((abandoned.dead, abandoned.replacement), (0, 1));
        assert!(!charge.accepts(first));
        let second = charge.start("second");
        assert_eq!(second.generation, 1);
        assert_eq!(charge.end(first), None);
        assert_eq!(charge.end(second), Some("second"));
        assert!(charge.abandon().is_none());
    }
}
