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
//! runners. See `docs/crates/mockapp.md` for the fixture format, CLI, and stdin
//! command reference.

mod buttons;
mod common_controls;
mod edit;
mod fixture;
mod hits;
mod list_view;
mod msaa;
mod stdin;
mod tree;
mod tree_view;
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
    /// Shows the window, for the end-to-end suite, which drives it with
    /// real keys; the cross-process tests leave it hidden.
    #[arg(long)]
    show: bool,
    /// Counts client registrations for events on the UIA root, as UIA
    /// reports them (`IRawElementProviderAdviseEvents`), for the tests that
    /// check a client removed its handlers; it changes the calls UIA makes,
    /// so the tests that count calls leave it off.
    #[arg(long)]
    count_registrations: bool,
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
    let mut root = fixture::load(&cli.fixture)?;
    let native = root.take_native();
    let edit_version_6 = root.edit_version_6();
    let tree = std::sync::Arc::new(std::sync::Mutex::new(tree::Tree::build(root)));
    if cli.count_registrations {
        uia::count_registrations();
    }
    let backend = match cli.backend {
        BackendArg::Uia => Backend::Uia,
        BackendArg::Msaa => Backend::Msaa,
    };
    window::run(backend, tree, &native, edit_version_6, &cli.title, cli.show)?;
    Ok(())
}
