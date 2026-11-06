//! A fact binary's command line:
//!
//! - `--case <name>`, repeatable: run only the named cases (all when none is
//!   named);
//! - `--trials <n>`: repeat each claim n times, its verdict then needing
//!   all n to agree; once by default, and a rate keeps its recorded n;
//! - `--recovery` and `--coalescing`: turn the client objects' connection
//!   recovery and event coalescing on; both are off by default;
//! - `--verbose`: print each traced UIA call with its time and result,
//!   the provider's call and message log, and each trial's figures
//!   ([`crate::note`]);
//! - `--pause`: before each case's first call, print each process id and
//!   wait for Enter, so that a debugger can attach;
//! - `--role <name> <arguments>`: run as a second client process
//!   ([`crate::role`]).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// `--pause`.
static PAUSE: AtomicBool = AtomicBool::new(false);

/// The pause line after which a paused process waits.
pub const PAUSE_WAIT: &str = "pause: attach the debugger, then press Enter";

/// Whether `--pause` was given; a second client process is given it too.
#[must_use]
pub fn pausing() -> bool {
    PAUSE.load(Ordering::SeqCst)
}

/// Under `--pause`, prints each process id with its role and waits: for
/// Enter, or in a second client process for the run's `continue`, which
/// the run sends once Enter is pressed there ([`crate::role::Role::line`]).
pub fn pause_here(processes: &[(&str, u32)]) {
    if !pausing() {
        return;
    }
    let in_role = crate::note::in_role();
    for (role, pid) in processes {
        let line = format!("pause: {role} is process {pid}");
        if in_role {
            crate::role::report(&line);
        } else {
            println!("{line}");
        }
    }
    if in_role {
        crate::role::report(PAUSE_WAIT);
    } else {
        println!("{PAUSE_WAIT}");
    }
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
}

/// `--trials`, 1 when not given.
static TRIALS: AtomicUsize = AtomicUsize::new(1);

/// How many times each claim runs: `--trials`, or once. A rate keeps its
/// recorded n.
#[must_use]
pub fn trials() -> usize {
    TRIALS.load(Ordering::SeqCst)
}

/// The parsed command line.
#[derive(Default)]
pub struct Args {
    cases: Vec<String>,
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
        crate::desktop::trampoline_if_asked();
        let mut args = Self::default();
        let mut words = std::env::args().skip(1);
        while let Some(word) = words.next() {
            match word.as_str() {
                "--case" => args.cases.push(words.next().expect("--case takes a name")),
                "--trials" => {
                    let n: usize = words
                        .next()
                        .and_then(|n| n.parse().ok())
                        .expect("--trials takes a count");
                    assert!(n > 0, "--trials takes a count above 0");
                    TRIALS.store(n, Ordering::SeqCst);
                }
                "--pause" => {
                    args.pause = true;
                    PAUSE.store(true, Ordering::SeqCst);
                }
                "--verbose" => crate::note::set_verbose(),
                "--recovery" => {
                    let settings = crate::uia::settings();
                    crate::uia::use_settings(crate::uia::Settings {
                        recovery: true,
                        ..settings
                    });
                }
                "--coalescing" => {
                    let settings = crate::uia::settings();
                    crate::uia::use_settings(crate::uia::Settings {
                        coalescing: true,
                        ..settings
                    });
                }
                "--role" => {
                    let name = words.next().expect("--role takes a name");
                    crate::note::set_role();
                    // The parent reads the role's process id first
                    // ([`crate::role::Role::start`]).
                    crate::role::report(&format!("pid {}", std::process::id()));
                    args.role = Some((name, words.by_ref().collect()));
                }
                other => panic!("unknown argument {other}"),
            }
        }
        args
    }

    /// Whether case `name` was named with `--case`: the cases that take
    /// over the desktop run only then.
    #[must_use]
    pub fn names(&self, name: &str) -> bool {
        self.cases.iter().any(|case| case == name)
    }

    /// Whether case `name` runs.
    #[must_use]
    pub fn wants(&self, name: &str) -> bool {
        self.cases.is_empty() || self.cases.iter().any(|case| case == name)
    }

    /// Under `--pause`, prints each process id with its role and waits for
    /// Enter.
    pub fn pause_for_debugger(&self, processes: &[(&str, u32)]) {
        pause_here(processes);
    }
}
