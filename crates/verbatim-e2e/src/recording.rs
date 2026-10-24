//! A video of each scenario, with Verbatim's speech (decision D16).
//!
//! [`Recording::start`] launches the agent's desktop capture
//! (`verbatim-agent capture-desktop`, `verbatim_agent::capture`) through the
//! agent, before Verbatim, and returns once ffmpeg has its first frame, so
//! the video shows Verbatim start. It has to run through the agent because
//! only a process in the interactive session can capture the desktop. The
//! capture duplicates the screen with DXGI Desktop Duplication and writes a
//! raw frame to an ffmpeg child at a steady rate, which ffmpeg encodes into
//! fragmented MP4 with the arguments [`capture_args`] gives. It does not use
//! ffmpeg's `gdigrab`, whose GDI blit of the screen waits while the
//! compositor renders: on a machine without 3D graphics that held it for
//! seconds while the console host drew a flood. Verbatim
//! records its own audio in every run, recording or not, so that recording
//! changes nothing Verbatim does: the launch always passes [`audio_env`],
//! naming a WAV file the mixer writes everything it plays into, in step
//! with the clock, with the time it started beside it. So the video has
//! exactly what Verbatim played, wherever its audio went and whatever else
//! the machine was playing.
//!
//! [`Recording::finish`] stops the capture, lines the audio up with the
//! video from the two start times (ffmpeg stamps each frame with the
//! wall-clock time it reads it, and logs the first as its input's start, to
//! the frame interval), muxes them into one MP4 with AAC audio on the
//! agent's machine, copies that here with ffmpeg's logs, and checks it
//! ([`verify`]). The capture is stopped with `q` on its standard input: it
//! closes ffmpeg's input, and ffmpeg encodes every frame it was given,
//! writes its last fragment, closes the file and exits with code 0, the
//! capture with it. The check asks that every encoded frame reached the
//! file and that the video reaches the end of the scenario's body. Killing
//! ffmpeg loses frames it did encode: the muxer holds every frame since the
//! last keyframe in memory, and the file output holds up to 32 KB of the
//! fragment before that in a buffer that is never flushed, so a killed
//! capture loses its last one to two seconds of encoded frames and ends in
//! a cut keyframe. The capture is killed only when it does not exit within
//! [`STOP_TIMEOUT`] of being told to stop, and that fails the scenario.
//!
//! Every scenario, recorded or not, starts from the desktop with every
//! window minimized (`crate::scenario::Scenario::launch`), so the video
//! shows the scenario's own windows; recording itself changes nothing on
//! the desktop.
//!
//! Recording is on unless [`RECORD_ENV`] is `0`. When it is on, a
//! recording that cannot start or finish, or that fails the check, fails
//! the scenario.

use std::io;
use std::path::Path;
use std::time::Duration;

use verbatim_agent::protocol::{AGENT_SELF, ProcessState};

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

/// The WAV file of Verbatim's audio in the run directory `dir`.
#[must_use]
pub fn audio_path(dir: &str) -> String {
    format!(r"{dir}\recording-audio.wav")
}

/// The variable that has Verbatim record its audio into [`audio_path`], set
/// on every run so recording changes nothing Verbatim does.
#[must_use]
pub fn audio_env(dir: &str) -> (String, String) {
    (AUDIO_ENV.to_owned(), audio_path(dir))
}

/// How long muxing, or decoding the result to check it, may take before it
/// is abandoned; a demo's mux is an encode.
const MUX_TIMEOUT: Duration = Duration::from_secs(120);
const DEMO_MUX_TIMEOUT: Duration = Duration::from_mins(15);

/// How long ffmpeg may take to capture its first frame once launched. It
/// took 0.2 s locally, through a Chocolatey shim; the bound, the same as
/// Verbatim's own start is given, leaves room for a cold start on a hosted
/// runner or a fresh VM, where the executable is scanned before it runs.
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the capture may take to exit once told to stop. It took 0.04 s
/// (2026-10-24), ffmpeg finishing its encoder and writing its last fragment;
/// the bound leaves room for a loaded machine whose encoder has fallen
/// seconds behind the capture.
pub const STOP_TIMEOUT: Duration = Duration::from_secs(15);

/// How long a killed ffmpeg may take to exit.
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
    /// The wall-clock time of the first frame, as ffmpeg logged it.
    video_start: f64,
    video: String,
    video_log: String,
    audio: String,
    muxed: String,
    mux_log: String,
    check_log: String,
    check_frames: String,
}

impl Recording {
    /// Starts capturing the desktop into files in `dir`, a directory on the
    /// agent's machine, and returns once ffmpeg has captured its first
    /// frame.
    ///
    /// # Errors
    ///
    /// Returns an error if ffmpeg cannot be launched, or does not capture a
    /// frame within [`FIRST_FRAME_TIMEOUT`], when it is stopped.
    pub fn start(agent: &mut AgentClient, dir: &str) -> io::Result<Self> {
        let ffmpeg = std::env::var(FFMPEG_ENV).unwrap_or_else(|_| "ffmpeg".to_owned());
        let demo = std::env::var(QUALITY_ENV).is_ok_and(|value| value == "demo");
        let mut recording = Self {
            ffmpeg,
            demo,
            video_pid: None,
            video_start: 0.0,
            video: format!(r"{dir}\recording-video.mp4"),
            video_log: format!(r"{dir}\recording-video.log"),
            audio: audio_path(dir),
            muxed: format!(r"{dir}\recording.mp4"),
            mux_log: format!(r"{dir}\recording-mux.log"),
            check_log: format!(r"{dir}\recording-check.log"),
            check_frames: format!(r"{dir}\recording-check-frames.txt"),
        };
        // A previous run's start time must not be taken for this one's, nor
        // its capture for the evidence of this one's first frame.
        agent.write_file(&format!("{}.start", recording.audio), b"")?;
        agent.delete_file(&recording.video)?;
        let mut args: Vec<String> = [
            "capture-desktop",
            "--framerate",
            &framerate(demo).to_string(),
            "--ffmpeg",
            &recording.ffmpeg,
            "--",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        args.extend(capture_args(&recording.video, demo));
        let pid = agent
            .launch_with_stdin(AGENT_SELF, &args, &recording.video_log)?
            .pid;
        recording.video_pid = Some(pid);
        match recording.first_frame(agent) {
            Ok(video_start) => {
                recording.video_start = video_start;
                Ok(recording)
            }
            Err(error) => match recording.stop(agent) {
                Ok(()) => Err(error),
                Err(stop_error) => Err(io::Error::other(format!("{error}; then {stop_error}"))),
            },
        }
    }

    /// Waits for the evidence of the capture's first frame, and returns the
    /// wall-clock time ffmpeg logged for it. ffmpeg opens its input, which
    /// reads the capture's first frames, and logs the input with its
    /// `start:` time before it creates its output file, so the output
    /// file's appearance is the evidence; the time is then read from the
    /// log.
    fn first_frame(&self, agent: &mut AgentClient) -> io::Result<f64> {
        let created = agent.wait_for_file(&self.video, FIRST_FRAME_TIMEOUT)?;
        let log = String::from_utf8_lossy(&agent.read_file(&self.video_log)?).into_owned();
        if !created {
            return Err(io::Error::other(format!(
                "ffmpeg did not capture its first frame within {FIRST_FRAME_TIMEOUT:?}; its log ({}) ends: {}",
                self.video_log,
                tail(&log)
            )));
        }
        video_start_seconds(&log).ok_or_else(|| {
            io::Error::other(format!(
                "ffmpeg created {} without logging its first frame's time; its log ({}) ends: {}",
                self.video,
                self.video_log,
                tail(&log)
            ))
        })
    }

    /// Stops the capture, muxes video and audio, copies the result to `to`
    /// on this machine with ffmpeg's logs beside it, and checks the result
    /// ([`verify`]), which must reach `body_end_ms`, when the scenario's
    /// body ended, in milliseconds since the Unix epoch. A video that was
    /// made is copied whatever the check finds, so a failed run can be
    /// watched.
    ///
    /// # Errors
    ///
    /// Returns an error if the capture did not stop cleanly, muxing failed,
    /// a copy failed, or the result fails the check.
    pub fn finish(
        &mut self,
        agent: &mut AgentClient,
        to: &Path,
        body_end_ms: u64,
    ) -> io::Result<()> {
        let mut problems: Vec<String> = self
            .finish_video(agent, to, body_end_ms)
            .err()
            .into_iter()
            .map(|error| error.to_string())
            .collect();
        let folder = to.parent().unwrap_or_else(|| Path::new("."));
        for log in [&self.video_log, &self.mux_log, &self.check_log] {
            let name = log.rsplit('\\').next().unwrap_or(log);
            if let Err(error) = agent.copy_file(log, &folder.join(name)) {
                problems.push(format!("could not copy {log}: {error}"));
            }
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(io::Error::other(problems.join("; ")))
        }
    }

    /// What [`Recording::finish`] does before it copies the logs.
    fn finish_video(
        &mut self,
        agent: &mut AgentClient,
        to: &Path,
        body_end_ms: u64,
    ) -> io::Result<()> {
        // A capture that had to be killed is still muxed, for what it shows.
        let stopped = self.stop(agent);
        let audio_start = agent
            .read_file(&format!("{}.start", self.audio))
            .ok()
            .and_then(|bytes| String::from_utf8_lossy(&bytes).trim().parse::<u64>().ok());
        #[expect(
            clippy::cast_precision_loss,
            reason = "a Unix time in milliseconds is exact in an f64 for millennia"
        )]
        let offset = audio_start.map(|ms| ms as f64 / 1_000.0 - self.video_start);
        let offset = offset.ok_or_else(|| {
            io::Error::other(format!(
                "Verbatim's audio has no start time beside {}",
                self.audio
            ))
        })?;
        self.mux(agent, offset)?;
        agent.copy_file(&self.muxed, to)?;
        stopped?;
        self.check(agent, body_end_ms)
    }

    /// Muxes the capture, with the audio at `offset`.
    fn mux(&self, agent: &mut AgentClient, offset: f64) -> io::Result<()> {
        let args = mux_args(
            &self.video,
            &self.audio,
            Some(offset),
            &self.muxed,
            self.demo,
        );
        self.run_ffmpeg(agent, "muxing the recording", &args, &self.mux_log)
    }

    /// Decodes the muxed recording and checks it with [`verify`], against
    /// the end of the scenario's body, `body_end_ms`.
    fn check(&self, agent: &mut AgentClient, body_end_ms: u64) -> io::Result<()> {
        let args = check_args(&self.muxed, &self.check_frames);
        self.run_ffmpeg(agent, "checking the recording", &args, &self.check_log)?;
        let mut read = |path: &str| {
            agent
                .read_file(path)
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        };
        let video_log = read(&self.video_log)?;
        let mux_log = read(&self.mux_log)?;
        let check_log = read(&self.check_log)?;
        let frames = read(&self.check_frames)?;
        #[expect(
            clippy::cast_precision_loss,
            reason = "a Unix time in milliseconds is exact in an f64 for millennia"
        )]
        let body_end = body_end_ms as f64 / 1_000.0;
        verify(
            &Logs {
                video: &video_log,
                mux: &mux_log,
                check: &check_log,
                frames: &frames,
            },
            self.video_start,
            body_end,
            framerate(self.demo),
        )
        .map_err(|problem| io::Error::other(format!("the recording {}: {problem}", self.muxed)))
    }

    /// Runs ffmpeg with `args`, its output in `log`, and waits up to the
    /// mux timeout for it to exit with code 0.
    fn run_ffmpeg(
        &self,
        agent: &mut AgentClient,
        what: &str,
        args: &[String],
        log: &str,
    ) -> io::Result<()> {
        let pid = agent
            .launch_process(&self.ffmpeg, args, None, &[], Some(log))?
            .pid;
        let timeout = if self.demo {
            DEMO_MUX_TIMEOUT
        } else {
            MUX_TIMEOUT
        };
        match agent.wait_for_exit(pid, timeout)? {
            ProcessState::Exited { exit_code: Some(0) } => Ok(()),
            state => {
                let killed = agent.kill_process(pid);
                let log = agent.read_file(log)?;
                Err(io::Error::other(format!(
                    "{what} failed ({state:?}, ended: {killed:?}): {}",
                    tail(&String::from_utf8_lossy(&log))
                )))
            }
        }
    }

    /// Stops the capture, if it is still running: tells it to stop with `q`
    /// on its standard input, and waits up to [`STOP_TIMEOUT`] for it to
    /// exit with code 0, which it does once ffmpeg has. Only when it does
    /// not is it killed, ffmpeg with it.
    ///
    /// # Errors
    ///
    /// Returns an error if the capture could not be told to stop, did not exit
    /// within [`STOP_TIMEOUT`], or exited with an error: in the first two
    /// cases it was killed, and the capture is cut short.
    pub fn stop(&mut self, agent: &mut AgentClient) -> io::Result<()> {
        let Some(pid) = self.video_pid.take() else {
            return Ok(());
        };
        let stopped = agent
            .write_stdin(pid, "q")
            .and_then(|()| agent.wait_for_exit(pid, STOP_TIMEOUT));
        let failure = match stopped {
            Ok(ProcessState::Exited { exit_code: Some(0) }) => return Ok(()),
            Ok(ProcessState::Exited { exit_code }) => {
                format!("the capture (pid {pid}) exited with code {exit_code:?} when told to stop")
            }
            Ok(ProcessState::Running) => format!(
                "the capture (pid {pid}) did not stop within {STOP_TIMEOUT:?} of being told to, so it was killed and the recording is cut short"
            ),
            Err(error) => format!(
                "the capture (pid {pid}) could not be told to stop ({error}), so it was killed and the recording is cut short"
            ),
        };
        agent.kill_process(pid)?;
        if agent.wait_for_exit(pid, EXIT_TIMEOUT)? == ProcessState::Running {
            return Err(io::Error::other(format!(
                "{failure}; it did not exit within {EXIT_TIMEOUT:?} of being killed"
            )));
        }
        let log = agent.read_file(&self.video_log)?;
        Err(io::Error::other(format!(
            "{failure}; its log ({}) ends: {}",
            self.video_log,
            tail(&String::from_utf8_lossy(&log))
        )))
    }
}

/// The capture's frame rate, in frames a second; a demo's is twice the
/// usual.
fn framerate(demo: bool) -> u32 {
    if demo { 30 } else { 15 }
}

/// ffmpeg's arguments encoding the capture's raw frames into fragmented
/// MP4 at `output`, after the raw input the capture itself names
/// (`verbatim_agent::capture`), a fragment starting at each keyframe, one
/// a second, with the zero-latency tuning keeping the encoder from holding
/// frames back.
///
/// Every frame keeps the wall-clock time ffmpeg read it at, so the video
/// keeps real time however far the capture falls behind on a loaded
/// machine: ffmpeg's default would give each frame the next slot of the
/// frame rate, and stretch or squeeze time.
///
/// The file is complete only once the capture is told to stop and ffmpeg
/// exits ([`Recording::stop`]): a fragment reaches the disk only once the
/// next keyframe arrives, and then not all of it until ffmpeg flushes its
/// file buffer. `demo` captures twice the frames, losslessly, for
/// [`mux_args`] to encode.
fn capture_args(output: &str, demo: bool) -> Vec<String> {
    let framerate = framerate(demo).to_string();
    let crf = if demo { "0" } else { "28" };
    [
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
        &framerate,
        "-fps_mode",
        "passthrough",
        "-enc_time_base",
        "1:1000",
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
/// copied as captured, or for a `demo` encoded with libx264's slow preset,
/// each frame keeping its time as [`capture_args`] does. Each line of the log is tagged with its level, such as `[error]`, for
/// [`verify`].
fn mux_args(
    video: &str,
    audio: &str,
    audio_offset: Option<f64>,
    output: &str,
    demo: bool,
) -> Vec<String> {
    let mut args: Vec<String> = [
        "-hide_banner",
        "-nostats",
        "-loglevel",
        "level+info",
        "-y",
        "-i",
        video,
    ]
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
            "-c:v",
            "libx264",
            "-preset",
            "slow",
            "-crf",
            "20",
            "-pix_fmt",
            "yuv420p",
            "-fps_mode",
            "passthrough",
            "-enc_time_base",
            "1:1000",
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

/// ffmpeg's arguments decoding every stream of `recording` and discarding
/// the result, logging errors alone, and listing each video frame with its
/// time and duration in `frames` (ffmpeg's `framecrc` format). The decoded
/// frames keep their times, which ffmpeg's default would round to the
/// frame interval, logging an error for two frames rounded onto one time.
fn check_args(recording: &str, frames: &str) -> Vec<String> {
    [
        "-hide_banner",
        "-nostats",
        "-v",
        "error",
        "-y",
        "-i",
        recording,
        "-fps_mode",
        "passthrough",
        "-enc_time_base",
        "demux",
        "-f",
        "null",
        "-",
        "-map",
        "0:v",
        "-c:v",
        "copy",
        "-f",
        "framecrc",
        frames,
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

/// The logs and frame list [`verify`] checks a recording from, as ffmpeg
/// wrote them on the agent's machine.
struct Logs<'a> {
    /// The capture's log, which ends with ffmpeg's final stats line.
    video: &'a str,
    /// The mux's log, each line tagged with its level ([`mux_args`]).
    mux: &'a str,
    /// The log of decoding the muxed recording ([`check_args`]).
    check: &'a str,
    /// The muxed recording's video frames, in ffmpeg's `framecrc` format.
    frames: &'a str,
}

/// Checks a muxed recording from its [`Logs`]. Fails when the mux logged
/// an error or a corrupt packet, which it does without failing; when
/// decoding logged anything; when the recording lacks a frame ffmpeg
/// encoded; or when it ends before the scenario's body did. Returns the
/// problem found.
///
/// That every frame ffmpeg encoded reached the file is checked against
/// the final stats line of the capture's log: its frame count must equal
/// the number of frames the recording lists, and its `time=`, the end of
/// the last encoded frame since the first frame, printed to the hundredth
/// of a second, must be within one frame at `framerate` frames a second of
/// the end of the last listed frame, its time plus one frame (a raw frame
/// lasts one frame interval). A capture killed before it wrote its last
/// fragments fails this.
///
/// That the recording covers the scenario is checked on the wall clock:
/// `video_start`, the time ffmpeg logged for the first frame, plus the
/// last frame's time must be at or after `body_end`, when the scenario's
/// body ended, both in seconds since the Unix epoch.
fn verify(logs: &Logs, video_start: f64, body_end: f64, framerate: u32) -> Result<(), String> {
    let mux_problems: Vec<&str> = logs
        .mux
        .lines()
        .filter(|line| {
            [
                "[error]",
                "[fatal]",
                "Packet corrupt",
                "corrupt input packet",
            ]
            .iter()
            .any(|sign| line.contains(sign))
        })
        .collect();
    if !mux_problems.is_empty() {
        return Err(format!(
            "was muxed from a damaged capture: {}",
            mux_problems.join(" | ")
        ));
    }
    if !logs.check.trim().is_empty() {
        return Err(format!("does not decode cleanly: {}", tail(logs.check)));
    }
    let (last, count) = video_frames(logs.frames).ok_or_else(|| {
        format!(
            "lists no video frames that can be read: {}",
            tail(logs.frames)
        )
    })?;
    let (encoded, encoded_last) = final_stats(logs.video).ok_or_else(|| {
        format!(
            "cannot be compared with the capture, whose log has no final stats line: {}",
            tail(logs.video)
        )
    })?;
    if count != encoded {
        return Err(format!("has {count} frames, but ffmpeg encoded {encoded}"));
    }
    let frame = 1.0 / f64::from(framerate);
    if (last + frame - encoded_last).abs() > frame {
        return Err(format!(
            "ends with a frame at {last:.3} s, but ffmpeg's last encoded frame ended at {encoded_last:.2} s"
        ));
    }
    let end = video_start + last;
    if end < body_end {
        return Err(format!(
            "ends {:.3} s before the scenario's body did",
            body_end - end
        ));
    }
    Ok(())
}

/// The time in seconds of the last video frame `frames` lists, in ffmpeg's
/// `framecrc` format, since its first frame, with the count of frames;
/// `None` when it lists none.
fn video_frames(frames: &str) -> Option<(f64, usize)> {
    let mut time_base = None;
    let mut span: Option<(i64, i64)> = None;
    let mut count = 0;
    for line in frames.lines() {
        if let Some(base) = line.strip_prefix("#tb 0: ") {
            let (numerator, denominator) = base.trim().split_once('/')?;
            time_base = Some((
                numerator.parse::<i64>().ok()?,
                denominator.parse::<i64>().ok()?,
            ));
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line.split(',').map(str::trim).collect();
        let [_stream, _dts, pts, ..] = fields.as_slice() else {
            continue;
        };
        let pts: i64 = pts.parse().ok()?;
        span = Some(span.map_or((pts, pts), |(first, last)| (first.min(pts), last.max(pts))));
        count += 1;
    }
    let (numerator, denominator) = time_base?;
    let (first, last) = span?;
    #[expect(
        clippy::cast_precision_loss,
        reason = "a video's length in its time base is far below 2^52"
    )]
    let last = (last - first) as f64 * numerator as f64 / denominator as f64;
    Some((last, count))
}

/// The frame count, and the time in seconds of the last encoded frame
/// since the first, from ffmpeg's final stats line in `log`, such as
/// `frame=   77 fps= 15 q=26.0 Lsize=    1955KiB time=00:00:05.07`, which
/// ffmpeg writes as it exits even with `-nostats`; `None` when the log
/// has none.
fn final_stats(log: &str) -> Option<(usize, f64)> {
    let line = log
        .split(['\r', '\n'])
        .rfind(|line| line.trim_start().starts_with("frame="))?;
    let value = |name: &str| line.split_once(name)?.1.split_whitespace().next();
    let frames = value("frame=")?.parse::<usize>().ok()?;
    let mut clock = value("time=")?.split(':');
    let (hours, minutes, seconds) = (
        clock.next()?.parse::<u32>().ok()?,
        clock.next()?.parse::<u32>().ok()?,
        clock.next()?.parse::<f64>().ok()?,
    );
    if clock.next().is_some() {
        return None;
    }
    Some((frames, f64::from(hours * 3_600 + minutes * 60) + seconds))
}

/// The wall-clock time ffmpeg read the capture's first frame at, which it
/// logs as the input's `start:`.
fn video_start_seconds(log: &str) -> Option<f64> {
    log.lines()
        .filter_map(|line| line.split_once("start: ").map(|(_, rest)| rest))
        .find_map(|rest| rest.split(',').next()?.trim().parse::<f64>().ok())
        .filter(|start| *start > 0.0)
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
        let log = "Input #0, rawvideo, from 'pipe:0':\n  Duration: N/A, start: 1792822606.666667, bitrate: 491520 kb/s\n";
        assert_eq!(video_start_seconds(log), Some(1_792_822_606.666_667));
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

    #[test]
    fn captures_a_demo_at_thirty_frames_a_second_losslessly() {
        assert_eq!((framerate(false), framerate(true)), (15, 30));
        let usual = capture_args("v.mp4", false).join(" ");
        assert!(usual.contains("-crf 28 -pix_fmt yuv420p -g 15 "), "{usual}");
        let demo = capture_args("v.mp4", true).join(" ");
        assert!(demo.contains("-crf 0 -pix_fmt yuv420p -g 30 "), "{demo}");
    }

    #[test]
    fn keeps_each_frame_at_the_time_it_was_captured() {
        let capture = capture_args("v.mp4", false).join(" ");
        assert!(
            capture.contains("-fps_mode passthrough -enc_time_base 1:1000 "),
            "{capture}"
        );
        let demo = mux_args("v.mp4", "a.wav", None, "out.mp4", true).join(" ");
        assert!(
            demo.contains("-fps_mode passthrough -enc_time_base 1:1000 "),
            "{demo}"
        );
    }

    /// ffmpeg 9's `framecrc` list of a capture of four frames, the last at
    /// 4.133 s, the third taken 3.9 s before the fourth because the capture
    /// fell behind.
    const FRAMES: &str = "#extradata 0:       37, 0xda620af1\n#software: Lavf63.1.102\n#tb 0: 1/16000\n#media_type 0: video\n#codec_id 0: h264\n#dimensions 0: 1280x800\n#sar 0: 0/1\n0,          0,          0,     1360,   230363, 0xedcabc3e\n0,       1360,       1360,     1648,      152, 0xbd631c51, F=0x0\n0,       3008,       3008,    63125,     2109, 0x2c90f08b, F=0x0\n0,      66133,      66133,     1067,     2985, 0x1977aa9c, F=0x0\n";

    /// The end of the capture's log for [`FRAMES`], as ffmpeg 9 writes it
    /// when told to stop.
    const VIDEO_LOG: &str = "[q] command received. Exiting.\n\n[out#0/mp4 @ 0] video:1952KiB audio:0KiB subtitle:0KiB other streams:0KiB global headers:0KiB muxing overhead: 0.152643%\nframe=    4 fps= 15 q=26.0 Lsize=    1955KiB time=00:00:04.20 bitrate=3153.7kbits/s speed=0.982x elapsed=0:00:04.20    \n[libx264 @ 0] frame I:1     Avg QP:17.33  size:282568\n";

    /// The wall-clock time of [`FRAMES`]' first frame.
    const START: f64 = 1_791_097_373.0;

    /// [`verify`] on [`FRAMES`] at 15 frames a second, with the logs given.
    fn verify_frames(video: &str, mux: &str, check: &str, body_end: f64) -> Result<(), String> {
        let logs = Logs {
            video,
            mux,
            check,
            frames: FRAMES,
        };
        verify(&logs, START, body_end, 15)
    }

    #[test]
    fn measures_the_video_from_its_frames_times() {
        assert_eq!(video_frames(FRAMES), Some((4.133_312_5, 4)));
        assert_eq!(video_frames("#tb 0: 1/16000\n"), None);
        assert_eq!(video_frames(""), None);
    }

    #[test]
    fn reads_ffmpegs_final_stats() {
        assert_eq!(final_stats(VIDEO_LOG), Some((4, 4.2)));
        assert_eq!(
            final_stats("frame=   77 fps= 15 time=01:02:05.07 bitrate=N/A\n"),
            Some((77, 3_725.07))
        );
        assert_eq!(final_stats("frame=   77 fps= 15 time=N/A\n"), None);
        assert_eq!(final_stats("[q] command received. Exiting.\n"), None);
    }

    #[test]
    fn passes_a_clean_recording_that_covers_the_body() {
        let mux_log =
            "[info] Output #0, mp4, to 'recording.mp4':\n[info] [out#0/mp4 @ 0] video:1954KiB\n";
        // The last frame, at 4.133 s, is after the body ended at 4.1 s.
        assert_eq!(verify_frames(VIDEO_LOG, mux_log, "", START + 4.1), Ok(()));
    }

    #[test]
    fn fails_a_recording_without_every_frame_ffmpeg_encoded() {
        let one_more = VIDEO_LOG.replace("frame=    4 ", "frame=    5 ");
        assert_eq!(
            verify_frames(&one_more, "", "", START),
            Err("has 4 frames, but ffmpeg encoded 5".to_owned())
        );
        // A last frame ending more than a frame (0.067 s) from 4.2 s.
        let later = VIDEO_LOG.replace("time=00:00:04.20", "time=00:00:04.28");
        assert_eq!(
            verify_frames(&later, "", "", START),
            Err(
                "ends with a frame at 4.133 s, but ffmpeg's last encoded frame ended at 4.28 s"
                    .to_owned()
            )
        );
        assert_eq!(
            verify_frames("[q] command received. Exiting.\n", "", "", START),
            Err("cannot be compared with the capture, whose log has no final stats line: [q] command received. Exiting.".to_owned())
        );
    }

    #[test]
    fn fails_a_recording_that_ends_before_the_body() {
        assert_eq!(
            verify_frames(VIDEO_LOG, "", "", START + 4.25),
            Err("ends 0.117 s before the scenario's body did".to_owned())
        );
    }

    #[test]
    fn fails_a_recording_muxed_from_a_cut_keyframe() {
        let mux_log = "[info] Stream mapping:\n[mov,mp4,m4a,3gp,3g2,mj2 @ 0] [warning] Packet corrupt (stream = 0, dts = 153600).\n[in#0/mov,mp4,m4a,3gp,3g2,mj2 @ 0] [warning] corrupt input packet in stream 0\n";
        assert_eq!(
            verify_frames(VIDEO_LOG, mux_log, "", START),
            Err("was muxed from a damaged capture: [mov,mp4,m4a,3gp,3g2,mj2 @ 0] [warning] Packet corrupt (stream = 0, dts = 153600). | [in#0/mov,mp4,m4a,3gp,3g2,mj2 @ 0] [warning] corrupt input packet in stream 0".to_owned())
        );
        assert_eq!(
            verify_frames(VIDEO_LOG, "[aac @ 0] [error] Too many bits\n", "", START),
            Err("was muxed from a damaged capture: [aac @ 0] [error] Too many bits".to_owned())
        );
    }

    #[test]
    fn fails_a_recording_that_does_not_decode_cleanly() {
        let check_log = "[h264 @ 0] Invalid NAL unit size (46141 > 26830).\n[h264 @ 0] Error splitting the input into NAL units.\n";
        assert_eq!(
            verify_frames(VIDEO_LOG, "", check_log, START),
            Err("does not decode cleanly: [h264 @ 0] Invalid NAL unit size (46141 > 26830). | [h264 @ 0] Error splitting the input into NAL units.".to_owned())
        );
    }
}
