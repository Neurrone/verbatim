//! Real applications serve UIA calls one at a time
//! (`uia-facts/real-applications-serve-uia-calls-one-at-a-time/readme.md`).
//!
//! - `findall`: the provider program's window with a tree of 20,000 nodes;
//!   a `FindAllBuildCache` of its descendants on one thread while another
//!   thread reads node 1's name back to back. Judged: some reads answer
//!   while the search runs, getting in between its requests; the reads'
//!   median and longest wait and the search's time are printed for trends.
//! - `suspended`, on the unlocked desktop, run only when named: Notepad,
//!   Edge, Firefox and VS Code, each started at its installed path when it
//!   is not already running; its top-level window's element read once,
//!   then the window's UI thread suspended, a name read of the window from
//!   another thread, and the thread resumed after 1 s, a deliberate stall.
//!   Judged: the read answers only after the resume.

use std::time::Duration;

use uia_facts_support::args::{Args, trials};
use uia_facts_support::provider::Provider;
use uia_facts_support::report::{Report, Verdict, relation, runs};
use uia_facts_support::stats::{median, range, summary_ms};
use uia_facts_support::uia::{self, Timed};
use uia_facts_support::{apps, now_us, precondition};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Threading::{
    OpenThread, ResumeThread, SuspendThread, THREAD_SUSPEND_RESUME,
};
use windows::Win32::UI::Accessibility::{TreeScope_Descendants, UIA_NamePropertyId};
use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

/// The search's tree: 20,000 nodes, as Edge's page had 20,000 elements.
const NODES: &str = "20000";

fn main() {
    let args = Args::parse();
    uia::mta();
    let mut report = Report::start("real-applications-serve-uia-calls-one-at-a-time");
    if args.wants("findall") {
        report.case("findall", "a 20,000-node tree searched with a cache request on one thread while another reads a name back to back");
        findall(&mut report);
    }
    if args.names("suspended") {
        report.case("suspended", "on the unlocked desktop: Notepad, Edge, Firefox and VS Code, each with its window's UI thread suspended for 1 s while another thread reads the window's name");
        suspended(&mut report);
    }
    report.finish();
}

fn findall(report: &mut Report) {
    let mut between = Vec::new();
    let mut medians = Vec::new();
    let mut longest = Vec::new();
    let mut searches = Vec::new();
    for _ in 0..trials() {
        let provider = Provider::launch(&["--windows", "uia:0", "--nodes", NODES]);
        let client = uia::client();
        uia::set_timeouts(&client, 2_000, 30_000);
        let root = uia::element(&client, provider.hwnd(0)).expect("the window's element");
        let first = uia::share(&uia::node(&client, &root, 1).expect("node 1"));
        let root = uia::share(&root);
        let search = uia::spawn(move || {
            let client = uia::client();
            let request =
                uia::cache_request(&client, &[UIA_NamePropertyId]).expect("a cache request");
            // SAFETY: a live client object.
            let condition = unsafe { client.CreateTrueCondition() }.expect("a condition");
            Timed::of(|| {
                // SAFETY: live element, condition and cache request.
                unsafe {
                    uia::resolve(&root).FindAllBuildCache(
                        TreeScope_Descendants,
                        &condition,
                        &request,
                    )
                }
            })
        });
        let element = uia::resolve(&first);
        let mut reads = Vec::new();
        while !search.is_finished() {
            reads.push(Timed::of(|| uia::name(&element)));
        }
        let search = search.join().expect("the search");
        assert_eq!(search.outcome, "ok", "the search answers");
        between.push(
            reads
                .iter()
                .any(|r| r.outcome == "ok" && r.end < search.end && r.start > search.start),
        );
        let times: Vec<f64> = reads.iter().map(Timed::ms).collect();
        medians.push(median(&times));
        longest.push(range(&times).1);
        searches.push(search.ms());
    }
    report.claim(
        "reads beside the search answer while it runs, between its requests",
        &runs(&between),
        "true",
        relation(&between),
    );
    report.claim(
        "the reads' median",
        &summary_ms(&medians),
        "0.4 ms beside Edge's 20,000-element page",
        Verdict::Trend,
    );
    report.claim(
        "the longest read",
        &summary_ms(&longest),
        "up to 696 ms beside Edge's page",
        Verdict::Trend,
    );
    report.claim("the search", &summary_ms(&searches), "", Verdict::Trend);
}

/// An application: its name, its path, its executable and its window's
/// class.
struct App {
    name: &'static str,
    path: std::path::PathBuf,
    exe: &'static str,
    class: &'static str,
}

fn the_apps() -> Vec<App> {
    let local = std::env::var_os("LOCALAPPDATA").map_or_else(
        || std::path::PathBuf::from(r"C:\Users\Default\AppData\Local"),
        std::path::PathBuf::from,
    );
    vec![
        App {
            name: "Notepad",
            path: apps::system_program("notepad.exe"),
            exe: "Notepad.exe",
            class: "Notepad",
        },
        App {
            name: "Edge",
            path: r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe".into(),
            exe: "msedge.exe",
            class: "Chrome_WidgetWin_1",
        },
        App {
            name: "Firefox",
            path: r"C:\Program Files\Mozilla Firefox\firefox.exe".into(),
            exe: "firefox.exe",
            class: "MozillaWindowClass",
        },
        App {
            name: "VS Code",
            path: local.join(r"Programs\Microsoft VS Code\Code.exe"),
            exe: "Code.exe",
            class: "Chrome_WidgetWin_1",
        },
    ]
}

/// Whether a process of executable `exe` runs.
fn running(exe: &str) -> bool {
    apps::process_ids().into_iter().any(|pid| {
        apps::image_of(pid)
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .is_some_and(|n| n.eq_ignore_ascii_case(exe))
    })
}

/// Suspends or resumes thread `id`.
fn suspend(id: u32, stop: bool) {
    // SAFETY: a thread id of a window this program found.
    let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, false, id) }.expect("the thread opens");
    if stop {
        // SAFETY: a live thread handle with suspend rights.
        let _ = unsafe { SuspendThread(thread) };
    } else {
        // SAFETY: as above.
        let _ = unsafe { ResumeThread(thread) };
    }
    // SAFETY: the handle opened above.
    let _ = unsafe { windows::Win32::Foundation::CloseHandle(thread) };
}

fn suspended(report: &mut Report) {
    for app in the_apps() {
        let claim = format!(
            "{}: a name read waits for the suspended UI thread",
            app.name
        );
        if let Err(reason) = precondition::unlocked_desktop() {
            report.unmet(&claim, &reason);
            continue;
        }
        if !app.path.is_file() {
            report.unmet(
                &claim,
                &format!("{} is not at {}", app.name, app.path.display()),
            );
            continue;
        }
        if running(app.exe) {
            report.unmet(
                &claim,
                &format!("{} is already running; close it first", app.name),
            );
            continue;
        }
        let mut waited = Vec::new();
        for _ in 0..trials() {
            let before = apps::top_windows();
            let mut child = std::process::Command::new(&app.path)
                .spawn()
                .expect("the application starts");
            uia_facts_support::note::waiting(&format!("up to 30 s, {}'s window", app.name));
            let hwnd = apps::wait_window(app.class, app.exe, &before, Duration::from_secs(30))
                .expect("its window appears");
            let client = uia::client();
            uia::set_timeouts(&client, 2_000, 30_000);
            let element = uia::share(&uia::element(&client, hwnd).expect("the window's element"));
            let _ = uia::name(&uia::resolve(&element));
            // SAFETY: a window handle; a null process id pointer is allowed.
            let thread = unsafe { GetWindowThreadProcessId(HWND(hwnd as *mut _), None) };
            suspend(thread, true);
            let read = uia::spawn(move || Timed::of(|| uia::name(&uia::resolve(&element))));
            uia::pause(Duration::from_secs(1), "the UI thread suspended");
            let resumed = now_us();
            suspend(thread, false);
            let read = read.join().expect("the read");
            waited.push(read.outcome == "ok" && read.end >= resumed);
            apps::kill(apps::window_pid(hwnd));
            let _ = child.kill();
            let _ = child.wait();
        }
        report.claim(&claim, &runs(&waited), "true", relation(&waited));
    }
}
