//! The synthesizer host protocol (decision D18): what Core and a
//! synthesizer host process say to each other over their two pipes.
//!
//! It is the [`SynthDriver`](crate::SynthDriver) contract made into
//! messages, so a host runs any driver unchanged: Core asks it to speak a
//! [`SpeechSequence`] and the host streams back PCM, marks, and the outcome.
//! Every message is framed by a kind byte and a length. Control messages
//! are JSON; PCM is sent as raw little-endian samples, since a synthesizer
//! streams far more of it than of anything else.
//!
//! Cancellation names the utterance it cancels, so a cancel that crosses
//! the utterance's own ending on the pipe can never cancel the next one.

use std::io::{self, Read, Write};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use verbatim_audio::PcmFormat;
use verbatim_model::UtteranceId;

use crate::driver::{IndexMark, SpeechSequence};
use crate::settings::{SettingDescriptor, SettingId, SettingValue};

/// A message from Core to a synthesizer host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToHost {
    /// Speak this sequence; the host answers with PCM and marks, then
    /// [`FromHost::Done`] or [`FromHost::Failed`].
    Speak(SpeechSequence),
    /// Stop speaking this utterance as soon as possible; it still ends with
    /// [`FromHost::Done`].
    Cancel(UtteranceId),
    /// Change one setting; the host answers with
    /// [`FromHost::SettingApplied`].
    SetSetting {
        /// The setting.
        id: SettingId,
        /// Its new value.
        value: SettingValue,
    },
}

/// What a host's synthesizer is, sent once when it is ready.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostDescription {
    /// The synthesizer's display name.
    pub display_name: String,
    /// Whether it places index marks itself
    /// ([`SynthDriver::places_marks`](crate::SynthDriver::places_marks)).
    pub places_marks: bool,
    /// Its settings, in display order.
    pub settings: Vec<SettingDescriptor>,
    /// Their current values.
    pub values: Vec<(SettingId, SettingValue)>,
}

/// A message from a synthesizer host to Core.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FromHost {
    /// The synthesizer started; the first message a host sends.
    Ready(HostDescription),
    /// The synthesizer could not start; the host exits after sending it.
    Unavailable(String),
    /// Audio of the utterance being spoken.
    Pcm(PcmFormat, Vec<i16>),
    /// The audio sent so far reaches this mark.
    Mark(IndexMark),
    /// The utterance's synthesis ended, completely or because it was
    /// cancelled.
    Done,
    /// The utterance's synthesis failed.
    Failed(String),
    /// The answer to [`ToHost::SetSetting`]: `None`, or why it was refused.
    SettingApplied(Option<String>),
}

/// The JSON-encoded part of [`FromHost`]: everything but PCM.
#[derive(Serialize, Deserialize)]
enum Control {
    Ready(HostDescription),
    Unavailable(String),
    Mark(IndexMark),
    Done,
    Failed(String),
    SettingApplied(Option<String>),
}

/// Frame kinds.
const JSON: u8 = 0;
const PCM: u8 = 1;

/// The ids of the synthesizers a synthesizer host can run, shared by Core,
/// which asks for one by id, and the host, which builds it.
pub mod synth_ids {
    /// Windows `OneCore` voices.
    pub const ONECORE: &str = "onecore";
}

/// The largest frame either side accepts: 16 MB, far beyond any message,
/// so a corrupt length is caught rather than allocated.
const MAX_FRAME: usize = 16 * 1024 * 1024;

/// Writes one message to a host.
///
/// # Errors
///
/// Returns the writer's error.
pub fn write_to_host(writer: &mut impl Write, message: &ToHost) -> io::Result<()> {
    write_json(writer, message)
}

/// Reads one message from Core, or `None` at the end of the stream.
///
/// # Errors
///
/// Returns the reader's error, or [`io::ErrorKind::InvalidData`] for a
/// malformed frame.
pub fn read_to_host(reader: &mut impl Read) -> io::Result<Option<ToHost>> {
    match read_frame(reader)? {
        None => Ok(None),
        Some((JSON, body)) => parse_json(&body).map(Some),
        Some((kind, _)) => Err(invalid(&format!("unexpected frame kind {kind} from Core"))),
    }
}

/// Writes one message to Core.
///
/// # Errors
///
/// Returns the writer's error.
pub fn write_from_host(writer: &mut impl Write, message: &FromHost) -> io::Result<()> {
    let control = match message {
        FromHost::Pcm(format, samples) => {
            let mut body = Vec::with_capacity(6 + samples.len() * 2);
            body.extend_from_slice(&format.sample_rate.to_le_bytes());
            body.extend_from_slice(&format.channels.to_le_bytes());
            for sample in samples {
                body.extend_from_slice(&sample.to_le_bytes());
            }
            return write_frame(writer, PCM, &body);
        }
        FromHost::Ready(description) => Control::Ready(description.clone()),
        FromHost::Unavailable(reason) => Control::Unavailable(reason.clone()),
        FromHost::Mark(mark) => Control::Mark(*mark),
        FromHost::Done => Control::Done,
        FromHost::Failed(reason) => Control::Failed(reason.clone()),
        FromHost::SettingApplied(refusal) => Control::SettingApplied(refusal.clone()),
    };
    write_json(writer, &control)
}

/// Reads one message from a host, or `None` at the end of the stream.
///
/// # Errors
///
/// Returns the reader's error, or [`io::ErrorKind::InvalidData`] for a
/// malformed frame.
pub fn read_from_host(reader: &mut impl Read) -> io::Result<Option<FromHost>> {
    let Some((kind, body)) = read_frame(reader)? else {
        return Ok(None);
    };
    match kind {
        PCM => {
            if body.len() < 6 || (body.len() - 6) % 2 != 0 {
                return Err(invalid("malformed PCM frame"));
            }
            let format = PcmFormat {
                sample_rate: u32::from_le_bytes([body[0], body[1], body[2], body[3]]),
                channels: u16::from_le_bytes([body[4], body[5]]),
            };
            let samples = body[6..]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| i16::from_le_bytes(*pair))
                .collect();
            Ok(Some(FromHost::Pcm(format, samples)))
        }
        JSON => Ok(Some(match parse_json::<Control>(&body)? {
            Control::Ready(description) => FromHost::Ready(description),
            Control::Unavailable(reason) => FromHost::Unavailable(reason),
            Control::Mark(mark) => FromHost::Mark(mark),
            Control::Done => FromHost::Done,
            Control::Failed(reason) => FromHost::Failed(reason),
            Control::SettingApplied(refusal) => FromHost::SettingApplied(refusal),
        })),
        other => Err(invalid(&format!(
            "unexpected frame kind {other} from a host"
        ))),
    }
}

fn write_json(writer: &mut impl Write, message: &impl Serialize) -> io::Result<()> {
    let body = serde_json::to_vec(message).map_err(io::Error::other)?;
    write_frame(writer, JSON, &body)
}

fn parse_json<T: DeserializeOwned>(body: &[u8]) -> io::Result<T> {
    serde_json::from_slice(body).map_err(|error| invalid(&error.to_string()))
}

/// Writes a kind byte, a little-endian `u32` length, and the body, in one
/// write so the peer never sees a frame split by another writer.
fn write_frame(writer: &mut impl Write, kind: u8, body: &[u8]) -> io::Result<()> {
    if body.len() > MAX_FRAME {
        return Err(invalid("frame too large"));
    }
    let length = u32::try_from(body.len()).map_err(|_| invalid("frame too large"))?;
    let mut frame = Vec::with_capacity(5 + body.len());
    frame.push(kind);
    frame.extend_from_slice(&length.to_le_bytes());
    frame.extend_from_slice(body);
    writer.write_all(&frame)?;
    writer.flush()
}

/// Reads one frame, or `None` when the stream ends cleanly between frames.
fn read_frame(reader: &mut impl Read) -> io::Result<Option<(u8, Vec<u8>)>> {
    let mut header = [0u8; 5];
    let mut read = 0;
    while read < header.len() {
        match reader.read(&mut header[read..]) {
            Ok(0) if read == 0 => return Ok(None),
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(count) => read += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            // A closed anonymous pipe reports a broken pipe: the peer is gone.
            Err(error) if read == 0 && error.kind() == io::ErrorKind::BrokenPipe => {
                return Ok(None);
            }
            Err(error) => return Err(error),
        }
    }
    let length = usize::try_from(u32::from_le_bytes([
        header[1], header[2], header[3], header[4],
    ]))
    .unwrap_or(usize::MAX);
    if length > MAX_FRAME {
        return Err(invalid("frame length out of range"));
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body)?;
    Ok(Some((header[0], body)))
}

fn invalid(detail: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, detail.to_owned())
}

#[cfg(test)]
mod tests {
    use verbatim_model::TraceId;

    use super::*;
    use crate::driver::SpeechItem;

    #[test]
    fn every_message_survives_the_trip_through_a_pipe() {
        let to_host = vec![
            ToHost::Speak(SpeechSequence {
                utterance: UtteranceId(3),
                trace_id: TraceId::mint(),
                language: Some("en".to_owned()),
                items: vec![
                    SpeechItem::Text("one".to_owned()),
                    SpeechItem::Mark(IndexMark(1)),
                ],
            }),
            ToHost::Cancel(UtteranceId(3)),
            ToHost::SetSetting {
                id: SettingId::new("rate"),
                value: SettingValue::Number(70),
            },
        ];
        let from_host = vec![
            FromHost::Ready(HostDescription {
                display_name: "Test".to_owned(),
                places_marks: true,
                settings: vec![SettingDescriptor::standard_numeric("rate", "setting-rate")],
                values: vec![(SettingId::new("rate"), SettingValue::Number(50))],
            }),
            FromHost::Pcm(
                PcmFormat {
                    sample_rate: 22_050,
                    channels: 1,
                },
                vec![1, -2, i16::MAX, i16::MIN],
            ),
            FromHost::Mark(IndexMark(1)),
            FromHost::Done,
            FromHost::Failed("no".to_owned()),
            FromHost::SettingApplied(Some("out of range".to_owned())),
            FromHost::Unavailable("missing".to_owned()),
        ];

        let mut pipe = Vec::new();
        for message in &to_host {
            write_to_host(&mut pipe, message).unwrap();
        }
        let mut reader = pipe.as_slice();
        for message in &to_host {
            assert_eq!(read_to_host(&mut reader).unwrap().as_ref(), Some(message));
        }
        assert_eq!(read_to_host(&mut reader).unwrap(), None);

        let mut pipe = Vec::new();
        for message in &from_host {
            write_from_host(&mut pipe, message).unwrap();
        }
        let mut reader = pipe.as_slice();
        for message in &from_host {
            assert_eq!(read_from_host(&mut reader).unwrap().as_ref(), Some(message));
        }
        assert_eq!(read_from_host(&mut reader).unwrap(), None);
    }
}
