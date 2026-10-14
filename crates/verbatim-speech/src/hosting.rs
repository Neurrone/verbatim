//! The synthesizer host protocol (decision D18): what Core and a
//! synthesizer host process say to each other over their two pipes.
//!
//! It is the [`SynthDriver`](crate::SynthDriver) contract made into
//! messages, so a host runs any driver unchanged: Core asks it to speak a
//! [`SpeechSequence`] and the host streams back PCM, marks, and the outcome.
//! Each message is one frame of the transport every pipe between Verbatim's
//! processes uses (`docs/design/communication.md`, section 4.2), which
//! lives in a Windows crate; this module gives each frame's body. Control
//! messages are JSON; PCM is raw little-endian samples behind a short
//! binary header, since a synthesizer streams far more of it than of
//! anything else.
//!
//! Cancellation names the utterance it cancels, so a cancel that crosses
//! the utterance's own ending on the pipe can never cancel the next one;
//! and everything the host says about an utterance names it, so Core can
//! ask for the next utterance right behind a cancel and drop the cancelled
//! one's last frames as they come (section 7.2).

use std::io;

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
    /// [`FromHost::Done`] or [`FromHost::Failed`], each naming its
    /// utterance.
    Speak(SpeechSequence),
    /// Stop speaking this utterance as soon as possible; it still ends with
    /// [`FromHost::Done`]. A cancel of an utterance already ended does
    /// nothing.
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
    /// Whether it speaks pitch changes itself
    /// ([`SynthDriver::changes_pitch`](crate::SynthDriver::changes_pitch)).
    #[serde(default)]
    pub changes_pitch: bool,
    /// Its settings, in display order.
    pub settings: Vec<SettingDescriptor>,
    /// Their current values.
    pub values: Vec<(SettingId, SettingValue)>,
}

/// A message from a synthesizer host to Core. Everything said about an
/// utterance names it, so Core can tell a cancelled utterance's last
/// frames, still on their way when the next utterance is asked for, from
/// the next utterance's, and drop them unread
/// (`docs/design/communication.md`, section 7.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FromHost {
    /// The synthesizer started; the first message a host sends.
    Ready(HostDescription),
    /// The synthesizer could not start; the host exits after sending it.
    Unavailable(String),
    /// Sent just before an utterance's first audio: when the host took in
    /// its request and when it wrote this, and how long its synthesizer
    /// took to the first chunk, for the latency ledger.
    FirstAudio(UtteranceId, HostTiming),
    /// Audio of an utterance.
    Pcm(UtteranceId, PcmFormat, Vec<i16>),
    /// The utterance's audio sent so far reaches this mark.
    Mark(UtteranceId, IndexMark),
    /// The utterance's synthesis ended, completely or because it was
    /// cancelled.
    Done(UtteranceId),
    /// The utterance's synthesis failed.
    Failed(UtteranceId, String),
    /// The answer to [`ToHost::SetSetting`]: `None`, or why it was refused.
    SettingApplied(Option<String>),
}

impl FromHost {
    /// The utterance the message is about; `None` for `Ready`,
    /// `Unavailable` and `SettingApplied`.
    #[must_use]
    pub fn utterance(&self) -> Option<UtteranceId> {
        match self {
            Self::FirstAudio(utterance, _)
            | Self::Pcm(utterance, _, _)
            | Self::Mark(utterance, _)
            | Self::Done(utterance)
            | Self::Failed(utterance, _) => Some(*utterance),
            Self::Ready(_) | Self::Unavailable(_) | Self::SettingApplied(_) => None,
        }
    }

    /// Whether the message ends its utterance: `Done` or `Failed`.
    #[must_use]
    pub fn ends_utterance(&self) -> bool {
        matches!(self, Self::Done(_) | Self::Failed(..))
    }
}

/// The host's stamps on an utterance's start ([`FromHost::FirstAudio`]):
/// times on the clock every process shares ([`now_us`]), and one duration
/// measured in the host alone.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostTiming {
    /// When the host read the request, in microseconds since the Unix
    /// epoch.
    pub received_at_us: u64,
    /// From the host taking the request to its synthesizer's first chunk.
    pub first_chunk_us: u64,
    /// When the host wrote this message, just before the first chunk, in
    /// microseconds since the Unix epoch.
    pub sent_at_us: u64,
}

/// Microseconds since the Unix epoch on the system's precise clock, which
/// is the same in every process, for the stamps that cross a process
/// boundary.
#[must_use]
pub fn now_us() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_micros()).unwrap_or(u64::MAX)
        })
}

/// The JSON-encoded part of [`FromHost`]: everything but PCM.
#[derive(Serialize, Deserialize)]
enum Control {
    Ready(HostDescription),
    FirstAudio(UtteranceId, HostTiming),
    Unavailable(String),
    Mark(UtteranceId, IndexMark),
    Done(UtteranceId),
    Failed(UtteranceId, String),
    SettingApplied(Option<String>),
}

/// The first byte of a PCM body. No JSON text starts with it, so it tells
/// PCM from every other message.
const PCM: u8 = 0;

/// The bytes of a PCM body before its samples: the kind, the utterance,
/// the sample rate and the channel count.
const PCM_HEADER: usize = 1 + 8 + 4 + 2;

/// The ids of the synthesizers a synthesizer host can run, shared by Core,
/// which asks for one by id, and the host, which builds it.
pub mod synth_ids {
    /// Windows `OneCore` voices.
    pub const ONECORE: &str = "onecore";

    /// eSpeak NG.
    pub const ESPEAK: &str = "espeak";
}

/// Appends the body of a message to a host to `body`: its JSON.
///
/// # Errors
///
/// Returns the encoding's error.
pub fn encode_to_host(message: &ToHost, body: &mut Vec<u8>) -> io::Result<()> {
    serde_json::to_writer(body, message).map_err(io::Error::other)
}

/// Reads a message to a host from its frame's body.
///
/// # Errors
///
/// Returns [`io::ErrorKind::InvalidData`] for a body that is not one.
pub fn decode_to_host(body: &[u8]) -> io::Result<ToHost> {
    parse_json(body)
}

/// Appends the body of a message from a host to `body`: JSON, or for PCM,
/// a zero byte, the utterance (a little-endian `u64`), the sample rate (a
/// `u32`) and the channel count (a `u16`), then the samples as
/// little-endian `i16`s.
///
/// # Errors
///
/// Returns the encoding's error.
pub fn encode_from_host(message: &FromHost, body: &mut Vec<u8>) -> io::Result<()> {
    let control = match message {
        FromHost::Pcm(utterance, format, samples) => {
            encode_pcm(*utterance, *format, samples, body);
            return Ok(());
        }
        FromHost::Ready(description) => Control::Ready(description.clone()),
        FromHost::FirstAudio(utterance, timing) => Control::FirstAudio(*utterance, *timing),
        FromHost::Unavailable(reason) => Control::Unavailable(reason.clone()),
        FromHost::Mark(utterance, mark) => Control::Mark(*utterance, *mark),
        FromHost::Done(utterance) => Control::Done(*utterance),
        FromHost::Failed(utterance, reason) => Control::Failed(*utterance, reason.clone()),
        FromHost::SettingApplied(refusal) => Control::SettingApplied(refusal.clone()),
    };
    serde_json::to_writer(body, &control).map_err(io::Error::other)
}

/// Appends the body of [`FromHost::Pcm`] for `utterance`'s `samples` to
/// `body`, as [`encode_from_host`] does, without copying the samples into
/// a message first.
pub fn encode_pcm(utterance: UtteranceId, format: PcmFormat, samples: &[i16], body: &mut Vec<u8>) {
    body.reserve(PCM_HEADER + samples.len() * 2);
    body.push(PCM);
    body.extend_from_slice(&utterance.0.to_le_bytes());
    body.extend_from_slice(&format.sample_rate.to_le_bytes());
    body.extend_from_slice(&format.channels.to_le_bytes());
    for sample in samples {
        body.extend_from_slice(&sample.to_le_bytes());
    }
}

/// Reads a message from a host from its frame's body.
///
/// # Errors
///
/// Returns [`io::ErrorKind::InvalidData`] for a body that is not one.
pub fn decode_from_host(body: &[u8]) -> io::Result<FromHost> {
    if body.first() == Some(&PCM) {
        if body.len() < PCM_HEADER || !(body.len() - PCM_HEADER).is_multiple_of(2) {
            return Err(invalid("malformed PCM frame"));
        }
        let (header, samples) = body.split_at(PCM_HEADER);
        let (mut utterance, mut sample_rate, mut channels) = ([0u8; 8], [0u8; 4], [0u8; 2]);
        utterance.copy_from_slice(&header[1..9]);
        sample_rate.copy_from_slice(&header[9..13]);
        channels.copy_from_slice(&header[13..15]);
        let utterance = UtteranceId(u64::from_le_bytes(utterance));
        let format = PcmFormat {
            sample_rate: u32::from_le_bytes(sample_rate),
            channels: u16::from_le_bytes(channels),
        };
        let samples = samples
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| i16::from_le_bytes(*pair))
            .collect();
        return Ok(FromHost::Pcm(utterance, format, samples));
    }
    Ok(match parse_json::<Control>(body)? {
        Control::Ready(description) => FromHost::Ready(description),
        Control::FirstAudio(utterance, timing) => FromHost::FirstAudio(utterance, timing),
        Control::Unavailable(reason) => FromHost::Unavailable(reason),
        Control::Mark(utterance, mark) => FromHost::Mark(utterance, mark),
        Control::Done(utterance) => FromHost::Done(utterance),
        Control::Failed(utterance, reason) => FromHost::Failed(utterance, reason),
        Control::SettingApplied(refusal) => FromHost::SettingApplied(refusal),
    })
}

fn parse_json<T: DeserializeOwned>(body: &[u8]) -> io::Result<T> {
    serde_json::from_slice(body).map_err(|error| invalid(&error.to_string()))
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
    fn every_message_survives_encoding_and_decoding() {
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
                changes_pitch: false,
                settings: vec![SettingDescriptor::standard_numeric("rate", "setting-rate")],
                values: vec![(SettingId::new("rate"), SettingValue::Number(50))],
            }),
            FromHost::Pcm(
                UtteranceId(u64::MAX - 1),
                PcmFormat {
                    sample_rate: 22_050,
                    channels: 1,
                },
                vec![1, -2, i16::MAX, i16::MIN],
            ),
            FromHost::FirstAudio(
                UtteranceId(3),
                HostTiming {
                    received_at_us: 1_000,
                    first_chunk_us: 2_500,
                    sent_at_us: 3_600,
                },
            ),
            FromHost::Mark(UtteranceId(3), IndexMark(1)),
            FromHost::Done(UtteranceId(3)),
            FromHost::Failed(UtteranceId(4), "no".to_owned()),
            FromHost::SettingApplied(Some("out of range".to_owned())),
            FromHost::Unavailable("missing".to_owned()),
        ];

        for message in &to_host {
            let mut body = Vec::new();
            encode_to_host(message, &mut body).unwrap();
            assert_eq!(&decode_to_host(&body).unwrap(), message);
        }
        for message in &from_host {
            let mut body = Vec::new();
            encode_from_host(message, &mut body).unwrap();
            assert_eq!(&decode_from_host(&body).unwrap(), message);
        }
    }

    #[test]
    fn a_pcm_body_cut_short_is_refused() {
        let mut body = Vec::new();
        encode_from_host(
            &FromHost::Pcm(
                UtteranceId(1),
                PcmFormat {
                    sample_rate: 22_050,
                    channels: 1,
                },
                vec![1, 2],
            ),
            &mut body,
        )
        .unwrap();
        body.pop();
        assert_eq!(
            decode_from_host(&body).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
}
