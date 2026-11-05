//! A fact binary's command line:
//!
//! - `--arm <name>`, repeatable: run only the named arms (all when none is
//!   named);
//! - `--trials <n>`: the trials of a rate, in place of the recorded n;
//! - `--pause`: before each arm's first call, print each process id and
//!   wait for Enter, so that a debugger can attach;
//! - `--role <name> <arguments>`: run as a second client process
//!   ([`crate::role`]).

/// The parsed command line.
#[derive(Default)]
pub struct Args {
    arms: Vec<String>,
    /// `--trials`, when given.
    pub trials: Option<usize>,
    /// `--pause`.
    pub pause: bool,
    /// `--role` and its arguments, when this process is a second client.
    pub role: Option<(String, Vec<String>)>,
}

impl Args {
    /// Parses this process's command line.
    ///
    /// # Panics
    ///
    /// On an unknown argument or a missing value.
    #[must_use]
    pub fn parse() -> Self {
        let mut args = Self::default();
        let mut words = std::env::args().skip(1);
        while let Some(word) = words.next() {
            match word.as_str() {
                "--arm" => args.arms.push(words.next().expect("--arm takes a name")),
                "--trials" => {
                    args.trials = Some(
                        words
                            .next()
                            .and_then(|n| n.parse().ok())
                            .expect("--trials takes a count"),
                    );
                }
                "--pause" => args.pause = true,
                "--role" => {
                    let name = words.next().expect("--role takes a name");
                    args.role = Some((name, words.by_ref().collect()));
                }
                other => panic!("unknown argument {other}"),
            }
        }
        args
    }

    /// Whether arm `name` runs.
    #[must_use]
    pub fn wants(&self, name: &str) -> bool {
        self.arms.is_empty() || self.arms.iter().any(|arm| arm == name)
    }

    /// The trials of a rate whose recorded n is `recorded`.
    #[must_use]
    pub fn trials_or(&self, recorded: usize) -> usize {
        self.trials.unwrap_or(recorded)
    }

    /// Under `--pause`, prints each process id with its role and waits for
    /// Enter.
    pub fn pause_for_debugger(&self, processes: &[(&str, u32)]) {
        if !self.pause {
            return;
        }
        for (role, pid) in processes {
            println!("pause: {role} is process {pid}");
        }
        println!("pause: attach the debugger, then press Enter");
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
    }
}
