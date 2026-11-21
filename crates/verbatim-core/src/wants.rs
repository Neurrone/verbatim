//! The requests Core's facts send as their own, outside the event queue,
//! in order with the queue's requests ([`crate::event_queue`]'s
//! `Logic::wanted`): one list and one token counter for every feature that
//! sends them, the focused terminal's reads and a progress bar's read.
//!
//! A request awaited under a token ends once, by the table's one answer
//! path, and its ending reaches `Logic::facts_answered` with the token,
//! which routes it to the feature that asked. The list is empty between
//! steps: whatever is wanted is taken as the queue settles, or by the
//! function that wanted it ([`crate::event_queue::Cx::send_wanted`]).

use serde::{Deserialize, Serialize};
use verbatim_model::request::Question;

use crate::event_queue::{CoreRequest, OTHER_DUE_US, Reader, RequestWindow};

/// A request Core's facts want sent: awaited under its token, or by
/// nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Wanted {
    token: Option<u64>,
    reader: Reader,
    window: RequestWindow,
    request: Question,
}

/// The requests Core's facts want sent, and the last token given.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Wants {
    /// The last token given to a request.
    tokens: u64,
    /// The requests wanted and not yet sent, in the order they were
    /// wanted.
    #[serde(skip)]
    wanted: Vec<Wanted>,
}

impl Wants {
    /// A new token.
    pub(crate) fn token(&mut self) -> u64 {
        self.tokens = self.tokens.saturating_add(1);
        self.tokens
    }

    /// Wants `request` of `reader`, naming `window`, awaited under `token`
    /// or by nothing.
    pub(crate) fn want(
        &mut self,
        token: Option<u64>,
        (reader, window): (Reader, RequestWindow),
        request: Question,
    ) {
        self.wanted.push(Wanted {
            token,
            reader,
            window,
            request,
        });
    }

    /// Whether requests are wanted.
    pub(crate) fn any(&self) -> bool {
        !self.wanted.is_empty()
    }

    /// The requests wanted, taken, each under Core's ordinary due time.
    pub(crate) fn take(&mut self) -> Vec<CoreRequest> {
        std::mem::take(&mut self.wanted)
            .into_iter()
            .map(|wanted| CoreRequest {
                token: wanted.token,
                reader: wanted.reader,
                window: wanted.window,
                request: wanted.request,
                due_after_us: OTHER_DUE_US,
            })
            .collect()
    }

    /// The requests wanted, taken, each with its token and what it asks,
    /// for the tests.
    #[cfg(test)]
    pub(crate) fn take_for_tests(&mut self) -> Vec<(Option<u64>, Question)> {
        std::mem::take(&mut self.wanted)
            .into_iter()
            .map(|wanted| (wanted.token, wanted.request))
            .collect()
    }
}
