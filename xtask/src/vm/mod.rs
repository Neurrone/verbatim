//! `cargo xtask vm <verb>`: the Hyper-V E2E harness (milestone M2, per
//! `docs/architecture.md` section 14).
//!
//! Every verb goes through the [`host::Host`] trait, which is the only
//! place Hyper-V-specific PowerShell lives; verb logic in the other modules
//! here talks to `&dyn host::Host` only. This is what lets the deferred CI
//! story (a QEMU/KVM host on Linux runners, per the architecture doc) reuse
//! the same in-guest agent and the same verbs later, by adding a second
//! `Host` implementation rather than rewriting anything in this module.

mod create;
mod deploy;
mod dotenv;
mod host;
mod lifecycle;
mod logs;
mod packer_build;
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
        Some("test") => test::test(&host, &repo_root),
        Some("logs") => logs_verb(&host, &repo_root, args.get(1)),
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
    eprintln!("  test             restore 'golden', deploy, then run the E2E suite against it");
    eprintln!("  logs [dir]       pull flight-recorder dumps and the agent log out of the guest");
    eprintln!("                   (default dir: artifacts/vm-logs)");
    eprintln!("  delete           remove the VM and its disks, for a clean rebuild");
}
