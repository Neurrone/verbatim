//! `--steps`: a file of steps, run through the end-to-end agent while the
//! recorder records.

use std::io::{self, BufReader};
use std::net::{Shutdown, TcpStream};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use verbatim_agent::protocol::{
    AGENT_PROTOCOL_VERSION, ForegroundOutcome, Frame, ReplyPayload, Request, RequestEnvelope,
};
use verbatim_control::protocol::{MessageReader, write_message};
use verbatim_control::send_keys::parse_all;

use crate::writer::{EarlyEnd, Reason};

/// How long a `--steps` run goes on recording after its last step
/// (Dickson, 2026-11-10).
const AFTER_THE_LAST_STEP: Duration = Duration::from_secs(2);

/// Bounds each of the agent's replies, as the end-to-end client does.
const AGENT_REPLY_TIMEOUT: Duration = Duration::from_secs(30);

/// One step of a steps file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// Press these key combinations in order (`Request::SendKeys`).
    Press(Vec<String>),
    /// Type this text (`Request::TypeText`).
    Type(String),
    /// Wait this many milliseconds, in the recorder.
    Pause(u64),
}

/// Parses a steps file, one step per line: `press <keys>`, `type <text>`,
/// `pause <ms>`; a blank line is skipped.
///
/// # Errors
///
/// Returns a message naming the line for anything else.
pub(crate) fn parse(text: &str) -> Result<Vec<Step>, String> {
    let mut steps = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        if line.trim().is_empty() {
            continue;
        }
        let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
        steps.push(match word {
            "press" => {
                let keys: Vec<String> = rest.split_whitespace().map(str::to_owned).collect();
                parse_all(&keys).map_err(|error| format!("step {number}: {error}"))?;
                if keys.is_empty() {
                    return Err(format!("step {number}: press names no key"));
                }
                Step::Press(keys)
            }
            "type" => Step::Type(rest.to_owned()),
            "pause" => {
                Step::Pause(rest.trim().parse().map_err(|_| {
                    format!("step {number}: pause needs milliseconds, not {rest:?}")
                })?)
            }
            other => return Err(format!("step {number}: unknown step {other:?}")),
        });
    }
    Ok(steps)
}

/// A connection to the end-to-end agent, past its `Hello`.
pub(crate) struct Agent {
    stream: TcpStream,
    reader: MessageReader<BufReader<TcpStream>>,
    next_id: u64,
}

impl Agent {
    /// Connects to `address` and completes the `Hello`.
    ///
    /// # Errors
    ///
    /// Returns a message if the agent cannot be reached or refuses.
    pub(crate) fn connect(address: &str) -> Result<Self, String> {
        let connect = || -> io::Result<Self> {
            let stream = TcpStream::connect(address)?;
            stream.set_read_timeout(Some(AGENT_REPLY_TIMEOUT))?;
            stream.set_write_timeout(Some(AGENT_REPLY_TIMEOUT))?;
            let reader = MessageReader::new(BufReader::new(stream.try_clone()?));
            let mut agent = Self {
                stream,
                reader,
                next_id: 1,
            };
            agent.request(Request::Hello {
                protocol_version: AGENT_PROTOCOL_VERSION,
            })?;
            Ok(agent)
        };
        connect().map_err(|error| format!("--agent {address}: {error}"))
    }

    /// Sends `request` and returns its answer.
    fn request(&mut self, request: Request) -> io::Result<ReplyPayload> {
        let id = self.next_id;
        self.next_id += 1;
        write_message(&mut self.stream, &RequestEnvelope { id, request })?;
        loop {
            match self.reader.read::<Frame>()? {
                Some(Frame::Reply { to, payload }) if to == id => return Ok(payload),
                Some(Frame::Error { to, message }) if to == id => {
                    return Err(io::Error::other(message));
                }
                Some(_) => {}
                None => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "the agent closed the connection",
                    ));
                }
            }
        }
    }

    /// Brings the window `hwnd` to the foreground.
    ///
    /// # Errors
    ///
    /// Returns a message if it does not come to the front.
    pub(crate) fn bring_forward(&mut self, hwnd: isize) -> Result<(), String> {
        let window = u64::from_ne_bytes(hwnd.to_ne_bytes());
        match self
            .request(Request::SetForeground { window })
            .map_err(|error| format!("the agent could not bring the window forward: {error}"))?
        {
            ReplyPayload::Foreground(ForegroundOutcome::InFront) => Ok(()),
            ReplyPayload::Foreground(outcome) => Err(format!(
                "the agent could not bring the window forward: {}",
                outcome.describe()
            )),
            other => Err(format!("the agent answered SetForeground with {other:?}")),
        }
    }

    /// Runs one step that goes through the agent.
    fn run(&mut self, step: &Step) -> io::Result<()> {
        let request = match step {
            Step::Press(keys) => Request::SendKeys { keys: keys.clone() },
            Step::Type(text) => Request::TypeText { text: text.clone() },
            Step::Pause(_) => return Ok(()),
        };
        self.request(request).map(drop)
    }
}

/// The running Steps thread, as main ends it.
pub(crate) struct StepsThread {
    ended: Sender<()>,
    socket: TcpStream,
    join: JoinHandle<()>,
}

impl StepsThread {
    /// Ends the thread: no further step is sent, a pause ends at once, and
    /// a read of the agent's answer is woken by the socket's shutdown.
    pub(crate) fn end(self) {
        drop(self.ended);
        let _ = self.socket.shutdown(Shutdown::Both);
        let _ = self.join.join();
    }
}

/// Starts the Steps thread: `steps` run in order on `agent`, and two
/// seconds after the last the recording ends early with
/// [`Reason::StepsFinished`], unless main has ended the thread first.
///
/// # Errors
///
/// Returns a message if the thread cannot start.
pub(crate) fn spawn(
    agent: Agent,
    steps: Vec<Step>,
    early: Arc<EarlyEnd>,
) -> Result<StepsThread, String> {
    let socket = agent
        .stream
        .try_clone()
        .map_err(|error| format!("could not copy the agent's socket: {error}"))?;
    let (ended, end_received) = mpsc::channel();
    let join = thread::Builder::new()
        .name("verbatim-event-record-steps".to_owned())
        .spawn(move || run_steps(agent, &steps, &end_received, &early))
        .map_err(|error| format!("could not start the steps thread: {error}"))?;
    Ok(StepsThread {
        ended,
        socket,
        join,
    })
}

/// Whether main has ended the Steps thread.
fn has_ended(end_received: &Receiver<()>) -> bool {
    !matches!(end_received.try_recv(), Err(TryRecvError::Empty))
}

/// Waits `wait`, returning whether main has ended the thread.
fn ended_within(end_received: &Receiver<()>, wait: Duration) -> bool {
    !matches!(
        end_received.recv_timeout(wait),
        Err(RecvTimeoutError::Timeout)
    )
}

/// Runs the steps. Ended only by main, the steps' end, or a failure.
fn run_steps(mut agent: Agent, steps: &[Step], end_received: &Receiver<()>, early: &EarlyEnd) {
    for step in steps {
        if has_ended(end_received) {
            return;
        }
        if let Step::Pause(ms) = step {
            if ended_within(end_received, Duration::from_millis(*ms)) {
                return;
            }
            continue;
        }
        if let Err(error) = agent.run(step) {
            if !has_ended(end_received) {
                early.end(Reason::StepFailed(error.to_string()));
            }
            return;
        }
    }
    if !ended_within(end_received, AFTER_THE_LAST_STEP) {
        early.end(Reason::StepsFinished);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_steps_parse_and_a_blank_line_is_skipped() {
        assert_eq!(
            parse("press control+l tab\n\ntype hello there\npause 250\n"),
            Ok(vec![
                Step::Press(vec!["control+l".to_owned(), "tab".to_owned()]),
                Step::Type("hello there".to_owned()),
                Step::Pause(250),
            ])
        );
    }

    #[test]
    fn an_unknown_step_is_refused_by_its_line() {
        assert_eq!(
            parse("press tab\nwait 5\n"),
            Err("step 2: unknown step \"wait\"".to_owned())
        );
    }

    #[test]
    fn an_unknown_key_is_refused_before_any_step_runs() {
        assert_eq!(
            parse("press ctrl+l\n"),
            Err("step 1: not a recognized modifier name: \"ctrl\"".to_owned())
        );
    }
}
