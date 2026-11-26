//! Typed requests and their outcomes: each request names the type of its
//! answer, which the queue decodes once, when the answer arrives, so an
//! entry's function sees only its own reply type (ADR 0003, "Core's code
//! is async functions").

use std::fmt::Debug;

use serde::{Deserialize, Serialize};
use verbatim_model::request::{CancelReason, Front, Question, Refusal, Reply};

use super::{Reader, RequestWindow};

/// When a reader's worker ran a request, as its answer stamps it
/// (`verbatim_model::request::Answer`), in microseconds on the run's shared
/// clock: 0 for both for an ending no worker made, Core's own or a
/// reader's refusal.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Stamps {
    /// When the worker began it.
    pub(crate) began_us: u64,
    /// When the worker ended it.
    pub(crate) ended_us: u64,
}

/// A request an entry's function can ask, with the type of its result.
pub(crate) trait Request: Debug + Unpin + 'static {
    /// What a result of this request is, decoded.
    type Reply: Debug + 'static;
    /// The reader the request goes to.
    fn reader(&self) -> Reader;
    /// The window it names, which the shell checks for a hang and the
    /// foreground rule tests.
    fn window(&self) -> RequestWindow;
    /// The request as it is sent.
    fn wire(&self) -> Question;
    /// A result as it arrived, with when its reader's worker ran it
    /// (`ran`), decoded against the request sent. The
    /// reply type holds every result a correct reader can answer the
    /// request with, its failures included (a text request's
    /// [`verbatim_model::TextFailure`], a batch's step outcomes), so
    /// `None` is only for a malformed answer: a result of another
    /// request's kind or shape.
    fn decode(&self, reply: Reply, ran: Stamps) -> Option<Self::Reply>;

    /// The shell's answer to a read of an application's current focus it
    /// refused because the window was hung, what it read in front,
    /// decoded: only that read's reply holds it, so for any other request
    /// it is `None`, a defect, logged as an undecodable answer.
    fn decode_in_front(&self, front: Front) -> Option<Self::Reply> {
        let _ = front;
        None
    }
}

/// Why a request was refused, as the function that asked it learns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Refused {
    /// The reader refused it, with its reason.
    Reader(Refusal),
    /// The reader answered a result that does not decode as the request's
    /// reply, a malformed answer: logged, and the reader is not replaced
    /// (Dickson, 2026-11-14).
    Undecodable,
}

/// How one request ended, as the function that asked it learns: the five
/// outcomes of ADR 0003, or the sixth that only Core gives, "no longer
/// awaited", with the reason Core stopped waiting or withdrew it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Outcome<T> {
    /// A result.
    Result(T),
    /// Refused, with a reason.
    Refused(Refused),
    /// The object is not there.
    NotThere,
    /// Not responding.
    NotResponding,
    /// The reader ended or stopped.
    ReaderEnded,
    /// Core no longer awaits it.
    NoLongerAwaited(CancelReason),
}

impl<T> Outcome<T> {
    /// The outcome, its result turned by `turn`.
    pub(crate) fn map<U>(self, turn: impl FnOnce(T) -> U) -> Outcome<U> {
        match self.split() {
            Ok(answer) => Outcome::Result(turn(answer)),
            Err(other) => other,
        }
    }

    /// The result, or the outcome that is not one, as an outcome of any
    /// request.
    pub(crate) fn split<U>(self) -> Result<T, Outcome<U>> {
        match self {
            Self::Result(answer) => Ok(answer),
            Self::Refused(refused) => Err(Outcome::Refused(refused)),
            Self::NotThere => Err(Outcome::NotThere),
            Self::NotResponding => Err(Outcome::NotResponding),
            Self::ReaderEnded => Err(Outcome::ReaderEnded),
            Self::NoLongerAwaited(reason) => Err(Outcome::NoLongerAwaited(reason)),
        }
    }
}
