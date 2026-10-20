//! Latency reporting for the M2 E2E harness.
//!
//! A non-audible run launches Verbatim with `VERBATIM_TEST_AUDIO=null`
//! ([`crate::scenario::Scenario`]), which plays through
//! [`verbatim_audio::SilentDevice`]: real time, no sound, so a timeline's
//! audio start is still measured at playback with no sound card in the
//! loop. An audible run ([`crate::scenario::AUDIBLE_ENV`]) plays through the
//! real device instead.
//!
//! What that does *not* mean is that every timeline completes. Focus
//! announcements are spoken at `Interrupt` priority, so each new focus
//! change cancels whatever is still speaking; an utterance cancelled before
//! its first frame plays never gets an audio-start time.

use std::io;

use verbatim_control::client::{Client as ControlClient, ok_or_error};
use verbatim_control::protocol::{Frame, LatencyRecord, ReplyPayload, Request};

/// Fetches the most recent `last_n` latency timelines with no printing and
/// no assertion: what [`crate::scenario::Scenario::latency_snapshot`] calls
/// for the registry's generic, best-effort per-scenario summary
/// (`crate::registry`'s own doc comment), so every scenario's run summary
/// carries latency counts.
///
/// # Errors
///
/// Returns an error if the request fails or the reply is not a `Latency`
/// reply.
pub fn fetch(control: &mut ControlClient, last_n: u32) -> io::Result<Vec<LatencyRecord>> {
    let frame = ok_or_error(control.request(Request::Latency { last_n })?)?;
    let Frame::Reply {
        payload: ReplyPayload::Latency(records),
        ..
    } = frame
    else {
        return Err(io::Error::other(format!(
            "unexpected reply to Latency: {frame:?}"
        )));
    };
    Ok(records)
}
