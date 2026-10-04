//! A video of each scenario, with Verbatim's speech (decision D16).
//!
//! [`Recording::start`] launches ffmpeg through the agent, before Verbatim,
//! capturing the desktop with `gdigrab` into fragmented MP4; it has to run
//! through the agent because only a process in the interactive session can
//! capture the desktop. Verbatim records its own audio: the launch passes
//! [`Recording::audio_env`], naming a WAV file the mixer writes everything
//! it plays into, in step with the clock, with the time it started beside
//! it. So the video has exactly what Verbatim played, wherever its audio
//! went and whatever else the machine was playing.
//!
//! [`Recording::finish`] ends the capture, lines the audio up with the
//! video from the two start times (`gdigrab` reports the wall-clock time of
//! its first frame), muxes them into one MP4 with AAC audio on the agent's
//! machine, and copies that here. The capture is ended with
//! `TerminateProcess`; the fragmented container means the file is still
//! playable, losing at most the fragment being written.
//!
//! Recording is on whenever ffmpeg can be started: set [`RECORD_ENV`] to
//! `0` to turn it off. A recording that cannot start or finish is reported
//! as a warning and never fails a scenario.

use std::io;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use verbatim_agent::protocol::ProcessState;

use crate::agent_client::AgentClient;

/// Set to `0` (or `false`) to run without recording.
pub const RECORD_ENV: &str = "VERBATIM_E2E_RECORD";

/// The ffmpeg executable on the agent's machine; `ffmpeg`, found on its
/// `PATH`, when unset. `cargo xtask vm test` sets it to the guest's copy.
pub const FFMPEG_ENV: &str = "VERBATIM_E2E_FFMPEG";

/// Set to `demo` for the smoother, sharper video `cargo xtask demo` makes
/// for the repository's `videos` folder: 30 frames a second captured
/// losslessly, then encoded with a slow preset once the scenario is over,
/// so the encoding never competes with Verbatim for the processor.
pub const QUALITY_ENV: &str = "VERBATIM_E2E_RECORD_QUALITY";

/// The variable naming the WAV file Verbatim records its audio into; read
/// by `verbatim-app`.
const AUDIO_ENV: &str = "VERBATIM_RECORD_AUDIO";

/// How long muxing may take before it is abandoned; a demo's is an encode.
const MUX_TIMEOUT: Duration = Duration::from_secs(120);
const DEMO_MUX_TIMEOUT: Duration = Duration::from_mins(15);

/// How long ffmpeg may take to exit once ended.
const EXIT_TIMEOUT: Duration = Duration::from_secs(10);

/// Whether this run records, from [`RECORD_ENV`].
#[must_use]
pub fn enabled() -> bool {
    std::env::var(RECORD_ENV).map_or(true, |value| {
        !(value == "0" || value.eq_ignore_ascii_case("false"))
    })
}

/// A capture in progress, its files in one directory on the agent's machine.
pub struct Recording {
    ffmpeg: String,
    demo: bool,
    video_pid: Option<u32>,
    video: String,
    video_log: String,
    audio: String,
    muxed: String,
    mux_log: String,
}

impl Recording {
    /// Starts capturing the desktop into files in `dir`, a directory on the
    /// agent's machine.
    ///
    /// # Errors
    ///
    /// Returns an error if ffmpeg cannot be launched.
    pub fn start(agent: &mut AgentClient, dir: &str) -> io::Result<Self> {
        let ffmpeg = std::env::var(FFMPEG_ENV).unwrap_or_else(|_| "ffmpeg".to_owned());
        let demo = std::env::var(QUALITY_ENV).is_ok_and(|value| value == "demo");
        let mut recording = Self {
            ffmpeg,
            demo,
            video_pid: None,
            video: format!(r"{dir}\recording-video.mp4"),
            video_log: format!(r"{dir}\recording-video.log"),
            audio: format!(r"{dir}\recording-audio.wav"),
            muxed: format!(r"{dir}\recording.mp4"),
            mux_log: format!(r"{dir}\recording-mux.log"),
        };
        // A previous run's start time must not be taken for this one's.
        agent.write_file(&format!("{}.start", recording.audio), b"")?;
        let pid = agent.launch_process(
            &recording.ffmpeg,
            &capture_args(&recording.video, demo),
            None,
            &[],
            Some(&recording.video_log),
        )?;
        recording.video_pid = Some(pid);
        Ok(recording)
    }

    /// The environment variable that has Verbatim record its audio for
    /// this recording.
    #[must_use]
    pub fn audio_env(&self) -> (String, String) {
        (AUDIO_ENV.to_owned(), self.audio.clone())
    }

    /// Ends the capture, muxes video and audio, and copies the result to
    /// `to` on this machine.
    ///
    /// # Errors
    ///
    /// Returns an error if the capture produced nothing usable, muxing
    /// failed, or the copy failed.
    pub fn finish(&mut self, agent: &mut AgentClient, to: &Path) -> io::Result<()> {
        self.stop(agent);
        let log = String::from_utf8_lossy(&agent.read_file(&self.video_log)?).into_owned();
        let video_start = video_start_seconds(&log).ok_or_else(|| {
            io::Error::other(format!(
                "ffmpeg's capture never started; its log ({}) ends: {}",
                self.video_log,
                tail(&log)
            ))
        })?;
        let audio_start = agent
            .read_file(&format!("{}.start", self.audio))
            .ok()
            .and_then(|bytes| String::from_utf8_lossy(&bytes).trim().parse::<u64>().ok());
        #[expect(
            clippy::cast_precision_loss,
            reason = "a Unix time in milliseconds is exact in an f64 for millennia"
        )]
        let offset = audio_start.map(|ms| ms as f64 / 1_000.0 - video_start);
        // Verbatim ended before saying anything leaves a WAV with no audio,
        // which ffmpeg cannot read: the video is still worth keeping.
        if let Err(error) = self.mux(agent, offset) {
            if offset.is_none() {
                return Err(error);
            }
            eprintln!("WARNING: {error}; saving the video without audio");
            self.mux(agent, None)?;
        }
        agent.copy_file(&self.muxed, to)
    }

    /// Muxes the capture, with the audio at `offset` unless it is `None`.
    fn mux(&self, agent: &mut AgentClient, offset: Option<f64>) -> io::Result<()> {
        let pid = agent.launch_process(
            &self.ffmpeg,
            &mux_args(&self.video, &self.audio, offset, &self.muxed, self.demo),
            None,
            &[],
            Some(&self.mux_log),
        )?;
        let timeout = if self.demo {
            DEMO_MUX_TIMEOUT
        } else {
            MUX_TIMEOUT
        };
        match wait_for_exit(agent, pid, timeout)? {
            Some(0) => Ok(()),
            code => {
                let _ = agent.kill_process(pid);
                let log = agent.read_file(&self.mux_log).unwrap_or_default();
                Err(io::Error::other(format!(
                    "muxing the recording failed ({code:?}): {}",
                    tail(&String::from_utf8_lossy(&log))
                )))
            }
        }
    }

    /// Ends the capture, if it is still running.
    pub fn stop(&mut self, agent: &mut AgentClient) {
        if let Some(pid) = self.video_pid.take() {
            let _ = agent.kill_process(pid);
            let _ = wait_for_exit(agent, pid, EXIT_TIMEOUT);
        }
    }
}

/// ffmpeg's arguments capturing the desktop into fragmented MP4 at
/// `output`: playable however it is ended. A hard stop loses only the
/// fragment being written: a fragment starts at each keyframe, one a
/// second, and the zero-latency tuning keeps the encoder from holding
/// frames back. The capture is ended after the scenario and Verbatim have
/// finished, so what is lost shows nothing of the run.
/// `demo` captures twice the frames, losslessly, for [`mux_args`] to
/// encode.
fn capture_args(output: &str, demo: bool) -> Vec<String> {
    let (framerate, crf) = if demo { ("30", "0") } else { ("15", "28") };
    [
        "-hide_banner",
        "-nostats",
        "-y",
        "-f",
        "gdigrab",
        "-framerate",
        framerate,
        "-thread_queue_size",
        "512",
        "-i",
        "desktop",
        // libx264's 4:2:0 output needs even dimensions.
        "-vf",
        "scale=trunc(iw/2)*2:trunc(ih/2)*2",
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-tune",
        "zerolatency",
        "-crf",
        crf,
        "-pix_fmt",
        "yuv420p",
        "-g",
        framerate,
        "-movflags",
        "+frag_keyframe+empty_moov+default_base_moof",
        output,
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

/// ffmpeg's arguments muxing `video` with `audio`, delayed by
/// `audio_offset` seconds (negative skips its start), into `output` with
/// AAC audio; video alone when `audio_offset` is `None`. The video is
/// copied as captured, or for a `demo` encoded with libx264's slow preset.
fn mux_args(
    video: &str,
    audio: &str,
    audio_offset: Option<f64>,
    output: &str,
    demo: bool,
) -> Vec<String> {
    let mut args: Vec<String> = ["-hide_banner", "-nostats", "-y", "-i", video]
        .into_iter()
        .map(str::to_owned)
        .collect();
    if let Some(offset) = audio_offset {
        if offset >= 0.0 {
            args.extend(["-itsoffset".to_owned(), format!("{offset:.3}")]);
        } else {
            args.extend(["-ss".to_owned(), format!("{:.3}", -offset)]);
        }
        args.extend(["-i".to_owned(), audio.to_owned()]);
        args.extend(
            ["-map", "0:v", "-map", "1:a", "-c:a", "aac", "-b:a", "128k"]
                .into_iter()
                .map(str::to_owned),
        );
    }
    let video_codec: &[&str] = if demo {
        &[
            "-c:v", "libx264", "-preset", "slow", "-crf", "20", "-pix_fmt", "yuv420p",
        ]
    } else {
        &["-c:v", "copy"]
    };
    args.extend(
        video_codec
            .iter()
            .chain(&["-movflags", "+faststart", output])
            .map(|&arg| arg.to_owned()),
    );
    args
}

/// The wall-clock time of `gdigrab`'s first frame, which ffmpeg logs as
/// the input's `start:`.
fn video_start_seconds(log: &str) -> Option<f64> {
    log.lines()
        .filter_map(|line| line.split_once("start: ").map(|(_, rest)| rest))
        .find_map(|rest| rest.split(',').next()?.trim().parse::<f64>().ok())
        .filter(|start| *start > 0.0)
}

/// Polls until `pid` exits, returning its exit code, or `Ok(None)` with it
/// still running after `timeout`.
fn wait_for_exit(agent: &mut AgentClient, pid: u32, timeout: Duration) -> io::Result<Option<i32>> {
    let deadline = Instant::now() + timeout;
    loop {
        if let ProcessState::Exited { exit_code } = agent.process_status(pid)? {
            return Ok(exit_code.or(Some(-1)));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn tail(log: &str) -> String {
    let lines: Vec<&str> = log.lines().rev().take(5).collect();
    lines.into_iter().rev().collect::<Vec<_>>().join(" | ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_capture_start_from_ffmpegs_log() {
        let log = "Input #0, gdigrab, from 'desktop':\n  Duration: N/A, start: 1791097373.167541, bitrate: 491526 kb/s\n";
        assert_eq!(video_start_seconds(log), Some(1_791_097_373.167_541));
        assert_eq!(
            video_start_seconds("Duration: N/A, start: 0.000000, bitrate"),
            None
        );
        assert_eq!(video_start_seconds("no input"), None);
    }

    #[test]
    fn delays_or_trims_the_audio_to_line_it_up() {
        let later = mux_args("v.mp4", "a.wav", Some(0.25), "out.mp4", false).join(" ");
        assert!(later.contains("-itsoffset 0.250 -i a.wav"), "{later}");
        let earlier = mux_args("v.mp4", "a.wav", Some(-0.5), "out.mp4", false).join(" ");
        assert!(earlier.contains("-ss 0.500 -i a.wav"), "{earlier}");
        let silent = mux_args("v.mp4", "a.wav", None, "out.mp4", false).join(" ");
        assert!(!silent.contains("a.wav"), "{silent}");
        assert!(silent.ends_with("-c:v copy -movflags +faststart out.mp4"));
        let demo = mux_args("v.mp4", "a.wav", None, "out.mp4", true).join(" ");
        assert!(demo.contains("-c:v libx264 -preset slow"), "{demo}");
    }
}
