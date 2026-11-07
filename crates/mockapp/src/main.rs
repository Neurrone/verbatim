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
//! runners. See `books/dev/src/reference/crates/mockapp.md` for the fixture format, CLI, and stdin
//! command reference.

mod buttons;
mod common_controls;
mod edit;
mod fixture;
mod held_message;
mod hits;
mod list_view;
mod menu;
mod msaa;
mod popup;
mod stdin;
mod tree;
mod tree_view;
mod uia;
mod uia_window;
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
    /// Has the UIA root report a window handle of 0, so a client holds the
    /// window's own element, runtime id `[42, window]`, without its handle.
    #[arg(long)]
    hide_window_handle: bool,
    /// Sets the named event this prefix and a printed line's first word
    /// name, such as `<prefix>applied` or `<prefix>held`, as mockapp prints
    /// that line, when the event exists: for the end-to-end harness, which
    /// waits on events rather than reading stdout.
    #[arg(long)]
    line_events: Option<String>,
    /// Makes the window an owned window of the top-level window titled
    /// this, found by its exact title when mockapp starts, as a dialog
    /// another process opens is owned by the window it opens over.
    #[arg(long)]
    owner_title: Option<String>,
}

/// The `--backend` values `mockapp` accepts, translated to
/// [`verbatim_model::Backend`] once parsed.
#[derive(Clone, Copy, ValueEnum)]
enum BackendArg {
    /// Answer as a UIA provider.
    Uia,
    /// Answer as an MSAA (`IAccessible`) provider.
    Msaa,
    /// Answer nothing for the window itself, as a plain Win32 window: its
    /// client area is Windows' own standard object, and only its real child
    /// controls, a UIA window among them, answer for themselves.
    Win32,
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
    // The root's window class names the host window's class.
    let window_class = root.window_class.clone();
    let frameless = root.frameless;
    let msaa_client = root.msaa_client;
    let dialog_keys = root.dialog_keys;
    let focus_on_activate = root.focus_on_activate;
    let native = root.take_native();
    let edit_version_6 = root.edit_version_6();
    let tree = std::sync::Arc::new(std::sync::Mutex::new(tree::Tree::build(root)));
    if cli.count_registrations {
        uia::count_registrations();
    }
    if cli.hide_window_handle {
        uia::hide_window_handle();
    }
    if let Some(prefix) = &cli.line_events {
        hits::set_line_events(prefix.clone());
    }
    let backend = match cli.backend {
        BackendArg::Uia => Some(Backend::Uia),
        BackendArg::Msaa => Some(Backend::Msaa),
        BackendArg::Win32 => None,
    };
    window::run(
        backend,
        tree,
        native,
        edit_version_6,
        (&cli.title, window_class.as_deref(), frameless),
        (cli.show, cli.owner_title.as_deref()),
        (msaa_client, dialog_keys, focus_on_activate),
    )?;
    Ok(())
}
