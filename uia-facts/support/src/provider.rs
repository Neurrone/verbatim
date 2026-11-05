//! The provider program's client side. [`Provider::launch`] starts
//! `uia-facts-provider.exe` from the fact binary's own folder, so a fact
//! always runs against the provider built with it, in a job object. Each
//! request waits for the provider's answer, which it gives once it has
//! acted, so a sample waits for evidence, not for time.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::Duration;

use crate::job::Job;

/// How long a request may take before the provider is taken to have
/// failed: longer than any hold a fact makes.
const ANSWER_LIMIT: Duration = Duration::from_secs(300);

/// One window of the provider program.
#[derive(Clone, Copy, Debug)]
pub struct Window {
    /// Its handle.
    pub hwnd: isize,
    /// The id of its UI thread.
    pub thread: u32,
}

/// One entry of the provider's log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// When it started, in microseconds of [`crate::now_us`]'s clock.
    pub start: u64,
    /// When it ended; equal to `start` for a message or an advise.
    pub end: u64,
    /// `call`, `msg`, `advise`, or `queued` (a sent or posted message
    /// reaching a busy thread, named `sent` or `posted`).
    pub kind: String,
    /// The provider method, the message, or the advise call.
    pub name: String,
    /// The window's index.
    pub window: Option<usize>,
    /// The node's index.
    pub node: Option<usize>,
    /// The thread it ran on.
    pub thread: u32,
    /// A message's object id or registered name; an advise's event id.
    pub extra: String,
}

impl Entry {
    fn parse(line: &str) -> Self {
        let words: Vec<&str> = line.split(' ').collect();
        assert!(
            words.len() >= 9 && words[0] == "entry",
            "a log line: {line}"
        );
        let number = |word: &str| word.parse::<u64>().expect("a number in a log line");
        let index = |word: &str| (word != "-").then(|| word.parse().expect("an index"));
        Self {
            start: number(words[1]),
            end: number(words[2]),
            kind: words[3].to_owned(),
            name: words[4].to_owned(),
            window: index(words[5]),
            node: index(words[6]),
            thread: u32::try_from(number(words[7])).expect("a thread id"),
            extra: if words[8] == "-" {
                String::new()
            } else {
                words[8..].join(" ")
            },
        }
    }

    /// Whether this is a call of provider method `name`.
    #[must_use]
    pub fn is_call(&self, name: &str) -> bool {
        self.kind == "call" && self.name == name
    }
}

/// A running provider program.
pub struct Provider {
    child: Child,
    stdin: ChildStdin,
    answers: Receiver<String>,
    notices: Receiver<String>,
    /// Its process id.
    pub pid: u32,
    /// Its windows, in the order of `--windows`.
    pub windows: Vec<Window>,
    _job: Job,
}

impl Provider {
    /// Starts the provider program with `args` (its command line, as its
    /// module documentation gives) and waits until it is ready.
    ///
    /// # Panics
    ///
    /// When it cannot be started or does not become ready.
    #[must_use]
    pub fn launch(args: &[&str]) -> Self {
        let exe = std::env::current_exe().expect("this executable's path");
        let path = exe
            .parent()
            .expect("this executable's folder")
            .join("uia-facts-provider.exe");
        let mut child = Command::new(&path)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("{} starts: {e}", path.display()));
        let job = Job::holding(&child);
        let stdin = child.stdin.take().expect("its standard input");
        let stdout = child.stdout.take().expect("its standard output");
        let (answer, answers) = channel();
        let (notice, notices) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let sent = if line.starts_with("held ") {
                    notice.send(line)
                } else {
                    answer.send(line)
                };
                if sent.is_err() {
                    break;
                }
            }
        });
        let mut windows = Vec::new();
        let pid = loop {
            let line = answers
                .recv_timeout(Duration::from_secs(30))
                .expect("the provider program starts");
            let words: Vec<&str> = line.split(' ').collect();
            match words.as_slice() {
                ["window", _, _, hwnd, thread] => windows.push(Window {
                    hwnd: hwnd.parse().expect("a window handle"),
                    thread: thread.parse().expect("a thread id"),
                }),
                ["ready", pid] => break pid.parse().expect("a process id"),
                _ => panic!("the provider program said {line}"),
            }
        };
        Self {
            child,
            stdin,
            answers,
            notices,
            pid,
            windows,
            _job: job,
        }
    }

    /// Window `index`'s handle.
    #[must_use]
    pub fn hwnd(&self, index: usize) -> isize {
        self.windows[index].hwnd
    }

    /// Sends `request` and returns the lines before its answer and the
    /// answer's text after `ok`.
    ///
    /// # Panics
    ///
    /// When the provider answers `err` or not at all.
    pub fn request_lines(&mut self, request: &str) -> (Vec<String>, String) {
        writeln!(self.stdin, "{request}").expect("the provider takes requests");
        self.stdin.flush().expect("the provider takes requests");
        let mut lines = Vec::new();
        loop {
            let line = self
                .answers
                .recv_timeout(ANSWER_LIMIT)
                .unwrap_or_else(|_| panic!("the provider answers {request}"));
            if line == "ok" {
                return (lines, String::new());
            }
            if let Some(answer) = line.strip_prefix("ok ") {
                return (lines, answer.to_owned());
            }
            assert!(
                !line.starts_with("err"),
                "the provider refused {request}: {line}"
            );
            lines.push(line);
        }
    }

    /// Sends `request` and returns its answer's text after `ok`.
    ///
    /// # Panics
    ///
    /// As [`Provider::request_lines`].
    pub fn request(&mut self, request: &str) -> String {
        self.request_lines(request).1
    }

    /// Waits for the next `held` notice: a provider call has started to
    /// wait on a hold. `None` when none comes within `limit`.
    ///
    /// # Panics
    ///
    /// When the provider program has ended.
    #[must_use]
    pub fn held(&self, limit: Duration) -> Option<String> {
        match self.notices.recv_timeout(limit) {
            Ok(line) => Some(line),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => panic!("the provider program ended"),
        }
    }

    /// Discards the `held` notices received so far.
    pub fn clear_held(&self) {
        while self.notices.try_recv().is_ok() {}
    }

    /// The log entries since the last read.
    pub fn log(&mut self) -> Vec<Entry> {
        let (lines, _) = self.request_lines("log");
        for line in &lines {
            crate::note::trace(&format!("provider log: {line}"));
        }
        lines.iter().map(|line| Entry::parse(line)).collect()
    }

    /// Reads the log until an entry satisfies `found`, or until `bound`
    /// has passed: the evidence awaited, or its absence over the bound.
    /// Answers every entry read and whether one was found.
    pub fn log_until(
        &mut self,
        bound: Duration,
        found: impl Fn(&Entry) -> bool,
    ) -> (Vec<Entry>, bool) {
        let start = std::time::Instant::now();
        let mut entries = Vec::new();
        loop {
            let read = self.log();
            let hit = read.iter().any(&found);
            entries.extend(read);
            if hit {
                return (entries, true);
            }
            if start.elapsed() >= bound {
                return (entries, false);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Provider {
    fn drop(&mut self) {
        // Closing the job kills it; this only reaps it.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
