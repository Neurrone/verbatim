//! Core's "tell me when this window is shown" (Dickson, 2026-11-19): a
//! focus read in a hidden dialog was answered with the dialog's window, and
//! Core asks to be told when that window is shown, so that the dialog's
//! text is read once there is any. The listener answers from the hook it
//! already has: the request goes to the `WinEvent` thread over a plain
//! channel and a nudge, the idle barrier's path, so the thread that hears
//! every show reads the window's visibility and keeps the waiter, in
//! order, with no lock on the hook path. A show sends Core nothing but the
//! answer, and wakes no thread the callback did not already run on.
//!
//! The waiter is the request's record ([`Waiters`]); its token is its one
//! answer path, which answers cancelled when dropped unanswered, as at the
//! listener's shutdown.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};

use verbatim_listener_logic::shown::{Shown, Visibility, Waiters};
use verbatim_model::WindowHandle;
use verbatim_model::request::{Answer, Cancel, Outcome, Reply, RequestNumber};

use super::Outgoing;
use super::asked::{handle, hwnd_of};

/// The listener's nudge that hands the `WinEvent` thread what waits on
/// its channel of shows, in the nudge's second word.
pub(super) const NUDGE_SHOWN: isize = 2;

/// What the pipe thread hands the `WinEvent` thread, in Core's order.
pub(super) enum ShownInput {
    /// Core asks to be told when `window` is shown; the token answers it.
    Request {
        window: WindowHandle,
        token: ShownToken,
    },
    /// Core cancelled one of its requests: an awaited show's is answered
    /// cancelled, any other's is not here.
    Cancel(Cancel),
}

/// The one answer path of a request for a window's show: answered once
/// ([`Self::answer`]), or cancelled when dropped unanswered.
pub(super) struct ShownToken {
    number: RequestNumber,
    outgoing: Option<Arc<Outgoing>>,
}

impl ShownToken {
    /// The token of request `number`, answering through `outgoing`.
    pub(super) fn new(number: RequestNumber, outgoing: &Arc<Outgoing>) -> Self {
        Self {
            number,
            outgoing: Some(Arc::clone(outgoing)),
        }
    }

    /// Answers the request `shown`.
    pub(super) fn answer(mut self, shown: Shown) {
        self.send(shown);
    }

    /// The request's number.
    pub(super) fn number(&self) -> RequestNumber {
        self.number
    }

    fn send(&mut self, shown: Shown) {
        let Some(outgoing) = self.outgoing.take() else {
            return;
        };
        let (outcome, detail) = match shown {
            Shown::Shown => (Outcome::Result(Reply::Shown), ""),
            Shown::NotThere => (Outcome::NotThere, ""),
            Shown::Cancelled => (Outcome::Ended, "cancelled"),
        };
        outgoing.answer(Answer {
            number: self.number,
            outcome,
            probes: Vec::new(),
            detail: detail.to_owned(),
        });
    }
}

impl Drop for ShownToken {
    fn drop(&mut self) {
        self.send(Shown::Cancelled);
    }
}

/// How the `WinEvent` thread reads a window's visibility when a request
/// for its show arrives, local calls that send it nothing.
pub(super) fn visibility(window: WindowHandle) -> Visibility {
    let hwnd = hwnd_of(window);
    if !verbatim_win32::window::window_exists(hwnd) {
        Visibility::Gone
    } else if verbatim_win32::window::window_is_visible(hwnd) {
        Visibility::Visible
    } else {
        Visibility::Hidden
    }
}

/// The waiters, on the `WinEvent` thread alone, shared by its handlers.
#[derive(Clone, Default)]
pub(super) struct OnEventThread {
    waiters: Rc<RefCell<Waiters<ShownToken>>>,
}

impl OnEventThread {
    /// Takes every input the pipe thread handed on `inputs`, in order,
    /// the window's visibility read by `visible` as each request is
    /// taken.
    pub(super) fn take(
        &self,
        inputs: &Receiver<ShownInput>,
        visible: impl Fn(WindowHandle) -> Visibility,
    ) {
        for input in inputs.try_iter() {
            match input {
                ShownInput::Request { window, token } => {
                    let number = token.number();
                    let answered =
                        self.waiters
                            .borrow_mut()
                            .request(number, window, visible(window), token);
                    if let Some((token, shown)) = answered {
                        token.answer(shown);
                    }
                }
                ShownInput::Cancel(cancel) => {
                    let answered = self.waiters.borrow_mut().cancel(cancel.number);
                    if let Some((token, shown)) = answered {
                        token.answer(shown);
                    }
                }
            }
        }
    }

    /// A window object's show of `window`, from the hook's second output.
    pub(super) fn shown(&self, window: isize) {
        if self.waiters.borrow().is_empty() {
            return;
        }
        let answered = self.waiters.borrow_mut().shown(handle(window));
        for (token, shown) in answered {
            token.answer(shown);
        }
    }

    /// A destruction of `window`'s window object.
    pub(super) fn destroyed(&self, window: isize) {
        if self.waiters.borrow().is_empty() {
            return;
        }
        let answered = self.waiters.borrow_mut().destroyed(handle(window));
        for (token, shown) in answered {
            token.answer(shown);
        }
    }
}

/// How many of the requests handed to the `WinEvent` thread the pipe thread
/// remembers, newest kept, so that only their cancels go there.
const HANDED_KEPT: usize = 16;

/// The pipe thread's end: hands an input to the `WinEvent` thread and
/// nudges it. It remembers the numbers of the requests it handed, so that a
/// cancel of any other request, a reading thread's, wakes no `WinEvent`
/// thread. The numbers are its own record of what it sent, judged by
/// nothing: a cancel of a request the thread has answered costs one nudge
/// that finds nothing, and one forgotten past [`HANDED_KEPT`] is answered
/// by the show, the destruction or the shutdown.
pub(super) struct ToEventThread {
    inputs: Sender<ShownInput>,
    /// The numbers of the requests handed, newest last; the pipe thread's
    /// alone.
    handed: std::cell::RefCell<std::collections::VecDeque<RequestNumber>>,
}

impl ToEventThread {
    /// The end sending on `inputs`.
    pub(super) fn new(inputs: Sender<ShownInput>) -> Self {
        Self {
            inputs,
            handed: std::cell::RefCell::default(),
        }
    }

    /// Hands Core's request for `window`'s show, answered by `token`, on,
    /// then nudges the thread with `nudge`. A thread that has ended dropped
    /// its end of the channel, and every input with it, so a request it
    /// never takes is answered cancelled as its token is dropped.
    pub(super) fn request(
        &self,
        (window, token): (WindowHandle, ShownToken),
        nudge: impl FnOnce() -> bool,
    ) {
        let mut handed = self.handed.borrow_mut();
        if handed.len() == HANDED_KEPT {
            handed.pop_front();
        }
        handed.push_back(token.number());
        drop(handed);
        self.hand(ShownInput::Request { window, token }, nudge);
    }

    /// Hands Core's `cancel` on, nudging the thread with `nudge`, only when
    /// it names a request handed to the thread; whether it did.
    pub(super) fn cancel(&self, cancel: Cancel, nudge: impl FnOnce() -> bool) -> bool {
        let mut handed = self.handed.borrow_mut();
        let Some(at) = handed.iter().position(|number| *number == cancel.number) else {
            return false;
        };
        handed.remove(at);
        drop(handed);
        self.hand(ShownInput::Cancel(cancel), nudge);
        true
    }

    fn hand(&self, input: ShownInput, nudge: impl FnOnce() -> bool) {
        if self.inputs.send(input).is_ok() {
            let _ = nudge();
        }
    }
}
