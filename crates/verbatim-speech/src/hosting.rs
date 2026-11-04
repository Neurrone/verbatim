//! The synthesizer host protocol (decision D18): what Core and a
//! synthesizer host process say to each other over their two pipes.
//!
//! It is the [`SynthDriver`](crate::SynthDriver) contract made into
//! messages, so a host runs any driver unchanged: Core asks it to speak a
//! [`SpeechSequence`] and the host streams back PCM, marks, and the outcome.
//! Each message is one frame of the transport every pipe between Verbatim's
//! processes uses (`docs/design/communication.md`, section 4.2), which
//! lives in a Windows crate; this module gives each frame's parts. Every
//! message is a JSON body; PCM's samples travel in the frame's raw section
//! as little-endian `i16`s, the body giving their place there, since a
//! synthesizer streams far more of them than of anything else.
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
/// times on the clock every process shares (`verbatim_process::clock` in the
/// Windows host), and one duration
/// measured in the host alone.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostTiming {
    /// When the host read the request, in microseconds since the Unix
    /// epoch.
    pub received_at_us: u64,
    /// From the host taking the request to its synthesizer's first chunk,
    /// any wait for its turn behind an utterance still ending included.
    pub first_chunk_us: u64,
    /// When the host wrote this message, just before the first chunk, in
    /// microseconds since the Unix epoch.
    pub sent_at_us: u64,
}

/// [`FromHost`] as its frame's JSON body gives it: PCM's samples are in
/// the frame's raw section, and the body gives their place there.
#[derive(Serialize, Deserialize)]
enum Wire {
    Ready(HostDescription),
    FirstAudio(UtteranceId, HostTiming),
    Unavailable(String),
    Pcm {
        utterance: UtteranceId,
        sample_rate: u32,
        channels: u16,
        /// Where the samples start in the raw section, in bytes, and how
        /// many bytes they take: always the whole section.
        samples: (usize, usize),
    },
    Mark(UtteranceId, IndexMark),
    Done(UtteranceId),
    Failed(UtteranceId, String),
    SettingApplied(Option<String>),
}

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

/// How the transport that carries these messages puts a frame together
/// (`docs/design/communication.md`, section 4.2): a JSON body alone, or a
/// body and then a raw section. This module decides which a message's
/// frame is ([`put_from_host`], [`put_pcm`]), so a PCM frame cannot be
/// written without its samples.
pub trait Framing {
    /// Appends a frame whose body `put_body` appends, with no raw section.
    ///
    /// # Errors
    ///
    /// Returns `put_body`'s error, or the transport's.
    fn body(&mut self, put_body: impl FnOnce(&mut Vec<u8>) -> io::Result<()>) -> io::Result<()>;

    /// Appends a frame whose body `put_body` appends, then its raw section,
    /// which `put_raw` appends.
    ///
    /// # Errors
    ///
    /// Returns `put_body`'s or `put_raw`'s error, or the transport's.
    fn body_and_raw(
        &mut self,
        put_body: impl FnOnce(&mut Vec<u8>) -> io::Result<()>,
        put_raw: impl FnOnce(&mut Vec<u8>) -> io::Result<()>,
    ) -> io::Result<()>;
}

/// Appends a message from a host's frame through `frame`: a
/// [`FromHost::Pcm`]'s body and its samples as the raw section, as
/// [`put_pcm`] does, and every other message its JSON body alone.
///
/// # Errors
///
/// Returns the encoding's error, or `frame`'s.
pub fn put_from_host(message: &FromHost, frame: &mut impl Framing) -> io::Result<()> {
    let wire = match message {
        FromHost::Pcm(utterance, format, samples) => {
            return put_pcm(*utterance, *format, samples, frame);
        }
        FromHost::Ready(description) => Wire::Ready(description.clone()),
        FromHost::FirstAudio(utterance, timing) => Wire::FirstAudio(*utterance, *timing),
        FromHost::Unavailable(reason) => Wire::Unavailable(reason.clone()),
        FromHost::Mark(utterance, mark) => Wire::Mark(*utterance, *mark),
        FromHost::Done(utterance) => Wire::Done(*utterance),
        FromHost::Failed(utterance, reason) => Wire::Failed(*utterance, reason.clone()),
        FromHost::SettingApplied(refusal) => Wire::SettingApplied(refusal.clone()),
    };
    frame.body(|body| serde_json::to_writer(body, &wire).map_err(io::Error::other))
}

/// Appends the frame of [`FromHost::Pcm`] for `utterance`'s `samples`
/// through `frame`, as [`put_from_host`] does, without copying the samples
/// into a message first: a body giving the utterance, the format, and the
/// samples' place in the raw section, then the samples as the raw section,
/// little-endian `i16`s, copied once.
///
/// # Errors
///
/// Returns the encoding's error, or `frame`'s.
pub fn put_pcm(
    utterance: UtteranceId,
    format: PcmFormat,
    samples: &[i16],
    frame: &mut impl Framing,
) -> io::Result<()> {
    let wire = Wire::Pcm {
        utterance,
        sample_rate: format.sample_rate,
        channels: format.channels,
        samples: (0, samples.len() * 2),
    };
    frame.body_and_raw(
        |body| serde_json::to_writer(body, &wire).map_err(io::Error::other),
        |raw| {
            raw.reserve(samples.len() * 2);
            for sample in samples {
                raw.extend_from_slice(&sample.to_le_bytes());
            }
            Ok(())
        },
    )
}

/// Reads a message from a host from its frame's body and raw section,
/// which only a PCM frame has.
///
/// # Errors
///
/// Returns [`io::ErrorKind::InvalidData`] for a body that is not one, a
/// PCM frame whose samples are not its whole raw section or are cut short,
/// and a raw section with any other message.
pub fn decode_from_host(body: &[u8], raw: Option<&[u8]>) -> io::Result<FromHost> {
    let message = match parse_json::<Wire>(body)? {
        Wire::Pcm {
            utterance,
            sample_rate,
            channels,
            samples,
        } => {
            let format = PcmFormat {
                sample_rate,
                channels,
            };
            return Ok(FromHost::Pcm(utterance, format, pcm_samples(samples, raw)?));
        }
        Wire::Ready(description) => FromHost::Ready(description),
        Wire::FirstAudio(utterance, timing) => FromHost::FirstAudio(utterance, timing),
        Wire::Unavailable(reason) => FromHost::Unavailable(reason),
        Wire::Mark(utterance, mark) => FromHost::Mark(utterance, mark),
        Wire::Done(utterance) => FromHost::Done(utterance),
        Wire::Failed(utterance, reason) => FromHost::Failed(utterance, reason),
        Wire::SettingApplied(refusal) => FromHost::SettingApplied(refusal),
    };
    if raw.is_some() {
        return Err(invalid(
            "a frame has a raw section its message does not take",
        ));
    }
    Ok(message)
}

/// The samples a PCM frame's body places at `(start, length)` in its raw
/// section, which they must fill.
fn pcm_samples((start, length): (usize, usize), raw: Option<&[u8]>) -> io::Result<Vec<i16>> {
    let raw = raw.ok_or_else(|| invalid("a PCM frame has no raw section"))?;
    if start != 0 || length != raw.len() {
        return Err(invalid("a PCM frame's samples are not its raw section"));
    }
    let (samples, rest) = raw.as_chunks::<2>();
    if !rest.is_empty() {
        return Err(invalid("a PCM frame's samples end inside a sample"));
    }
    Ok(samples
        .iter()
        .map(|pair| i16::from_le_bytes(*pair))
        .collect())
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
            let mut frame = Parts::default();
            put_from_host(message, &mut frame).unwrap();
            assert_eq!(
                &decode_from_host(&frame.body, frame.raw.as_deref()).unwrap(),
                message
            );
        }
    }

    /// A frame's parts as [`Framing`] put them.
    #[derive(Default)]
    struct Parts {
        body: Vec<u8>,
        raw: Option<Vec<u8>>,
    }

    impl Framing for Parts {
        fn body(
            &mut self,
            put_body: impl FnOnce(&mut Vec<u8>) -> io::Result<()>,
        ) -> io::Result<()> {
            put_body(&mut self.body)
        }

        fn body_and_raw(
            &mut self,
            put_body: impl FnOnce(&mut Vec<u8>) -> io::Result<()>,
            put_raw: impl FnOnce(&mut Vec<u8>) -> io::Result<()>,
        ) -> io::Result<()> {
            put_body(&mut self.body)?;
            put_raw(self.raw.insert(Vec::new()))
        }
    }

    #[test]
    fn a_pcm_frames_body_names_its_samples_place_and_they_follow_as_little_endian() {
        let mut frame = Parts::default();
        let format = PcmFormat {
            sample_rate: 22_050,
            channels: 1,
        };
        put_pcm(UtteranceId(7), format, &[1, -2], &mut frame).unwrap();
        assert_eq!(
            String::from_utf8(frame.body).unwrap(),
            r#"{"Pcm":{"utterance":7,"sample_rate":22050,"channels":1,"samples":[0,4]}}"#
        );
        assert_eq!(frame.raw, Some(vec![1, 0, 0xFE, 0xFF]));
    }

    #[test]
    fn a_malformed_pcm_frame_is_refused() {
        let pcm = |samples: &str| {
            format!(
                r#"{{"Pcm":{{"utterance":1,"sample_rate":22050,"channels":1,"samples":{samples}}}}}"#
            )
        };
        let done = br#"{"Done":1}"#;
        for (body, raw) in [
            // Samples cut short: the raw section shorter than the body says.
            (pcm("[0,4]"), Some(&[1u8, 0, 2][..])),
            // A raw section longer than the samples the body names.
            (pcm("[0,2]"), Some(&[1, 0, 2, 0][..])),
            // Samples placed outside the raw section.
            (pcm("[2,4]"), Some(&[1, 0, 2, 0][..])),
            // An odd number of bytes, ending inside a sample.
            (pcm("[0,3]"), Some(&[1, 0, 2][..])),
            // No raw section at all.
            (pcm("[0,0]"), None),
            // A raw section with a message that takes none.
            (String::from_utf8(done.to_vec()).unwrap(), Some(&[][..])),
        ] {
            assert_eq!(
                decode_from_host(body.as_bytes(), raw).unwrap_err().kind(),
                io::ErrorKind::InvalidData,
                "{body} with {raw:?}"
            );
        }
    }
}
