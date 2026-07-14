//! `mockapp` — the scripted UIA and MSAA provider test host (architecture
//! section 13, layer 2).
//!
//! Loads a JSON fixture tree, creates one real top-level Win32 window, and
//! answers `WM_GETOBJECT` as either a genuine out-of-process UIA provider
//! (`IRawElementProviderSimple`, `IRawElementProviderFragment`, and
//! `IRawElementProviderFragmentRoot`) or a genuine `IAccessible` (MSAA)
//! provider over the scripted tree, so Verbatim's real client stacks —
//! `verbatim-uia` and `verbatim-ia2` — and its arbitration logic can be
//! exercised cross-process with no real applications, on plain CI Windows
//! runners. See `docs/overview.md` for the fixture format, CLI, and stdin
//! command reference.

mod fixture;
mod msaa;
mod stdin;
mod tree;
mod uia;
mod window;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, ValueEnum};
use verbatim_model::Backend;

/// Scripted UIA and MSAA provider host for cross-process client-stack tests.
#[derive(Parser)]
#[command(
    name = "mockapp",
    about = "Scripted UIA and MSAA provider host for cross-process client-stack tests"
)]
struct Cli {
    /// Path to a fixture JSON file describing the scripted tree.
    #[arg(long)]
    fixture: PathBuf,
    /// Which provider backend to answer `WM_GETOBJECT` as.
    #[arg(long)]
    backend: BackendArg,
    /// The host window's title.
    #[arg(long, default_value = "mockapp")]
    title: String,
}

/// The `--backend` values `mockapp` accepts, translated to
/// [`verbatim_model::Backend`] once parsed.
#[derive(Clone, Copy, ValueEnum)]
enum BackendArg {
    /// Answer as a UIA provider.
    Uia,
    /// Answer as an MSAA (`IAccessible`) provider.
    Msaa,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("mockapp: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: &Cli) -> Result<(), Box<dyn std::error::Error>> {
    let root = fixture::load(&cli.fixture)?;
    let tree = std::sync::Arc::new(std::sync::Mutex::new(tree::Tree::build(root)));
    let backend = match cli.backend {
        BackendArg::Uia => Backend::Uia,
        BackendArg::Msaa => Backend::Msaa,
    };
    window::run(backend, tree, &cli.title)?;
    Ok(())
}
