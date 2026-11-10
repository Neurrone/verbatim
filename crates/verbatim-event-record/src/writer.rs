//! The writing path: every callback stamps its line under one lock, so the
//! file's order is the time order, and one thread serializes and writes,
//! so a slow disk delays no handler.

use std::io::{self, Write};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Instant;

use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle, RawHandle};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Threading::{CreateEventW, SetEvent};

use crate::behind::{Focus, Pairs};
use crate::line::{EventData, behind_line, event_line, registered_line};

/// How far behind its Windows timestamp an MSAA event must arrive to
/// write a `behind` line, in milliseconds.
const MSAA_LATE_MS: u32 = 100;

/// Why the recording ended early.
#[derive(Debug)]
pub(crate) enum Reason {
    /// The file could not be written.
    WriteFailed(String),
    /// A step failed.
    StepFailed(String),
    /// The `--window` target was destroyed.
    WindowDestroyed,
    /// The steps ended, and their two seconds with them.
    StepsFinished,
    /// A console control event, such as Ctrl+C, asked for it.
    ConsoleControl,
    /// Main's wait failed.
    WaitFailed(String),
}

impl Reason {
    /// The exit code: failures exit 1.
    pub(crate) fn exit_code(&self) -> u32 {
        match self {
            Self::WriteFailed(_) | Self::StepFailed(_) | Self::WaitFailed(_) => 1,
            Self::WindowDestroyed | Self::StepsFinished | Self::ConsoleControl => 0,
        }
    }

    /// The reason, as the closing message gives it.
    pub(crate) fn describe(&self) -> String {
        match self {
            Self::WriteFailed(error) => format!("the file could not be written: {error}"),
            Self::StepFailed(error) => format!("a step failed: {error}"),
            Self::WindowDestroyed => "the target window was destroyed".to_owned(),
            Self::StepsFinished => "the steps finished".to_owned(),
            Self::ConsoleControl => "a console control event asked it to".to_owned(),
            Self::WaitFailed(error) => format!("waiting for the recording's end failed: {error}"),
        }
    }
}

/// Ends the recording early, with the one reason given first: a Windows
/// event, so that main can wait on it beside its other handles.
pub(crate) struct EarlyEnd {
    event: OwnedHandle,
    reason: OnceLock<Reason>,
}

impl EarlyEnd {
    /// A new one, not set.
    ///
    /// # Errors
    ///
    /// Returns a message if the event cannot be created.
    pub(crate) fn new() -> Result<Self, String> {
        // SAFETY: a manual-reset event, unnamed, with default security.
        let event = unsafe { CreateEventW(None, true, false, None) }
            .map_err(|error| format!("could not create the end event: {error}"))?;
        Ok(Self {
            // SAFETY: a valid handle just returned, owned by nothing else.
            event: unsafe { OwnedHandle::from_raw_handle(event.0 as RawHandle) },
            reason: OnceLock::new(),
        })
    }

    /// Ends the recording early for `reason`, if nothing has yet. Never
    /// blocks.
    pub(crate) fn end(&self, reason: Reason) {
        let _ = self.reason.set(reason);
        // SAFETY: the event this value owns, open for its life.
        let _ = unsafe { SetEvent(self.handle()) };
    }

    /// The event, for main's wait.
    pub(crate) fn handle(&self) -> HANDLE {
        HANDLE(self.event.as_raw_handle())
    }

    /// Why the recording ended early, once it has.
    pub(crate) fn reason(&self) -> Option<&Reason> {
        self.reason.get()
    }
}

/// What the writer is sent.
#[derive(Debug)]
pub(crate) enum Message {
    /// An event's line.
    Event(Box<EventArrival>),
    /// A window's registration has completed.
    Registered(isize),
    /// The recording has ended: flush and return.
    End,
}

/// One event, as a callback sends it.
#[derive(Debug)]
pub(crate) struct EventArrival {
    /// What its line holds.
    pub data: EventData,
    /// The focus event it is for the pairing reducer, when it is one that
    /// can pair.
    pub focus: Option<Focus>,
    /// How many milliseconds before arrival Windows raised it, for an
    /// MSAA event.
    pub raised_ms_ago: Option<u32>,
}

/// A message with its arrival time.
pub(crate) struct Stamped {
    t_us: u64,
    message: Message,
}

/// The one way to the writer: the clock is read and the line sent under
/// one lock, so the file's order is the `t_us` order. A send after the
/// writer's receiver is dropped fails, and is ignored.
pub(crate) struct Arrivals {
    start: Instant,
    sender: Mutex<Sender<Stamped>>,
}

impl Arrivals {
    /// The way to the writer `sender` leads to, the clock starting now.
    pub(crate) fn new(sender: Sender<Stamped>) -> Self {
        Self {
            start: Instant::now(),
            sender: Mutex::new(sender),
        }
    }

    /// When the recording started.
    pub(crate) fn started(&self) -> Instant {
        self.start
    }

    /// Stamps and sends `message`. Held for microseconds, and never waits
    /// on anything else.
    pub(crate) fn arrive(&self, message: Message) {
        let sender = self.sender.lock().unwrap_or_else(PoisonError::into_inner);
        let t_us = u64::try_from(self.start.elapsed().as_micros()).unwrap_or(u64::MAX);
        let _ = sender.send(Stamped { t_us, message });
    }
}

/// The writer thread: writes `header`, then every line, flushing whenever
/// the channel is empty, until `End`. A write error ends the recording
/// early and discards later lines; `End` still returns.
///
/// # Errors
///
/// Returns the first write error, the final flush's included, so the
/// closing message and the exit code say whether the file is whole.
pub(crate) fn serve<W: Write>(
    mut out: W,
    header: &str,
    received: &Receiver<Stamped>,
    early: &Arc<EarlyEnd>,
) -> Result<(), String> {
    let mut failure: Option<String> = None;
    let report = |result: io::Result<()>, failure: &mut Option<String>| {
        if let Err(error) = result
            && failure.is_none()
        {
            *failure = Some(error.to_string());
            early.end(Reason::WriteFailed(error.to_string()));
        }
    };
    report(writeln!(out, "{header}"), &mut failure);
    let mut pairs = Pairs::default();
    loop {
        let stamped = match received.try_recv() {
            Ok(stamped) => stamped,
            Err(TryRecvError::Empty) => {
                report(out.flush(), &mut failure);
                match received.recv() {
                    Ok(stamped) => stamped,
                    Err(_) => break,
                }
            }
            Err(TryRecvError::Disconnected) => break,
        };
        let t_us = stamped.t_us;
        let lines = match stamped.message {
            Message::End => {
                report(out.flush(), &mut failure);
                break;
            }
            Message::Registered(window) => vec![registered_line(t_us, window)],
            Message::Event(arrival) => event_lines(t_us, &arrival, &mut pairs),
        };
        if failure.is_some() {
            continue;
        }
        for line in lines {
            report(writeln!(out, "{line}"), &mut failure);
        }
    }
    failure.map_or(Ok(()), Err)
}

/// `arrival`'s line, and any `behind` line it causes.
fn event_lines(t_us: u64, arrival: &EventArrival, pairs: &mut Pairs) -> Vec<String> {
    let mut lines = vec![event_line(t_us, &arrival.data)];
    if arrival.raised_ms_ago.is_some_and(|ago| ago > MSAA_LATE_MS) {
        let late = arrival.raised_ms_ago.map_or(0, u64::from);
        lines.push(behind_line(t_us, "msaa", late));
    }
    if let Some(focus) = arrival.focus
        && let Some(late) = pairs.arrive(focus, t_us)
    {
        lines.push(behind_line(t_us, "uia_focus", late));
    }
    lines
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    /// A file whose flush fails, as a full disk's does at the final flush.
    struct FlushFails;

    impl Write for FlushFails {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::other("the disk is full"))
        }
    }

    #[test]
    fn a_flush_that_fails_at_the_end_is_the_writers_result() {
        let (sender, received) = mpsc::channel();
        let early = Arc::new(EarlyEnd::new().expect("an end event"));
        sender
            .send(Stamped {
                t_us: 0,
                message: Message::End,
            })
            .expect("the channel is open");
        assert_eq!(
            serve(FlushFails, "header", &received, &early),
            Err("the disk is full".to_owned())
        );
        assert_eq!(
            early.reason().map(Reason::describe),
            Some("the file could not be written: the disk is full".to_owned())
        );
    }
}
