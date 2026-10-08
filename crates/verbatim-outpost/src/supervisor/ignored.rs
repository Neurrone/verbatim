//! Processes Verbatim was told to ignore entirely: the end-to-end harness
//! names the owner's own Windows Terminal, which must never be read while a
//! test runs on the owner's desktop (`phase6-design.md`, "Windows Terminal
//! crash of 2026-10-08"). Each is held open from the moment it is named, so
//! its pid names that process and no other for as long as Verbatim runs,
//! and a process that later reuses the pid of one that exited is never
//! ignored by mistake.

#![forbid(unsafe_code)]

use verbatim_model::Pid;

use super::process::Target;

/// The environment variable the harness sets, before Verbatim starts, to
/// the pids of the processes to ignore, separated by commas. Real users
/// never set it, so for them nothing is ignored.
pub const IGNORE_PIDS_ENV: &str = "VERBATIM_IGNORE_PIDS";

/// The command-line option that hands the focus listener the same pids.
pub(crate) const IGNORE_PIDS_ARG: &str = "--ignore-pids";

/// Processes to ignore entirely: no fact from them is routed, no outpost is
/// started for them, and Core never takes them as its focus or attention.
#[derive(Debug, Default)]
pub struct IgnoredProcesses {
    held: Vec<(Pid, Target)>,
}

impl IgnoredProcesses {
    /// Holds each process `pids` names that is running now; one that has
    /// exited, or cannot be opened, is left out and logged, as it can
    /// raise no event.
    #[must_use]
    pub fn hold(pids: impl IntoIterator<Item = Pid>) -> Self {
        let mut held: Vec<(Pid, Target)> = Vec::new();
        for pid in pids {
            if held.iter().any(|(known, _)| *known == pid) {
                continue;
            }
            match Target::open(pid) {
                Ok(target) => held.push((pid, target)),
                Err(not_held) => {
                    tracing::info!(%pid, %not_held, "a process to ignore is not held");
                }
            }
        }
        if !held.is_empty() {
            let pids: Vec<u32> = held.iter().map(|(pid, _)| pid.0).collect();
            tracing::info!(?pids, "these processes are ignored entirely");
        }
        Self { held }
    }

    /// The processes [`IGNORE_PIDS_ENV`] names in this process's
    /// environment, held; none when it is unset.
    #[must_use]
    pub fn from_env() -> Self {
        match std::env::var(IGNORE_PIDS_ENV) {
            Ok(list) => Self::hold(parse_pids(&list)),
            Err(_) => Self::default(),
        }
    }

    /// Whether `pid` names an ignored process.
    #[must_use]
    pub fn contains(&self, pid: Pid) -> bool {
        self.held.iter().any(|(held, _)| *held == pid)
    }

    /// Whether nothing is ignored.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.held.is_empty()
    }

    /// The ignored pids, as the listener's command line takes them:
    /// separated by commas.
    pub(crate) fn list(&self) -> String {
        self.held
            .iter()
            .map(|(pid, _)| pid.0.to_string())
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// The pids in `list`, separated by commas; anything that is not a pid is
/// skipped.
#[must_use]
pub fn parse_pids(list: &str) -> Vec<Pid> {
    list.split(',')
        .filter_map(|pid| pid.trim().parse().ok())
        .filter(|&pid| pid != 0)
        .map(Pid)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_of_pids_parses_and_skips_what_is_not_one() {
        assert_eq!(parse_pids(" 12,x,,0, 345 "), vec![Pid(12), Pid(345)]);
    }

    #[test]
    fn this_process_is_held_and_ignored_and_others_are_not() {
        let own = Pid(std::process::id());
        let ignored = IgnoredProcesses::hold([own, own]);
        assert!(ignored.contains(own));
        assert_eq!(ignored.list(), own.0.to_string());
        assert!(!ignored.contains(Pid(own.0 + 4)));
    }
}
