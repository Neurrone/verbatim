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

/// The flag of a listener's generation.
const GENERATION_ARG: &str = "--generation";

/// The command-line option that hands the focus listener the pids of the
/// processes Verbatim ignores ([`crate::held::IgnoredProcesses`]).
pub const IGNORE_PIDS_ARG: &str = "--ignore-pids";

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
    /// names its log (`--spare`; `books/dev/src/reference/adr/0001-focus.md`, "Spare outposts").
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
/// --pipe-out <handle> --generation <n>`, then
/// `--ignore-pids <pids>` when any process is ignored.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListenerArguments {
    /// The listener's generation, its launch's id: Core's views and
    /// requests name it, and the listener drops any naming another.
    pub generation: u64,
    /// The processes Verbatim ignores entirely, whose facts the listener
    /// drops.
    pub ignored: Vec<Pid>,
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
        let generation = self.generation;
        format!(
            "{LISTENER_ARG} --pipe-in {pipe_in} --pipe-out {pipe_out} {GENERATION_ARG} {generation}{ignore}"
        )
    }

    /// The arguments in `args`, the command line after the program's name;
    /// `None` when they do not begin with [`LISTENER_ARG`], are incomplete
    /// (the pipes and the generation are required), or carry an outpost's
    /// options.
    #[must_use]
    pub fn parse(args: &[String]) -> Option<(Pipes, Self)> {
        if args.first().map(String::as_str) != Some(LISTENER_ARG) {
            return None;
        }
        let mut pipe_in = None;
        let mut pipe_out = None;
        let mut generation = None;
        let mut arguments = Self::default();
        let mut index = 1;
        while index < args.len() {
            let value = args.get(index + 1).map_or("", String::as_str);
            match args[index].as_str() {
                "--pipe-in" => pipe_in = value.parse().ok(),
                "--pipe-out" => pipe_out = value.parse().ok(),
                GENERATION_ARG => generation = value.parse().ok(),
                IGNORE_PIDS_ARG => arguments.ignored = crate::held::parse_pids(value),
                "--target-pid" | "--spare" | "--classic-uia" | "--attach" => return None,
                _ => {}
            }
            index += 2;
        }
        arguments.generation = generation?;
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
                ListenerArguments {
                    generation: 1,
                    ignored: Vec::new(),
                },
                "--listener --pipe-in 412 --pipe-out 416 --generation 1",
            ),
            (
                ListenerArguments {
                    generation: 7,
                    ignored: vec![Pid(30), Pid(31)],
                },
                "--listener --pipe-in 412 --pipe-out 416 --generation 7 --ignore-pids 30,31",
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
            ListenerArguments::parse(&split(
                "--listener --pipe-in 412 --pipe-out 416 --generation 1 --spare 7"
            )),
            None
        );
        assert_eq!(
            ListenerArguments::parse(&split("--listener --pipe-in 412 --pipe-out 416")),
            None,
            "a listener's generation is required"
        );
    }
}
