//! Core's supervisor's pure decisions (architecture section 1, decisions D9
//! and D13): the outpost generations' typestates ([`generation`]), and when
//! an outpost is stuck, when an idle one is retired, how long a failed
//! outpost's replacement waits, how facts that arrive during a spawn are
//! held, which facts may start an outpost, and which spare outposts are
//! kept and given ([`policy`]). The supervisor's owner, in
//! `verbatim-supervisor`, gathers their inputs, reads the clocks, and
//! carries out what they decide.

use std::time::Instant;

pub mod generation;
pub mod policy;

/// Why an outpost generation ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndReason {
    /// Its pipe closed: it exited or crashed.
    Exited,
    /// The owner ended it for not answering pings or for piling up abandoned
    /// workers: it was asked to shut down and killed if it did not exit in
    /// time.
    Killed,
    /// The owner retired it: its application had not held attention for two
    /// minutes and Core held none of its nodes.
    Retired,
    /// Its application exited, so the owner shut it down.
    TargetExited,
}

impl std::fmt::Display for EndReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            EndReason::Exited => "exited",
            EndReason::Killed => "killed",
            EndReason::Retired => "retired",
            EndReason::TargetExited => "target exited",
        })
    }
}

/// When verbatim.exe read a fact from the listener: the monotonic clock's
/// instant and the run's shared clock's microseconds, read together, which
/// the supervisor stamps the fact's relay with when it routes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Received {
    /// The instant, on the monotonic clock.
    pub at: Instant,
    /// The same moment on the run's shared clock, in microseconds.
    pub at_us: u64,
}
