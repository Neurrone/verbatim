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

/// TCP port the in-guest agent listens on.
pub(crate) const AGENT_PORT: u16 = verbatim_agent::protocol::DEFAULT_PORT;

/// Verbatim's install directory in the guest; `xtask vm deploy` writes
/// `verbatim.exe`, `verbatim-outpost.exe` (resolved next to it, per
/// `verbatim-outpost`'s supervisor), and `settings.toml` here.
pub(crate) const VERBATIM_DIR: &str = r"C:\VerbatimLab\verbatim";

/// The in-guest agent's install directory, matching
/// `vm/scripts/Initialize-VerbatimHarness.ps1`'s `VerbatimAgent` scheduled
/// task and firewall rule.
pub(crate) const AGENT_DIR: &str = r"C:\VerbatimLab\agent";

/// The guest's copy of ffmpeg, which `xtask vm deploy` stages from the
/// vendored `vm/vendor/ffmpeg`. Its tools directory is created by the base
/// image provisioner and, failing that, by `Copy-VMFile -CreateFullPath`.
/// `xtask vm test` names it in `VERBATIM_E2E_FFMPEG`, so the suite's
/// per-scenario recording (`verbatim_e2e::recording`) runs it through the
/// agent rather than looking for ffmpeg on the guest's `PATH`.
pub(crate) const FFMPEG_GUEST_PATH: &str = r"C:\VerbatimLab\tools\ffmpeg.exe";

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
        Some("restore") => lifecycle::restore(&host, &repo_root, args.get(1).map(String::as_str)),
        Some("deploy") => deploy_verb(&host, &repo_root),
        Some("test") => match parse_test_flags(&args[1..]) {
            Ok(flags) => test::test(&host, &repo_root, flags),
            Err(message) => {
                eprintln!("xtask vm test: {message}");
                print_usage();
                return ExitCode::from(2);
            }
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

/// Parses `test`'s flags — `--restore`, `--list`,
/// and the repeatable `--scenario <name>` and `--group <name>` — accepted in
/// any order and independently. Returns a [`test::TestFlags`], or the first
/// unrecognized argument (or a `--scenario`/`--group` missing its value) as
/// `Err`. There is no `--audible` flag anymore: `test` is audible by default
/// now — see `test::test`'s own doc comment for why.
fn parse_test_flags(args: &[String]) -> Result<test::TestFlags, String> {
    let mut flags = test::TestFlags::default();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--restore" => flags.restore = true,
            "--list" => flags.list = true,
            "--scenario" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "--scenario requires a value".to_owned())?;
                flags.scenarios.push(value.clone());
            }
            "--group" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "--group requires a value".to_owned())?;
                flags.groups.push(value.clone());
            }
            other => return Err(format!("unknown argument '{other}'")),
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
    eprintln!("  test             deploy onto the running guest (starting it if needed), then run");
    eprintln!("                   the E2E suite against it, audible (eSpeak NG through real");
    eprintln!("                   WASAPI), heard live over a connected `cargo xtask vm connect`");
    eprintln!("                   session; every scenario is recorded, video with Verbatim's own");
    eprintln!("                   audio, to target/e2e-artifacts/<scenario>/<scenario>.mp4");
    eprintln!("                   (VERBATIM_E2E_RECORD=0 turns that off), whether or not a");
    eprintln!("                   session is connected; --restore first restores 'golden' for a");
    eprintln!("                   clean guest, as an acceptance run needs; --scenario <name> and");
    eprintln!("                   --group <name> (each repeatable) select which scenarios run;");
    eprintln!("                   with neither given, every registered scenario runs; --list");
    eprintln!("                   prints the scenario registry (name and group) and exits");
    eprintln!("                   without touching the VM; all flags may be given, in any order");
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

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| (*arg).to_owned()).collect()
    }

    #[test]
    fn test_does_not_restore_unless_asked() {
        let flags = parse_test_flags(&args(&[])).expect("no flags parse");
        assert!(
            !flags.restore,
            "restoring must be opt-in, never the default"
        );
    }

    #[test]
    fn restore_flag_opts_in() {
        let flags = parse_test_flags(&args(&["--restore"])).expect("flags parse");
        assert!(flags.restore);
    }

    #[test]
    fn the_retired_no_restore_flag_is_rejected() {
        assert!(parse_test_flags(&args(&["--no-restore"])).is_err());
    }
}
