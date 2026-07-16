//! Latency reporting for the M2 E2E harness.
//!
//! Every scenario runs Verbatim with `VERBATIM_TEST_AUDIO=null`
//! ([`crate::scenario::Scenario`]), which swaps in [`verbatim_audio::NullSink`]
//! — a device-free sink that still emits the `audio_started` tracing event
//! on an utterance's first (discarded) PCM write, so a timeline completes
//! with no sound card in the loop.
//!
//! What that does *not* mean is that every timeline completes. Focus
//! announcements are spoken at `Interrupt` priority, so each new focus
//! change cancels whatever is still speaking; an utterance cancelled before
//! its first PCM write never reaches audio and never gets an audio-start
//! time, exactly as `verbatim-inspect` documents ("an interrupted utterance
//! simply never gets the follow-up"). Walking a dialog quickly, as the M1
//! regression does, interrupts most of what it queues — so this module
//! requires that speech reached audio *at all*, and reports every timeline,
//! rather than demanding that none of them were interrupted.

use std::io;

use verbatim_control::client::{Client as ControlClient, ok_or_error};
use verbatim_control::protocol::{Frame, LatencyRecord, ReplyPayload, Request};

/// The M3 pipeline latency budget in milliseconds: the deterministic
/// event-observed-to-speech-queued latency, synth-independent (it ends
/// where synthesis begins). A breach is *reported* by the scenario runner —
/// a prominent warning line plus the per-scenario summary's measured
/// maxima — and never asserted.
///
/// Report-only is a recorded decision, made after two rounds of evidence
/// that a wall-clock assertion inside a shared, variably loaded VM measures
/// the host's scheduling, not Verbatim's code. An initial 50 ms budget (set
/// from lightly loaded runs measuring 1 to 8 ms) flaked at 58 ms on a
/// loaded run; raised to 200 ms, it flaked again at 277 ms during a full
/// audible suite run — and a dedicated rerun of the identical build
/// measured 3 ms. A real pipeline regression (a reducer accidentally doing
/// blocking I/O, say) still shows up unmistakably in the always-reported
/// maxima; the enforced budget returns in M8, as a tight number from
/// eSpeak's reference measurement in a controlled environment. The
/// end-to-end `OneCore` smoke number the roadmap also mentions is likewise
/// not enforced: observed audio latency ranges past 1300 ms for a long
/// utterance (synthesis time scales with text), too variable for any
/// stable threshold.
pub const PIPELINE_BUDGET_MS: u64 = 200;

/// The worst event-to-queue (pipeline) latency across `records`, or `None`
/// when no timeline recorded a queue time.
#[must_use]
pub fn max_event_to_queue_ms(records: &[LatencyRecord]) -> Option<u64> {
    records
        .iter()
        .filter_map(|record| {
            record
                .speech_queued_at_ms
                .map(|queued| queued.saturating_sub(record.event_observed_at_ms))
        })
        .max()
}

/// Fetches the most recent `last_n` latency timelines with no printing and
/// no assertion — the raw building block [`report`] wraps, and what
/// [`crate::scenario::Scenario::latency_snapshot`] calls for the registry's
/// generic, best-effort per-scenario summary (`crate::registry`'s own doc
/// comment): every scenario's run summary carries latency counts this way,
/// not just scenarios (like `m1_exit_regression`) that call [`report`]
/// themselves as part of what they assert.
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

/// Fetches the most recent `last_n` latency timelines ([`fetch`]), prints
/// one fact per line (trace id, event-to-queue and event-to-audio
/// milliseconds, and whether the utterance was interrupted before audio
/// began), and asserts that at least one timeline reached audio — proof the
/// whole path from an observed event to a playing utterance works end to
/// end.
///
/// # Errors
///
/// Returns an error if the request fails or the reply is not a `Latency`
/// reply.
///
/// # Panics
///
/// Panics if no returned record reached audio at all, which would mean
/// speech never made it out of the pipeline — but only under the capture
/// synthesizer's instant [`verbatim_audio::NullSink`], where reaching audio
/// is immediate. Under a real synthesizer (audible mode) this invariant does
/// not hold: a real voice takes long enough to start that this suite's pace
/// — each step waits only for an utterance to be *queued*, then moves focus,
/// interrupting it at `Interrupt` priority — legitimately interrupts every
/// utterance before its first sample plays. That is correct screen-reader
/// behavior, not a pipeline failure, so the assertion is skipped when
/// [`crate::scenario::is_audible`]; the timelines are still fetched and
/// printed.
pub fn report(control: &mut ControlClient, last_n: u32) -> io::Result<Vec<LatencyRecord>> {
    let records = fetch(control, last_n)?;

    let mut reached_audio = 0usize;
    for record in &records {
        let to_queue = record
            .speech_queued_at_ms
            .map(|queued| queued.saturating_sub(record.event_observed_at_ms));
        let to_audio = record
            .audio_started_at_ms
            .map(|audio| audio.saturating_sub(record.event_observed_at_ms));
        if to_audio.is_some() {
            reached_audio += 1;
        }
        println!(
            "latency trace {} event-to-queue {} ms event-to-audio {}",
            record.trace_id,
            to_queue.map_or_else(|| "?".to_owned(), |ms| ms.to_string()),
            to_audio.map_or_else(
                || "(interrupted before audio)".to_owned(),
                |ms| format!("{ms} ms")
            ),
        );
    }
    println!(
        "latency: {reached_audio} of {} timelines reached audio; the rest were interrupted by a later announcement",
        records.len()
    );
    // See this function's doc comment: reaching audio is a capture-synth
    // invariant, not a real-synth one, so the assertion is capture-mode only.
    if !crate::scenario::is_audible() {
        assert!(
            reached_audio > 0 || records.is_empty(),
            "no utterance reached audio in {} timelines; speech never left the pipeline",
            records.len()
        );
    }
    Ok(records)
}
