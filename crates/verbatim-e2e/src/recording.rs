//! A video of each scenario, with Verbatim's speech (decision D16).
//!
//! [`Recording::start`] launches ffmpeg through the agent, before Verbatim,
//! capturing the desktop with `gdigrab` into fragmented MP4, and returns
//! once ffmpeg has captured its first frame, so the video shows Verbatim
//! start. It has to run through the agent because only a process in the
//! interactive session can capture the desktop. Verbatim records its own
//! audio in every run, recording or not, so that recording changes nothing
//! Verbatim does: the launch always passes [`audio_env`], naming a WAV file
//! the mixer writes everything it plays into, in step with the clock, with
//! the time it started beside it. So the video has exactly what Verbatim
//! played, wherever its audio went and whatever else the machine was
//! playing.
//!
//! [`Recording::finish`] stops the capture, lines the audio up with the
//! video from the two start times (`gdigrab` reports the wall-clock time of
//! its first frame), muxes them into one MP4 with AAC audio on the agent's
//! machine, copies that here with ffmpeg's logs, and checks it
//! ([`verify`]). The capture is stopped as a user stops ffmpeg, with `q` on
//! its standard input, so it encodes the frames it holds, writes its last
//! fragment, and exits. Killing it loses far more than the fragment being
//! written: the muxer holds every frame since the last keyframe in memory,
//! and the file output holds up to 32 KB of the fragment before that in a
//! buffer that is never flushed, so a killed capture loses its last one to
//! two seconds and ends in a cut keyframe. ffmpeg is killed only when it
//! does not exit within [`STOP_TIMEOUT`] of being told to stop, and that
//! fails the scenario.
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

/// How long ffmpeg may take to exit once told to stop. It took 0.4 s,
/// encoding the frames it held and writing its last fragment; the bound
/// leaves room for a loaded machine whose encoder has fallen seconds
/// behind the capture.
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
    /// When this machine saw the evidence of the first frame.
    first_frame_seen: Instant,
    /// The least the capture ran for: from `first_frame_seen` to the
    /// request to stop. Set by [`Recording::stop`].
    captured: Option<Duration>,
    video: String,
    video_log: String,
    audio: String,
    muxed: String,
    mux_log: String,
    check_log: String,
    check_progress: String,
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
            first_frame_seen: Instant::now(),
            captured: None,
            video: format!(r"{dir}\recording-video.mp4"),
            video_log: format!(r"{dir}\recording-video.log"),
            audio: audio_path(dir),
            muxed: format!(r"{dir}\recording.mp4"),
            mux_log: format!(r"{dir}\recording-mux.log"),
            check_log: format!(r"{dir}\recording-check.log"),
            check_progress: format!(r"{dir}\recording-check-progress.txt"),
        };
        // A previous run's start time must not be taken for this one's, nor
        // its capture for the evidence of this one's first frame.
        agent.write_file(&format!("{}.start", recording.audio), b"")?;
        agent.delete_file(&recording.video)?;
        let pid = agent
            .launch_with_stdin(
                &recording.ffmpeg,
                &capture_args(&recording.video, demo),
                &recording.video_log,
            )?
            .pid;
        recording.video_pid = Some(pid);
        match recording.first_frame(agent) {
            Ok(video_start) => {
                recording.video_start = video_start;
                recording.first_frame_seen = Instant::now();
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
    /// for `gdigrab` captures the first frame, and logs the input with its
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
    /// ([`verify`]). A video that was made is copied whatever the check
    /// finds, so a failed run can be watched.
    ///
    /// # Errors
    ///
    /// Returns an error if the capture did not stop cleanly, muxing failed,
    /// a copy failed, or the result fails the check.
    pub fn finish(&mut self, agent: &mut AgentClient, to: &Path) -> io::Result<()> {
        let mut problems: Vec<String> = self
            .finish_video(agent, to)
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
    fn finish_video(&mut self, agent: &mut AgentClient, to: &Path) -> io::Result<()> {
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
        self.check(agent)
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

    /// Decodes the muxed recording and checks it with [`verify`].
    fn check(&self, agent: &mut AgentClient) -> io::Result<()> {
        let args = check_args(&self.muxed, &self.check_progress);
        self.run_ffmpeg(agent, "checking the recording", &args, &self.check_log)?;
        let mut read = |path: &str| {
            agent
                .read_file(path)
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        };
        let mux_log = read(&self.mux_log)?;
        let check_log = read(&self.check_log)?;
        let progress = read(&self.check_progress)?;
        let captured = self.captured.ok_or_else(|| {
            io::Error::other("the recording was checked before the capture stopped")
        })?;
        verify(
            &mux_log,
            &check_log,
            &progress,
            captured,
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

    /// Stops the capture, if it is still running: tells ffmpeg to stop with
    /// `q` on its standard input, and waits up to [`STOP_TIMEOUT`] for it
    /// to exit with code 0. Only when it does not is it killed.
    ///
    /// # Errors
    ///
    /// Returns an error if ffmpeg could not be told to stop, did not exit
    /// within [`STOP_TIMEOUT`], or exited with an error: in the first two
    /// cases it was killed, and the capture is cut short.
    pub fn stop(&mut self, agent: &mut AgentClient) -> io::Result<()> {
        let Some(pid) = self.video_pid.take() else {
            return Ok(());
        };
        self.captured = Some(self.first_frame_seen.elapsed());
        let stopped = agent
            .write_stdin(pid, "q")
            .and_then(|()| agent.wait_for_exit(pid, STOP_TIMEOUT));
        let failure = match stopped {
            Ok(ProcessState::Exited { exit_code: Some(0) }) => return Ok(()),
            Ok(ProcessState::Exited { exit_code }) => {
                format!("ffmpeg (pid {pid}) exited with code {exit_code:?} when told to stop")
            }
            Ok(ProcessState::Running) => format!(
                "ffmpeg (pid {pid}) did not stop within {STOP_TIMEOUT:?} of being told to, so it was killed and the recording is cut short"
            ),
            Err(error) => format!(
                "ffmpeg (pid {pid}) could not be told to stop ({error}), so it was killed and the recording is cut short"
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

/// ffmpeg's arguments capturing the desktop into fragmented MP4 at
/// `output`, a fragment starting at each keyframe, one a second, with the
/// zero-latency tuning keeping the encoder from holding frames back. The
/// file is complete only once ffmpeg is told to stop and exits
/// ([`Recording::stop`]): a fragment reaches the disk only once the next
/// keyframe arrives, and then not all of it until ffmpeg flushes its file
/// buffer. `demo` captures twice the frames, losslessly, for [`mux_args`]
/// to encode.
fn capture_args(output: &str, demo: bool) -> Vec<String> {
    let framerate = framerate(demo).to_string();
    let crf = if demo { "0" } else { "28" };
    [
        "-hide_banner",
        "-nostats",
        "-y",
        "-f",
        "gdigrab",
        "-framerate",
        &framerate,
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
        &framerate,
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
/// Each line of the log is tagged with its level, such as `[error]`, for
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

/// ffmpeg's arguments decoding every stream of `recording` and discarding
/// the result, logging errors alone, and writing its progress, with the
/// count of video frames decoded, to `progress`.
fn check_args(recording: &str, progress: &str) -> Vec<String> {
    [
        "-hide_banner",
        "-nostats",
        "-v",
        "error",
        "-progress",
        progress,
        "-i",
        recording,
        "-f",
        "null",
        "-",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

/// Checks a muxed recording, from the mux's log (`mux_log`), the log of
/// decoding it with [`check_args`] (`check_log`), and that decoding's
/// `progress`. Fails when the mux logged an error or a corrupt packet,
/// which it does without failing; when decoding logged anything; or when
/// the video is shorter than the capture ran (`captured`), by more than
/// one frame at `framerate` frames a second. Returns the problem found.
fn verify(
    mux_log: &str,
    check_log: &str,
    progress: &str,
    captured: Duration,
    framerate: u32,
) -> Result<(), String> {
    let mux_problems: Vec<&str> = mux_log
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
    if !check_log.trim().is_empty() {
        return Err(format!("does not decode cleanly: {}", tail(check_log)));
    }
    let frames = decoded_frames(progress)
        .ok_or_else(|| format!("has no count of decoded frames in: {}", tail(progress)))?;
    let frame = 1.0 / f64::from(framerate);
    let length = f64::from(frames) * frame;
    if length + frame < captured.as_secs_f64() {
        return Err(format!(
            "lasts {length:.3} s ({frames} frames), but the capture ran for at least {:.3} s",
            captured.as_secs_f64()
        ));
    }
    Ok(())
}

/// The count of video frames decoded, from ffmpeg's `-progress` output: the
/// last report's `frame=`, once the last report says `progress=end`.
fn decoded_frames(progress: &str) -> Option<u32> {
    let mut frames = None;
    for line in progress.lines() {
        match line.trim().split_once('=') {
            Some(("frame", count)) => frames = count.parse().ok(),
            Some(("progress", "end")) => return frames,
            _ => {}
        }
    }
    None
}

/// The wall-clock time of `gdigrab`'s first frame, which ffmpeg logs as
/// the input's `start:`.
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

    #[test]
    fn captures_a_demo_at_thirty_frames_a_second_losslessly() {
        let usual = capture_args("v.mp4", false).join(" ");
        assert!(usual.contains("-framerate 15 "), "{usual}");
        assert!(usual.contains("-crf 28 -pix_fmt yuv420p -g 15 "), "{usual}");
        let demo = capture_args("v.mp4", true).join(" ");
        assert!(demo.contains("-framerate 30 "), "{demo}");
        assert!(demo.contains("-crf 0 -pix_fmt yuv420p -g 30 "), "{demo}");
    }

    /// The end of ffmpeg's `-progress` output for a clean four-second
    /// capture, as ffmpeg 9 writes it.
    const PROGRESS: &str =
        "frame=30\nprogress=continue\nframe=63\nfps=0.00\nout_time_us=4200000\nprogress=end\n";

    #[test]
    fn counts_the_frames_decoded_once_decoding_ended() {
        assert_eq!(decoded_frames(PROGRESS), Some(63));
        assert_eq!(decoded_frames("frame=30\nprogress=continue\n"), None);
        assert_eq!(decoded_frames(""), None);
    }

    #[test]
    fn passes_a_clean_recording_as_long_as_the_capture() {
        let mux_log =
            "[info] Output #0, mp4, to 'recording.mp4':\n[info] [out#0/mp4 @ 0] video:1954KiB\n";
        // 63 frames at 15 a second last 4.2 s; a capture of 4.25 s is
        // within one frame of that.
        assert_eq!(
            verify(mux_log, "", PROGRESS, Duration::from_millis(4_250), 15),
            Ok(())
        );
    }

    #[test]
    fn fails_a_recording_shorter_than_the_capture_by_more_than_a_frame() {
        assert_eq!(
            verify("", "", PROGRESS, Duration::from_millis(4_300), 15),
            Err("lasts 4.200 s (63 frames), but the capture ran for at least 4.300 s".to_owned())
        );
    }

    #[test]
    fn fails_a_recording_muxed_from_a_cut_keyframe() {
        let mux_log = "[info] Stream mapping:\n[mov,mp4,m4a,3gp,3g2,mj2 @ 0] [warning] Packet corrupt (stream = 0, dts = 153600).\n[in#0/mov,mp4,m4a,3gp,3g2,mj2 @ 0] [warning] corrupt input packet in stream 0\n";
        assert_eq!(
            verify(mux_log, "", PROGRESS, Duration::from_secs(4), 15),
            Err("was muxed from a damaged capture: [mov,mp4,m4a,3gp,3g2,mj2 @ 0] [warning] Packet corrupt (stream = 0, dts = 153600). | [in#0/mov,mp4,m4a,3gp,3g2,mj2 @ 0] [warning] corrupt input packet in stream 0".to_owned())
        );
        assert_eq!(
            verify(
                "[aac @ 0] [error] Too many bits\n",
                "",
                PROGRESS,
                Duration::from_secs(4),
                15
            ),
            Err("was muxed from a damaged capture: [aac @ 0] [error] Too many bits".to_owned())
        );
    }

    #[test]
    fn fails_a_recording_that_does_not_decode_cleanly() {
        let check_log = "[h264 @ 0] Invalid NAL unit size (46141 > 26830).\n[h264 @ 0] Error splitting the input into NAL units.\n";
        assert_eq!(
            verify("", check_log, PROGRESS, Duration::from_secs(4), 15),
            Err("does not decode cleanly: [h264 @ 0] Invalid NAL unit size (46141 > 26830). | [h264 @ 0] Error splitting the input into NAL units.".to_owned())
        );
    }
}
