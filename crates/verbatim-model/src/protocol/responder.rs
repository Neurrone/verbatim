//! The answering side of a request within one process
//! (`docs/design/ownership.md`, "Queries have exactly one answer"): a
//! [`Responder`] answers its asker once, and a responder dropped unanswered
//! answers "gone", so no path that loses a request leaves its asker
//! waiting. Across processes a killed child runs no `Drop`, so there the
//! asking side's table answers instead.

/// Answers one request once: [`Responder::answer`] consumes it, and its
/// `Drop` answers `None`, "gone", unless it was answered.
#[must_use = "a responder dropped unanswered answers its asker \"gone\""]
pub struct Responder<T> {
    answer: Option<Box<dyn FnOnce(Option<T>) + Send>>,
}

impl<T> Responder<T> {
    /// A responder that hands its one answer to `answer`: `Some` with the
    /// answer, or `None` when it is dropped unanswered.
    pub fn new(answer: impl FnOnce(Option<T>) + Send + 'static) -> Self {
        Self {
            answer: Some(Box::new(answer)),
        }
    }

    /// Answers the request with `value`.
    pub fn answer(mut self, value: T) {
        if let Some(answer) = self.answer.take() {
            answer(Some(value));
        }
    }
}

impl<T> Drop for Responder<T> {
    fn drop(&mut self) {
        if let Some(answer) = self.answer.take() {
            answer(None);
        }
    }
}

impl<T> std::fmt::Debug for Responder<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Responder")
            .field("answered", &self.answer.is_none())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    /// The answers a responder gave, in order.
    type Answers = Arc<Mutex<Vec<Option<u32>>>>;

    /// A responder, and the answers it gave.
    fn recorded() -> (Responder<u32>, Answers) {
        let answers = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&answers);
        let responder = Responder::new(move |answer| kept.lock().expect("answers").push(answer));
        (responder, answers)
    }

    #[test]
    fn a_responder_answered_answers_once_and_its_drop_adds_nothing() {
        let (responder, answers) = recorded();
        responder.answer(7);
        assert_eq!(*answers.lock().expect("answers"), [Some(7)]);
    }

    #[test]
    fn a_responder_dropped_unanswered_answers_gone() {
        let (responder, answers) = recorded();
        drop(responder);
        assert_eq!(*answers.lock().expect("answers"), [None]);
    }
}
