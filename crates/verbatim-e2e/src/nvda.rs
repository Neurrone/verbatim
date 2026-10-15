//! The NVDA transcript add-on's client (`docs/nvda-transcript.md`), shared
//! by `cargo xtask nvda capture` and the harness's compare mode
//! ([`crate::compare`]).
//!
//! The add-on, installed into NVDA, records each speech sequence NVDA
//! queues while a client is connected and serves the records as JSON
//! lines on 127.0.0.1, port [`PORT_BASE`] plus the Windows session id of
//! the NVDA that loaded it. [`Transcript`] is one connection, past its
//! `Hello`; [`Transcript::entries_after`] reads the entries recorded after
//! a sequence number. The add-on offers no push, so a reader polls it,
//! every [`POLL_INTERVAL`], and takes NVDA's speech as settled once it has
//! queued nothing new for a quiet period ([`QuietWait`]), the settling rule
//! of NVDA's own system tests.
//!
//! For compare mode, [`speech_source`] turns the transcript into the frame
//! source and request sink a [`crate::speech::SpeechCollector`] reads
//! Verbatim's speech connection through, so the collector and the
//! scenarios need nothing of NVDA's own. A reader thread polls the
//! transcript. Each speech entry becomes a queued utterance with an id of
//! its own, the flattened text, and the entry's time on the harness's
//! clock, followed at once by its ending, heard in full, since the add-on
//! reports only what NVDA queued and nothing of when it played. Any other
//! entry, such as a cancel, goes on the timeline as an NVDA event and
//! produces no frame. The sink answers `AwaitIdle` once NVDA has queued
//! nothing for the quiet period, bounded by the request's own timeout,
//! which is the capture's settling rule applied as the harness's barrier.

use std::io::{self, BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use verbatim_control::protocol::{Frame, ReplyPayload, Request};
use verbatim_model::{TraceId, UtteranceEnding, UtteranceId};

use crate::speech::{FrameSource, RequestSink};
use crate::timeline::Timeline;

/// The add-on listens on this port plus its Windows session id.
pub const PORT_BASE: u16 = 44100;

/// The add-on protocol version this client speaks.
pub const PROTOCOL_VERSION: u64 = 1;

/// How often a reader polls for new entries. NVDA's own system tests poll
/// no faster than every 10 ms so as not to starve NVDA's core.
pub const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// How long NVDA must have queued nothing for its speech to count as
/// settled, by default: the settling rule of NVDA's own system tests.
pub const DEFAULT_QUIET: Duration = Duration::from_secs(1);

/// The add-on's port for the NVDA in Windows session `session_id`.
///
/// # Errors
///
/// Returns an error if the port would be out of range.
pub fn session_port(session_id: u32) -> io::Result<u16> {
    u16::try_from(u32::from(PORT_BASE) + session_id)
        .map_err(|_| io::Error::other(format!("session id {session_id} is out of range")))
}

/// A connection to the add-on, past its `Hello`.
pub struct Transcript {
    stream: TcpStream,
    reader: BufReader<TcpStream>,
    next_id: u64,
}

impl Transcript {
    /// Connects to the add-on on `port` and says `Hello`, returning the
    /// connection and the `Hello` reply's payload, which holds
    /// `nvda_version` and `session_id`. The add-on starts recording now,
    /// and its entries' times count from now.
    ///
    /// # Errors
    ///
    /// Returns an error, saying NVDA and the add-on must be running in the
    /// agent's session, if nothing listens on `port`, or an error if the
    /// `Hello` fails.
    pub fn connect(port: u16) -> io::Result<(Self, Value)> {
        let stream = TcpStream::connect(("127.0.0.1", port)).map_err(|error| {
            io::Error::other(format!(
                "no NVDA transcript add-on on port {port} ({error}); is NVDA running in the \
                 agent's session with the add-on installed?"
            ))
        })?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        let reader = BufReader::new(stream.try_clone()?);
        let mut transcript = Self {
            stream,
            reader,
            next_id: 1,
        };
        let hello =
            transcript.request(&json!({"Hello": {"protocol_version": PROTOCOL_VERSION}}))?;
        Ok((transcript, hello["Hello"].clone()))
    }

    fn request(&mut self, request: &Value) -> io::Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let mut line = json!({"id": id, "request": request}).to_string();
        line.push('\n');
        self.stream.write_all(line.as_bytes())?;
        let mut reply = String::new();
        if self.reader.read_line(&mut reply)? == 0 {
            return Err(io::Error::other("the add-on closed the connection"));
        }
        let frame: Value = serde_json::from_str(&reply).map_err(io::Error::other)?;
        if let Some(message) = frame["Error"]["message"].as_str() {
            return Err(io::Error::other(format!("the add-on refused: {message}")));
        }
        Ok(frame["Reply"]["payload"].clone())
    }

    /// The entries recorded after the one numbered `seq`, oldest first.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn entries_after(&mut self, seq: u64) -> io::Result<Vec<TranscriptEntry>> {
        let payload = self.request(&json!({"Read": {"after": seq}}))?;
        Ok(payload["Entries"]["entries"]
            .as_array()
            .map(|entries| entries.iter().map(TranscriptEntry::from_json).collect())
            .unwrap_or_default())
    }
}

/// One entry of the transcript.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranscriptEntry {
    /// Its sequence number, which a later read starts after; `None` when
    /// the add-on sent none.
    pub seq: Option<u64>,
    /// Milliseconds since the `Hello`, on the add-on's clock.
    pub ms: i64,
    /// What NVDA did.
    pub event: TranscriptEvent,
}

/// What one transcript entry records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TranscriptEvent {
    /// A speech sequence NVDA queued: its priority and its text, flattened
    /// by the add-on as NVDA's system tests flatten it.
    Speech {
        /// NVDA's speech priority, such as `normal`.
        priority: String,
        /// The flattened text.
        text: String,
    },
    /// Any other event, such as a cancel, by its name in lower case.
    Other(String),
}

impl TranscriptEntry {
    /// The entry the add-on sent as `entry`, `{seq, ms, event}`.
    #[must_use]
    pub fn from_json(entry: &Value) -> Self {
        Self {
            seq: entry["seq"].as_u64(),
            ms: entry["ms"].as_i64().unwrap_or(0),
            event: TranscriptEvent::from_json(&entry["event"]),
        }
    }
}

impl TranscriptEvent {
    /// The event the add-on sent as `event`: `{"Speech": {text, priority}}`,
    /// or another event's name, such as `"Cancel"`.
    #[must_use]
    pub fn from_json(event: &Value) -> Self {
        if let Some(speech) = event.get("Speech") {
            Self::Speech {
                priority: speech["priority"].as_str().unwrap_or("?").to_owned(),
                text: speech["text"].as_str().unwrap_or("").to_owned(),
            }
        } else {
            Self::Other(event.as_str().unwrap_or("?").to_lowercase())
        }
    }
}

/// The settling rule: speech has settled once nothing new has been
/// queued for the quiet period since the wait began or since the last
/// entry, whichever is later, and the wait gives up at its timeout.
#[derive(Clone, Copy, Debug)]
pub struct QuietWait {
    quiet: Duration,
    last_activity: Instant,
    deadline: Instant,
}

/// Where a [`QuietWait`] stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Settling {
    /// Not quiet for long enough yet.
    Waiting,
    /// Quiet for the quiet period.
    Settled,
    /// The timeout passed before it was quiet for long enough.
    TimedOut,
}

impl QuietWait {
    /// A wait beginning at `now`, settled after `quiet` with no entry, and
    /// giving up after `timeout`.
    #[must_use]
    pub fn starting_at(now: Instant, quiet: Duration, timeout: Duration) -> Self {
        Self {
            quiet,
            last_activity: now,
            deadline: now + timeout,
        }
    }

    /// Records that entries arrived at `at`, which restarts the quiet
    /// period.
    pub fn activity(&mut self, at: Instant) {
        self.last_activity = self.last_activity.max(at);
    }

    /// Where the wait stands at `now`.
    #[must_use]
    pub fn settling(&self, now: Instant) -> Settling {
        if now.saturating_duration_since(self.last_activity) >= self.quiet {
            Settling::Settled
        } else if now >= self.deadline {
            Settling::TimedOut
        } else {
            Settling::Waiting
        }
    }
}

/// Where the compare-mode reader polls entries from: the add-on's
/// [`Transcript`], or a scripted sequence in this module's unit tests.
pub(crate) trait EntrySource: Send {
    fn entries_after(&mut self, seq: u64) -> io::Result<Vec<TranscriptEntry>>;
}

impl EntrySource for Transcript {
    fn entries_after(&mut self, seq: u64) -> io::Result<Vec<TranscriptEntry>> {
        Transcript::entries_after(self, seq)
    }
}

/// What the reader thread passes the frame source, in the order it
/// happened.
enum Item {
    /// A frame for the collector.
    Frame(Box<Frame>),
    /// An NVDA event other than speech, such as a cancel, for the
    /// timeline.
    Event(String),
    /// The reader's own remark, such as speech that did not settle, for
    /// the timeline.
    Note(String),
    /// The transcript failed; nothing follows.
    Failed(io::Error),
}

/// The `AwaitIdle` waiting for NVDA's speech to settle.
struct PendingIdle {
    to: u64,
    wait: QuietWait,
    timeout: Duration,
}

/// What the reader thread and the sink share: the `AwaitIdle` waiting to
/// be answered.
#[derive(Default)]
struct Shared {
    idle: Option<PendingIdle>,
}

/// The reader thread's state: it polls the transcript and passes on what
/// it reads.
struct Poller {
    entries: Box<dyn EntrySource>,
    last_seq: u64,
    /// The `Hello`'s time, in milliseconds since the Unix epoch: the
    /// entries' times count from it.
    hello_unix_ms: u64,
    /// The id the next speech entry is queued under, counting up from 1.
    next_utterance: u64,
    items: mpsc::Sender<Item>,
    shared: Arc<Mutex<Shared>>,
}

impl Poller {
    /// Reads the entries recorded since the last poll, passes each on, and
    /// answers the waiting `AwaitIdle`, if any, once NVDA has been quiet
    /// for its quiet period or its timeout has passed, the time now being
    /// `now`. Returns `false` once the transcript has failed or the source
    /// has gone, when the reader stops.
    fn poll_once(&mut self, now: Instant) -> bool {
        let entries = match self.entries.entries_after(self.last_seq) {
            Ok(entries) => entries,
            Err(error) => {
                let _ = self.items.send(Item::Failed(error));
                return false;
            }
        };
        let arrived = !entries.is_empty();
        for entry in entries {
            self.last_seq = entry.seq.unwrap_or(self.last_seq);
            if !self.pass(entry) {
                return false;
            }
        }
        let mut shared = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(pending) = &mut shared.idle else {
            return true;
        };
        if arrived {
            pending.wait.activity(now);
        }
        let note = match pending.wait.settling(now) {
            Settling::Waiting => return true,
            Settling::Settled => None,
            Settling::TimedOut => Some(format!(
                "NVDA's speech did not settle within {:?}",
                pending.timeout
            )),
        };
        let to = pending.to;
        shared.idle = None;
        drop(shared);
        let passed = note.is_none_or(|note| self.items.send(Item::Note(note)).is_ok());
        passed
            && self
                .items
                .send(Item::Frame(Box::new(Frame::Reply {
                    to,
                    payload: ReplyPayload::Idle,
                })))
                .is_ok()
    }

    /// Passes one entry on: speech as a queued utterance heard in full,
    /// anything else as an NVDA event. `false` once the source has gone.
    fn pass(&mut self, entry: TranscriptEntry) -> bool {
        match entry.event {
            TranscriptEvent::Speech { text, .. } => {
                let utterance = UtteranceId(self.next_utterance);
                self.next_utterance += 1;
                let queued_at_ms = self
                    .hello_unix_ms
                    .saturating_add(u64::try_from(entry.ms).unwrap_or(0));
                // NVDA's speech carries no trace of Verbatim's: each
                // utterance gets one of its own, which nothing reads.
                let queued = Frame::Speech {
                    utterance,
                    trace_id: TraceId::mint(),
                    text,
                    event_observed_at_ms: None,
                    queued_at_ms,
                };
                let ended = Frame::SpeechEnded {
                    utterance,
                    ending: UtteranceEnding::Completed,
                };
                self.items.send(Item::Frame(Box::new(queued))).is_ok()
                    && self.items.send(Item::Frame(Box::new(ended))).is_ok()
            }
            TranscriptEvent::Other(name) => self.items.send(Item::Event(name)).is_ok(),
        }
    }
}

/// The frame source over the reader thread: frames as they come, and
/// NVDA's other events and the reader's notes put on the timeline in their
/// place among them, since this is read on the collector's reader thread,
/// which records each frame as it returns it.
struct NvdaFrames {
    items: mpsc::Receiver<Item>,
    timeline: Timeline,
}

impl FrameSource for NvdaFrames {
    fn next_frame(&mut self) -> io::Result<Frame> {
        loop {
            match self.items.recv() {
                Ok(Item::Frame(frame)) => return Ok(*frame),
                Ok(Item::Event(name)) => self.timeline.push_nvda_event(&name),
                Ok(Item::Note(note)) => self.timeline.push_note(&note),
                Ok(Item::Failed(error)) => {
                    return Err(io::Error::other(format!(
                        "the NVDA transcript failed: {error}"
                    )));
                }
                Err(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::ConnectionAborted,
                        "the NVDA transcript's reader has stopped",
                    ));
                }
            }
        }
    }
}

/// The request sink: `SubscribeSpeech` answered at once, `AwaitIdle` once
/// NVDA's speech has settled, and anything else refused.
struct NvdaRequests {
    /// Where answers given at once go; `None` once shut down, so the
    /// source's read ends when the reader thread's sender goes too.
    items: Option<mpsc::Sender<Item>>,
    shared: Arc<Mutex<Shared>>,
    quiet: Duration,
    next_id: u64,
    /// Dropped to stop the reader thread.
    stop: Option<mpsc::Sender<()>>,
    reader: Option<JoinHandle<()>>,
}

impl NvdaRequests {
    fn answer(&self, frame: Frame) -> io::Result<()> {
        self.items
            .as_ref()
            .ok_or_else(|| io::Error::from(io::ErrorKind::BrokenPipe))?
            .send(Item::Frame(Box::new(frame)))
            .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))
    }
}

impl RequestSink for NvdaRequests {
    fn send(&mut self, request: Request) -> io::Result<u64> {
        self.next_id += 1;
        let to = self.next_id;
        match request {
            Request::SubscribeSpeech => self.answer(Frame::Reply {
                to,
                payload: ReplyPayload::Ok,
            })?,
            Request::AwaitIdle { timeout_ms, .. } => {
                let timeout = Duration::from_millis(timeout_ms);
                let mut shared = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
                shared.idle = Some(PendingIdle {
                    to,
                    wait: QuietWait::starting_at(Instant::now(), self.quiet, timeout),
                    timeout,
                });
            }
            other => self.answer(Frame::Error {
                to,
                message: format!("NVDA's transcript has no equivalent of {other:?}"),
            })?,
        }
        Ok(to)
    }

    fn shutdown(&mut self) -> io::Result<()> {
        self.items = None;
        self.stop = None;
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        Ok(())
    }
}

/// The reader, the frame source, and the sink over `entries`, with no
/// thread started: `hello_unix_ms` is when the `Hello` was answered, and
/// `quiet` the quiet period `AwaitIdle` waits for.
fn parts(
    entries: Box<dyn EntrySource>,
    hello_unix_ms: u64,
    timeline: Timeline,
    quiet: Duration,
) -> (Poller, NvdaFrames, NvdaRequests) {
    let (items, received) = mpsc::channel();
    let shared = Arc::new(Mutex::new(Shared::default()));
    let poller = Poller {
        entries,
        last_seq: 0,
        hello_unix_ms,
        next_utterance: 1,
        items: items.clone(),
        shared: Arc::clone(&shared),
    };
    let frames = NvdaFrames {
        items: received,
        timeline,
    };
    let requests = NvdaRequests {
        items: Some(items),
        shared,
        quiet,
        next_id: 0,
        stop: None,
        reader: None,
    };
    (poller, frames, requests)
}

/// The frame source and request sink a [`crate::speech::SpeechCollector`]
/// reads NVDA's speech through, from `entries` (the transcript, connected
/// at `hello_unix_ms`, in milliseconds since the Unix epoch), with
/// `AwaitIdle` answered once NVDA has queued nothing for `quiet`. A reader
/// thread polls `entries` every [`POLL_INTERVAL`] until the sink is shut
/// down; the source puts NVDA's other events and the reader's notes on
/// `timeline`.
pub(crate) fn speech_source(
    entries: Box<dyn EntrySource>,
    hello_unix_ms: u64,
    timeline: Timeline,
    quiet: Duration,
) -> io::Result<(Box<dyn FrameSource>, Box<dyn RequestSink>)> {
    let (mut poller, frames, mut requests) = parts(entries, hello_unix_ms, timeline, quiet);
    let (stop, stopped) = mpsc::channel::<()>();
    let reader = thread::Builder::new()
        .name("nvda transcript".to_owned())
        .spawn(move || {
            // The add-on offers no push, so the transcript is read once
            // every poll interval, until the sink drops `stop`.
            while poller.poll_once(Instant::now()) {
                match stopped.recv_timeout(POLL_INTERVAL) {
                    Err(RecvTimeoutError::Timeout) => {}
                    Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
                }
            }
        })?;
    requests.stop = Some(stop);
    requests.reader = Some(reader);
    Ok((Box::new(frames), Box::new(requests)))
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::mpsc::TryRecvError;

    use super::*;

    /// Hands out one batch of entries per poll, then none, or fails once
    /// its batches run out when `fail_after` is set.
    struct Scripted {
        batches: VecDeque<Vec<TranscriptEntry>>,
        fail_after: bool,
    }

    impl EntrySource for Scripted {
        fn entries_after(&mut self, _seq: u64) -> io::Result<Vec<TranscriptEntry>> {
            match self.batches.pop_front() {
                Some(batch) => Ok(batch),
                None if self.fail_after => Err(io::Error::other("connection reset")),
                None => Ok(Vec::new()),
            }
        }
    }

    fn speech(seq: u64, ms: i64, text: &str) -> TranscriptEntry {
        TranscriptEntry {
            seq: Some(seq),
            ms,
            event: TranscriptEvent::Speech {
                priority: "normal".to_owned(),
                text: text.to_owned(),
            },
        }
    }

    fn scripted(
        batches: Vec<Vec<TranscriptEntry>>,
        fail_after: bool,
        timeline: Timeline,
    ) -> (Poller, NvdaFrames, NvdaRequests) {
        parts(
            Box::new(Scripted {
                batches: batches.into(),
                fail_after,
            }),
            10_000,
            timeline,
            Duration::from_secs(1),
        )
    }

    /// Every item passed on so far, without waiting.
    fn passed(frames: &NvdaFrames) -> Vec<String> {
        let mut out = Vec::new();
        loop {
            match frames.items.try_recv() {
                Ok(Item::Frame(frame)) => out.push(format!("{frame:?}")),
                Ok(Item::Event(name)) => out.push(format!("event {name}")),
                Ok(Item::Note(note)) => out.push(format!("note {note}")),
                Ok(Item::Failed(error)) => out.push(format!("failed {error}")),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return out,
            }
        }
    }

    fn reply(to: u64) -> String {
        format!(
            "{:?}",
            Frame::Reply {
                to,
                payload: ReplyPayload::Idle
            }
        )
    }

    #[test]
    fn the_add_on_s_json_reads_as_entries() {
        let entry = TranscriptEntry::from_json(&json!({
            "seq": 4,
            "ms": 120,
            "event": {"Speech": {"priority": "now", "text": "a\nb"}}
        }));
        assert_eq!(
            entry,
            TranscriptEntry {
                seq: Some(4),
                ms: 120,
                event: TranscriptEvent::Speech {
                    priority: "now".to_owned(),
                    text: "a\nb".to_owned()
                }
            }
        );
        assert_eq!(
            TranscriptEvent::from_json(&json!("Cancel")),
            TranscriptEvent::Other("cancel".to_owned())
        );
    }

    #[test]
    fn speech_becomes_utterances_heard_in_full_and_a_cancel_goes_on_the_timeline() {
        let timeline = Timeline::new();
        let cancel = TranscriptEntry {
            seq: Some(2),
            ms: 300,
            event: TranscriptEvent::Other("cancel".to_owned()),
        };
        let (mut poller, mut frames, _requests) = scripted(
            vec![vec![
                speech(1, 250, "Desktop list"),
                cancel,
                speech(3, 400, ""),
            ]],
            false,
            timeline.clone(),
        );
        assert!(poller.poll_once(Instant::now()));
        assert_eq!(poller.last_seq, 3);

        let first = frames.next_frame().expect("a frame");
        let Frame::Speech {
            utterance,
            text,
            event_observed_at_ms,
            queued_at_ms,
            ..
        } = first
        else {
            panic!("expected speech, got {first:?}");
        };
        assert_eq!(
            (utterance, text.as_str(), event_observed_at_ms, queued_at_ms),
            (UtteranceId(1), "Desktop list", None, 10_250)
        );
        assert_eq!(
            frames.next_frame().expect("a frame"),
            Frame::SpeechEnded {
                utterance: UtteranceId(1),
                ending: UtteranceEnding::Completed
            }
        );
        assert_eq!(timeline.render(), "(nothing recorded)");
        // The cancel is put on the timeline as the next frame is read.
        let third = frames.next_frame().expect("a frame");
        assert!(
            matches!(&third, Frame::Speech { utterance: UtteranceId(2), text, queued_at_ms: 10_400, .. } if text.is_empty()),
            "{third:?}"
        );
        assert!(timeline.render().ends_with("nvda cancel"));
    }

    #[test]
    fn await_idle_is_answered_once_nvda_has_queued_nothing_for_the_quiet_period() {
        let start = Instant::now();
        let (mut poller, frames, mut requests) = scripted(
            vec![Vec::new(), vec![speech(1, 500, "Name")], Vec::new()],
            false,
            Timeline::new(),
        );
        let id = requests
            .send(Request::AwaitIdle {
                after_input: None,
                timeout_ms: 10_000,
            })
            .expect("sends");
        // Quiet since the request, but not yet for the quiet period.
        assert!(poller.poll_once(start + Duration::from_millis(500)));
        assert_eq!(passed(&frames), Vec::<String>::new());
        // An entry restarts the quiet period.
        assert!(poller.poll_once(start + Duration::from_millis(900)));
        assert_eq!(passed(&frames).len(), 2);
        assert!(poller.poll_once(start + Duration::from_millis(1_800)));
        assert_eq!(passed(&frames), Vec::<String>::new());
        assert!(poller.poll_once(start + Duration::from_millis(1_950)));
        assert_eq!(passed(&frames), vec![reply(id)]);
        // Answered once only.
        assert!(poller.poll_once(start + Duration::from_millis(3_000)));
        assert_eq!(passed(&frames), Vec::<String>::new());
    }

    #[test]
    fn await_idle_is_answered_at_its_timeout_with_a_note_when_nvda_never_settles() {
        let start = Instant::now();
        let (mut poller, frames, mut requests) = scripted(
            vec![vec![speech(1, 0, "one")], vec![speech(2, 0, "two")]],
            false,
            Timeline::new(),
        );
        let id = requests
            .send(Request::AwaitIdle {
                after_input: None,
                timeout_ms: 1_500,
            })
            .expect("sends");
        assert!(poller.poll_once(start + Duration::from_millis(800)));
        assert_eq!(passed(&frames).len(), 2);
        assert!(poller.poll_once(start + Duration::from_millis(1_600)));
        let items = passed(&frames);
        assert_eq!(
            items[2..],
            [
                "note NVDA's speech did not settle within 1.5s".to_owned(),
                reply(id)
            ]
        );
    }

    #[test]
    fn subscribe_is_accepted_at_once_and_other_requests_are_refused() {
        let (_poller, frames, mut requests) = scripted(Vec::new(), false, Timeline::new());
        let subscribed = requests.send(Request::SubscribeSpeech).expect("sends");
        let refused = requests.send(Request::Status).expect("sends");
        assert_eq!(
            passed(&frames),
            vec![
                format!(
                    "{:?}",
                    Frame::Reply {
                        to: subscribed,
                        payload: ReplyPayload::Ok
                    }
                ),
                format!(
                    "{:?}",
                    Frame::Error {
                        to: refused,
                        message: "NVDA's transcript has no equivalent of Status".to_owned()
                    }
                ),
            ]
        );
    }

    #[test]
    fn a_failed_transcript_ends_the_source_with_its_error() {
        let (mut poller, mut frames, _requests) = scripted(Vec::new(), true, Timeline::new());
        assert!(!poller.poll_once(Instant::now()));
        let error = frames.next_frame().expect_err("the transcript failed");
        assert_eq!(
            error.to_string(),
            "the NVDA transcript failed: connection reset"
        );
    }

    #[test]
    fn the_quiet_wait_settles_after_the_quiet_period_since_the_last_activity() {
        let start = Instant::now();
        let mut wait =
            QuietWait::starting_at(start, Duration::from_secs(1), Duration::from_secs(3));
        assert_eq!(
            wait.settling(start + Duration::from_millis(999)),
            Settling::Waiting
        );
        assert_eq!(
            wait.settling(start + Duration::from_secs(1)),
            Settling::Settled
        );
        wait.activity(start + Duration::from_millis(2_500));
        assert_eq!(
            wait.settling(start + Duration::from_millis(2_900)),
            Settling::Waiting
        );
        assert_eq!(
            wait.settling(start + Duration::from_secs(3)),
            Settling::TimedOut
        );
    }
}
