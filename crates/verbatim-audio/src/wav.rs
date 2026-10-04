//! Recording what Verbatim plays to a WAV file (decision D16).
//!
//! The recorder is an [`AudioTap`]: it receives every frame once it has
//! played, and writes it as 16-bit PCM. Between utterances the mixer plays
//! nothing, so the recorder fills the gaps with silence from the clock: a
//! block of frames that has just finished playing is placed so that it
//! ends at the time it was received. The file then runs in step with real
//! time from the moment the recorder was created, which is written beside
//! it (`<file>.start`, Unix time in milliseconds) so the recording can be
//! lined up with a screen capture made by another program.
//!
//! The header is brought up to date after every write, so the file is
//! valid even when Verbatim is ended without warning, as the end-to-end
//! harness ends it. The file is written on a thread of the recorder's own:
//! the tap runs on the audio thread, which a slow disk must never stall.

use std::fs::File;
use std::io::{self, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use tracing::warn;

use crate::{AudioTap, DeviceFormat};

/// How far a block may be off its place on the clock before silence is
/// inserted: one device period and some, so ordinary scheduling jitter
/// does not add up to clicks of silence.
const SLACK_MS: u64 = 20;

/// Writes played audio to a WAV file, in step with real time.
pub struct WavRecorder {
    blocks: Sender<(Vec<f32>, DeviceFormat, Instant)>,
}

/// The writing thread's state.
struct Writer {
    path: PathBuf,
    file: File,
    started: Instant,
    /// The format of the file, set by the first audio.
    format: Option<DeviceFormat>,
    /// Frames written so far, silence included.
    frames: u64,
    failed: bool,
}

impl WavRecorder {
    /// Creates `path` (and `<path>.start`, holding the creation time), and
    /// starts the recording's clock.
    ///
    /// # Errors
    ///
    /// Returns the error creating either file.
    pub fn create(path: &Path) -> io::Result<Self> {
        let mut file = File::create(path)?;
        // The header's place; it is filled in once there is audio.
        file.write_all(&[0; 44])?;
        // The two clocks are read together, so the start time written for
        // other programs is the one the file keeps step with.
        let started = Instant::now();
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let mut start = path.as_os_str().to_owned();
        start.push(".start");
        std::fs::write(PathBuf::from(start), now_ms.to_string())?;
        let mut writer = Writer {
            path: path.to_path_buf(),
            file,
            started,
            format: None,
            frames: 0,
            failed: false,
        };
        let (blocks, received) = mpsc::channel::<(Vec<f32>, DeviceFormat, Instant)>();
        thread::Builder::new()
            .name("verbatim-wav-recorder".to_owned())
            .spawn(move || {
                for (samples, format, played_at) in received {
                    writer.played(&samples, format, played_at);
                }
            })?;
        Ok(Self { blocks })
    }
}

impl Writer {
    fn write(
        &mut self,
        samples: &[f32],
        format: DeviceFormat,
        played_at: Instant,
    ) -> io::Result<()> {
        let format = *self.format.get_or_insert(format);
        let channels = u64::from(format.channels.max(1));
        let block = samples.len() as u64 / channels;
        // Where this block belongs: it had just finished playing when the
        // tap received it.
        let now = u64::try_from(
            played_at
                .saturating_duration_since(self.started)
                .as_millis(),
        )
        .unwrap_or(u64::MAX);
        let ends_at = now * u64::from(format.sample_rate) / 1_000;
        let starts_at = ends_at.saturating_sub(block);
        let slack = SLACK_MS * u64::from(format.sample_rate) / 1_000;
        let mut bytes = Vec::with_capacity(samples.len() * 2);
        if starts_at > self.frames + slack {
            let silence = (starts_at - self.frames) * channels;
            bytes.resize(usize::try_from(silence).unwrap_or(0) * 2, 0);
            self.frames = starts_at;
        }
        for sample in samples {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "the sample is clamped to the 16-bit range first"
            )]
            let value = (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16;
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        self.frames += block;
        self.file.seek(SeekFrom::End(0))?;
        self.file.write_all(&bytes)?;
        self.write_header(format)
    }

    /// Writes the 44-byte header for the data written so far.
    fn write_header(&mut self, format: DeviceFormat) -> io::Result<()> {
        let data_bytes =
            u32::try_from(self.file.seek(SeekFrom::End(0))?.saturating_sub(44)).unwrap_or(u32::MAX);
        let channels = format.channels.max(1);
        let block_align = channels * 2;
        let mut header = Vec::with_capacity(44);
        header.extend_from_slice(b"RIFF");
        header.extend_from_slice(&(data_bytes.saturating_add(36)).to_le_bytes());
        header.extend_from_slice(b"WAVEfmt ");
        header.extend_from_slice(&16u32.to_le_bytes());
        header.extend_from_slice(&1u16.to_le_bytes());
        header.extend_from_slice(&channels.to_le_bytes());
        header.extend_from_slice(&format.sample_rate.to_le_bytes());
        header.extend_from_slice(&(format.sample_rate * u32::from(block_align)).to_le_bytes());
        header.extend_from_slice(&block_align.to_le_bytes());
        header.extend_from_slice(&16u16.to_le_bytes());
        header.extend_from_slice(b"data");
        header.extend_from_slice(&data_bytes.to_le_bytes());
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(&header)?;
        self.file.flush()
    }
}

impl AudioTap for WavRecorder {
    fn played(&mut self, samples: &[f32], format: DeviceFormat) {
        if !samples.is_empty() {
            // The writer only ends with the recorder, so this cannot fail
            // while it matters.
            let _ = self.blocks.send((samples.to_vec(), format, Instant::now()));
        }
    }
}

impl Writer {
    fn played(&mut self, samples: &[f32], format: DeviceFormat, played_at: Instant) {
        if self.failed {
            return;
        }
        if self.format.is_some_and(|recorded| {
            (recorded.sample_rate, recorded.channels) != (format.sample_rate, format.channels)
        }) {
            // A device reopened in another format; the file keeps its first.
            warn!(target: "verbatim::audio", path = %self.path.display(), "the audio device changed format; recording stops");
            self.failed = true;
            return;
        }
        if let Err(error) = self.write(samples, format, played_at) {
            warn!(target: "verbatim::audio", path = %self.path.display(), %error, "recording audio failed; recording stops");
            self.failed = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_a_valid_wav_with_silence_where_nothing_played() {
        let dir = std::env::temp_dir().join(format!("verbatim-wav-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("audio.wav");
        let format = DeviceFormat {
            sample_rate: 1_000,
            channels: 1,
            buffer_frames: 10,
        };
        let mut recorder = WavRecorder::create(&path).unwrap();
        recorder.played(&[0.5; 100], format);
        std::thread::sleep(std::time::Duration::from_millis(300));
        recorder.played(&[0.5; 100], format);
        // Dropping the recorder ends the writer once it has written both.
        drop(recorder);
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        while std::fs::metadata(&path).map_or(0, |m| m.len()) < 44 + 2 * 200
            && Instant::now() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        std::thread::sleep(std::time::Duration::from_millis(50));

        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[36..40], b"data");
        let data = u32::from_le_bytes(bytes[40..44].try_into().unwrap()) as usize;
        assert_eq!(
            data,
            bytes.len() - 44,
            "the header matches the data written"
        );
        let frames = data / 2;
        // The second block, received 300 ms in, is placed to end there:
        // 100 frames, about 100 frames of silence, then 100 frames.
        assert!(
            (290..=330).contains(&frames),
            "silence fills the pause: {frames} frames"
        );
        let sample =
            |frame: usize| i16::from_le_bytes([bytes[44 + frame * 2], bytes[45 + frame * 2]]);
        assert!(sample(50) > 0, "the first block is audio");
        assert_eq!(sample(150), 0, "the pause is silence");
        assert!(sample(frames - 1) > 0, "the second block is audio");
        let start = std::fs::read_to_string(dir.join("audio.wav.start")).unwrap();
        assert!(
            start.parse::<u128>().is_ok(),
            "the start time is written: {start:?}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
