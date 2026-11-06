//! A remote operation's keyboard focus read can differ from a plain read
//! (`uia-facts/a-remote-operations-keyboard-focus-read-can-differ-from-a-plain-read/readme.md`).
//!
//! Each read is of `HasKeyboardFocus`, plainly (`CurrentHasKeyboardFocus`)
//! and inside a remote operation.
//!
//! - `provider`: the provider program's hidden window 0, its tree's focus
//!   on node 3; node 3, node 4 and the window's own element read both
//!   ways. The provider answers for itself; the case shows whether the two
//!   reads differ on a provider that does.
//! - `windows-terminal`, on the unlocked desktop, run only when named:
//!   Windows Terminal from `UIA_FACTS_WT` (the folder of the portable
//!   1.24.12741.0 copy), a second window opened in its running process
//!   (`wt.exe -w new`), and every focus a desktop-wide focus handler hears
//!   from its process within 5 s after the second window is asked for,
//!   each sender read both ways; three launches.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use uia_facts_support::args::{Args, trials};
use uia_facts_support::events::{self, Handler};
use uia_facts_support::precondition;
use uia_facts_support::provider::Provider;
use uia_facts_support::report::{Report, Verdict, rate, runs};
use uia_facts_support::uia::Shared;
use uia_facts_support::{apps, rop, uia};
use windows::Win32::UI::Accessibility::{IUIAutomationElement, UIA_HasKeyboardFocusPropertyId};

/// Launches of Windows Terminal, as the end-to-end runs made three.
const LAUNCHES: usize = 3;

fn main() {
    let args = Args::parse();
    uia::mta();
    let mut report =
        Report::start("a-remote-operations-keyboard-focus-read-can-differ-from-a-plain-read");
    if args.wants("provider") {
        report.case("provider", "moving the hidden provider window's tree focus to node 3; reading node 3's, node 4's and the window's keyboard focus plainly and inside a remote operation");
        provider(&mut report);
    }
    if args.names("windows-terminal") {
        report.case("windows-terminal", "on the unlocked desktop: Windows Terminal with a second window opened in its process; each focus sender from it read plainly and inside a remote operation");
        windows_terminal(&mut report);
    }
    report.finish();
}

/// `HasKeyboardFocus` read plainly, as `true` or `false`, or the error.
fn plain(element: &IUIAutomationElement) -> String {
    // SAFETY: a live element.
    match unsafe { element.CurrentHasKeyboardFocus() } {
        Ok(value) => value.as_bool().to_string(),
        Err(error) => uia::hresult(error.code()),
    }
}

/// `HasKeyboardFocus` read inside a remote operation, as `true` or
/// `false`, or the error.
fn remote(element: &IUIAutomationElement) -> String {
    match rop::property(element, UIA_HasKeyboardFocusPropertyId.0, false) {
        Ok(value) => value.to_lowercase(),
        Err(error) => uia::hresult(error.code()),
    }
}

/// Both reads of `element`, as `plain/remote`.
fn both(element: &IUIAutomationElement) -> String {
    format!("{}/{}", plain(element), remote(element))
}

fn provider(report: &mut Report) {
    let mut answers = Vec::new();
    for _ in 0..trials() {
        let mut provider = Provider::launch(&["--windows", "uia:0"]);
        provider.request("focus 0 3");
        let client = uia::client();
        let root = uia::element(&client, provider.hwnd(0)).expect("the window's element");
        let three = uia::node(&client, &root, 3).expect("node 3");
        let four = uia::node(&client, &root, 4).expect("node 4");
        answers.push(format!(
            "node 3 {}, node 4 {}, the window {}",
            both(&three),
            both(&four),
            both(&root)
        ));
    }
    report.claim(
        "the provider program's reads, plain/remote operation",
        &runs(&answers),
        "",
        Verdict::New,
    );
}

/// The folder of the portable Windows Terminal.
fn terminal_folder() -> Result<std::path::PathBuf, String> {
    let folder = std::env::var_os("UIA_FACTS_WT")
        .map(std::path::PathBuf::from)
        .ok_or("UIA_FACTS_WT is not set to the portable Windows Terminal's folder")?;
    if folder.join("wt.exe").is_file() {
        Ok(folder)
    } else {
        Err(format!("no wt.exe in {}", folder.display()))
    }
}

fn windows_terminal(report: &mut Report) {
    let claim = "launches in which a focus sender answered true inside a remote operation and false plainly";
    let folder = match precondition::unlocked_desktop().and_then(|()| terminal_folder()) {
        Ok(folder) => folder,
        Err(reason) => {
            report.unmet(claim, &reason);
            return;
        }
    };
    let mut differed = 0;
    for _ in 0..LAUNCHES {
        let senders: Arc<Mutex<Vec<(Instant, Shared)>>> = Arc::default();
        let kept = Arc::clone(&senders);
        let (handler, _reports) = Handler::calling(Some(Box::new(move |sender| {
            kept.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((Instant::now(), uia::share(sender)));
        })));
        let client = uia::client();
        let cache = uia::cache_request(&client, &[]).expect("a cache request");
        let focus = events::as_focus_handler(&handler);
        // SAFETY: live client, cache request and handler.
        unsafe { client.AddFocusChangedEventHandler(&cache, &focus) }.expect("the focus handler");
        let class = "CASCADIA_HOSTING_WINDOW_CLASS";
        let before = apps::top_windows();
        let mut first = std::process::Command::new(folder.join("WindowsTerminal.exe"))
            .spawn()
            .expect("Windows Terminal starts");
        uia_facts_support::note::waiting("up to 20 s, Windows Terminal's first window");
        let one = apps::wait_window(
            class,
            "WindowsTerminal.exe",
            &before,
            Duration::from_secs(20),
        )
        .expect("its first window");
        let before = apps::top_windows();
        let second_asked = Instant::now();
        // wt.exe hands the request to the running process and ends.
        let _ = std::process::Command::new(folder.join("wt.exe"))
            .args(["-w", "new"])
            .spawn()
            .expect("wt.exe starts")
            .wait();
        uia_facts_support::note::waiting("up to 20 s, Windows Terminal's second window");
        let two = apps::wait_window(
            class,
            "WindowsTerminal.exe",
            &before,
            Duration::from_secs(20),
        )
        .expect("its second window");
        let pid = apps::window_pid(two);
        assert_eq!(apps::window_pid(one), pid, "one process holds both windows");
        uia::pause(
            Duration::from_secs(5),
            "listening to the second window's focus",
        );
        let heard: Vec<Shared> = senders
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|(at, _)| *at >= second_asked)
            .map(|(_, s)| s.clone())
            .collect();
        let mut pairs = Vec::new();
        for sender in &heard {
            let element = uia::resolve(sender);
            // SAFETY: a live element.
            if unsafe { element.CurrentProcessId() }.ok() != i32::try_from(pid).ok() {
                continue;
            }
            pairs.push((plain(&element), remote(&element)));
        }
        uia_facts_support::note::evidence(&format!(
            "a launch's senders, plain and remote: {pairs:?}"
        ));
        if pairs.iter().any(|(p, r)| p == "false" && r == "true") {
            differed += 1;
        }
        // SAFETY: a live client object.
        let _ = unsafe { client.RemoveAllEventHandlers() };
        apps::kill(pid);
        let _ = first.kill();
        let _ = first.wait();
    }
    report.claim(
        claim,
        &format!("{differed} of {LAUNCHES}"),
        "3 of 3",
        rate(differed, LAUNCHES, 3, 3),
    );
}
