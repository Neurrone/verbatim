//! The raw probe answers 0x10000 for a window with a UIA provider
//! (`uia-facts/the-raw-probe-answers-0x10000-for-a-window-with-a-uia-provider/readme.md`).
//!
//! - `targets`: Verbatim's raw probe (`verbatim_uia::raw_probe`) and
//!   `UiaHasServerSideProvider` on each named target, each with its
//!   expected answer: the provider program's UIA window, its window with
//!   its own `IAccessible` and no UIA provider, and its plain Win32
//!   window; and a console host started directly (`conhost.exe cmd.exe`)
//!   on a desktop of the program's own, probed once its window answers a
//!   message, so that it has pumped.

use std::time::Duration;

use uia_facts_support::args::{Args, trials};
use uia_facts_support::console::Console;
use uia_facts_support::provider::Provider;
use uia_facts_support::report::{Report, deterministic, runs};
use uia_facts_support::role::{self, Role};
use uia_facts_support::{exit_now, uia};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_NULL};

/// The desktop the console host runs on.
const DESKTOP: &str = "uia-facts-raw-probe";

fn main() {
    let args = Args::parse();
    uia::mta();
    if let Some((name, _)) = &args.role {
        assert_eq!(name, "console", "the one role is console");
        console_role();
    }
    let mut report =
        Report::start("the-raw-probe-answers-0x10000-for-a-window-with-a-uia-provider");
    if args.wants("targets") {
        report.case("targets", "probing the provider program's UIA, MSAA and plain windows and a console host's window with the raw probe and UiaHasServerSideProvider");
        targets(&mut report);
    }
    report.finish();
}

/// Both probes of window `hwnd`: the raw probe's answer and UIA's.
fn probes(hwnd: isize) -> String {
    let raw = match verbatim_uia::raw_probe(hwnd) {
        verbatim_uia::RawProbe::Provider => "0x10000".to_owned(),
        verbatim_uia::RawProbe::Answered(answer) => format!("{answer:#x}"),
        other => format!("{other:?}"),
    };
    format!("raw {raw}, UIA {}", uia::has_server_side_provider(hwnd))
}

/// A console host on this role's desktop, probed once it answers
/// `WM_NULL`: reports `probes <answers>`.
fn console_role() -> ! {
    let console = Console::start();
    let mut result = 0usize;
    // SAFETY: a window handle; the answer is written to a local.
    let answered = unsafe {
        SendMessageTimeoutW(
            HWND(console.hwnd as *mut _),
            WM_NULL,
            WPARAM(0),
            LPARAM(0),
            SMTO_ABORTIFHUNG,
            10_000,
            Some(&raw mut result),
        )
    };
    assert!(answered.0 != 0, "the console window answers");
    role::report(&format!("probes {}", probes(console.hwnd)));
    drop(console);
    exit_now(0)
}

fn targets(report: &mut Report) {
    let mut seen: [Vec<String>; 4] = Default::default();
    for _ in 0..trials() {
        let provider = Provider::launch(&["--windows", "uia:0,msaa:0,plain:0"]);
        for (i, answers) in seen.iter_mut().enumerate().take(3) {
            answers.push(probes(provider.hwnd(i)));
        }
        let console = Role::start_on(Some(DESKTOP), "console", &[]);
        let line = console
            .line(Duration::from_secs(30))
            .expect("the console role reports");
        seen[3].push(line.strip_prefix("probes ").unwrap_or(&line).to_owned());
    }
    let expected = [
        ("the provider program's UIA window", "raw 0x10000, UIA true"),
        (
            "its window with its own IAccessible and no UIA provider",
            "raw 0x0, UIA false",
        ),
        ("its plain Win32 window", "raw 0x0, UIA false"),
        ("a console host's window", "raw 0x10000, UIA true"),
    ];
    for ((target, answer), answers) in expected.into_iter().zip(&seen) {
        report.claim(
            target,
            &runs(answers),
            answer,
            deterministic(answers, &answer.to_owned()),
        );
    }
}
