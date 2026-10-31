//! The launch arguments the supervisor gives its children on their command
//! lines, defined once for the end that builds them and the end that
//! parses them: an outpost's options, the pids the focus listener ignores,
//! and the applications whose outposts have their own hooks in.

#![forbid(unsafe_code)]

use verbatim_model::{OutpostId, Pid};

/// How an outpost reads its application, fixed for its whole life. The
/// supervisor passes it on each outpost's command line, from
/// `settings.toml`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutpostOptions {
    /// Whether a UIA focus's ancestors are read with a remote operation
    /// where the window's provider supports one (`uia.remote_operations`
    /// in `settings.toml`, on by default); off forces the classic walk.
    pub remote_operations: bool,
}

impl Default for OutpostOptions {
    fn default() -> Self {
        Self {
            remote_operations: true,
        }
    }
}

/// The command-line option that hands the focus listener the pids of the
/// processes Verbatim ignores ([`crate::held::IgnoredProcesses`]).
pub const IGNORE_PIDS_ARG: &str = "--ignore-pids";

/// The command-line option that tells a listener the applications whose
/// outposts have their own hooks in.
pub const HOOKS_IN_ARG: &str = "--hooks-in";

/// The applications and outposts in `list`, as the listener's `--hooks-in`
/// option gives them: `pid:outpost` pairs separated by commas. Anything
/// that is not such a pair is skipped.
#[must_use]
pub fn parse_hooks_in(list: &str) -> Vec<(Pid, OutpostId)> {
    list.split(',')
        .filter_map(|pair| {
            let (pid, outpost) = pair.trim().split_once(':')?;
            Some((Pid(pid.parse().ok()?), OutpostId(outpost.parse().ok()?)))
        })
        .collect()
}

/// `hooks_in` as [`HOOKS_IN_ARG`] gives it.
#[must_use]
pub fn format_hooks_in(hooks_in: &[(Pid, OutpostId)]) -> String {
    hooks_in
        .iter()
        .map(|(pid, outpost)| format!("{}:{}", pid.0, outpost.0))
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_applications_with_hooks_in_round_trip_through_the_command_line() {
        let hooks_in = [(Pid(12), OutpostId(3)), (Pid(40), OutpostId(7))];
        assert_eq!(parse_hooks_in(&format_hooks_in(&hooks_in)), hooks_in);
        assert_eq!(parse_hooks_in(" 12:3,x,9, 40:7 "), hooks_in);
        assert_eq!(parse_hooks_in(""), Vec::<(Pid, OutpostId)>::new());
    }
}
