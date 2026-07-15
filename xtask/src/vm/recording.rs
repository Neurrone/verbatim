//! `cargo xtask vm test --record`: launches ffmpeg inside the guest's
//! interactive session to capture desktop video and VB-CABLE loopback
//! audio for the duration of one scenario, then pulls the result back to
//! the host as a playable file named after that scenario.
//!
//! Milestone M3 Track B moved the recording boundary from the whole suite
//! to one scenario at a time: `xtask::vm::test` now starts and stops a
//! recording around each scenario's own `cargo test` subprocess
//! individually — which is also that scenario's setup and teardown, since
//! both run inside the same subprocess — rather than one recording
//! wrapping every scenario in the run. [`pull_recording`] takes the
//! scenario's name for exactly this reason: it is the file name prefix
//! `docs/tooling.md` documents, not an afterthought.
//!
//! ffmpeg must be launched through the in-guest agent's `LaunchProcess`,
//! not PowerShell Direct: `gdigrab` needs a real interactive desktop to
//! capture, and PowerShell Direct (like `WinRM`) runs in a non-interactive
//! session with no desktop at all — the same session-isolation rule
//! `docs/tooling.md`'s Troubleshooting section documents for every other
//! part of this harness. This module therefore speaks the agent's own wire
//! protocol (`verbatim_agent::protocol`) directly for the three requests it
//! needs (`Hello`, `LaunchProcess`, `ProcessStatus`, `KillProcess`), rather
//! than depending on `verbatim-e2e`'s `AgentClient` — that crate is
//! test-only, and pulling it into this binary for three request kinds
//! would be a heavier dependency than a small client of its own.
//!
//! The recording is written as fragmented MP4
//! (`+frag_keyframe+empty_moov+default_base_moof`) specifically so a hard
//! [`stop_recording`] (`TerminateProcess` via the existing `KillProcess`
//! request, not a graceful stdin `q`) still leaves a playable file: each
//! completed fragment is independently valid, so the worst a mid-fragment
//! kill costs is the last, still-in-progress fragment, never the whole
//! file. This sidesteps needing a new graceful-stop mechanism in the agent
//! protocol.

use std::io::BufReader;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{fs, thread};

use verbatim_agent::protocol::{
    AGENT_PROTOCOL_VERSION, Frame, ProcessState, ReplyPayload, Request, RequestEnvelope,
};
use verbatim_control::protocol::{read_message, write_message};

use super::dotenv::GuestCredentials;
use super::host::Host;
use super::{VERBATIM_DIR, VM_NAME, VmResult};

/// ffmpeg's install path in the guest, matching
/// `vm/scripts/Initialize-VerbatimHarness.ps1`'s `Install-Ffmpeg`.
const FFMPEG_GUEST_PATH: &str = r"C:\VerbatimLab\tools\ffmpeg.exe";

/// ffprobe's install path in the guest, next to ffmpeg — installed by the
/// same `Install-Ffmpeg` function, from the same downloaded build.
const FFPROBE_GUEST_PATH: &str = r"C:\VerbatimLab\tools\ffprobe.exe";

/// `vm/scripts/Set-DefaultAudioRenderDevice.ps1`'s content, embedded at
/// compile time and sent over PowerShell Direct by [`pin_default_render_device`].
/// See that file's own doc comment for why the guest's default render
/// device needs pinning explicitly rather than trusted as incidental (a
/// `cargo xtask vm connect` session can add a Remote Audio endpoint and
/// switch the default to it), and why this is `include_str!`-ed rather
/// than read from a guest-side copy: unlike
/// `vm/scripts/Initialize-VerbatimHarness.ps1`, which can only reach it via
/// a Packer `file` provisioner uploading a persistent guest copy first,
/// this process runs on the host and can just send the text directly, so it
/// works even against a guest whose golden checkpoint predates that
/// provisioner change.
const DEFAULT_AUDIO_DEVICE_PIN_SCRIPT: &str =
    include_str!("../../../vm/scripts/Set-DefaultAudioRenderDevice.ps1");

/// A recording is treated as having no usable audio if no stream reports
/// `codec_type: "audio"` with at least this much duration — guarding
/// against a stream that exists in the container but never actually
/// received samples (a near-zero duration) as much as one that never
/// appears at all.
const MIN_AUDIO_DURATION_SECONDS: f64 = 0.5;

/// The WASAPI loopback capture endpoint VB-CABLE exposes, named exactly as
/// Windows reports it (confirmed live) — the dshow input ffmpeg captures
/// audio from. See `vm/scripts/Initialize-VerbatimHarness.ps1`'s
/// `Install-VbCableAudioDriver`.
const VB_CABLE_CAPTURE_DEVICE: &str = "CABLE Output (VB-Audio Virtual Cable)";

/// How long [`start_recording`] waits before [`confirm_recording_alive`]
/// checks that ffmpeg is still running — long enough for ffmpeg to have
/// opened both the `gdigrab` and `dshow` inputs and failed already if it is
/// going to (a missing executable, a missing capture device, or a bad
/// argument all fail within this window in practice), short enough not to
/// waste much of the run's wall-clock budget on every `--record` use.
const STARTUP_SETTLE: Duration = Duration::from_secs(2);

/// How long [`stop_recording`] waits after `KillProcess` before this
/// module considers the guest file safe to pull — giving the OS a moment
/// to finish flushing whatever ffmpeg had already written before
/// termination reached it.
const STOP_SETTLE: Duration = Duration::from_secs(2);

/// The guest-side path the recording is written to, alongside Verbatim's
/// own install directory.
fn recording_guest_path() -> String {
    format!(r"{VERBATIM_DIR}\recording.mp4")
}

/// Pins the guest's default audio render device to VB-CABLE, over
/// PowerShell Direct, before every `--record` run — see
/// [`DEFAULT_AUDIO_DEVICE_PIN_SCRIPT`]'s own doc comment for why this must
/// be asserted at record time (a `cargo xtask vm connect` session earlier
/// can have left the guest's default pointed at a since-disconnected
/// Remote Audio endpoint instead of VB-CABLE). The script itself verifies
/// the pin took effect for all three audio roles and throws if not, which
/// propagates here as an `Err`.
///
/// A currently *connected* RDP session hides VB-CABLE from this call
/// entirely (the mutual-exclusivity constraint `xtask::vm::test`'s own doc
/// comment describes — the same reason a connected session also makes
/// ffmpeg's own `dshow` open fail later), so this can legitimately fail in
/// the ordinary course of things, not just from a misconfigured image.
/// `xtask::vm::test` treats a failure here as non-fatal, degrading to a
/// video-only recording rather than aborting the run.
///
/// # Errors
///
/// Returns an error if the `Host::run_in_guest` call fails, or the pin
/// script itself throws (device not found, or the pin did not verify).
pub(crate) fn pin_default_render_device(
    host: &dyn Host,
    credentials: &GuestCredentials,
) -> VmResult<()> {
    host.run_in_guest(VM_NAME, credentials, DEFAULT_AUDIO_DEVICE_PIN_SCRIPT)?;
    Ok(())
}

/// Launches ffmpeg in the guest through the agent, capturing the desktop
/// (`gdigrab`) and, when `with_audio` is set, [`VB_CABLE_CAPTURE_DEVICE`]
/// (`dshow`) into [`recording_guest_path`] as fragmented MP4. Returns the
/// guest-side pid, which [`stop_recording`] needs.
///
/// `with_audio` is false for the video-only fallback `xtask::vm::test`
/// retries with after an audio-capturing launch fails to come up — see that
/// module's doc comment for why that failure mode is expected (a connected
/// RDP session hides the VB-CABLE capture device) rather than aborted on.
///
/// # Errors
///
/// Returns an error if the agent connection or `LaunchProcess` request
/// fails, or the agent refuses to launch ffmpeg (for example because
/// `Install-Ffmpeg` never ran on this image).
pub(crate) fn start_recording(agent_addr: &str, with_audio: bool) -> VmResult<u32> {
    let mut client = RecordingAgentClient::connect(agent_addr)?;
    let args = ffmpeg_args(&recording_guest_path(), with_audio);
    match client.request(Request::LaunchProcess {
        command: FFMPEG_GUEST_PATH.to_owned(),
        args,
        working_dir: None,
        env: Vec::new(),
        stderr_to: Some(format!(r"{VERBATIM_DIR}\ffmpeg-recording.log")),
    })? {
        Frame::Reply {
            payload: ReplyPayload::Launched { pid },
            ..
        } => Ok(pid),
        Frame::Error { message, .. } => Err(format!("agent refused to launch ffmpeg: {message}")),
        other @ Frame::Reply { .. } => Err(format!("unexpected reply launching ffmpeg: {other:?}")),
    }
}

/// The full ffmpeg argument list capturing the desktop, and — when
/// `with_audio` is set — VB-CABLE's loopback device, into fragmented MP4 at
/// `output_path`. Pure (no guest contact) so it can be unit tested directly.
///
/// `with_audio: false` omits the `dshow` input and its `-c:a`/`-b:a` codec
/// options entirely rather than pointing them at a device that will fail to
/// open — the video-only fallback [`start_recording`] launches when a
/// connected RDP session has already made VB-CABLE's capture device
/// invisible in this session (see `xtask::vm::test`'s own doc comment).
///
/// `-movflags +frag_keyframe+empty_moov+default_base_moof` is the piece
/// that matters most either way: see this module's own doc comment for why
/// a fragmented, moov-free container is what makes a hard
/// `TerminateProcess` stop still leave a playable file. `-g 30` at 15 fps
/// bounds a fragment to two seconds, so a kill loses at most that much of
/// the tail.
fn ffmpeg_args(output_path: &str, with_audio: bool) -> Vec<String> {
    let video_input = [
        "-y",
        "-f",
        "gdigrab",
        "-framerate",
        "15",
        "-thread_queue_size",
        "512",
        "-i",
        "desktop",
    ]
    .into_iter()
    .map(str::to_owned);

    let audio_input = if with_audio {
        vec![
            "-f".to_owned(),
            "dshow".to_owned(),
            "-thread_queue_size".to_owned(),
            "512".to_owned(),
            "-rtbufsize".to_owned(),
            "150M".to_owned(),
            "-i".to_owned(),
            format!("audio={VB_CABLE_CAPTURE_DEVICE}"),
        ]
    } else {
        Vec::new()
    };

    let mut codec_options = vec![
        "-movflags".to_owned(),
        "+frag_keyframe+empty_moov+default_base_moof".to_owned(),
        "-c:v".to_owned(),
        "libx264".to_owned(),
        "-preset".to_owned(),
        "ultrafast".to_owned(),
        "-pix_fmt".to_owned(),
        "yuv420p".to_owned(),
        "-g".to_owned(),
        "30".to_owned(),
    ];
    if with_audio {
        codec_options.extend([
            "-c:a".to_owned(),
            "aac".to_owned(),
            "-b:a".to_owned(),
            "128k".to_owned(),
        ]);
    }

    video_input
        .chain(audio_input)
        .chain(codec_options)
        .chain(std::iter::once(output_path.to_owned()))
        .collect()
}

/// Confirms ffmpeg is still running [`STARTUP_SETTLE`] after
/// [`start_recording`] launched it, so a bad device name, a missing
/// executable, or any other immediate failure is reported as an error
/// naming where to look (the captured stderr log) rather than only
/// surfacing later as a recording pull that finds an empty or missing
/// file.
///
/// # Errors
///
/// Returns an error if the agent connection or `ProcessStatus` request
/// fails, or ffmpeg has already exited.
pub(crate) fn confirm_recording_alive(agent_addr: &str, pid: u32) -> VmResult<()> {
    thread::sleep(STARTUP_SETTLE);
    let mut client = RecordingAgentClient::connect(agent_addr)?;
    match client.request(Request::ProcessStatus { pid })? {
        Frame::Reply {
            payload: ReplyPayload::ProcessStatus(ProcessState::Running),
            ..
        } => Ok(()),
        Frame::Reply {
            payload: ReplyPayload::ProcessStatus(ProcessState::Exited { exit_code }),
            ..
        } => Err(format!(
            "ffmpeg exited immediately after launch (exit code {exit_code:?}); check \
             {VERBATIM_DIR}\\ffmpeg-recording.log in the guest"
        )),
        other => Err(format!(
            "unexpected reply checking ffmpeg's status: {other:?}"
        )),
    }
}

/// Terminates the ffmpeg process started by [`start_recording`] and waits
/// [`STOP_SETTLE`] for its last written fragment to settle before the
/// caller pulls the file. Termination, not a graceful stop, is deliberate
/// — see this module's own doc comment.
///
/// # Errors
///
/// Returns an error if the agent connection or `KillProcess` request
/// fails.
pub(crate) fn stop_recording(agent_addr: &str, pid: u32) -> VmResult<()> {
    let mut client = RecordingAgentClient::connect(agent_addr)?;
    client.request(Request::KillProcess { pid })?;
    thread::sleep(STOP_SETTLE);
    Ok(())
}

/// Pulls [`recording_guest_path`] back to the host over PowerShell Direct
/// (`Host::read_guest_file`, the same mechanism `xtask vm logs` already
/// uses for flight-recorder dumps — Hyper-V's `Copy-VMFile` only copies
/// host-to-guest, never the other direction), writing it to
/// `artifacts/vm-recordings` under `repo_root`, named after `scenario_name`
/// — the scenario this particular recording covers (milestone M3 Track B:
/// one recording per scenario, not one per whole run). Returns the host
/// path written.
///
/// Before pulling, this probes the guest-side file with ffprobe (over the
/// same PowerShell Direct channel — no host-side ffprobe dependency) to
/// decide the filename: `<scenario_name>-<unix-seconds>.mp4` when a real
/// audio stream was found, or `<scenario_name>-<unix-seconds>-no-audio.mp4`
/// otherwise, so a silently video-only recording is never mistaken for a
/// complete one just by its name. For the VM `--record` path this should
/// essentially always resolve to the plain name, since
/// [`pin_default_render_device`] runs before every capture; the suffix is
/// the signal that audio capture genuinely failed. A probe that cannot run
/// at all (as opposed to running and finding no audio) is treated the same
/// conservative way — logged and named `-no-audio` — since "uncertain"
/// should never render as the more confident, unsuffixed name. This same
/// probe-then-name rule is meant to be reused by a future host-side/CI
/// recorder, where a video-only result is an ordinary outcome (no loopback
/// device or no `OneCore` voice on the runner) rather than a failure.
///
/// The timestamp is a plain Unix-seconds integer rather than a calendar
/// date: unique and sortable without pulling in a date/time crate for one
/// call site, matching this workspace's existing preference for shelling
/// out to what the platform already provides over adding a dependency
/// (`Host::local_file_hash_sha256`'s own doc comment makes the same
/// tradeoff for hashing).
///
/// # Errors
///
/// Returns an error if the guest file cannot be read, the output directory
/// cannot be created, or the file cannot be written. A failed or
/// unparseable probe is not an error here — see above.
pub(crate) fn pull_recording(
    host: &dyn Host,
    credentials: &GuestCredentials,
    repo_root: &Path,
    scenario_name: &str,
) -> VmResult<PathBuf> {
    let has_audio = match probe_has_audio(host, credentials) {
        Ok(has_audio) => has_audio,
        Err(error) => {
            println!(
                "xtask vm test: could not probe the recording for audio ({error}); naming it \
                 -no-audio to be safe"
            );
            false
        }
    };

    let bytes = host.read_guest_file(VM_NAME, credentials, &recording_guest_path())?;

    let out_dir = repo_root.join("artifacts").join("vm-recordings");
    fs::create_dir_all(&out_dir)
        .map_err(|error| format!("could not create {}: {error}", out_dir.display()))?;
    let suffix = if has_audio { "" } else { "-no-audio" };
    let out_path = out_dir.join(format!(
        "{scenario_name}-{}{suffix}.mp4",
        unix_seconds_now()
    ));
    fs::write(&out_path, bytes)
        .map_err(|error| format!("could not write {}: {error}", out_path.display()))?;
    Ok(out_path)
}

/// Runs ffprobe inside the guest against [`recording_guest_path`] over
/// PowerShell Direct and reports whether it found an audio stream at least
/// [`MIN_AUDIO_DURATION_SECONDS`] long.
///
/// # Errors
///
/// Returns an error if the `Host::run_in_guest` call itself fails (ffprobe
/// missing, guest unreachable). A successful run whose output cannot be
/// parsed as the expected JSON is not an error — [`parse_has_audio`]
/// reports that as "no audio", the same conservative default
/// [`pull_recording`] uses for an outright probe failure.
fn probe_has_audio(host: &dyn Host, credentials: &GuestCredentials) -> VmResult<bool> {
    // A single -show_entries with a colon-separated stream:format section
    // list, not two separate -show_entries flags: confirmed live that
    // passing -show_entries twice (even each with a distinct section)
    // makes ffprobe misparse the following filename as a repeated
    // "duration" argument instead of the input path.
    let script = format!(
        r#"& "{FFPROBE_GUEST_PATH}" -v error -of json -show_entries "stream=codec_type,duration:format=duration" "{path}""#,
        path = recording_guest_path(),
    );
    let output = host.run_in_guest(VM_NAME, credentials, &script)?;
    Ok(parse_has_audio(&output))
}

/// Pure parse of ffprobe's `-of json` output: true if any stream reports
/// `codec_type: "audio"` with a `duration` field parsing to at least
/// [`MIN_AUDIO_DURATION_SECONDS`]. Any shape that is not that — unparseable
/// JSON, no `streams` array, no audio entry, a missing or unparseable
/// duration — is `false`, deliberately: this function's whole purpose is
/// deciding a "confident yes" from everything else, not the reverse.
fn parse_has_audio(ffprobe_json: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(ffprobe_json.trim()) else {
        return false;
    };
    let Some(streams) = value.get("streams").and_then(serde_json::Value::as_array) else {
        return false;
    };
    streams.iter().any(|stream| {
        stream.get("codec_type").and_then(serde_json::Value::as_str) == Some("audio")
            && stream
                .get("duration")
                .and_then(serde_json::Value::as_str)
                .and_then(|duration| duration.parse::<f64>().ok())
                .is_some_and(|duration| duration >= MIN_AUDIO_DURATION_SECONDS)
    })
}

fn unix_seconds_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

/// A minimal client speaking `verbatim_agent::protocol` directly, just
/// enough for this module's three request kinds. See this module's own doc
/// comment for why this exists instead of depending on `verbatim-e2e`'s
/// own, fuller `AgentClient`.
struct RecordingAgentClient {
    stream: TcpStream,
    reader: BufReader<TcpStream>,
    next_id: u64,
}

impl RecordingAgentClient {
    fn connect(addr: &str) -> VmResult<Self> {
        let stream = TcpStream::connect(addr)
            .map_err(|error| format!("could not connect to the agent at {addr}: {error}"))?;
        let reader =
            BufReader::new(stream.try_clone().map_err(|error| {
                format!("could not clone the agent connection to {addr}: {error}")
            })?);
        let mut client = Self {
            stream,
            reader,
            next_id: 1,
        };
        match client.request(Request::Hello {
            protocol_version: AGENT_PROTOCOL_VERSION,
        })? {
            Frame::Reply {
                payload: ReplyPayload::Hello { .. },
                ..
            } => Ok(client),
            Frame::Error { message, .. } => {
                Err(format!("agent Hello was refused by {addr}: {message}"))
            }
            other @ Frame::Reply { .. } => {
                Err(format!("unexpected reply to Hello from {addr}: {other:?}"))
            }
        }
    }

    fn request(&mut self, request: Request) -> VmResult<Frame> {
        let id = self.next_id;
        self.next_id += 1;
        write_message(&mut self.stream, &RequestEnvelope { id, request })
            .map_err(|error| format!("could not write an agent request: {error}"))?;
        loop {
            let frame: Frame = read_message(&mut self.reader)
                .map_err(|error| format!("could not read an agent reply: {error}"))?
                .ok_or_else(|| "the agent closed the connection".to_owned())?;
            match &frame {
                Frame::Reply { to, .. } | Frame::Error { to, .. } if *to == id => {
                    return Ok(frame);
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffmpeg_args_names_the_vb_cable_capture_device_and_fragmented_output() {
        let args = ffmpeg_args(r"C:\VerbatimLab\verbatim\recording.mp4", true);
        assert!(args.contains(&"audio=CABLE Output (VB-Audio Virtual Cable)".to_owned()));
        assert!(
            args.contains(&"+frag_keyframe+empty_moov+default_base_moof".to_owned()),
            "expected fragmented-mp4 movflags so an abrupt stop still leaves a playable file"
        );
        assert_eq!(
            args.last(),
            Some(&r"C:\VerbatimLab\verbatim\recording.mp4".to_owned()),
            "the output path must be the final argument"
        );
    }

    #[test]
    fn ffmpeg_args_captures_both_gdigrab_and_dshow_inputs_when_audio_is_requested() {
        let args = ffmpeg_args("out.mp4", true);
        assert!(args.windows(2).any(|pair| pair == ["-f", "gdigrab"]));
        assert!(args.windows(2).any(|pair| pair == ["-f", "dshow"]));
        assert_eq!(
            args.iter().filter(|arg| arg.as_str() == "-i").count(),
            2,
            "expected exactly one -i for the desktop input and one for the audio input"
        );
        assert!(
            args.contains(&"-c:a".to_owned()),
            "expected an audio codec option"
        );
    }

    #[test]
    fn ffmpeg_args_omits_the_dshow_input_and_audio_codec_when_audio_is_not_requested() {
        let args = ffmpeg_args("out.mp4", false);
        assert!(args.windows(2).any(|pair| pair == ["-f", "gdigrab"]));
        assert!(
            !args.windows(2).any(|pair| pair == ["-f", "dshow"]),
            "video-only recording must not attempt to open the VB-CABLE capture device"
        );
        assert_eq!(
            args.iter().filter(|arg| arg.as_str() == "-i").count(),
            1,
            "expected only the desktop -i input"
        );
        assert!(
            !args
                .iter()
                .any(|arg| arg == "audio=CABLE Output (VB-Audio Virtual Cable)"),
            "video-only recording must not name the VB-CABLE capture device at all"
        );
        assert!(
            !args.contains(&"-c:a".to_owned()),
            "video-only recording must not carry an audio codec option"
        );
    }

    #[test]
    fn recording_guest_path_sits_alongside_verbatims_install_directory() {
        assert_eq!(
            recording_guest_path(),
            r"C:\VerbatimLab\verbatim\recording.mp4"
        );
    }

    #[test]
    fn parse_has_audio_true_for_an_audio_stream_above_the_threshold() {
        let json = r#"{"streams":[{"codec_type":"video","duration":"18.15"},{"codec_type":"audio","duration":"18.18"}],"format":{"duration":"18.18"}}"#;
        assert!(parse_has_audio(json));
    }

    #[test]
    fn parse_has_audio_false_with_no_audio_stream() {
        let json = r#"{"streams":[{"codec_type":"video","duration":"18.15"}],"format":{"duration":"18.15"}}"#;
        assert!(!parse_has_audio(json));
    }

    #[test]
    fn parse_has_audio_false_for_a_near_zero_duration_audio_stream() {
        let json = r#"{"streams":[{"codec_type":"audio","duration":"0.02"}],"format":{"duration":"18.15"}}"#;
        assert!(!parse_has_audio(json));
    }

    #[test]
    fn parse_has_audio_false_for_unparseable_output() {
        assert!(!parse_has_audio("not json"));
        assert!(!parse_has_audio(""));
    }

    #[test]
    fn parse_has_audio_false_when_duration_is_missing() {
        let json = r#"{"streams":[{"codec_type":"audio"}]}"#;
        assert!(!parse_has_audio(json));
    }
}
