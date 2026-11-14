//! The async form's typed requests: each names the reader it goes to, its
//! wire form and the type of its answer, which the queue decodes once, when
//! the answer arrives, so an entry's function sees only its own reply type.

use std::convert::Infallible;
use std::fmt::Debug;

use super::super::harness::{self, Reader, Stop};

/// A request an entry's function can ask, with the type of its result.
pub(crate) trait Request: Debug + Unpin + 'static {
    /// What a result of this request is, decoded.
    type Reply: Debug + 'static;
    /// The reader the request goes to.
    fn reader(&self) -> Reader;
    /// The request as it is sent.
    fn wire(&self) -> harness::Request;
    /// The result as it arrived, decoded; `None` when the reader answered
    /// with a reply of another request, which is a protocol error.
    fn decode(reply: harness::Reply) -> Option<Self::Reply>;
}

/// How one request ended, as the function that asked it learns: the five
/// outcomes of ADR 0003 ("Readers tell, Core asks"), or Core's own give-up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Outcome<T> {
    /// A result.
    Result(T),
    /// Refused, with a reason.
    Refused(String),
    /// The object is not there.
    NotThere,
    /// Not responding.
    NotResponding,
    /// The reader ended or stopped.
    ReaderEnded,
    /// Core gave the request up and told its reader to cancel.
    GivenUp(Stop),
}

impl Outcome<Infallible> {
    /// This outcome, which has no result, as an outcome of any request.
    pub(crate) fn widen<T>(self) -> Outcome<T> {
        match self {
            Outcome::Result(never) => match never {},
            Outcome::Refused(reason) => Outcome::Refused(reason),
            Outcome::NotThere => Outcome::NotThere,
            Outcome::NotResponding => Outcome::NotResponding,
            Outcome::ReaderEnded => Outcome::ReaderEnded,
            Outcome::GivenUp(stop) => Outcome::GivenUp(stop),
        }
    }
}

/// The listener reads the element it kept for a focus tell.
#[derive(Debug)]
pub(crate) struct ElementRead {
    pub token: u32,
}

/// The element a focus tell named, read.
#[derive(Debug)]
pub(crate) struct Element {
    pub name: String,
    pub route: u32,
}

impl Request for ElementRead {
    type Reply = Element;

    fn reader(&self) -> Reader {
        Reader::Listener
    }

    fn wire(&self) -> harness::Request {
        harness::Request::FocusElement { token: self.token }
    }

    fn decode(reply: harness::Reply) -> Option<Element> {
        match reply {
            harness::Reply::Element { name, route } => Some(Element { name, route }),
            _ => None,
        }
    }
}

/// The application's outpost completes a focus read from the route.
#[derive(Debug)]
pub(crate) struct CompleteFocus {
    pub app: u32,
    pub route: u32,
}

/// A completed focus read.
#[derive(Debug)]
pub(crate) struct Completion {
    pub role: String,
}

impl Request for CompleteFocus {
    type Reply = Completion;

    fn reader(&self) -> Reader {
        Reader::Outpost(self.app)
    }

    fn wire(&self) -> harness::Request {
        harness::Request::CompleteFocus { route: self.route }
    }

    fn decode(reply: harness::Reply) -> Option<Completion> {
        match reply {
            harness::Reply::Completed { role } => Some(Completion { role }),
            _ => None,
        }
    }
}

/// The outpost reads the caret and holds it as a key's starting point.
#[derive(Debug)]
pub(crate) struct Caret {
    pub app: u32,
}

/// The caret, held by the outpost as the key's starting point.
#[derive(Debug)]
pub(crate) struct CaretPosition;

impl Request for Caret {
    type Reply = CaretPosition;

    fn reader(&self) -> Reader {
        Reader::Outpost(self.app)
    }

    fn wire(&self) -> harness::Request {
        harness::Request::Caret
    }

    fn decode(reply: harness::Reply) -> Option<CaretPosition> {
        match reply {
            harness::Reply::Caret => Some(CaretPosition),
            _ => None,
        }
    }
}

/// The outpost watches for a key's effect within the bound.
#[derive(Debug)]
pub(crate) struct Watch {
    pub app: u32,
    pub within_ms: u32,
}

/// The line the key moved to.
#[derive(Debug)]
pub(crate) struct Line {
    pub text: String,
}

impl Request for Watch {
    type Reply = Line;

    fn reader(&self) -> Reader {
        Reader::Outpost(self.app)
    }

    fn wire(&self) -> harness::Request {
        harness::Request::Watch {
            within_ms: self.within_ms,
        }
    }

    fn decode(reply: harness::Reply) -> Option<Line> {
        match reply {
            harness::Reply::Watched { line } => Some(Line { text: line }),
            _ => None,
        }
    }
}
