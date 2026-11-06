//! A new connection fails while another process's remote operation is
//! queued
//! (`uia-facts/a-new-connection-fails-while-another-processs-remote-operation-is-queued/readme.md`).
//!
//! The scene: the provider program P with window 0 on UI thread 0, window
//! 1 on UI thread 1 and window 2 on UI thread 0; process A, a second client
//! that holds window 0's element and has set UIA up in P with one remote
//! operation; and this process, B, which makes the call under test. A
//! queued operation: P's UI thread 0 busy until released, A's remote
//! operation on window 0 sent, and P's log showing a message reaching the
//! busy thread after it was sent. B's call follows; a call that sends the
//! window a message is released once P logs that message, any other after
//! a deliberate 100 ms, since nothing marks a request waiting in UIA's
//! channel. Every trial has a new P and a new A.
//!
//! Cases run by default (what Verbatim relies on):
//!
//! - `collision`: B's `ElementFromHandleBuildCache` and B's
//!   `UiaHasServerSideProvider` of window 0, each in a trial of its own,
//!   and each with no operation queued as the control.
//! - `spared`: during the queued operation, each in a trial of its own:
//!   B's read of an element it holds; A's own lookup and probe; B's remote
//!   operation; B's MSAA call through UIA's bridge; B's raw probe; B's
//!   walk up from window 2, made window 0's child.
//!
//! Cases run only when named:
//!
//! - `again`: B's probe of window 0 once before the operation, then again
//!   during it.
//! - `whole-process`: B's probe of window 1, on the idle UI thread.
//! - `order`: B's lookup sent first, waiting for the busy thread, then A's
//!   operation.
//! - `walk-up-across`: B's walk up from the window of a second provider
//!   process, made window 0's child.
//! - `delay`: B's read of window 1's element, on the idle thread, during
//!   the queued operation, and with no operation as the control.
//! - `second-form`: 42 trials of a third client's read held in P's
//!   provider while B and a fourth client look window 0 up at once.
//! - `conhost`: a fresh console host per trial on a desktop of the
//!   program's own, its window thread suspended, A's operation and a
//!   client's lookup; and the provider program stalled the same way.
//! - `getfocus`, on the unlocked desktop: B's `GetFocusedElement` with
//!   window 0 shown and in front, during the queued operation.

use std::time::Duration;

use uia_facts_support::args::{Args, trials};
use uia_facts_support::precondition;
use uia_facts_support::provider::Provider;
use uia_facts_support::report::{Report, Verdict, deterministic, rate, relation, runs};
use uia_facts_support::role::{self, Role};
use uia_facts_support::stats::summary_ms;
use uia_facts_support::uia::{self, Shared, Timed};
use uia_facts_support::{exit_now, now_us, rop};
use verbatim_uia::RawProbe;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Threading::{
    OpenThread, ResumeThread, SuspendThread, THREAD_SUSPEND_RESUME,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    AccessibleObjectFromWindow, IAccessible, IUIAutomation6, IUIAutomationElement,
    UIA_ProviderDescriptionPropertyId,
};
use windows::Win32::UI::WindowsAndMessaging::OBJID_CLIENT;
use windows::core::Interface;

/// P's windows: 0 and 2 on UI thread 0, 1 on UI thread 1.
const LAYOUT: &str = "uia:0,uia:1,uia:0";
/// The pause before a release where nothing marks the call's arrival.
const UNMARKED: Duration = Duration::from_millis(100);
/// A case: its name, whether it runs by default, what it does, and its run.
type Case = (&'static str, bool, &'static str, fn(&mut Report));

/// The desktop the console host cases run on.
const DESKTOP: &str = "uia-facts-collision";

/// A client object with the cases' timeouts: UIA's default connection
/// timeout, and a transaction timeout longer than any stall.
fn client() -> IUIAutomation6 {
    let client = uia::client();
    uia::set_timeouts(&client, 2_000, 30_000);
    client
}

fn main() {
    let args = Args::parse();
    uia::mta();
    if let Some((name, rest)) = &args.role {
        match name.as_str() {
            "client" => client_role(rest),
            "console" => console_role(),
            other => panic!("unknown role {other}"),
        }
    }
    let mut report =
        Report::start("a-new-connection-fails-while-another-processs-remote-operation-is-queued");
    let cases: [Case; 10] = [
        (
            "collision",
            true,
            "queueing A's remote operation in the busy provider; B looks window 0 up and probes it, each in a trial of its own, and with nothing queued",
            collision,
        ),
        (
            "spared",
            true,
            "queueing A's remote operation; B reads a held element, A looks up and probes its own window, B runs a remote operation, an MSAA call, the raw probe and walks up from window 0's child window, each in a trial of its own",
            spared,
        ),
        (
            "again",
            false,
            "B probes window 0, then queues A's remote operation and probes it again",
            again,
        ),
        (
            "whole-process",
            false,
            "queueing A's remote operation on UI thread 0; B probes window 1, on the idle UI thread",
            whole_process,
        ),
        (
            "order",
            false,
            "B looks window 0 up while the thread is busy, then A's remote operation is queued",
            order,
        ),
        (
            "walk-up-across",
            false,
            "making a second provider process's window window 0's child; queueing A's remote operation; B walks up from it",
            walk_up_across,
        ),
        (
            "delay",
            false,
            "queueing A's remote operation on UI thread 0; B reads an element of window 1, on the idle thread, and the same with nothing queued",
            delay,
        ),
        (
            "second-form",
            false,
            "42 trials: a third client's read held in the provider while B and a fourth client look window 0 up at once",
            second_form,
        ),
        (
            "conhost",
            false,
            "on a desktop of its own: a fresh console host's window thread suspended; A's remote operation and a client's lookup; the provider program stalled the same way",
            conhost,
        ),
        (
            "getfocus",
            false,
            "on the unlocked desktop: window 0 shown and in front; queueing A's remote operation; B reads the focused element",
            getfocus,
        ),
    ];
    for (name, default, what, run) in cases {
        if (default && args.wants(name)) || args.names(name) {
            report.case(name, what);
            run(&mut report);
        }
    }
    report.finish();
}

/// The second client: `<hwnd> <setup> <node>`. When `setup` is `yes`,
/// fetches window `hwnd`'s element and sets UIA up in its process with one
/// remote operation; otherwise it has connected to nothing. When `node` is
/// `yes`, fetches the window's node 1. Then takes commands, each run on a
/// thread of its own and answered when done, after `sent <µs>` as it
/// starts: `rop` (a remote operation reading the window's name), `read`
/// (node 1's name), `lookup <hwnd>` (a lookup by handle, classified) and
/// `probe <hwnd>` (`UiaHasServerSideProvider`).
fn client_role(rest: &[String]) -> ! {
    let hwnd: isize = rest[0].parse().expect("a window handle");
    let mine = client();
    let root = (rest[1] == "yes").then(|| {
        let root = uia::element(&mine, hwnd).expect("the window's element");
        rop::name(&root).expect("the first remote operation");
        root
    });
    let first = (rest[2] == "yes").then(|| {
        let root = root.as_ref().expect("node 1 is fetched after the setup");
        uia::share(&uia::node(&mine, root, 1).expect("node 1"))
    });
    let root = root.map(|r| uia::share(&r));
    role::report("ready");
    for command in role::commands() {
        let words: Vec<String> = command.split(' ').map(str::to_owned).collect();
        let (root, first) = (root.clone(), first.clone());
        uia::spawn(move || {
            role::report(&format!("sent {}", now_us()));
            let answer = match words[0].as_str() {
                "rop" => {
                    let root = root.expect("the window was fetched");
                    let call = Timed::of(|| rop::name(&uia::resolve(&root)));
                    format!("rop {} {}", call.outcome, call.end)
                }
                "read" => {
                    let first = first.expect("node 1 was fetched");
                    let call = Timed::of(|| uia::name(&uia::resolve(&first)));
                    format!("read {} {}", call.outcome, call.end)
                }
                "lookup" => {
                    let start = now_us();
                    let served = uia::lookup(&client(), words[1].parse().expect("a window"));
                    format!("lookup {served} {} {}", now_us() - start, now_us())
                }
                "probe" => {
                    let start = now_us();
                    let yes = uia::has_server_side_provider(words[1].parse().expect("a window"));
                    format!("probe {yes} {} {}", now_us() - start, now_us())
                }
                other => panic!("unknown command {other}"),
            };
            role::report(&answer);
        });
    }
    exit_now(0)
}

/// Starts a client role on window `hwnd`, on `desktop` when given, and
/// waits until it is ready.
fn start_client(desktop: Option<&str>, hwnd: isize, setup: bool, node: bool) -> Role {
    let yes = |b: bool| if b { "yes" } else { "no" }.to_owned();
    let role = Role::start_on(
        desktop,
        "client",
        &[hwnd.to_string(), yes(setup), yes(node)],
    );
    assert_eq!(role.expect_line(), "ready", "the client is ready");
    role
}

/// Sends `command` to `role` and answers the `sent` time it reports.
fn send(role: &mut Role, command: &str) -> u64 {
    role.send(command);
    let line = role.expect_line();
    line.strip_prefix("sent ")
        .and_then(|t| t.parse().ok())
        .unwrap_or_else(|| panic!("the client reports its start: {line}"))
}

/// The words of `role`'s next answer.
fn answer(role: &Role) -> Vec<String> {
    role.expect_line().split(' ').map(str::to_owned).collect()
}

/// Waits for P's busy thread to log a message arriving after `after`.
fn reached(provider: &mut Provider, after: u64) {
    let (_, found) = provider.log_until(Duration::from_secs(10), |e| {
        e.kind == "queued" && e.start >= after
    });
    assert!(found, "the message reaches the busy thread");
}

/// A scene: P with its layout, and A set up in P.
struct Scene {
    provider: Provider,
    a: Role,
}

impl Scene {
    fn new() -> Self {
        let provider = Provider::launch(&["--windows", LAYOUT, "--bridge"]);
        let a = start_client(None, provider.hwnd(0), true, false);
        Self { provider, a }
    }

    /// Makes UI thread 0 busy and, when `operation`, queues A's remote
    /// operation, by P's log.
    fn queue(&mut self, operation: bool) {
        self.provider.log();
        self.provider.request("busy 0");
        if operation {
            let sent = send(&mut self.a, "rop");
            reached(&mut self.provider, sent);
        }
    }

    /// Releases the busy thread; answers when.
    fn release(&mut self) -> u64 {
        let released = now_us();
        self.provider.request("release");
        released
    }

    /// A's operation's answer, after the release.
    fn operation_done(&self) {
        let words = answer(&self.a);
        assert_eq!(words[0], "rop", "A's operation answers");
    }
}

/// Runs B's call `call` on a thread during A's queued operation (or the
/// busy thread alone, without `operation`), releasing once P logs the call's
/// message when `marked`, or after [`UNMARKED`]; answers the call's value
/// and time and the release.
fn during<T: Send + 'static>(
    scene: &mut Scene,
    operation: bool,
    marked: bool,
    call: impl FnOnce() -> T + Send + 'static,
) -> (T, f64, u64) {
    scene.queue(operation);
    let start = now_us();
    let thread = uia::spawn(move || {
        let (value, took) = uia::timed(call);
        (value, uia::ms(took))
    });
    if marked {
        reached(&mut scene.provider, start);
    } else {
        uia::pause(UNMARKED, "before the release");
    }
    let released = scene.release();
    let (value, ms) = thread.join().expect("B's call");
    if operation {
        scene.operation_done();
    }
    (value, ms, released)
}

/// The note before a call that may wait out UIA's answer of about 2 or 3
/// seconds.
fn about(seconds: u32) {
    uia_facts_support::note::waiting(&format!(
        "up to about {seconds} s, B's call answering wrongly after the release"
    ));
}

fn collision(report: &mut Report) {
    let mut lookups = Vec::new();
    let mut lookup_ms = Vec::new();
    let mut probes = Vec::new();
    let mut probe_ms = Vec::new();
    let mut controls = Vec::new();
    for _ in 0..trials() {
        for operation in [true, false] {
            let mut scene = Scene::new();
            let hwnd = scene.provider.hwnd(0);
            if operation {
                about(2);
            }
            let (served, ms, _) = during(&mut scene, operation, true, move || {
                uia::lookup(&client(), hwnd)
            });
            let mut scene_b = Scene::new();
            let hwnd_b = scene_b.provider.hwnd(0);
            if operation {
                about(3);
            }
            let (yes, probe_took, _) = during(&mut scene_b, operation, true, move || {
                uia::has_server_side_provider(hwnd_b)
            });
            if operation {
                lookups.push(served);
                lookup_ms.push(ms);
                probes.push(yes);
                probe_ms.push(probe_took);
            } else {
                controls.push((served, yes));
            }
        }
    }
    report.claim(
        "B's ElementFromHandleBuildCache during A's queued remote operation answers",
        &runs(&lookups),
        "UIA's window proxy",
        deterministic(&lookups, &"proxy".to_owned()),
    );
    report.claim(
        "its time from the call",
        &summary_ms(&lookup_ms),
        "about 2 s (2,300 to 2,337 ms after a 300 ms busy period)",
        Verdict::Trend,
    );
    report.claim(
        "B's UiaHasServerSideProvider during A's queued remote operation answers",
        &runs(&probes),
        "false",
        deterministic(&probes, &false),
    );
    report.claim(
        "its time from the call",
        &summary_ms(&probe_ms),
        "about 3 s",
        Verdict::Trend,
    );
    report.claim(
        "control, nothing queued: (lookup, probe)",
        &runs(&controls),
        "its own provider, true",
        deterministic(&controls, &("own".to_owned(), true)),
    );
}

/// `AccessibleObjectFromWindow` of `hwnd`'s client object, then its name.
fn msaa_name(hwnd: isize) -> String {
    let mut object: *mut std::ffi::c_void = std::ptr::null_mut();
    // SAFETY: any window handle; `object` receives an interface pointer of
    // the IID asked for.
    let made = unsafe {
        AccessibleObjectFromWindow(
            HWND(hwnd as *mut _),
            OBJID_CLIENT.0.cast_unsigned(),
            &IAccessible::IID,
            &raw mut object,
        )
    };
    if let Err(error) = made {
        return uia::hresult(error.code());
    }
    // SAFETY: AccessibleObjectFromWindow returned an IAccessible with a
    // reference this takes over.
    let object = unsafe { IAccessible::from_raw(object) };
    // SAFETY: a live object; CHILDID_SELF, an I4 of 0.
    match unsafe { object.get_accName(&VARIANT::from(0_i32)) } {
        Ok(name) => name.to_string(),
        Err(error) => uia::hresult(error.code()),
    }
}

/// The parent of `child` in the raw view, its provider description
/// cached, classified.
fn walk_up(client: &IUIAutomation6, child: &IUIAutomationElement) -> String {
    let request =
        uia::cache_request(client, &[UIA_ProviderDescriptionPropertyId]).expect("a cache request");
    // SAFETY: a live client object.
    let walker = unsafe { client.RawViewWalker() }.expect("the raw view walker");
    // SAFETY: a live walker, element and cache request.
    uia::served_by(&unsafe { walker.GetParentElementBuildCache(child, &request) })
}

#[expect(
    clippy::too_many_lines,
    reason = "one trial per spared call, read in turn"
)]
fn spared(report: &mut Report) {
    let mut held = Vec::new();
    let mut own = Vec::new();
    let mut operation = Vec::new();
    let mut msaa = Vec::new();
    let mut raw = Vec::new();
    let mut up = Vec::new();
    for _ in 0..trials() {
        // B's read of an element it holds.
        let mut scene = Scene::new();
        let b = client();
        let root = uia::element(&b, scene.provider.hwnd(0)).expect("window 0");
        let first: Shared = uia::share(&uia::node(&b, &root, 1).expect("node 1"));
        let (read, _, released) = during(&mut scene, true, false, move || {
            Timed::of(|| uia::name(&uia::resolve(&first)))
        });
        held.push(read.outcome == "ok" && read.end >= released);

        // A's own lookup and probe during its own operation.
        let mut scene = Scene::new();
        let hwnd = scene.provider.hwnd(0);
        scene.queue(true);
        // A's lookup of a window it is connected to sends it no message,
        // so nothing marks its arrival (e12 answered in 1.1 ms).
        let _ = send(&mut scene.a, &format!("lookup {hwnd}"));
        uia::pause(UNMARKED, "before the release");
        scene.release();
        let mut lines = [answer(&scene.a), answer(&scene.a)];
        lines.sort_by_key(|w| w[0] == "rop");
        assert_eq!(lines[1][0], "rop", "A's operation answers");
        let looked = lines[0][1].clone();
        let mut scene = Scene::new();
        let hwnd = scene.provider.hwnd(0);
        scene.queue(true);
        let _ = send(&mut scene.a, &format!("probe {hwnd}"));
        uia::pause(UNMARKED, "before the release");
        scene.release();
        let mut lines = [answer(&scene.a), answer(&scene.a)];
        lines.sort_by_key(|w| w[0] == "rop");
        uia_facts_support::note::evidence(&format!(
            "A's own lookup {looked:?}, probe {:?}",
            lines[0]
        ));
        own.push((looked, lines[0][1].clone()));

        // B's remote operation, its first into P, reading window 0's name.
        let mut scene = Scene::new();
        let b = client();
        let root: Shared = uia::share(&uia::element(&b, scene.provider.hwnd(0)).expect("window 0"));
        let (named, _, _) = during(&mut scene, true, false, move || {
            rop::name(&uia::resolve(&root)).unwrap_or_else(|e| uia::hresult(e.code()))
        });
        operation.push(named);

        // B's MSAA call through UIA's bridge.
        let mut scene = Scene::new();
        let hwnd = scene.provider.hwnd(0);
        let (named, _, _) = during(&mut scene, true, true, move || msaa_name(hwnd));
        msaa.push(named);

        // B's raw probe.
        let mut scene = Scene::new();
        let hwnd = scene.provider.hwnd(0);
        let (answered, _, _) = during(&mut scene, true, true, move || {
            verbatim_uia::raw_probe(hwnd) == RawProbe::Provider
        });
        raw.push(answered);

        // B's walk up from window 2, window 0's child.
        let mut scene = Scene::new();
        let (zero, two) = (scene.provider.hwnd(0), scene.provider.hwnd(2));
        scene.provider.request(&format!("parent 2 {zero}"));
        let b = client();
        let child: Shared = uia::share(&uia::element(&b, two).expect("window 2"));
        let (served, _, _) = during(&mut scene, true, false, move || {
            walk_up(&client(), &uia::resolve(&child))
        });
        up.push(served);
    }
    report.claim(
        "B's read of an element it holds is only delayed: answered after the release",
        &runs(&held),
        "true",
        relation(&held),
    );
    report.claim(
        "A's own lookup and probe during its own queued operation: (lookup, probe)",
        &runs(&own),
        "its own provider, true",
        deterministic(&own, &("own".to_owned(), "true".to_owned())),
    );
    report.claim(
        "B's remote operation reads the provider's own name",
        &runs(&operation),
        "window 0, the provider's own (not the proxy's title)",
        deterministic(&operation, &"window 0".to_owned()),
    );
    report.claim(
        "B's MSAA name through UIA's bridge",
        &runs(&msaa),
        "window 0, the provider's own",
        deterministic(&msaa, &"window 0".to_owned()),
    );
    report.claim(
        "B's raw probe answers 0x10000",
        &runs(&raw),
        "true",
        deterministic(&raw, &true),
    );
    report.claim(
        "B's walk up from window 0's child window, within the process, reaches",
        &runs(&up),
        "its own provider",
        deterministic(&up, &"own".to_owned()),
    );
}

fn again(report: &mut Report) {
    let mut outcomes = Vec::new();
    for _ in 0..trials() {
        let mut scene = Scene::new();
        let hwnd = scene.provider.hwnd(0);
        let before = uia::has_server_side_provider(hwnd);
        about(3);
        let (after, _, _) = during(&mut scene, true, true, move || {
            uia::has_server_side_provider(hwnd)
        });
        outcomes.push((before, after));
    }
    report.claim(
        "B's probe before the operation, then during it",
        &runs(&outcomes),
        "true, then false",
        deterministic(&outcomes, &(true, false)),
    );
}

fn whole_process(report: &mut Report) {
    let mut outcomes = Vec::new();
    let mut times = Vec::new();
    for _ in 0..trials() {
        let mut scene = Scene::new();
        let hwnd = scene.provider.hwnd(1);
        about(3);
        let (yes, ms, _) = during(&mut scene, true, false, move || {
            uia::has_server_side_provider(hwnd)
        });
        outcomes.push(yes);
        times.push(ms);
    }
    report.claim(
        "B's probe of the idle UI thread's window answers",
        &runs(&outcomes),
        "false",
        deterministic(&outcomes, &false),
    );
    report.claim(
        "its time",
        &summary_ms(&times),
        "about 3,015 ms",
        Verdict::Trend,
    );
}

fn order(report: &mut Report) {
    let mut outcomes = Vec::new();
    for _ in 0..trials() {
        let mut scene = Scene::new();
        let hwnd = scene.provider.hwnd(0);
        scene.provider.log();
        scene.provider.request("busy 0");
        let start = now_us();
        about(2);
        let lookup = uia::spawn(move || uia::lookup(&client(), hwnd));
        reached(&mut scene.provider, start);
        let sent = send(&mut scene.a, "rop");
        reached(&mut scene.provider, sent);
        scene.release();
        outcomes.push(lookup.join().expect("B's lookup"));
        scene.operation_done();
    }
    report.claim(
        "B's lookup sent before A's operation answers",
        &runs(&outcomes),
        "UIA's window proxy",
        deterministic(&outcomes, &"proxy".to_owned()),
    );
}

fn walk_up_across(report: &mut Report) {
    let mut outcomes = Vec::new();
    let mut times = Vec::new();
    for _ in 0..trials() {
        let mut scene = Scene::new();
        let mut other = Provider::launch(&["--windows", "uia:0"]);
        other.request(&format!("parent 0 {}", scene.provider.hwnd(0)));
        let b = client();
        let child: Shared = uia::share(&uia::element(&b, other.hwnd(0)).expect("the child window"));
        about(2);
        let (served, ms, _) = during(&mut scene, true, false, move || {
            walk_up(&client(), &uia::resolve(&child))
        });
        outcomes.push(served);
        times.push(ms);
    }
    report.claim(
        "B's walk up from another process's child window into window 0 reaches",
        &runs(&outcomes),
        "UIA's window proxy",
        deterministic(&outcomes, &"proxy".to_owned()),
    );
    report.claim(
        "its time",
        &summary_ms(&times),
        "about 2,045 ms",
        Verdict::Trend,
    );
}

fn delay(report: &mut Report) {
    let mut delayed = Vec::new();
    let mut control = Vec::new();
    let mut control_ms = Vec::new();
    for _ in 0..trials() {
        for operation in [true, false] {
            let mut scene = Scene::new();
            let b = client();
            let root = uia::element(&b, scene.provider.hwnd(1)).expect("window 1");
            let first: Shared = uia::share(&uia::node(&b, &root, 1).expect("node 1"));
            let (read, _, released) = during(&mut scene, operation, false, move || {
                Timed::of(|| uia::name(&uia::resolve(&first)))
            });
            if operation {
                delayed.push(read.outcome == "ok" && read.end >= released);
            } else {
                control.push(read.outcome == "ok" && read.end < released);
                control_ms.push(read.ms());
            }
        }
    }
    report.claim(
        "B's read into the idle UI thread waits for the busy thread's release",
        &runs(&delayed),
        "true",
        relation(&delayed),
    );
    report.claim(
        "control, nothing queued: it answers during the busy period",
        &runs(&control),
        "true",
        relation(&control),
    );
    report.claim(
        "the control's time",
        &summary_ms(&control_ms),
        "under 1.1 ms",
        Verdict::Trend,
    );
}

fn second_form(report: &mut Report) {
    const N: usize = 42;
    let mut failed = 0;
    let mut times = Vec::new();
    for _ in 0..N {
        let mut provider = Provider::launch(&["--windows", "uia:0"]);
        let hwnd = provider.hwnd(0);
        let mut x = start_client(None, hwnd, true, true);
        let mut d = start_client(None, hwnd, false, false);
        provider.request("hold GetPropertyValue w=0 n=1");
        let _ = send(&mut x, "read");
        provider
            .held(Duration::from_secs(10))
            .expect("X's read is held");
        let mine = uia::spawn(move || uia::timed(|| uia::lookup(&client(), hwnd)));
        let _ = send(&mut d, &format!("lookup {hwnd}"));
        // The release 300 ms after the lookups, as the original made it:
        // nothing marks a request reaching a held provider.
        uia::pause(Duration::from_millis(300), "before the release");
        provider.request("release");
        let theirs = answer(&d);
        let (mine, took) = mine.join().expect("B's lookup");
        if mine != "own" {
            times.push(uia::ms(took));
        }
        if theirs[1] != "own" {
            times.push(theirs[2].parse::<f64>().expect("a time") / 1000.0);
        }
        if mine != "own" || theirs[1] != "own" {
            failed += 1;
        }
        let _ = answer(&x);
    }
    report.claim(
        "trials in which one of two new connections beside a third client's held call answers otherwise than its own provider",
        &format!("{failed} of {N}"),
        "3 of 42",
        rate(failed, N, 3, 42),
    );
    if !times.is_empty() {
        report.claim(
            "the failed connections' times",
            &summary_ms(&times),
            "2,306.6 to 2,318.6 ms",
            Verdict::Trend,
        );
    }
}

/// A console host started directly on this role's desktop: reports
/// `console <hwnd> <thread id>` once its window is found, and holds it
/// until told `stop`.
fn console_role() -> ! {
    let console = uia_facts_support::console::Console::start();
    role::report(&format!("console {} {}", console.hwnd, console.thread));
    for command in role::commands() {
        if command == "stop" {
            break;
        }
    }
    drop(console);
    exit_now(0)
}

/// Suspends or resumes thread `id`.
fn suspend(id: u32, suspend: bool) {
    // SAFETY: a thread id of a process this program started.
    let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, false, id) }.expect("the thread opens");
    if suspend {
        // SAFETY: a live thread handle with suspend rights.
        let _ = unsafe { SuspendThread(thread) };
    } else {
        // SAFETY: as above.
        let _ = unsafe { ResumeThread(thread) };
    }
    // SAFETY: the handle opened above.
    let _ = unsafe { windows::Win32::Foundation::CloseHandle(thread) };
}

/// One stalled trial on the desktop: window `hwnd`, its thread `thread`
/// suspended, A's operation sent, then B's lookup; resumed after both.
fn suspended_trial(hwnd: isize, thread: u32) -> String {
    let mut a = start_client(Some(DESKTOP), hwnd, true, false);
    let mut b = start_client(Some(DESKTOP), hwnd, false, false);
    suspend(thread, true);
    let _ = send(&mut a, "rop");
    uia::pause(
        Duration::from_millis(5),
        "between the operation and the lookup",
    );
    let _ = send(&mut b, &format!("lookup {hwnd}"));
    uia::pause(Duration::from_millis(300), "the stall, as e35 made it");
    suspend(thread, false);
    let served = answer(&b)[1].clone();
    let _ = answer(&a);
    served
}

fn conhost(report: &mut Report) {
    let mut consoles = Vec::new();
    let mut providers = Vec::new();
    for _ in 0..trials().max(1) {
        let mut console = Role::start_on(Some(DESKTOP), "console", &[]);
        let words = answer(&console);
        assert_eq!(words[0], "console", "the console host starts");
        let hwnd: isize = words[1].parse().expect("a window");
        let thread: u32 = words[2].parse().expect("a thread");
        uia_facts_support::note::waiting("up to about 2 s, a lookup during the stall");
        consoles.push(suspended_trial(hwnd, thread));
        console.send("stop");
        let provider = Provider::launch_on(Some(DESKTOP), &["--windows", "uia:0"]);
        let thread = provider.windows[0].thread;
        uia_facts_support::note::waiting("up to about 2 s, a lookup during the stall");
        providers.push(suspended_trial(provider.hwnd(0), thread));
    }
    report.claim(
        "a fresh console host's window, its thread suspended, looked up during A's operation",
        &runs(&consoles),
        "conhost's own element",
        deterministic(&consoles, &"own".to_owned()),
    );
    report.claim(
        "the provider program stalled the same way",
        &runs(&providers),
        "UIA's window proxy",
        deterministic(&providers, &"proxy".to_owned()),
    );
}

fn getfocus(report: &mut Report) {
    if let Err(reason) = precondition::unlocked_desktop() {
        report.unmet("B's GetFocusedElement during the queued operation", &reason);
        return;
    }
    let mut outcomes = Vec::new();
    for _ in 0..trials() {
        let mut scene = Scene::new();
        assert_eq!(
            scene.provider.request("foreground 0"),
            "true",
            "window 0 is in front"
        );
        let (served, _, _) = during(&mut scene, true, false, move || {
            let b = client();
            let request = uia::cache_request(&b, &[UIA_ProviderDescriptionPropertyId])
                .expect("a cache request");
            // SAFETY: a live client object and cache request.
            uia::served_by(&unsafe { b.GetFocusedElementBuildCache(&request) })
        });
        outcomes.push(served);
    }
    report.claim(
        "B's GetFocusedElement during the queued operation",
        &runs(&outcomes),
        "its own provider (3 of 3)",
        deterministic(&outcomes, &"own".to_owned()),
    );
}
