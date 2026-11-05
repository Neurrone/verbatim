//! A second client process: the fact binary started again with
//! `--role <name> <arguments>`, in a job object that kills it when the
//! parent drops it. It reports lines on its standard output and reads
//! commands, one a line, on its standard input. It ends with
//! [`crate::exit_now`], as mockapp's runner does, because of UIA's exit
//! fault.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use crate::job::Job;

/// A second client process.
pub struct Role {
    child: Child,
    stdin: Mutex<ChildStdin>,
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
        Self::start_on(None, name, args)
    }

    /// Starts this executable as role `name` with `args`, on desktop
    /// `desktop` when given ([`crate::desktop`]).
    ///
    /// # Panics
    ///
    /// When it cannot be started.
    #[must_use]
    pub fn start_on(desktop: Option<&str>, name: &str, args: &[String]) -> Self {
        let exe = std::env::current_exe().expect("this executable's path");
        let mut words: Vec<String> = crate::uia::settings()
            .flags()
            .into_iter()
            .map(str::to_owned)
            .collect();
        words.extend(crate::note::verbose().then(|| "--verbose".to_owned()));
        words.extend(crate::args::pausing().then(|| "--pause".to_owned()));
        words.push("--role".to_owned());
        words.push(name.to_owned());
        words.extend(args.iter().cloned());
        if let Some(desktop) = desktop {
            words = crate::desktop::wrap(desktop, &exe, &words);
        }
        let mut child = Command::new(exe)
            .args(words)
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
            stdin: Mutex::new(stdin),
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
        self.write(command);
    }

    fn write(&self, command: &str) {
        let mut stdin = self
            .stdin
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        writeln!(stdin, "{command}").expect("the second client takes commands");
        stdin.flush().expect("the second client takes commands");
    }

    /// The next line it reports, or `None` within `limit`. Under
    /// `--pause`, its pause lines are printed instead, and once it waits,
    /// Enter here lets it go on ([`crate::args::pause_here`]).
    #[must_use]
    pub fn line(&self, limit: Duration) -> Option<String> {
        loop {
            let text = self.lines.recv_timeout(limit).ok()?;
            if text == crate::args::PAUSE_WAIT {
                println!("{text}");
                let mut enter = String::new();
                let _ = std::io::stdin().read_line(&mut enter);
                self.write("continue");
            } else if text.starts_with("pause: ") {
                println!("{text}");
            } else {
                return Some(text);
            }
        }
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

impl Role {
    /// Waits up to `limit` for the process to end; answers its exit code,
    /// or `None` when it is still running.
    ///
    /// # Panics
    ///
    /// When its state cannot be read.
    #[must_use]
    pub fn wait(&mut self, limit: Duration) -> Option<i32> {
        let start = std::time::Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().expect("the process's state") {
                return Some(status.code().unwrap_or(-1));
            }
            if start.elapsed() >= limit {
                return None;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
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
