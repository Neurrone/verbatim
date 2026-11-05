//! A second client process: the fact binary started again with
//! `--role <name> <arguments>`, in a job object that kills it when the
//! parent drops it. It reports lines on its standard output and reads
//! commands, one a line, on its standard input. It ends with
//! [`crate::exit_now`], as mockapp's runner does, because of UIA's exit
//! fault.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use crate::job::Job;

/// A second client process.
pub struct Role {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    /// Its process id.
    pub pid: u32,
    _job: Job,
}

impl Role {
    /// Starts this executable as role `name` with `args`.
    ///
    /// # Panics
    ///
    /// When it cannot be started.
    #[must_use]
    pub fn start(name: &str, args: &[String]) -> Self {
        let exe = std::env::current_exe().expect("this executable's path");
        let mut child = Command::new(exe)
            .args(crate::uia::settings().flags())
            .args(crate::note::verbose().then_some("--verbose"))
            .arg("--role")
            .arg(name)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("the second client starts");
        let job = Job::holding(&child);
        let stdin = child.stdin.take().expect("its standard input");
        let stdout = child.stdout.take().expect("its standard output");
        let (line, lines) = channel();
        let role = name.to_owned();
        std::thread::spawn(move || {
            for text in BufReader::new(stdout).lines() {
                let Ok(text) = text else { break };
                crate::note::evidence(&format!("{role} process: {text}"));
                if line.send(text).is_err() {
                    break;
                }
            }
        });
        let pid = child.id();
        Self {
            child,
            stdin,
            lines,
            pid,
            _job: job,
        }
    }

    /// Sends one command line.
    ///
    /// # Panics
    ///
    /// When the process has ended.
    pub fn send(&mut self, command: &str) {
        writeln!(self.stdin, "{command}").expect("the second client takes commands");
        self.stdin
            .flush()
            .expect("the second client takes commands");
    }

    /// The next line it reports, or `None` within `limit`.
    #[must_use]
    pub fn line(&self, limit: Duration) -> Option<String> {
        self.lines.recv_timeout(limit).ok()
    }

    /// The next line it reports.
    ///
    /// # Panics
    ///
    /// When none comes within a minute.
    #[must_use]
    pub fn expect_line(&self) -> String {
        self.line(Duration::from_secs(60))
            .expect("the second client reports")
    }
}

impl Drop for Role {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// In a second client: writes one line to the parent.
pub fn report(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

/// In a second client: the parent's command lines, until it closes them.
pub fn commands() -> impl Iterator<Item = String> {
    std::io::stdin().lines().map_while(Result::ok)
}
