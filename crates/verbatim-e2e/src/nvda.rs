//! The NVDA transcript add-on's client (`docs/nvda-transcript.md`), for
//! `cargo xtask nvda capture` and the end-to-end harness.
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

use std::io::{self, BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

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

#[cfg(test)]
mod tests {
    use super::*;

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
