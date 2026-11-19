//! The waiters for a window's show (Dickson, 2026-11-19): Core's
//! `AwaitShown` requests, each kept on the listener's `WinEvent` thread,
//! the thread that hears every show and destruction, from the request
//! until its one answer. A waiter is the request's own record and exists
//! only while the request is out: the listener learns which window is
//! awaited from Core's request alone, and keeps no list of its own.
//!
//! The table judges no window: the thread reads whether the window is
//! visible when the request arrives ([`Visibility`]), and hands it each
//! window object's show and destruction. The answer goes through the
//! waiter's token, its one answer path: a token dropped unanswered, as
//! when the table is dropped at the listener's shutdown, answers
//! cancelled itself.

use verbatim_model::WindowHandle;
use verbatim_model::request::RequestNumber;

/// The awaited window as the `WinEvent` thread read it when the request
/// arrived.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Visibility {
    /// `IsWindowVisible` is true.
    Visible,
    /// A window, not visible.
    Hidden,
    /// No window (`IsWindow` is false).
    Gone,
}

/// How a waiter is answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shown {
    /// The window is shown.
    Shown,
    /// The window is gone: no window when asked, or destroyed while
    /// awaited.
    NotThere,
    /// Core cancelled the request.
    Cancelled,
}

/// One waiter: Core's request, the window it awaits, and its answer
/// token.
#[derive(Debug)]
struct Waiter<T> {
    number: RequestNumber,
    window: WindowHandle,
    token: T,
}

/// The waiters, in the order their requests arrived. `T` is the answer
/// token, which the listener writes to Core's pipe.
#[derive(Debug)]
pub struct Waiters<T> {
    waiting: Vec<Waiter<T>>,
}

impl<T> Default for Waiters<T> {
    fn default() -> Self {
        Self {
            waiting: Vec::new(),
        }
    }
}

impl<T> Waiters<T> {
    /// Core's request `number` for `window`'s show, its token `token`,
    /// with `window` as the thread read it on the request's arrival: a
    /// visible window is answered shown and one that is no window not
    /// there, at once, handing back the token with its answer; a hidden
    /// one is kept.
    pub fn request(
        &mut self,
        number: RequestNumber,
        window: WindowHandle,
        visibility: Visibility,
        token: T,
    ) -> Option<(T, Shown)> {
        match visibility {
            Visibility::Visible => Some((token, Shown::Shown)),
            Visibility::Gone => Some((token, Shown::NotThere)),
            Visibility::Hidden => {
                self.waiting.push(Waiter {
                    number,
                    window,
                    token,
                });
                None
            }
        }
    }

    /// A window object's show of `window`, whatever its class: every
    /// waiter for it is answered shown, and ends.
    pub fn shown(&mut self, window: WindowHandle) -> Vec<(T, Shown)> {
        self.end_for(window, Shown::Shown)
    }

    /// A destruction of `window`'s window object: every waiter for it is
    /// answered not there, and ends.
    pub fn destroyed(&mut self, window: WindowHandle) -> Vec<(T, Shown)> {
        self.end_for(window, Shown::NotThere)
    }

    /// Core cancelled request `number`: its waiter, if it is one, is
    /// answered cancelled, and ends.
    pub fn cancel(&mut self, number: RequestNumber) -> Option<(T, Shown)> {
        let at = self
            .waiting
            .iter()
            .position(|waiter| waiter.number == number)?;
        Some((self.waiting.remove(at).token, Shown::Cancelled))
    }

    /// Whether no waiter is kept: a show then costs one comparison and no
    /// search.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.waiting.is_empty()
    }

    fn end_for(&mut self, window: WindowHandle, answer: Shown) -> Vec<(T, Shown)> {
        if self.waiting.is_empty() {
            return Vec::new();
        }
        let (ended, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.waiting)
            .into_iter()
            .partition(|waiter| waiter.window == window);
        self.waiting = kept;
        ended
            .into_iter()
            .map(|waiter| (waiter.token, answer))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: WindowHandle = WindowHandle(70);
    const OTHER: WindowHandle = WindowHandle(71);

    /// A request's token, by its number.
    #[derive(Debug, PartialEq, Eq)]
    struct Token(u64);

    fn number(raw: u64) -> RequestNumber {
        RequestNumber(raw)
    }

    /// The table holding request `raw`'s waiter for `W`, hidden.
    fn holding(raw: u64) -> Waiters<Token> {
        let mut waiters = Waiters::default();
        assert_eq!(
            waiters.request(number(raw), W, Visibility::Hidden, Token(raw)),
            None
        );
        waiters
    }

    #[test]
    fn a_visible_window_is_answered_shown_at_once() {
        let mut waiters = Waiters::default();
        assert_eq!(
            waiters.request(number(1), W, Visibility::Visible, Token(1)),
            Some((Token(1), Shown::Shown))
        );
        assert!(waiters.is_empty());
    }

    #[test]
    fn no_window_is_answered_not_there_at_once() {
        let mut waiters = Waiters::default();
        assert_eq!(
            waiters.request(number(1), W, Visibility::Gone, Token(1)),
            Some((Token(1), Shown::NotThere))
        );
        assert!(waiters.is_empty());
    }

    #[test]
    fn a_hidden_window_keeps_its_waiter() {
        let waiters = holding(1);
        assert!(!waiters.is_empty());
    }

    #[test]
    fn the_windows_show_answers_its_waiter_shown_and_ends_it() {
        let mut waiters = holding(1);
        assert_eq!(waiters.shown(OTHER), []);
        assert_eq!(waiters.shown(W), [(Token(1), Shown::Shown)]);
        assert!(waiters.is_empty());
        assert_eq!(waiters.shown(W), []);
    }

    #[test]
    fn the_windows_destruction_answers_its_waiter_not_there() {
        let mut waiters = holding(1);
        assert_eq!(waiters.destroyed(OTHER), []);
        assert_eq!(waiters.destroyed(W), [(Token(1), Shown::NotThere)]);
        assert!(waiters.is_empty());
    }

    #[test]
    fn a_cancel_answers_its_waiter_cancelled() {
        let mut waiters = holding(1);
        assert_eq!(waiters.cancel(number(2)), None);
        assert_eq!(
            waiters.cancel(number(1)),
            Some((Token(1), Shown::Cancelled))
        );
        assert!(waiters.is_empty());
        assert_eq!(waiters.cancel(number(1)), None);
    }

    /// Two requests for one window, as a re-take beside a focus's read:
    /// one show answers both, in their order, and leaves another window's.
    #[test]
    fn one_show_answers_every_waiter_for_its_window() {
        let mut waiters = holding(1);
        assert_eq!(
            waiters.request(number(2), W, Visibility::Hidden, Token(2)),
            None
        );
        assert_eq!(
            waiters.request(number(3), OTHER, Visibility::Hidden, Token(3)),
            None
        );
        assert_eq!(
            waiters.shown(W),
            [(Token(1), Shown::Shown), (Token(2), Shown::Shown)]
        );
        assert_eq!(
            waiters.cancel(number(3)),
            Some((Token(3), Shown::Cancelled))
        );
    }
}
