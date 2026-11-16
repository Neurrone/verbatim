//! Core's supervisor's pure decisions (architecture section 1, decisions D9
//! and D13): the outpost generation record Core's reducer owns
//! (`record`), and when a listener or spare is stuck, how long a failed
//! outpost's replacement waits, and which spare outposts are kept and
//! given ([`policy`]). The supervisor's owner, in `verbatim-supervisor`,
//! reports the process facts, reads the clocks, and carries out what they
//! decide. Every time they take is in microseconds
//! on the run's shared clock (`verbatim_process::clock::now_us`), given
//! with the input.

pub mod policy;
pub(crate) mod record;

/// Why an outpost generation ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndReason {
    /// Its pipe closed: it exited or crashed.
    Exited,
    /// It was ended for not answering pings or for piling up abandoned
    /// workers: it was asked to shut down and killed if it did not exit in
    /// time.
    Killed,
    /// Its application exited, so the owner shut it down.
    TargetExited,
}

impl std::fmt::Display for EndReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            EndReason::Exited => "exited",
            EndReason::Killed => "killed",
            EndReason::TargetExited => "target exited",
        })
    }
}

/// When verbatim.exe read a fact from the listener, on the run's shared
/// clock, which the supervisor stamps the fact's relay with when it routes
/// it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Received {
    /// The moment, in microseconds on the run's shared clock.
    pub at_us: u64,
}
