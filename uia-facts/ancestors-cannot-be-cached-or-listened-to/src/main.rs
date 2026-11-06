//! Ancestors cannot be cached or listened to: an ancestor-scoped event
//! registration hears no ancestor's event
//! (`uia-facts/ancestors-cannot-be-cached-or-listened-to/readme.md`).
//!
//! In the provider program's tree, node 5's parent is node 1 and node 1's
//! is the window's root, node 0. This process registers name changes on
//! node 5 with each scope that takes in its parent or ancestors, made
//! directly and as an event handler group, and, as a control, on node 1
//! with element scope. Node 1 and then the root are renamed, each raising
//! its name change. On Windows' taskbar the same is done with the clock
//! button, whose name changes each minute: a registration on its first
//! child, ancestors scope, and the control on the button itself.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use uia_facts_support::args::{Args, trials};
use uia_facts_support::events::{self, Handler, OnEvent};
use uia_facts_support::provider::Provider;
use uia_facts_support::report::{Report, Verdict, deterministic, rate, relation, runs};
use uia_facts_support::{note, now_us, precondition, uia};
use windows::Win32::UI::Accessibility::{
    IUIAutomation6, IUIAutomationCacheRequest, IUIAutomationElement, TreeScope,
    TreeScope_Ancestors, TreeScope_Children, TreeScope_Element, TreeScope_Parent,
    UIA_ClassNamePropertyId, UIA_NamePropertyId, UIA_TreeControlTypeId,
};
use windows::core::BSTR;

fn main() {
    let args = Args::parse();
    uia::mta();
    let mut report = Report::start("ancestors-cannot-be-cached-or-listened-to");
    if args.wants("provider") {
        report.case("provider", "name changes registered on node 5 with parent and ancestor scopes, directly and as a group, and on node 1 as a control; node 1 and then the root renamed");
        provider(&args, &mut report);
    }
    // The item takes the keyboard focus, so the case needs the desktop and
    // runs only when named.
    if args.names("tree") {
        report.case("tree", "a tree view whose item has the keyboard focus, in front on the unlocked desktop; name changes registered on the focused item with element and ancestors scope, element scope and ancestors scope, and on the tree view as a control; the tree view renamed");
        tree(&args, &mut report);
    }
    // The clock changes once a minute: the case runs only when named.
    if args.names("taskbar") {
        report.case("taskbar", "name changes registered on the taskbar clock button's first child, ancestors scope, element and ancestors scope, and element scope, and on the button itself; waiting for the clock's next minute, once a trial");
        taskbar(&mut report);
    }
    report.finish();
}

/// What a registration heard: each sender's cached class name and name.
type Heard = Arc<Mutex<Vec<(String, String)>>>;

/// A cache request for the name and the class name.
fn cache(client: &IUIAutomation6) -> IUIAutomationCacheRequest {
    uia::cache_request(client, &[UIA_NamePropertyId, UIA_ClassNamePropertyId])
        .expect("a cache request")
}

/// A handler that keeps each sender's class name and name in `heard`.
fn keeping(heard: Heard) -> OnEvent {
    Box::new(move |sender: &IUIAutomationElement| {
        // SAFETY: a live element UIA passed to the callback.
        let class = unsafe { sender.CachedClassName() }
            .map_or_else(|_| String::new(), |c: BSTR| c.to_string());
        // SAFETY: as above.
        let name =
            unsafe { sender.CachedName() }.map_or_else(|_| String::new(), |c: BSTR| c.to_string());
        heard
            .lock()
            .expect("a lock no one poisons")
            .push((class, name));
    })
}

/// Registers name changes on `element` with `scope`, directly or as a
/// group; answers what it hears.
fn register(
    client: &IUIAutomation6,
    element: &IUIAutomationElement,
    scope: TreeScope,
    group: bool,
) -> Heard {
    let heard: Heard = Arc::new(Mutex::new(Vec::new()));
    let (handler, _reports) = Handler::calling(Some(keeping(heard.clone())));
    let handler = events::as_property_handler(&handler);
    let request = cache(client);
    if group {
        // SAFETY: a live client.
        let g = unsafe { client.CreateEventHandlerGroup() }.expect("a group");
        // SAFETY: live group, cache request and handler.
        unsafe {
            g.AddPropertyChangedEventHandler(scope, &request, &handler, &[UIA_NamePropertyId])
        }
        .expect("the group's handler");
        events::add_group(client, element, &g).expect("the group registered");
    } else {
        // SAFETY: live client, element, cache request and handler.
        unsafe {
            client.AddPropertyChangedEventHandlerNativeArray(
                element,
                scope,
                &request,
                &handler,
                &[UIA_NamePropertyId],
            )
        }
        .expect("the handler registered");
    }
    heard
}

fn provider(args: &Args, report: &mut Report) {
    let scopes = [
        ("parent", TreeScope_Parent),
        ("ancestors", TreeScope_Ancestors),
        (
            "element and parent",
            TreeScope(TreeScope_Element.0 | TreeScope_Parent.0),
        ),
        (
            "element and ancestors",
            TreeScope(TreeScope_Element.0 | TreeScope_Ancestors.0),
        ),
    ];
    let mut heard_by: Vec<Vec<usize>> = vec![Vec::new(); scopes.len() * 2];
    let mut control = Vec::new();
    for _ in 0..trials() {
        let mut provider = Provider::launch(&["--windows", "uia:0"]);
        args.pause_for_debugger(&[("provider", provider.pid), ("client", std::process::id())]);
        let client = uia::client();
        let root = uia::element(&client, provider.hwnd(0)).expect("the window's element");
        let node1 = uia::node(&client, &root, 1).expect("node 1");
        let node5 = uia::node(&client, &root, 5).expect("node 5");
        let registrations: Vec<Heard> = [false, true]
            .iter()
            .flat_map(|group| {
                scopes
                    .iter()
                    .map(|(_, scope)| (*scope, *group))
                    .collect::<Vec<_>>()
            })
            .map(|(scope, group)| register(&client, &node5, scope, group))
            .collect();
        let own = register(&client, &node1, TreeScope_Element, false);
        provider.request("rename 0 1 parent");
        provider.request("raise name 0 1");
        provider.request("rename 0 0 root");
        provider.request("raise name 0 0");
        // The control hears node 1's change; every registration's event,
        // if any, is delivered in turn by the same worker within this.
        let start = now_us();
        while own.lock().expect("a lock no one poisons").is_empty() && now_us() < start + 3_000_000
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        uia::pause(
            Duration::from_millis(500),
            "for any event behind the control's",
        );
        control.push(own.lock().expect("a lock no one poisons").len());
        for (i, heard) in registrations.iter().enumerate() {
            let heard = heard.lock().expect("a lock no one poisons").clone();
            note::evidence(&format!("registration {i}: {heard:?}"));
            heard_by[i].push(heard.len());
        }
    }
    report.claim(
        "the control, element scope on node 1: name changes heard",
        &runs(&control),
        "1 (mockapp's group, docs/parity/backends.md)",
        deterministic(&control, &1),
    );
    for (g, way) in ["directly", "as a group"].iter().enumerate() {
        for (s, (scope, _)) in scopes.iter().enumerate() {
            let counts = &heard_by[g * scopes.len() + s];
            report.claim(
                &format!(
                    "node 5, {scope} scope, made {way}: name changes heard from node 1 and the root"
                ),
                &runs(counts),
                "none (mockapp, made as a group and directly)",
                deterministic(counts, &0),
            );
        }
    }
}

/// Dickson's design: a tree view (node 1, of control type `TreeView`)
/// whose item, node 5, has the keyboard focus; registrations on the
/// focused item with NVDA's scope, Verbatim's and the ancestors alone; the
/// tree view's name change raised through
/// `UiaRaiseAutomationPropertyChangedEvent`.
fn tree(args: &Args, report: &mut Report) {
    if let Err(reason) = precondition::unlocked_desktop() {
        report.unmet("tree: the focused item's registrations", &reason);
        return;
    }
    let scopes = [
        (
            "element and ancestors",
            TreeScope(TreeScope_Element.0 | TreeScope_Ancestors.0),
        ),
        ("element", TreeScope_Element),
        ("ancestors", TreeScope_Ancestors),
    ];
    let mut heard_by: Vec<Vec<usize>> = vec![Vec::new(); scopes.len()];
    let mut control = Vec::new();
    let mut paths = Vec::new();
    for _ in 0..trials() {
        let mut provider = Provider::launch(&["--windows", "uia:0", "--tree-view"]);
        args.pause_for_debugger(&[("provider", provider.pid), ("client", std::process::id())]);
        let front = provider.request("foreground 0");
        provider.request("focus 0 5");
        provider.request("raise focus 0 5");
        let client = uia::client();
        let root = uia::element(&client, provider.hwnd(0)).expect("the window's element");
        let view = uia::node(&client, &root, 1).expect("the tree view");
        // SAFETY: a live client.
        let focused = unsafe { client.GetFocusedElement() }.expect("the focused element");
        let id = |e: &IUIAutomationElement| {
            // SAFETY: a live element.
            unsafe { e.CurrentAutomationId() }
                .map_or_else(|e| uia::hresult(e.code()), |b| b.to_string())
        };
        let kind = |e: &IUIAutomationElement| {
            // SAFETY: a live element.
            unsafe { e.CurrentControlType() }.map_or(0, |t| t.0)
        };
        // SAFETY: a live client.
        let walker = unsafe { client.ControlViewWalker() }.expect("the control view walker");
        // SAFETY: live walker and element.
        let parent = unsafe { walker.GetParentElement(&focused) }.expect("the item's parent");
        let path = format!(
            "in front {front}; focused element {} of control type {}; its parent {} of control type {}",
            id(&focused),
            kind(&focused),
            id(&parent),
            kind(&parent)
        );
        note::evidence(&path);
        paths.push(
            id(&focused) == "n5" && id(&parent) == "n1" && kind(&parent) == UIA_TreeControlTypeId.0,
        );
        let registrations: Vec<Heard> = scopes
            .iter()
            .map(|(_, scope)| register(&client, &focused, *scope, false))
            .collect();
        let own = register(&client, &view, TreeScope_Element, false);
        provider.request("rename 0 1 renamed");
        provider.request("raise name 0 1");
        let start = now_us();
        while own.lock().expect("a lock no one poisons").is_empty() && now_us() < start + 3_000_000
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        uia::pause(
            Duration::from_millis(500),
            "for any event behind the control's",
        );
        let from_view =
            |h: &[(String, String)]| h.iter().filter(|(_, name)| name == "renamed").count();
        control.push(from_view(&own.lock().expect("a lock no one poisons")));
        for (i, heard) in registrations.iter().enumerate() {
            let heard = heard.lock().expect("a lock no one poisons").clone();
            note::evidence(&format!("{} scope on the item: {heard:?}", scopes[i].0));
            heard_by[i].push(from_view(&heard));
        }
        // SAFETY: a live client.
        unsafe { client.RemoveAllEventHandlers() }.expect("the registrations removed");
    }
    report.claim(
        "the focused element is the tree view's item, and UIA reads its parent as the tree view",
        &runs(&paths),
        "a scope over ancestors needs that path",
        relation(&paths),
    );
    report.claim(
        "the control, element scope on the tree view: its name changes heard",
        &runs(&control),
        "1",
        deterministic(&control, &1),
    );
    for (i, (scope, _)) in scopes.iter().enumerate() {
        report.claim(
            &format!("the focused item, {scope} scope: the tree view's name changes heard"),
            &runs(&heard_by[i]),
            "",
            Verdict::New,
        );
    }
}

/// The minutes `taskbar` watches.
const MINUTES: usize = 2;

#[expect(
    clippy::too_many_lines,
    reason = "a minute a trial with four registrations, then their claims"
)]
fn taskbar(report: &mut Report) {
    let client = uia::client();
    // SAFETY: a live client.
    let desktop = unsafe { client.GetRootElement() }.expect("the desktop's element");
    let tray = class_child(&client, &desktop, "Shell_TrayWnd", TreeScope_Children);
    let clock = tray.and_then(|tray| {
        class_child(
            &client,
            &tray,
            "SystemTray.OmniButton",
            windows::Win32::UI::Accessibility::TreeScope_Descendants,
        )
    });
    let Some(clock) = clock else {
        report.unmet(
            "the taskbar clock: the child's ancestors registration",
            "no taskbar clock button (class SystemTray.OmniButton) was found",
        );
        return;
    };
    // SAFETY: a live client.
    let walker = unsafe { client.RawViewWalker() }.expect("the raw view walker");
    // SAFETY: live walker and element.
    let Ok(child) = (unsafe { walker.GetFirstChildElement(&clock) }) else {
        report.unmet(
            "the taskbar clock: the child's ancestors registration",
            "the clock button has no child",
        );
        return;
    };
    let mut seen = Vec::new();
    let mut ancestors_only = Vec::new();
    let mut with_element = 0;
    let mut element_only = 0;
    // Two minutes, whatever --trials says: at the recorded 11 of 12, two
    // minutes with none would differ ((1/12) squared is below 0.05).
    for _ in 0..MINUTES {
        let on_child = register(&client, &child, TreeScope_Ancestors, false);
        let on_both = register(
            &client,
            &child,
            TreeScope(TreeScope_Element.0 | TreeScope_Ancestors.0),
            false,
        );
        // Verbatim's scope, beside NVDA's, in the same minutes.
        let on_element = register(&client, &child, TreeScope_Element, false);
        let on_button = register(&client, &clock, TreeScope_Element, false);
        note::waiting("up to 62 s, the clock's next minute");
        let start = now_us();
        let mut heard_minute = false;
        while now_us() < start + 62_000_000 {
            if !on_button.lock().expect("a lock no one poisons").is_empty() {
                heard_minute = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        uia::pause(
            Duration::from_millis(500),
            "for any event behind the button's",
        );
        // SAFETY: a live client.
        unsafe { client.RemoveAllEventHandlers() }.expect("the registrations removed");
        let button = on_button.lock().expect("a lock no one poisons").clone();
        let heard = on_child.lock().expect("a lock no one poisons").clone();
        let both = on_both.lock().expect("a lock no one poisons").clone();
        let element = on_element.lock().expect("a lock no one poisons").clone();
        note::evidence(&format!(
            "the button's registration heard {button:?}; the child's, ancestors {heard:?}; element and ancestors {both:?}; element {element:?}"
        ));
        let from_button = |h: &[(String, String)]| {
            h.iter()
                .filter(|(class, _)| class == "SystemTray.OmniButton")
                .count()
        };
        seen.push(heard_minute);
        ancestors_only.push(from_button(&heard));
        if from_button(&both) > 0 {
            with_element += 1;
        }
        if from_button(&element) > 0 {
            element_only += 1;
        }
    }
    report.claim(
        "the clock button's own registration hears its name change at the minute",
        &runs(&seen),
        "true (docs/parity/backends.md)",
        relation(&seen),
    );
    let n = seen.len();
    // Judged only over minutes the button's own registration heard.
    if seen.iter().all(|s| *s) {
        report.claim(
            "the registration on the button's first child, ancestors scope: the button's name changes heard",
            &runs(&ancestors_only),
            "none (docs/parity/backends.md)",
            deterministic(&ancestors_only, &0),
        );
        report.claim(
            "the registration on the button's first child, element and ancestors scope: minutes in which it heard the button's name change",
            &format!("{with_element} of {n}"),
            "11 of 12: 1 of 2 (8e3ef116's message), 4 of 4 (93fed1a0's first run), 4 of 4 (de138138's run) and 2 of 2 (the element-scope run)",
            rate(with_element, n, 11, 12),
        );
        report.claim(
            "the registration on the button's first child, element scope alone: minutes in which it heard the button's name change",
            &format!("{element_only} of {n}"),
            "",
            Verdict::New,
        );
    } else {
        report.unmet(
            "the taskbar clock: the child's registrations",
            "the button's own registration missed a minute",
        );
    }
}

/// The first element under `parent`, within `scope`, of class `class`.
fn class_child(
    client: &IUIAutomation6,
    parent: &IUIAutomationElement,
    class: &str,
    scope: TreeScope,
) -> Option<IUIAutomationElement> {
    // SAFETY: a live client and a VARIANT holding a string.
    let condition = unsafe {
        client.CreatePropertyCondition(
            UIA_ClassNamePropertyId,
            &windows::Win32::System::Variant::VARIANT::from(BSTR::from(class)),
        )
    }
    .ok()?;
    // SAFETY: live element and condition.
    unsafe { parent.FindFirst(scope, &condition) }.ok()
}
