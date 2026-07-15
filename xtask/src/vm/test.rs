//! `cargo xtask vm test`: builds the current source, restores the golden
//! checkpoint, stages and copies the build onto it, discovers the guest's
//! IP, then runs `crates/verbatim-e2e`'s suite on the host against it.
//!
//! The build ([`deploy::build`]) deliberately runs *before* the checkpoint
//! restore, not after: [`deploy::run`]'s original ordering built only after
//! restoring, so a compile failure wasted the restore and the next attempt,
//! once the code was fixed, paid for another one. Building first means a
//! compile failure costs zero VM state changes — the guest is never touched
//! at all until the source is known to build. `--no-restore` does not
//! change this ordering: the build still runs first regardless, since a
//! broken build is exactly as pointless to discover after skipping the
//! restore as after performing it.
//!
//! The suite's two host-filesystem steps — checking `verbatim.exe` exists
//! and writing the capture-synth `settings.toml` next to it — are correct
//! only in runner-direct mode, where the suite and Verbatim share a
//! filesystem. Here `VERBATIM_E2E_VERBATIM_EXE` names a path inside the
//! guest, so this verb sets `VERBATIM_E2E_REMOTE` as well, which tells
//! `verbatim_e2e::Scenario::launch` to skip both: [`super::deploy::stage_and_copy`]
//! has already staged that same configuration inside the guest.
//!
//! `--no-restore` (`no_restore` here) skips the checkpoint restore and its
//! post-restore agent wait entirely, deploying straight onto whatever the
//! guest is currently running. This exists purely for fast local iteration
//! on top of [`deploy::stage_and_copy`]'s own hash-skipping — restore plus
//! its agent wait is most of a normal run's wall-clock cost. It is never
//! appropriate for an acceptance run, since the guest may carry state left
//! over from a previous test.
//!
//! `test` is audible by default now, unconditionally: [`deploy::stage_and_copy`]
//! always stages a `settings.toml` selecting the real `OneCore` synthesizer
//! (there is no more capture-synth choice on the VM path — see that
//! function's own doc comment), and `VERBATIM_E2E_AUDIBLE=1` is always set
//! on the suite process, so `verbatim_e2e::scenario::Scenario::launch`
//! (reading that variable itself, per-launch, through the agent) omits
//! `VERBATIM_TEST_AUDIO=null` for the guest-side Verbatim it launches. There
//! is no more `--audible` flag: a silent, unrecorded headless VM run
//! produces nothing observable and has no purpose, so the old
//! capture-synth-by-default behavior is gone. Whether a human actually
//! *hears* anything depends only on whether a session is listening: over a
//! connected `cargo xtask vm connect` session, Verbatim's real speech plays
//! to that session's own audio; headless, it plays to VB-CABLE with nobody
//! capturing it unless `--record` is also given.
//!
//! `--record` (`record` here) captures the run as a video with audio: before
//! the suite, [`recording::pin_default_render_device`] re-asserts VB-CABLE
//! as the guest's default render device (undoing any stale pin a prior,
//! now-disconnected `cargo xtask vm connect` session left behind), then
//! [`recording::start_recording`] launches ffmpeg inside the guest's
//! interactive session through the agent (see that module's own doc comment
//! for why it must go through the agent and not PowerShell Direct); after
//! the suite, [`recording::stop_recording`] terminates it and
//! [`recording::pull_recording`] copies the result to
//! `artifacts/vm-recordings` on the host.
//!
//! `--paced` (`paced` here) makes every speech assertion additionally wait for
//! the matched utterance's audio to finish before the next input is injected,
//! so a human watching over `cargo xtask vm connect` — or a recording — hears
//! each utterance in full instead of having it cut off by the next keystroke.
//! It sets `VERBATIM_E2E_PACED` for the suite process (`verbatim_e2e`'s
//! `PACED_ENV`); the collector waits on the control plane's per-utterance
//! `SpeechFinished` frame. It changes only timing, never what is asserted, so
//! an ordinary fast run leaves it off. `--record` implies it, since a
//! recording whose speech is clipped defeats the purpose of recording.
//!
//! **Recording audio and a connected RDP session are mutually exclusive.**
//! The moment an RDP session (`cargo xtask vm connect`, plain `mstsc.exe`,
//! or `vmconnect.exe`'s Enhanced Session) is connected to the guest, Windows
//! replaces that session's audio with a "Remote Audio" endpoint and the
//! VB-CABLE capture device becomes invisible within it — proven live with
//! both ffmpeg and, independently, `SoX` failing identically to open it. This
//! is ordinary Windows/RDP session-audio behavior, not a bug this harness
//! can work around from inside the guest: two dead ends were tried and
//! abandoned — configuring "Listen to this device" on the capture endpoint
//! via the registry (the property-store keys are protected even from
//! SYSTEM, and re-enumeration wipes them anyway) and a `SoX`-based forwarder
//! from the cable to Remote Audio (`SoX` cannot open the cable in the RDP
//! session for the exact same reason ffmpeg cannot). So this module does not
//! try: [`recording::pin_default_render_device`] failing, or ffmpeg exiting
//! immediately after launch with audio, is treated as expected fallout of a
//! connected RDP session, not aborted on — see [`test`]'s own code below.
//! The practical rule: to *hear* a run live, connect first and run without
//! `--record`; to *record* a run, make sure nothing is connected first.
//!
//! Recording setup and teardown are attempted even when the suite itself
//! fails, so a failing run's video is still pulled for diagnosis — see this
//! module's own error accumulation below. A failure specifically opening
//! the audio input degrades to a video-only recording (tagged `-no-audio`)
//! with a warning printed, rather than aborting the whole run: the suite
//! still runs and its video is still pulled either way.

use std::path::Path;
use std::process::Command;

use super::host::{Host, wait_for_agent};
use super::recording;
use super::{AGENT_PORT, CHECKPOINT_NAME, VERBATIM_DIR, VM_NAME, VmResult, deploy, dotenv};

/// The flags `cargo xtask vm test` accepts, all defaulting to off:
/// `--no-restore` skips the checkpoint restore, `--record` captures a video,
/// and `--paced` waits for each utterance to finish before the next input.
/// See this module's own doc comment for the details, and `parse_test_flags`
/// in `super` for the parsing.
#[derive(Clone, Copy, Default)]
pub(crate) struct TestFlags {
    pub no_restore: bool,
    pub record: bool,
    pub paced: bool,
}

/// # Errors
///
/// Returns an error if the checkpoint restore (when not skipped), the
/// deploy, IP discovery, or the E2E suite itself fails. A recording failure
/// never aborts the run — see this module's own doc comment — so `record`
/// contributes no new error case of its own; recording problems are printed
/// as warnings and, if a video was at least pulled, folded into the
/// accumulated error message alongside a suite failure, never in place of
/// running the suite.
pub(crate) fn test(host: &dyn Host, repo_root: &Path, flags: TestFlags) -> VmResult<()> {
    let TestFlags {
        no_restore,
        record,
        paced,
    } = flags;
    // `--record` implies pacing: a recording nobody can follow because each
    // utterance is cut off by the next keystroke defeats the point of
    // recording (see this module's own doc comment and `verbatim_e2e`'s
    // `PACED_ENV`).
    let paced = paced || record;
    let credentials = dotenv::load_guest_credentials(repo_root)?;

    println!(
        "xtask vm test: building first, before touching the VM, so a compile failure leaves \
         guest state untouched"
    );
    let built = deploy::build(repo_root)?;

    if no_restore {
        println!(
            "xtask vm test: --no-restore set — SKIPPING the checkpoint restore; guest state \
             may be dirty from a previous run; do not use --no-restore for an acceptance run"
        );
    } else {
        println!("xtask vm test: restoring checkpoint '{CHECKPOINT_NAME}'");
        host.restore_checkpoint(VM_NAME, CHECKPOINT_NAME)?;
        host.start_vm(VM_NAME)?;
        wait_for_agent(host, VM_NAME)?;
    }

    println!(
        "xtask vm test: audible by default — deploying and running with the real OneCore \
         synthesizer; connect first with `cargo xtask vm connect` to hear a run live, or add \
         --record to capture it (the two are mutually exclusive per run — see this module's \
         own doc comment)"
    );
    if record {
        println!(
            "xtask vm test: --record set — capturing desktop video and VB-CABLE audio to \
             artifacts/vm-recordings; this requires no RDP session to be connected right now"
        );
    }

    println!("xtask vm test: staging and copying the build onto the guest");
    deploy::stage_and_copy(host, repo_root, &credentials, built)?;
    wait_for_agent(host, VM_NAME)?;

    let ip = host.guest_ip(VM_NAME)?;
    let endpoint = format!("{ip}:{AGENT_PORT}");
    let guest_exe = format!(r"{VERBATIM_DIR}\verbatim.exe");

    let recording_pid = if record {
        start_recording_with_fallback(host, &credentials, &endpoint)
    } else {
        None
    };

    println!("xtask vm test: running the E2E suite against {endpoint}");
    let mut command = Command::new(env!("CARGO"));
    // --no-fail-fast: a failure in one test binary must not hide the others'
    // results — with several scenarios and occasional environment flakes,
    // one run should always report the complete picture. The overall exit
    // status still fails if anything failed.
    command
        .args([
            "test",
            "-p",
            "verbatim-e2e",
            "--no-fail-fast",
            "--",
            "--test-threads=1",
        ])
        .env("VERBATIM_E2E_ENDPOINT", &endpoint)
        .env("VERBATIM_E2E_VERBATIM_EXE", &guest_exe)
        .env("VERBATIM_E2E_REMOTE", "1")
        .env("VERBATIM_E2E_AUDIBLE", "1")
        .current_dir(repo_root);
    if paced {
        // Each speech assertion waits for the utterance's audio to finish
        // before the next input, so a recording or a live watcher hears every
        // utterance in full (verbatim_e2e's PACED_ENV).
        command.env("VERBATIM_E2E_PACED", "1");
    }
    let status = command
        .status()
        .map_err(|error| format!("failed to launch cargo test: {error}"))?;

    // Every failure from here on is accumulated rather than returned
    // immediately: a failing suite must not skip stopping and pulling the
    // recording (the video of a failing run is exactly what a human wants
    // to look at), and a recording failure must not hide a suite failure
    // that already happened.
    let mut errors = Vec::new();
    if status.success() {
        println!("xtask vm test: E2E suite passed");
    } else {
        errors.push(format!("E2E suite failed: {status}"));
    }

    if let Some(pid) = recording_pid {
        println!("xtask vm test: stopping the recording");
        if let Err(error) = recording::stop_recording(&endpoint, pid) {
            errors.push(format!("could not stop the recording cleanly: {error}"));
        }
        match recording::pull_recording(host, &credentials, repo_root) {
            Ok(path) => println!("xtask vm test: recording saved to {}", path.display()),
            Err(error) => errors.push(format!("could not pull the recording: {error}")),
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

/// Starts the `--record` capture, tolerating exactly the failure mode this
/// module's own doc comment documents: a connected RDP session hiding the
/// VB-CABLE capture device. Tries pinning VB-CABLE as the default render
/// device and launching ffmpeg with audio; if either step fails, warns and
/// falls back to a video-only launch instead of propagating the error.
/// Returns the guest-side ffmpeg pid on any successful launch (with or
/// without audio), or `None` if even the video-only fallback could not be
/// started — in which case the suite still runs, just unrecorded.
///
/// [`recording::pull_recording`]'s own ffprobe-based check is what actually
/// decides the `-no-audio` filename tag later; this function does not need
/// to thread that decision through itself.
fn start_recording_with_fallback(
    host: &dyn Host,
    credentials: &dotenv::GuestCredentials,
    endpoint: &str,
) -> Option<u32> {
    println!(
        "xtask vm test: pinning the guest's default audio render device to VB-CABLE (a \
         connected cargo xtask vm connect session can otherwise have switched it to Remote \
         Audio since the last restore)"
    );
    let with_audio = match recording::pin_default_render_device(host, credentials) {
        Ok(()) => true,
        Err(error) => {
            println!(
                "xtask vm test: WARNING — could not pin VB-CABLE as the default render device \
                 ({error}); this is expected if an RDP session is currently connected (RDP \
                 hides the VB-CABLE capture device — see this module's own doc comment); \
                 falling back to a video-only recording"
            );
            false
        }
    };

    println!("xtask vm test: starting the ffmpeg recording in the guest's interactive session");
    let attempt = recording::start_recording(endpoint, with_audio)
        .and_then(|pid| recording::confirm_recording_alive(endpoint, pid).map(|()| pid));
    match attempt {
        Ok(pid) => {
            println!("xtask vm test: recording started (guest pid {pid}, audio: {with_audio})");
            return Some(pid);
        }
        Err(error) if with_audio => {
            println!(
                "xtask vm test: WARNING — ffmpeg failed to start with audio ({error}); this is \
                 expected if an RDP session is currently connected; retrying video-only"
            );
        }
        Err(error) => {
            println!(
                "xtask vm test: WARNING — ffmpeg failed to start even video-only ({error}); \
                 continuing the suite without a recording"
            );
            return None;
        }
    }

    let fallback = recording::start_recording(endpoint, false)
        .and_then(|pid| recording::confirm_recording_alive(endpoint, pid).map(|()| pid));
    match fallback {
        Ok(pid) => {
            println!("xtask vm test: video-only recording started (guest pid {pid})");
            Some(pid)
        }
        Err(error) => {
            println!(
                "xtask vm test: WARNING — video-only ffmpeg launch also failed ({error}); \
                 continuing the suite without a recording"
            );
            None
        }
    }
}
