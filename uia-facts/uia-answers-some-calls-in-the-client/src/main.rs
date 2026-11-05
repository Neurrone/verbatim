//! UIA answers some calls in the client
//! (`uia-facts/uia-answers-some-calls-in-the-client/readme.md`).
//!
//! Every count is of the provider program's own calls (its log's `call`
//! entries) that start between a call's start and its return, against one
//! UIA window.
//!
//! - `no-call`: the calls that make no provider call: a `Cached*` read; a
//!   second `GetRuntimeId`; `GetRuntimeId` of a sender inside its event
//!   callback; `CompareElements` of two applications' elements with one
//!   application's UI thread busy; the parent of the window's element; the
//!   names of the window's title bar and its parts.
//! - `focus-elsewhere`: `GetFocusedElement` while the provider is not in
//!   front.
//! - `focus-here`: `GetFocusedElement` with the provider's window brought
//!   to the foreground. Needs the unlocked desktop.
//! - `lapse`: UI Automation's copy of an element's runtime id lapsing, on
//!   node 1 (`First`), through Verbatim's own `Registration`: the cold and
//!   warm registration; the copy warm with no pause and lapsed after 1 s;
//!   the copy kept per element; the workaround's warm registrations and
//!   free moves after pauses of 0, 0.5, 1 and 1.5 s.
//! - `late-calls`: `AdviseEventRemoved` arriving after a removal returns,
//!   and no other provider call after a registration, a removal or a read
//!   returns.
//! - `search-reread`: whether a search re-reads its start's runtime id,
//!   by the time since the start was fetched: 600 ms or more, not at 300.
//! - `operations`: each client operation of the explainer's "What one
//!   request is", one at a time, each marked for a debugger with
//!   `OutputDebugString` (`uia-facts op <name> begin` and `end`), and
//!   under `--pause` preceded by a wait for Enter; prints the provider calls
//!   each made, for trends. The requests guide
//!   (`uia-facts/requests-per-client-operation/readme.md`) counts the
//!   requests with cdb around these markers.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use uia_facts_support::args::{Args, trials};
use uia_facts_support::events::{self, Handler, Report as Event};
use uia_facts_support::note;
use uia_facts_support::precondition;
use uia_facts_support::provider::{Entry, Provider};
use uia_facts_support::report::{Report, Verdict, deterministic, relation, runs};
use uia_facts_support::uia::{self, Shared};
use uia_facts_support::{now_us, rop};
use verbatim_uia::{FOCUS_PROPERTIES, Part, PropertyEvent, Registration, Scope, Subscription};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    IUIAutomation6, IUIAutomationElement, TreeScope_Children, TreeScope_Descendants,
    TreeScope_Element, TreeScope_Subtree, UIA_ControlTypePropertyId, UIA_NamePropertyId,
    UIA_TitleBarControlTypeId,
};

fn main() {
    let args = Args::parse();
    uia::mta();
    let mut report = Report::start("uia-answers-some-calls-in-the-client");
    if args.wants("no-call") {
        report.case("no-call", "making each call that should stay in the client; counting the provider calls during it");
        no_call(&mut report);
    }
    if args.wants("focus-elsewhere") {
        report.case("focus-elsewhere", "reading the focused element with another process in front; counting the provider calls");
        focus(&mut report, false);
    }
    if args.wants("focus-here") {
        report.case("focus-here", "bringing the provider's window to the front; reading the focused element; counting the provider calls");
        match precondition::unlocked_desktop() {
            Ok(()) => focus(&mut report, true),
            Err(reason) => report.unmet("GetFocusedElement with the provider in front", &reason),
        }
    }
    if args.wants("lapse") {
        report.case("lapse", "registering on, moving to and reading node 1 after pauses; counting the provider calls each makes");
        lapse(&args, &mut report);
    }
    if args.wants("late-calls") {
        report.case("late-calls", "registering, reading and removing; watching the provider's log for 500 ms after each returns");
        late_calls(&mut report);
    }
    if args.wants("search-reread") {
        report.case("search-reread", "searching from node 1 fetched 0 to 1,000 ms before; counting node 1's runtime id reads during the search");
        search_reread(&mut report);
    }
    if args.wants("operations") {
        report.case(
            "operations",
            "making each client operation of the explainer once; counting its provider calls",
        );
        operations(&args, &mut report);
    }
    report.finish();
}

/// The provider calls that started between `from` and `to`.
fn calls_between(log: &[Entry], from: u64, to: u64) -> Vec<String> {
    log.iter()
        .filter(|e| e.kind == "call" && e.start >= from && e.start <= to)
        .map(|e| e.name.clone())
        .collect()
}

/// Runs `call` and answers the provider calls made while it ran.
fn counted<T>(provider: &mut Provider, call: impl FnOnce() -> T) -> (T, Vec<String>) {
    provider.log();
    let from = now_us();
    let value = call();
    let to = now_us();
    let calls = calls_between(&provider.log(), from, to);
    note::evidence(&format!("provider calls: {calls:?}"));
    (value, calls)
}

struct Window {
    provider: Provider,
    client: IUIAutomation6,
    root: IUIAutomationElement,
    first: IUIAutomationElement,
}

fn window() -> Window {
    window_after(|_| {})
}

/// The window, `before` run with the provider's process id before this
/// process first connects to it.
fn window_after(before: impl FnOnce(u32)) -> Window {
    let provider = Provider::launch(&["--windows", "uia:0"]);
    before(provider.pid);
    let client = uia::client();
    uia::set_timeouts(&client, 2_000, 5_000);
    let root = uia::element(&client, provider.hwnd(0)).expect("the window's element");
    let first = uia::node(&client, &root, 1).expect("node 1");
    Window {
        provider,
        client,
        root,
        first,
    }
}

/// Something done to the window during a pause.
type Touch = Box<dyn Fn(&Window)>;

fn runtime_id(element: &IUIAutomationElement) -> Vec<i32> {
    verbatim_uia::runtime_id(element)
}

#[expect(clippy::too_many_lines, reason = "one block per call, read in turn")]
fn no_call(report: &mut Report) {
    let mut cached = Vec::new();
    let mut second_id = Vec::new();
    let mut in_callback = Vec::new();
    let mut compare = Vec::new();
    let mut parent = Vec::new();
    let mut title = Vec::new();
    for _ in 0..trials() {
        let mut w = window();
        let request =
            uia::cache_request(&w.client, &[UIA_NamePropertyId]).expect("a cache request");
        // SAFETY: live element and cache request.
        let fetched = unsafe { w.first.BuildUpdatedCache(&request) }.expect("the cache is built");
        // SAFETY: a live element with the name cached.
        let (_, calls) = counted(&mut w.provider, || unsafe { fetched.CachedName() });
        cached.push(calls.len());

        let _ = runtime_id(&w.first);
        let (_, calls) = counted(&mut w.provider, || runtime_id(&w.first));
        second_id.push(calls.len());

        let window_of_call = Arc::new(Mutex::new((0u64, 0u64)));
        let recorded = Arc::clone(&window_of_call);
        let (handler, reports) =
            Handler::calling(Some(Box::new(move |sender: &IUIAutomationElement| {
                let from = now_us();
                let _ = verbatim_uia::runtime_id(sender);
                *recorded.lock().expect("the window") = (from, now_us());
            })));
        let node = uia::node(&w.client, &w.root, 5).expect("node 5");
        let cache = events::name_cache(&w.client).expect("a cache request");
        let property = events::as_property_handler(&handler);
        // SAFETY: live client, element, cache request and handler.
        unsafe {
            w.client.AddPropertyChangedEventHandlerNativeArray(
                &node,
                TreeScope_Element,
                &cache,
                &property,
                &[UIA_NamePropertyId],
            )
        }
        .expect("the registration");
        w.provider.log();
        w.provider.request("rename 0 5 changed");
        w.provider.request("raise name 0 5");
        let heard = matches!(
            reports.recv_timeout(Duration::from_secs(10)),
            Ok(Event::Heard(_))
        );
        let (from, to) = *window_of_call.lock().expect("the window");
        in_callback.push(heard.then(|| calls_between(&w.provider.log(), from, to).len()));

        let mut other = Provider::launch(&["--windows", "uia:0"]);
        let other_root = uia::element(&w.client, other.hwnd(0)).expect("the other window");
        let other_first = uia::node(&w.client, &other_root, 1).expect("its node 1");
        let _ = (runtime_id(&w.first), runtime_id(&other_first));
        other.log();
        w.provider.log();
        other.request("busy 0");
        let from = now_us();
        // SAFETY: two live elements.
        let same = unsafe { w.client.CompareElements(&w.first, &other_first) };
        let to = now_us();
        other.request("release");
        let (other_log, _) = other.log_until(Duration::from_millis(500), |e| e.kind == "call");
        let ours = calls_between(&w.provider.log(), from, to).len();
        let theirs = other_log
            .iter()
            .filter(|e| e.kind == "call" && e.start >= from)
            .count();
        // Answered without waiting on the busy application: well inside
        // any timeout it would otherwise have waited.
        compare.push((
            same.map(windows::core::BOOL::as_bool).ok(),
            ours + theirs,
            to - from < 100_000,
        ));

        // SAFETY: a live client object.
        let walker = unsafe { w.client.RawViewWalker() }.expect("the raw view walker");
        // SAFETY: live walker and element.
        let (desktop, calls) = counted(&mut w.provider, || unsafe {
            walker.GetParentElement(&w.root)
        });
        // SAFETY: a live client object.
        let root_element = unsafe { w.client.GetRootElement() }.expect("the desktop");
        let is_desktop = desktop.ok().map(|d| {
            // SAFETY: two live elements.
            unsafe { w.client.CompareElements(&d, &root_element) }
                .is_ok_and(windows::core::BOOL::as_bool)
        });
        parent.push((is_desktop, calls.len()));

        // SAFETY: a live client object; a VARIANT holding the control type.
        let condition = unsafe {
            w.client.CreatePropertyCondition(
                UIA_ControlTypePropertyId,
                &VARIANT::from(UIA_TitleBarControlTypeId.0),
            )
        }
        .expect("a condition");
        // SAFETY: live element and condition.
        let bar =
            unsafe { w.root.FindFirst(TreeScope_Children, &condition) }.expect("the title bar");
        // SAFETY: a live client object.
        let all = unsafe { w.client.CreateTrueCondition() }.expect("a condition");
        // SAFETY: live element and condition.
        let parts =
            unsafe { bar.FindAll(TreeScope_Descendants, &all) }.expect("the title bar's parts");
        let (names, calls) = counted(&mut w.provider, || {
            // SAFETY: a live array.
            let count = unsafe { parts.Length() }.unwrap_or(0);
            let mut names = vec![uia::name(&bar).is_ok()];
            for i in 0..count {
                // SAFETY: an index inside the array.
                names.push(unsafe { parts.GetElement(i) }.is_ok_and(|p| uia::name(&p).is_ok()));
            }
            names.len()
        });
        title.push((names, calls.len()));
    }
    report.claim(
        "a Cached* read: provider calls",
        &runs(&cached),
        "0",
        deterministic(&cached, &0),
    );
    report.claim(
        "GetRuntimeId after the first read: provider calls",
        &runs(&second_id),
        "0",
        deterministic(&second_id, &0),
    );
    report.claim(
        "GetRuntimeId of a sender inside its callback: provider calls",
        &runs(&in_callback),
        "Some(0)",
        deterministic(&in_callback, &Some(0)),
    );
    let compared: Vec<(Option<bool>, bool)> = compare
        .iter()
        .map(|(same, _, before)| (*same, *before))
        .collect();
    report.claim(
        "CompareElements of two applications' elements, one busy: (the answer, within 100 ms)",
        &runs(&compared),
        "(Some(false), true)",
        deterministic(&compared, &(Some(false), true)),
    );
    let compare_calls: Vec<usize> = compare.iter().map(|c| c.1).collect();
    report.claim(
        "CompareElements: provider calls in either application",
        &runs(&compare_calls),
        "0",
        deterministic(&compare_calls, &0),
    );
    report.claim(
        "the parent of the window's element: (the desktop, provider calls)",
        &runs(&parent),
        "(Some(true), 0)",
        deterministic(&parent, &(Some(true), 0)),
    );
    let title_calls: Vec<usize> = title.iter().map(|t| t.1).collect();
    report.claim(
        &format!(
            "the title bar's and its {} parts' names: provider calls",
            title.first().map_or(0, |t| t.0.saturating_sub(1))
        ),
        &runs(&title_calls),
        "0",
        deterministic(&title_calls, &0),
    );
}

fn focus(report: &mut Report, in_front: bool) {
    let mut results = Vec::new();
    for _ in 0..trials() {
        let mut w = window();
        if in_front {
            let front = w.provider.request("foreground 0");
            assert_eq!(
                front, "true",
                "the provider's window comes to the foreground"
            );
        }
        // SAFETY: a live client object.
        let (focused, calls) = counted(&mut w.provider, || unsafe { w.client.GetFocusedElement() });
        // SAFETY: a live element.
        let pid = focused
            .ok()
            .and_then(|f| unsafe { f.CurrentProcessId() }.ok());
        results.push((
            pid == Some(i32::try_from(w.provider.pid).expect("a process id")),
            !calls.is_empty(),
        ));
    }
    if in_front {
        report.claim(
            "GetFocusedElement with the provider in front: (its element, provider calls made)",
            &runs(&results),
            "(true, true): 6 requests into mockapp",
            deterministic(&results, &(true, true)),
        );
    } else {
        report.claim(
            "GetFocusedElement with another process in front: (the provider's element, provider calls made)",
            &runs(&results),
            "(false, false): its requests go to the process in front",
            deterministic(&results, &(false, false)),
        );
    }
}

/// A focus-following registration on `element` through Verbatim's own
/// `Registration`, as `call_counts::uia_event_registrations_cost_exactly`
/// makes it.
fn follow(element: Shared) -> Registration {
    Registration::with_parts(
        vec![Part::shaped::<PropertyEvent>(Subscription::Properties {
            properties: FOCUS_PROPERTIES.to_vec(),
            callback: Arc::new(|_, _| {}),
        })],
        Scope::Elements(vec![element]),
    )
    .expect("the focus-following registration")
}

/// The provider calls of a `GetRuntimeId` read of `element` after `pause`,
/// its copy renewed just before the pause.
fn read_after(w: &mut Window, element: &IUIAutomationElement, pause: Duration) -> usize {
    let _ = runtime_id(element);
    uia::pause(pause, "the pause before the GetRuntimeId read");
    counted(&mut w.provider, || runtime_id(element)).1.len()
}

#[expect(clippy::too_many_lines, reason = "the lapse's claims, read in turn")]
fn lapse(args: &Args, report: &mut Report) {
    let mut w = window();
    let cold = Duration::from_millis(1_000);
    args.pause_for_debugger(&[("provider", w.provider.pid), ("client", std::process::id())]);

    let mut costs = Vec::new();
    let mut sum_holds = Vec::new();
    for _ in 0..trials() {
        let _ = runtime_id(&w.first);
        let (warm_registration, warm) = counted(&mut w.provider, || follow(uia::share(&w.first)));
        drop(warm_registration);
        let _ = runtime_id(&w.first);
        uia::pause(cold, "for the copy of the runtime id to lapse");
        let (cold_registration, cold_calls) =
            counted(&mut w.provider, || follow(uia::share(&w.first)));
        drop(cold_registration);
        let first = w.first.clone();
        let lapsed_read = read_after(&mut w, &first, cold);
        sum_holds.push(cold_calls.len() == warm.len() + lapsed_read);
        costs.push((warm.len(), cold_calls.len(), lapsed_read));
    }
    report.claim(
        "the registration's provider calls: (warm, cold after 1 s, a lapsed GetRuntimeId read alone)",
        &runs(&costs),
        "(3, 9, 6) on mockapp, re-recorded on the provider program",
        Verdict::New,
    );
    report.claim(
        "the cold registration costs the warm one plus a lapsed GetRuntimeId read",
        &runs(&sum_holds),
        "true",
        relation(&sum_holds),
    );

    let first = w.first.clone();
    let warm_reads: Vec<bool> = (0..trials())
        .map(|_| read_after(&mut w, &first, Duration::ZERO) == 0)
        .collect();
    report.claim(
        "with no pause, a GetRuntimeId read makes no provider call",
        &runs(&warm_reads),
        "true",
        relation(&warm_reads),
    );
    let cold_reads: Vec<bool> = (0..trials())
        .map(|_| read_after(&mut w, &first, cold) > 0)
        .collect();
    report.claim(
        "after a pause of 1 s, a GetRuntimeId read makes provider calls",
        &runs(&cold_reads),
        "true",
        relation(&cold_reads),
    );

    let second = uia::node(&w.client, &w.root, 6).expect("node 6");
    let others: Vec<(&str, Touch)> = vec![
        (
            "First's name",
            Box::new(|w: &Window| {
                let _ = uia::name(&w.first);
            }),
        ),
        (
            "another button's name",
            Box::new(move |_: &Window| {
                let _ = uia::name(&second);
            }),
        ),
        (
            "the window's name",
            Box::new(|w: &Window| {
                let _ = uia::name(&w.root);
            }),
        ),
    ];
    let mut kept = Vec::new();
    for (what, touch) in &others {
        let lapsed: Vec<bool> = (0..trials())
            .map(|_| {
                let _ = runtime_id(&w.first);
                uia::pause(cold, "for the copy of the runtime id to lapse");
                touch(&w);
                !counted(&mut w.provider, || runtime_id(&w.first))
                    .1
                    .is_empty()
            })
            .collect();
        kept.push((*what, lapsed));
    }
    let other_button = uia::share(&uia::node(&w.client, &w.root, 7).expect("node 7"));
    let lapsed: Vec<bool> = (0..trials())
        .map(|_| {
            let _ = runtime_id(&w.first);
            uia::pause(cold, "for the copy of the runtime id to lapse");
            drop(follow(other_button.clone()));
            !counted(&mut w.provider, || runtime_id(&w.first))
                .1
                .is_empty()
        })
        .collect();
    kept.push(("a registration on another button", lapsed));
    for (what, lapsed) in kept {
        report.claim(
            &format!("after {what}, First's copy has lapsed all the same"),
            &runs(&lapsed),
            "true",
            relation(&lapsed),
        );
    }

    let pauses = [0_u64, 500, 1_000, 1_500];
    let warm_count = {
        let _ = runtime_id(&w.first);
        counted(&mut w.provider, || follow(uia::share(&w.first)))
            .1
            .len()
    };
    let mut registrations = Vec::new();
    for _ in 0..trials() {
        for pause in pauses {
            uia::pause(
                Duration::from_millis(pause),
                "the pause before the registration",
            );
            let _ = runtime_id(&w.first);
            let calls = counted(&mut w.provider, || follow(uia::share(&w.first)))
                .1
                .len();
            registrations.push(calls == warm_count);
        }
    }
    report.claim(
        "with GetRuntimeId just before, registrations after pauses of 0, 0.5, 1 and 1.5 s cost the warm calls",
        &runs(&registrations),
        "true",
        relation(&registrations),
    );
    let registration = follow(uia::share(&w.first));
    let mut moves = Vec::new();
    for _ in 0..trials() {
        for pause in pauses {
            uia::pause(Duration::from_millis(pause), "the pause before the move");
            let _ = runtime_id(&w.first);
            let again = uia::share(&w.first);
            let calls = counted(&mut w.provider, || {
                registration.retarget(Scope::Elements(vec![again]));
                registration.settle();
            })
            .1
            .len();
            moves.push(calls == 0);
        }
    }
    report.claim(
        "with GetRuntimeId just before, moves to the element followed after pauses of 0, 0.5, 1 and 1.5 s make no provider call",
        &runs(&moves),
        "true",
        relation(&moves),
    );
    drop(registration);
}

fn late_calls(report: &mut Report) {
    let mut removed_late = Vec::new();
    let mut none_after = Vec::new();
    for _ in 0..trials() {
        let mut w = window();
        let (handler, _reports) = Handler::make();
        let group = events::focus_group(&w.client, &handler).expect("a group");
        w.provider.log();
        // Each call's span: its provider calls must all start inside it.
        let mut spans = Vec::new();
        let start = now_us();
        events::add_group(&w.client, &w.root, &group).expect("the registration");
        spans.push((start, now_us()));
        let start = now_us();
        let _ = uia::name(&w.first);
        spans.push((start, now_us()));
        let start = now_us();
        events::remove_group(&w.client, &w.root, &group).expect("the removal");
        let removed_at = now_us();
        spans.push((start, removed_at));
        // Watched 500 ms for the removal's late advise and any stray call.
        let (log, _) = w.provider.log_until(Duration::from_millis(500), |e| {
            e.name == "AdviseEventRemoved"
        });
        let removed = log
            .iter()
            .find(|e| e.name == "AdviseEventRemoved")
            .map(|e| e.start);
        removed_late.push(removed.is_some_and(|t| t > removed_at));
        let stray = log
            .iter()
            .filter(|e| e.kind == "call")
            .filter(|e| {
                !spans
                    .iter()
                    .any(|(from, to)| e.start >= *from && e.start <= *to)
            })
            .count();
        none_after.push(stray);
    }
    report.claim(
        "AdviseEventRemoved arrives after the removal returns",
        &runs(&removed_late),
        "true",
        relation(&removed_late),
    );
    report.claim(
        "no other provider call comes after a registration, a read or a removal returns",
        &runs(&none_after),
        "0",
        deterministic(&none_after, &0),
    );
}

/// Writes `text` to an attached debugger.
fn mark(text: &str) {
    let wide: Vec<u16> = text.encode_utf16().chain([0]).collect();
    // SAFETY: a live, terminated string.
    unsafe {
        windows::Win32::System::Diagnostics::Debug::OutputDebugStringW(windows::core::PCWSTR(
            wide.as_ptr(),
        ));
    }
}

/// One operation of "What one request is".
type Operation = Box<dyn Fn(&Window) -> windows::core::Result<()>>;

#[expect(
    clippy::too_many_lines,
    reason = "one entry per operation, read as a table"
)]
fn operations(args: &Args, report: &mut Report) {
    // Under --pause, the first pause comes before the first connection, so
    // a debugger sees the channel made.
    let mut w = window_after(|pid| {
        args.pause_for_debugger(&[("provider", pid), ("client", std::process::id())]);
    });
    // SAFETY: a live client object.
    let walker = unsafe { w.client.RawViewWalker() }.expect("the raw view walker");
    let request = uia::cache_request(&w.client, &[UIA_NamePropertyId, UIA_ControlTypePropertyId])
        .expect("a cache request");
    // SAFETY: a live client object.
    let all = unsafe { w.client.CreateTrueCondition() }.expect("a condition");
    let deep = uia::automation_id(&w.client, "n21").expect("a condition");
    let hwnd = w.provider.hwnd(0);
    let client = w.client.clone();
    let operations: Vec<(&str, Operation)> = vec![
        (
            "CurrentName",
            Box::new(|w: &Window| uia::name(&w.first).map(|_| ())),
        ),
        (
            "GetRuntimeId, the first read",
            Box::new(|w: &Window| {
                let fresh = uia::node(&w.client, &w.root, 6)?;
                mark("uia-facts op GetRuntimeId fetched");
                let _ = verbatim_uia::runtime_id(&fresh);
                Ok(())
            }),
        ),
        ("a walker's first child", {
            let walker = walker.clone();
            // SAFETY: live walker and element.
            Box::new(move |w: &Window| unsafe { walker.GetFirstChildElement(&w.root) }.map(|_| ()))
        }),
        ("a walker's next sibling", {
            let walker = walker.clone();
            Box::new(move |w: &Window| {
                // SAFETY: live walker and element.
                unsafe { walker.GetNextSiblingElement(&w.first) }.map(|_| ())
            })
        }),
        ("a walker's step to the window's root", {
            let walker = walker.clone();
            // SAFETY: live walker and element.
            Box::new(move |w: &Window| unsafe { walker.GetParentElement(&w.first) }.map(|_| ()))
        }),
        ("FindFirst three levels down", {
            let deep = deep.clone();
            Box::new(move |w: &Window| {
                // SAFETY: live element and condition.
                unsafe { w.root.FindFirst(TreeScope_Descendants, &deep) }.map(|_| ())
            })
        }),
        ("FindFirstBuildCache three levels down", {
            let (deep, request) = (deep.clone(), request.clone());
            Box::new(move |w: &Window| {
                // SAFETY: live element, condition and cache request.
                unsafe {
                    w.root
                        .FindFirstBuildCache(TreeScope_Descendants, &deep, &request)
                }
                .map(|_| ())
            })
        }),
        ("FindAll of the window's descendants", {
            let all = all.clone();
            Box::new(move |w: &Window| {
                // SAFETY: live element and condition.
                unsafe { w.root.FindAll(TreeScope_Descendants, &all) }.map(|_| ())
            })
        }),
        ("FindAllBuildCache of the window's descendants", {
            let (all, request) = (all.clone(), request.clone());
            Box::new(move |w: &Window| {
                // SAFETY: live element, condition and cache request.
                unsafe {
                    w.root
                        .FindAllBuildCache(TreeScope_Descendants, &all, &request)
                }
                .map(|_| ())
            })
        }),
        ("BuildUpdatedCache of two properties", {
            let request = request.clone();
            // SAFETY: live element and cache request.
            Box::new(move |w: &Window| unsafe { w.first.BuildUpdatedCache(&request) }.map(|_| ()))
        }),
        (
            "ElementFromHandle",
            Box::new(move |_: &Window| uia::element(&client, hwnd).map(|_| ())),
        ),
        (
            "the toggle pattern and Toggle",
            Box::new(|w: &Window| {
                let second = uia::node(&w.client, &w.root, 2)?;
                // SAFETY: a live element; the toggle pattern's interface.
                let toggle: windows::Win32::UI::Accessibility::IUIAutomationTogglePattern = unsafe {
                    second
                        .GetCurrentPatternAs(windows::Win32::UI::Accessibility::UIA_TogglePatternId)
                }?;
                mark("uia-facts op Toggle pattern fetched");
                // SAFETY: a live pattern.
                unsafe { toggle.Toggle() }
            }),
        ),
        (
            "the invoke pattern and Invoke",
            Box::new(|w: &Window| {
                // SAFETY: a live element; the invoke pattern's interface.
                let invoke: windows::Win32::UI::Accessibility::IUIAutomationInvokePattern = unsafe {
                    w.first
                        .GetCurrentPatternAs(windows::Win32::UI::Accessibility::UIA_InvokePatternId)
                }?;
                mark("uia-facts op Invoke pattern fetched");
                // SAFETY: a live pattern.
                unsafe { invoke.Invoke() }
            }),
        ),
        (
            "the document range and GetText",
            Box::new(|w: &Window| {
                let notes = uia::node(&w.client, &w.root, 3)?;
                // SAFETY: a live element; the text pattern's interface.
                let text: windows::Win32::UI::Accessibility::IUIAutomationTextPattern = unsafe {
                    notes.GetCurrentPatternAs(windows::Win32::UI::Accessibility::UIA_TextPatternId)
                }?;
                // SAFETY: a live pattern.
                let range = unsafe { text.DocumentRange() }?;
                mark("uia-facts op DocumentRange fetched");
                // SAFETY: a live range.
                unsafe { range.GetText(-1) }.map(|_| ())
            }),
        ),
        (
            "adding and removing a name-change handler on one element, subtree scope",
            Box::new(|w: &Window| {
                let (handler, _reports) = Handler::make();
                let property = events::as_property_handler(&handler);
                let cache = events::name_cache(&w.client)?;
                // SAFETY: live client, element, cache request and handler.
                unsafe {
                    w.client.AddPropertyChangedEventHandlerNativeArray(
                        &w.first,
                        TreeScope_Subtree,
                        &cache,
                        &property,
                        &[UIA_NamePropertyId],
                    )
                }?;
                mark("uia-facts op handler added");
                // SAFETY: as above.
                unsafe {
                    w.client
                        .RemovePropertyChangedEventHandler(&w.first, &property)
                }
            }),
        ),
        (
            "a remote operation reading a name",
            Box::new(|w: &Window| rop::name(&w.first).map(|_| ())),
        ),
        ("releasing the elements of a FindAll", {
            let all = all.clone();
            Box::new(move |w: &Window| {
                // SAFETY: live element and condition.
                let found = unsafe { w.root.FindAll(TreeScope_Descendants, &all) }?;
                mark("uia-facts op FindAll returned; releasing");
                drop(found);
                Ok(())
            })
        }),
    ];
    let mut rows = Vec::new();
    for (name, operation) in &operations {
        args.pause_for_debugger(&[("provider", w.provider.pid), ("client", std::process::id())]);
        println!("operation: {name}");
        mark(&format!("uia-facts op {name} begin"));
        w.provider.log();
        let from = now_us();
        let result = operation(&w);
        let to = now_us();
        mark(&format!("uia-facts op {name} end"));
        let calls = calls_between(&w.provider.log(), from, to);
        rows.push(format!(
            "{name}: {} ({} provider calls)",
            uia::outcome(&result),
            calls.len()
        ));
    }
    report.claim(
        "each operation's provider calls",
        &rows.join("; "),
        "",
        Verdict::Trend,
    );
}

fn search_reread(report: &mut Report) {
    let mut rows = Vec::new();
    let mut by_pause = Vec::new();
    for pause in [0_u64, 300, 600, 700, 1_000] {
        let runs3: Vec<usize> = (0..trials())
            .map(|_| {
                let mut w = window();
                // The start is node 1, a fragment with a runtime id of its
                // own (the window's element takes its id from its handle),
                // and the search finds its child, node 6.
                let condition = uia::name_is(&w.client, "node 6").expect("a condition");
                let start = uia::node(&w.client, &w.root, 1).expect("node 1, fetched now");
                uia::pause(
                    Duration::from_millis(pause),
                    "the pause since the start was fetched",
                );
                w.provider.log();
                let from = now_us();
                // SAFETY: live element and condition.
                let found = unsafe { start.FindFirst(TreeScope_Descendants, &condition) };
                let to = now_us();
                assert!(found.is_ok(), "the search finds node 6");
                // The start's own runtime id reads; the found node's are
                // the result's, not a re-read.
                w.provider
                    .log()
                    .iter()
                    .filter(|e| e.is_call("GetRuntimeId") && e.node == Some(1))
                    .filter(|e| e.start >= from && e.start <= to)
                    .count()
            })
            .collect();
        rows.push(format!("{pause} ms: {}", runs(&runs3)));
        by_pause.push(runs3);
    }
    // Only 0 and 1,000 ms are judged, each run's pair as one outcome; 300,
    // 600 and 700 ms are printed for trends.
    let per_run: Vec<(usize, usize)> = (0..trials())
        .map(|run| (by_pause[0][run], by_pause[4][run]))
        .collect();
    report.claim(
        "a search's reads of its start's runtime id, at pauses of 0 and 1,000 ms",
        &runs(&per_run),
        "(0, 1)",
        deterministic(&per_run, &(0, 1)),
    );
    report.claim(
        "the same, at every pause: 0, 300, 600, 700 and 1,000 ms",
        &rows.join("; "),
        "",
        Verdict::Trend,
    );
}
