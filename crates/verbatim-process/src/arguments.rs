//! The launch arguments the supervisor gives its children on their command
//! lines, defined once for the end that builds them and the end that
//! parses them: an outpost's ([`OutpostArguments`]) and the focus
//! listener's ([`ListenerArguments`]), each with its command line and its
//! parse. Each executable's `main` parses its own and refuses the other's:
//! the listener's always begin with `--listener`, which an outpost refuses,
//! and the listener refuses an outpost's options.

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

/// The flag a listener's arguments begin with.
pub const LISTENER_ARG: &str = "--listener";

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

/// The values of the two pipe ends a child inherits, passed on its command
/// line as decimal: the one it reads commands from, then the one it writes
/// messages to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pipes {
    /// The end the child reads commands from.
    pub pipe_in: usize,
    /// The end the child writes messages to.
    pub pipe_out: usize,
}

/// What an outpost watches, fixed for its life (decision D9).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutpostTarget {
    /// One application, by its process id (`--target-pid`).
    Application(Pid),
    /// None yet: a spare, by the supervisor's number for its launch, which
    /// names its log (`--spare`; `docs/design/focus-pipeline.md`, section
    /// 19).
    Spare(OutpostId),
}

/// An outpost's launch arguments: `--pipe-in <handle> --pipe-out <handle>`,
/// then `--target-pid <pid>` or `--spare <launch>`, then `--classic-uia`
/// when remote operations are off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutpostArguments {
    /// What it watches.
    pub target: OutpostTarget,
    /// How it reads its application.
    pub options: OutpostOptions,
}

impl OutpostArguments {
    /// The command line, given the child's pipe ends.
    #[must_use]
    pub fn command_line(&self, pipes: Pipes) -> String {
        let Pipes { pipe_in, pipe_out } = pipes;
        let target = match self.target {
            OutpostTarget::Application(pid) => format!("--target-pid {}", pid.0),
            OutpostTarget::Spare(launch) => format!("--spare {}", launch.0),
        };
        let classic_uia = if self.options.remote_operations {
            ""
        } else {
            " --classic-uia"
        };
        format!("--pipe-in {pipe_in} --pipe-out {pipe_out} {target}{classic_uia}")
    }

    /// The arguments in `args`, the command line after the program's name;
    /// `None` when they are incomplete, name both an application and a
    /// spare, or are a listener's.
    #[must_use]
    pub fn parse(args: &[String]) -> Option<(Pipes, Self)> {
        let mut pipe_in = None;
        let mut pipe_out = None;
        let mut target_pid = None;
        let mut spare = None;
        let mut options = OutpostOptions::default();
        let mut index = 0;
        while index < args.len() {
            let value = args.get(index + 1);
            match args[index].as_str() {
                LISTENER_ARG => return None,
                "--pipe-in" => pipe_in = value.and_then(|value| value.parse().ok()),
                "--pipe-out" => pipe_out = value.and_then(|value| value.parse().ok()),
                "--target-pid" => target_pid = value.and_then(|value| value.parse().ok()),
                "--spare" => spare = value.and_then(|value| value.parse().ok()),
                // A bare flag, not a valued option: it consumes no
                // following argument.
                "--classic-uia" => {
                    options.remote_operations = false;
                    index += 1;
                    continue;
                }
                _ => {}
            }
            index += 2;
        }
        let target = match (target_pid, spare) {
            (Some(pid), None) => OutpostTarget::Application(Pid(pid)),
            (None, Some(launch)) => OutpostTarget::Spare(OutpostId(launch)),
            _ => return None,
        };
        Some((
            Pipes {
                pipe_in: pipe_in?,
                pipe_out: pipe_out?,
            },
            Self { target, options },
        ))
    }
}

/// The focus listener's launch arguments: `--listener --pipe-in <handle>
/// --pipe-out <handle>`, then `--ignore-pids <pids>` when any process is
/// ignored, then `--hooks-in <pairs>` when any application's outpost has
/// its own hooks in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListenerArguments {
    /// The processes Verbatim ignores entirely, whose facts the listener
    /// drops.
    pub ignored: Vec<Pid>,
    /// The applications whose outposts have their own `WinEvent` hooks in,
    /// whose MSAA facts the listener does not forward.
    pub hooks_in: Vec<(Pid, OutpostId)>,
}

impl ListenerArguments {
    /// The command line, given the child's pipe ends.
    #[must_use]
    pub fn command_line(&self, pipes: Pipes) -> String {
        let Pipes { pipe_in, pipe_out } = pipes;
        let ignore = if self.ignored.is_empty() {
            String::new()
        } else {
            let pids: Vec<String> = self.ignored.iter().map(|pid| pid.0.to_string()).collect();
            format!(" {IGNORE_PIDS_ARG} {}", pids.join(","))
        };
        let hooked = if self.hooks_in.is_empty() {
            String::new()
        } else {
            format!(" {HOOKS_IN_ARG} {}", format_hooks_in(&self.hooks_in))
        };
        format!("{LISTENER_ARG} --pipe-in {pipe_in} --pipe-out {pipe_out}{ignore}{hooked}")
    }

    /// The arguments in `args`, the command line after the program's name;
    /// `None` when they do not begin with [`LISTENER_ARG`], are incomplete,
    /// or carry an outpost's options.
    #[must_use]
    pub fn parse(args: &[String]) -> Option<(Pipes, Self)> {
        if args.first().map(String::as_str) != Some(LISTENER_ARG) {
            return None;
        }
        let mut pipe_in = None;
        let mut pipe_out = None;
        let mut arguments = Self::default();
        let mut index = 1;
        while index < args.len() {
            let value = args.get(index + 1).map_or("", String::as_str);
            match args[index].as_str() {
                "--pipe-in" => pipe_in = value.parse().ok(),
                "--pipe-out" => pipe_out = value.parse().ok(),
                IGNORE_PIDS_ARG => arguments.ignored = crate::held::parse_pids(value),
                HOOKS_IN_ARG => arguments.hooks_in = parse_hooks_in(value),
                "--target-pid" | "--spare" | "--classic-uia" | "--attach" => return None,
                _ => {}
            }
            index += 2;
        }
        Some((
            Pipes {
                pipe_in: pipe_in?,
                pipe_out: pipe_out?,
            },
            arguments,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PIPES: Pipes = Pipes {
        pipe_in: 412,
        pipe_out: 416,
    };

    /// `line` split as a child's command line is, after its program's name.
    fn split(line: &str) -> Vec<String> {
        line.split(' ').map(str::to_owned).collect()
    }

    #[test]
    fn the_applications_with_hooks_in_round_trip_through_the_command_line() {
        let hooks_in = [(Pid(12), OutpostId(3)), (Pid(40), OutpostId(7))];
        assert_eq!(parse_hooks_in(&format_hooks_in(&hooks_in)), hooks_in);
        assert_eq!(parse_hooks_in(" 12:3,x,9, 40:7 "), hooks_in);
        assert_eq!(parse_hooks_in(""), Vec::<(Pid, OutpostId)>::new());
    }

    #[test]
    fn an_outposts_arguments_round_trip_through_the_command_line() {
        for (arguments, line) in [
            (
                OutpostArguments {
                    target: OutpostTarget::Application(Pid(1234)),
                    options: OutpostOptions::default(),
                },
                "--pipe-in 412 --pipe-out 416 --target-pid 1234",
            ),
            (
                OutpostArguments {
                    target: OutpostTarget::Spare(OutpostId(7)),
                    options: OutpostOptions {
                        remote_operations: false,
                    },
                },
                "--pipe-in 412 --pipe-out 416 --spare 7 --classic-uia",
            ),
        ] {
            assert_eq!(arguments.command_line(PIPES), line);
            assert_eq!(
                OutpostArguments::parse(&split(line)),
                Some((PIPES, arguments))
            );
        }
    }

    #[test]
    fn a_listeners_arguments_round_trip_through_the_command_line() {
        for (arguments, line) in [
            (
                ListenerArguments::default(),
                "--listener --pipe-in 412 --pipe-out 416",
            ),
            (
                ListenerArguments {
                    ignored: vec![Pid(30), Pid(31)],
                    hooks_in: vec![(Pid(12), OutpostId(3))],
                },
                "--listener --pipe-in 412 --pipe-out 416 --ignore-pids 30,31 --hooks-in 12:3",
            ),
        ] {
            assert_eq!(arguments.command_line(PIPES), line);
            assert_eq!(
                ListenerArguments::parse(&split(line)),
                Some((PIPES, arguments))
            );
        }
    }

    #[test]
    fn each_executable_refuses_the_others_arguments() {
        let listener = ListenerArguments::default().command_line(PIPES);
        let outpost = OutpostArguments {
            target: OutpostTarget::Application(Pid(1234)),
            options: OutpostOptions::default(),
        }
        .command_line(PIPES);
        assert_eq!(OutpostArguments::parse(&split(&listener)), None);
        assert_eq!(ListenerArguments::parse(&split(&outpost)), None);
        assert_eq!(
            ListenerArguments::parse(&split("--listener --pipe-in 412 --pipe-out 416 --spare 7")),
            None
        );
    }
}
