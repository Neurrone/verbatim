//! `cargo xtask vm <verb>`: the Hyper-V E2E harness (milestone M2, per
//! `docs/architecture.md` section 14).
//!
//! Every verb goes through the [`host::Host`] trait, which is the only
//! place Hyper-V-specific PowerShell lives; verb logic in the other modules
//! here talks to `&dyn host::Host` only. This is what lets the deferred CI
//! story (a QEMU/KVM host on Linux runners, per the architecture doc) reuse
//! the same in-guest agent and the same verbs later, by adding a second
//! `Host` implementation rather than rewriting anything in this module.

mod connect;
mod create;
mod deploy;
mod dotenv;
mod host;
mod lifecycle;
mod logs;
mod packer_build;
mod recording;
mod test;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

pub(crate) use host::VmResult;

/// The Hyper-V VM name every verb operates on. `create` fails rather than
/// picking a fresh name if a VM by this name already exists; `delete`
/// clears the way for a rebuild.
pub(crate) const VM_NAME: &str = "verbatim";

/// The checkpoint name `create` takes and `restore` defaults to.
pub(crate) const CHECKPOINT_NAME: &str = "golden";

/// TCP port the in-guest agent listens on
/// (`verbatim_agent::protocol::DEFAULT_PORT`, duplicated here rather than
/// depending on `verbatim-agent` just for one constant).
pub(crate) const AGENT_PORT: u16 = 44_001;

/// Verbatim's install directory in the guest; `xtask vm deploy` writes
/// `verbatim.exe`, `verbatim-outpost.exe` (resolved next to it, per
/// `verbatim-outpost`'s supervisor), and `settings.toml` here.
pub(crate) const VERBATIM_DIR: &str = r"C:\VerbatimLab\verbatim";

/// The in-guest agent's install directory, matching
/// `vm/scripts/Initialize-VerbatimHarness.ps1`'s `VerbatimAgent` scheduled
/// task and firewall rule.
pub(crate) const AGENT_DIR: &str = r"C:\VerbatimLab\agent";

/// The in-guest tools directory where `xtask vm deploy` stages the vendored
/// `ffmpeg.exe` and `ffprobe.exe`, and where `--record` launches and probes
/// them from (`recording.rs`'s `FFMPEG_GUEST_PATH`). Created by the base
/// image provisioner and, failing that, by `Copy-VMFile -CreateFullPath`.
pub(crate) const TOOLS_DIR: &str = r"C:\VerbatimLab\tools";

/// Entry point for `cargo xtask vm <verb> [args...]`; `args` excludes the
/// leading `vm` token itself.
pub(crate) fn run(args: &[String]) -> ExitCode {
    let repo_root = repo_root();
    let host = host::HyperVHost;

    let result = match args.first().map(String::as_str) {
        Some("create") => {
            let skip_build = match args.get(1).map(String::as_str) {
                Some("--skip-build") => true,
                Some(other) => {
                    return unknown_arg("create", other);
                }
                None => false,
            };
            create::create(&host, &repo_root, skip_build)
        }
        Some("start") => lifecycle::start(&host),
        Some("stop") => lifecycle::stop(&host),
        Some("restart") => lifecycle::restart(&host),
        Some("restore") => lifecycle::restore(&host, args.get(1).map(String::as_str)),
        Some("deploy") => deploy_verb(&host, &repo_root),
        Some("test") => match parse_test_flags(&args[1..]) {
            Ok(flags) => test::test(&host, &repo_root, flags),
            Err(other) => return unknown_arg("test", &other),
        },
        Some("logs") => logs_verb(&host, &repo_root, args.get(1)),
        Some("connect") => match args.get(1).map(String::as_str) {
            Some("--forget") => connect::connect(&host, &repo_root, true),
            Some(other) => return unknown_arg("connect", other),
            None => connect::connect(&host, &repo_root, false),
        },
        Some("delete") => lifecycle::delete(&host),
        Some(other) => Err(format!("unknown verb '{other}'")),
        None => {
            print_usage();
            return ExitCode::from(2);
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("xtask vm: {message}");
            ExitCode::FAILURE
        }
    }
}

fn unknown_arg(verb: &str, arg: &str) -> ExitCode {
    eprintln!("xtask vm {verb}: unknown argument '{arg}'");
    print_usage();
    ExitCode::from(2)
}

/// Parses `test`'s flags — `--no-restore`, `--record`, and `--paced` —
/// accepted in any order and independently. Returns a [`test::TestFlags`], or
/// the first unrecognized argument as `Err`. There is no `--audible` flag
/// anymore: `test` is audible by default now — see `test::test`'s own doc
/// comment for why.
fn parse_test_flags(args: &[String]) -> Result<test::TestFlags, String> {
    let mut flags = test::TestFlags::default();
    for arg in args {
        match arg.as_str() {
            "--no-restore" => flags.no_restore = true,
            "--record" => flags.record = true,
            "--paced" => flags.paced = true,
            other => return Err(other.to_owned()),
        }
    }
    Ok(flags)
}

fn deploy_verb(host: &dyn host::Host, repo_root: &Path) -> VmResult<()> {
    let credentials = dotenv::load_guest_credentials(repo_root)?;
    deploy::run(host, repo_root, &credentials)
}

fn logs_verb(
    host: &dyn host::Host,
    repo_root: &Path,
    out_dir_arg: Option<&String>,
) -> VmResult<()> {
    let credentials = dotenv::load_guest_credentials(repo_root)?;
    let out_dir = match out_dir_arg {
        Some(dir) => PathBuf::from(dir),
        None => repo_root.join("artifacts").join("vm-logs"),
    };
    logs::run(host, &credentials, &out_dir)
}

/// The workspace root, computed from this crate's own manifest directory
/// (`<root>/xtask`), so `cargo xtask vm` behaves the same regardless of the
/// caller's working directory.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives one directory under the workspace root")
        .to_path_buf()
}

fn print_usage() {
    eprintln!("usage: cargo xtask vm <verb> [args]");
    eprintln!("verbs:");
    eprintln!(
        "  create           build the image with Packer, import and rename it to 'verbatim',"
    );
    eprintln!("                   start it, deploy a build, then checkpoint 'golden';");
    eprintln!("                   --skip-build reuses an already-exported Packer image");
    eprintln!("  start            start the VM and wait for the agent to answer");
    eprintln!("  stop             stop the VM");
    eprintln!("  restart          restart the VM and wait for the agent to answer");
    eprintln!("  restore [name]   restore a checkpoint (default 'golden') and wait for the agent");
    eprintln!(
        "  deploy           build verbatim.exe, verbatim-outpost.exe, and the agent, and copy"
    );
    eprintln!("                   them (plus settings.toml) into the guest");
    eprintln!("  test             restore 'golden', deploy, then run the E2E suite against it,");
    eprintln!(
        "                   audible by default (real OneCore synthesizer, real WASAPI): heard"
    );
    eprintln!(
        "                   live over a connected `cargo xtask vm connect` session, or played"
    );
    eprintln!(
        "                   to VB-CABLE unheard when headless; --no-restore skips the restore"
    );
    eprintln!("                   and reuses the live guest as-is — faster for iteration, but");
    eprintln!(
        "                   the guest may carry state from a previous run; never use it for an"
    );
    eprintln!(
        "                   acceptance run; --record additionally captures desktop video and"
    );
    eprintln!(
        "                   VB-CABLE audio to an mp4 under artifacts/vm-recordings — recording"
    );
    eprintln!(
        "                   audio and a connected RDP session are mutually exclusive (RDP hides"
    );
    eprintln!(
        "                   the VB-CABLE capture device), so --record against a connected guest"
    );
    eprintln!(
        "                   degrades to video-only, tagged -no-audio, with a warning, rather"
    );
    eprintln!("                   than aborting; --paced waits for each utterance's audio to");
    eprintln!("                   finish before the next keystroke so speech is heard in full");
    eprintln!("                   (implied by --record); all flags may be given, in any order");
    eprintln!("  logs [dir]       pull flight-recorder dumps and the agent log out of the guest");
    eprintln!("                   (default dir: artifacts/vm-logs)");
    eprintln!(
        "  connect          start the VM if needed, enable Remote Desktop in the guest once,"
    );
    eprintln!(
        "                   store its test credentials in this host's Credential Manager, then"
    );
    eprintln!(
        "                   open mstsc with audio redirected to this computer; --forget removes"
    );
    eprintln!("                   the stored credentials and exits without connecting");
    eprintln!("  delete           remove the VM and its disks, for a clean rebuild");
}
