//! The one small driver that sends an outpost Core's requests, for
//! everything that runs an outpost without Core: the development attach
//! mode (`verbatim-outpost --attach`) and mockapp's integration harness
//! (`docs/design/readers-tell-core-asks-build1.md`, "The outpost in this
//! build"). Since an outpost hooks nothing of its own but a console host's
//! console events, it reads only what it is asked; the driver numbers each
//! request as Core's request table would and hands it to the outpost as
//! the I/O thread hands it a `CoreToOutpost::Request`. Its answers come
//! back on the outpost's outbound pipe as `OutpostToCore::Answer`, which
//! the driver's owner reads.

#![forbid(unsafe_code)]

use verbatim_model::WindowHandle;
use verbatim_model::property::{PROPERTIES, PropertyId};
use verbatim_model::protocol::CoreToOutpost;
use verbatim_model::request::{Ask, Cancel, CancelReason, Request, RequestNumber, WindowVerdict};

use crate::outpost::Outpost;

/// Sends requests to one outpost, numbering them from 1.
pub struct Driver<'a> {
    outpost: &'a Outpost,
    last: u64,
}

impl<'a> Driver<'a> {
    /// A driver of `outpost`, whose first request is number 1.
    #[must_use]
    pub fn new(outpost: &'a Outpost) -> Self {
        Self::after(outpost, RequestNumber(0))
    }

    /// A driver of `outpost` that goes on numbering after `last`, the last
    /// request an earlier driver of it sent.
    #[must_use]
    pub fn after(outpost: &'a Outpost, last: RequestNumber) -> Self {
        Self {
            outpost,
            last: last.0,
        }
    }

    /// The number of the last request sent, `RequestNumber(0)` before any.
    #[must_use]
    pub fn last(&self) -> RequestNumber {
        RequestNumber(self.last)
    }

    /// Sends `request` about `window`, whose backend verdict is `verdict`
    /// (`WindowVerdict::ProbeFirst` when none is in force), and returns its
    /// number, which its answer carries.
    pub fn ask(
        &mut self,
        window: WindowHandle,
        verdict: WindowVerdict,
        request: Request,
    ) -> RequestNumber {
        self.last = self.last.saturating_add(1);
        let number = RequestNumber(self.last);
        self.outpost
            .handle_command(&CoreToOutpost::Request(Box::new(Ask {
                number,
                window,
                verdict,
                request: request.into(),
            })));
        number
    }

    /// Sends `ask`, a request Core's request table numbered itself, as the
    /// I/O thread hands it on: for a harness that runs Core, whose
    /// numbers this driver's own must not be mixed with.
    pub fn send(&self, ask: Ask) {
        self.outpost
            .handle_command(&CoreToOutpost::Request(Box::new(ask)));
    }

    /// Cancels request `number` for `reason`.
    pub fn cancel(&self, number: RequestNumber, reason: CancelReason) {
        self.outpost
            .handle_command(&CoreToOutpost::Cancel(Cancel { number, reason }));
    }

    /// Asks for the application's current focus with every property and
    /// every text detail, no ancestors known, as Core asks it at its start
    /// with every setting on: the read the attach mode prints.
    pub fn current_focus(&mut self, window: WindowHandle) -> RequestNumber {
        self.ask(
            window,
            WindowVerdict::ProbeFirst,
            Request::CurrentFocus {
                hidden: verbatim_model::request::HiddenRoot::Check,
                set: every_property(),
                known: Vec::new(),
                details: verbatim_model::TextDetails::ALL,
                depth: verbatim_model::request::FocusDepth::Full,
            },
        )
    }
}

/// Every property of the shared table.
#[must_use]
pub fn every_property() -> Vec<PropertyId> {
    PROPERTIES.iter().map(|entry| entry.id).collect()
}
