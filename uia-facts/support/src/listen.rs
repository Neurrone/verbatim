//! A listening client: a second client process ([`crate::role`]) that makes
//! the registrations its spec names and reports every event it hears, so a
//! fact can compare client processes that differ in one thing.
//!
//! The parent side is [`Listener`]; a fact's `main` hands the role named
//! [`ROLE`] to [`run`]. The spec is a list of words:
//!
//! - `cto=<ms>`, `tto=<ms>`: the process's connection and transaction
//!   timeouts, set through the first client object; 2,000 and 20,000 ms,
//!   UIA's defaults, when not given.
//! - `sta`: the first client object is made, and its registrations added,
//!   on a thread of a single-threaded apartment that pumps messages.
//! - `older`: the first client object is the older `CUIAutomation`
//!   ([`crate::uia::older_client`]), whose timeouts cannot be set.
//! - `desk`: a desktop-wide focus handler (`AddFocusChangedEventHandler`)
//!   on the first client object; `desk2` a second one on it; `desks=<n>` n
//!   more on it; `deskobj` one
//!   on a second client object on the same thread; `deskthr` one on a
//!   second client object on a thread of its own.
//! - `group=<hwnd>`: the focus event in an event handler group on that
//!   window, subtree scope. This and `name`, `text` and `notification`
//!   may each be given more than once.
//! - `name=<hwnd>`: name changes on that window's subtree; `namethr=<hwnd>`
//!   the same on a client object on a thread of its own.
//! - `text=<hwnd>`: text changed events on that window's subtree.
//! - `notification=<hwnd>`: notifications on that window's subtree;
//!   `desknotification` notifications on the desktop's subtree.
//! - `deskname`: name changes on the desktop's subtree, with a cache
//!   request.
//! - `clearmap`: each client object's proxy factory mapping emptied
//!   (`ClearTable`); `nofocusmap`: every `WinEvent` mapped to the focus
//!   event removed from it; `vbmap`: the `WinEvent`s mapped to the events
//!   Verbatim handles removed from it, focus kept, as Verbatim's client
//!   does (`crates/verbatim-uia/src/proxy.rs`).
//! - `hook=<pid>`: an out-of-context `WinEvent` hook for focus, name-change
//!   and foreground events from that process, 0 for every process.
//!
//! - `cache=<name|none|pid|base>`: what every registration caches: the
//!   sender's name, the default; nothing; its process id; or NVDA's base
//!   cache request, thirteen properties and the text pattern.
//!
//! Lines reported: `ready` once
//! every registration is made; `heard <kind> <name> <µs> <thread id>` for
//! each event (kind `focus`, `name`, `text`, `notification`); `hook
//! <event> <hwnd> <µs>` for each hooked `WinEvent`; and, for the command
//! `read <hwnd>`, `read <outcome> <start µs> <end µs>`, a name read of
//! that window made on a new thread with a client object of its own; for
//! the command `cpu`, `cpu <µs>`, the process's processor time.

use std::sync::mpsc::Receiver;
use std::time::Duration;

use windows::Win32::Foundation::{FILETIME, HWND};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
use windows::Win32::UI::Accessibility::{
    IUIAutomation, IUIAutomation6, IUIAutomationCacheRequest, IUIAutomationEventHandler,
    TreeScope_Subtree, UIA_ActiveTextPositionChangedEventId, UIA_AutomationFocusChangedEventId,
    UIA_AutomationPropertyChangedEventId, UIA_EVENT_ID, UIA_MenuOpenedEventId, UIA_NamePropertyId,
    UIA_NotificationEventId, UIA_PROPERTY_ID, UIA_SelectionItem_ElementSelectedEventId,
    UIA_Text_TextChangedEventId, UIA_Text_TextSelectionChangedEventId,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, EVENT_OBJECT_FOCUS, EVENT_OBJECT_NAMECHANGE, EVENT_SYSTEM_FOREGROUND,
    GetMessageW, MSG, TranslateMessage,
};
use windows::core::Interface;

use crate::events::{self, Handler, Report};
use crate::role::{self, Role};
use crate::uia;
use crate::winevents::Hook;

/// The role name a fact's `main` hands to [`run`].
pub const ROLE: &str = "listen";

/// A listening client process, started and ready.
pub struct Listener {
    role: Role,
    /// The lines [`Listener::next`] passed over, which
    /// [`Listener::lines_until`] answers first.
    skipped: std::cell::RefCell<Vec<Line>>,
    /// Its name in the fact's output.
    pub label: String,
}

/// One line a listener reported.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Line {
    /// An event: its kind, the sender's cached name, when and on which
    /// thread its handler ran.
    Heard {
        /// `focus`, `name`, `text` or `notification`.
        kind: String,
        /// The sender's cached name.
        name: String,
        /// When, on [`crate::now_us`]'s clock.
        at: u64,
        /// The handler's thread.
        thread: u32,
    },
    /// A hooked `WinEvent`.
    Hook {
        /// The event's number.
        event: u32,
        /// The window it names.
        hwnd: isize,
        /// When.
        at: u64,
    },
    /// A name read's outcome and its start and end.
    Read {
        /// `ok` or the HRESULT's name.
        outcome: String,
        /// Its start.
        start: u64,
        /// Its end.
        end: u64,
    },
    /// The process's processor time, in microseconds.
    Cpu(u64),
}

impl Line {
    fn parse(text: &str) -> Option<Self> {
        let words: Vec<&str> = text.split(' ').collect();
        let number = |w: &str| w.parse::<u64>().ok();
        match words.as_slice() {
            ["heard", kind, name, at, thread] => Some(Line::Heard {
                kind: (*kind).to_owned(),
                name: (*name).to_owned(),
                at: number(at)?,
                thread: u32::try_from(number(thread)?).ok()?,
            }),
            ["hook", event, hwnd, at] => Some(Line::Hook {
                event: u32::try_from(number(event)?).ok()?,
                hwnd: hwnd.parse().ok()?,
                at: number(at)?,
            }),
            ["read", outcome, start, end] => Some(Line::Read {
                outcome: (*outcome).to_owned(),
                start: number(start)?,
                end: number(end)?,
            }),
            ["cpu", us] => Some(Line::Cpu(number(us)?)),
            _ => None,
        }
    }
}

impl Listener {
    /// Starts a listener named `label` with `spec` and waits until its
    /// registrations are made.
    ///
    /// # Panics
    ///
    /// When it does not become ready within a minute.
    #[must_use]
    pub fn start(label: &str, spec: &[String]) -> Self {
        Self::start_on(None, label, spec)
    }

    /// Starts a listener named `label` with `spec` on desktop `desktop`
    /// when given ([`crate::desktop`]), and waits until its registrations
    /// are made.
    ///
    /// # Panics
    ///
    /// When it does not become ready within a minute.
    #[must_use]
    pub fn start_on(desktop: Option<&str>, label: &str, spec: &[String]) -> Self {
        let role = Role::start_on(desktop, ROLE, spec);
        // A desktop focus handler hears the current focus as it registers;
        // what comes before `ready` belongs to no trial.
        while role.expect_line() != "ready" {}
        Self {
            role,
            skipped: std::cell::RefCell::new(Vec::new()),
            label: label.to_owned(),
        }
    }

    /// Its process id.
    #[must_use]
    pub fn pid(&self) -> u32 {
        self.role.pid
    }

    /// Sends a command: `read <hwnd>` or `cpu`.
    pub fn send(&mut self, command: &str) {
        self.role.send(command);
    }

    /// Every line it reports until `until` on [`crate::now_us`]'s clock,
    /// and every line already received when that time has passed, the
    /// lines [`Listener::next`] passed over first.
    #[must_use]
    pub fn lines_until(&self, until: u64) -> Vec<Line> {
        let mut lines = std::mem::take(&mut *self.skipped.borrow_mut());
        loop {
            let left = until.saturating_sub(crate::now_us());
            match self.role.line(Duration::from_micros(left)) {
                Some(text) => {
                    crate::note::trace(&format!("{}: {text}", self.label));
                    lines.extend(Line::parse(&text));
                }
                None => return lines,
            }
        }
    }

    /// The next line that `wanted` accepts, within `limit`; the lines it
    /// passes over are kept for [`Listener::lines_until`].
    #[must_use]
    pub fn next(&self, limit: Duration, wanted: impl Fn(&Line) -> bool) -> Option<Line> {
        let until = crate::now_us() + u64::try_from(limit.as_micros()).unwrap_or(u64::MAX);
        loop {
            let left = until.saturating_sub(crate::now_us());
            let text = self.role.line(Duration::from_micros(left.max(1)))?;
            crate::note::trace(&format!("{}: {text}", self.label));
            if let Some(line) = Line::parse(&text) {
                if wanted(&line) {
                    return Some(line);
                }
                self.skipped.borrow_mut().push(line);
            }
        }
    }
}

/// The first time, among `lines`, an event of `kind` from a sender named
/// `name` was heard.
#[must_use]
pub fn heard_at(lines: &[Line], kind: &str, name: &str) -> Option<u64> {
    lines.iter().find_map(|line| match line {
        Line::Heard {
            kind: k,
            name: n,
            at,
            ..
        } if k == kind && n == name => Some(*at),
        _ => None,
    })
}

/// The kind word of event or property `id`.
fn kind_of(id: i32) -> String {
    match id {
        x if x == UIA_AutomationFocusChangedEventId.0 => "focus".to_owned(),
        x if x == UIA_NamePropertyId.0 => "name".to_owned(),
        x if x == UIA_Text_TextChangedEventId.0 => "text".to_owned(),
        x if x == UIA_NotificationEventId.0 => "notification".to_owned(),
        other => other.to_string(),
    }
}

/// Forwards every event `reports` brings to the parent, on a thread of its
/// own.
fn forward(reports: Receiver<Report>) {
    std::thread::spawn(move || {
        for report in reports {
            if let Report::Heard(heard) = report {
                let name = if heard.name.is_empty() {
                    "-".to_owned()
                } else {
                    heard.name.replace(' ', "_")
                };
                role::report(&format!(
                    "heard {} {name} {} {}",
                    kind_of(heard.id),
                    heard.at,
                    heard.thread
                ));
            }
        }
    });
}

/// A new handler whose events are forwarded to the parent.
fn handler() -> IUIAutomationEventHandler {
    let (handler, reports) = Handler::make();
    forward(reports);
    handler
}

/// The registrations of one client object.
struct Registrations<'a> {
    client: &'a IUIAutomation,
    cache: IUIAutomationCacheRequest,
}

impl Registrations<'_> {
    fn desk(&self) {
        let handler = events::as_focus_handler(&handler());
        // SAFETY: live client, cache request and handler.
        unsafe {
            self.client
                .AddFocusChangedEventHandler(&self.cache, &handler)
        }
        .expect("a desktop focus handler");
    }

    fn name(&self, hwnd: isize) {
        let element = self.element(hwnd);
        self.name_on(&element);
    }

    fn name_on(&self, element: &windows::Win32::UI::Accessibility::IUIAutomationElement) {
        let handler = events::as_property_handler(&handler());
        // SAFETY: live client, element, cache request and handler; the
        // slice is the one property.
        unsafe {
            self.client.AddPropertyChangedEventHandlerNativeArray(
                element,
                TreeScope_Subtree,
                &self.cache,
                &handler,
                &[UIA_NamePropertyId],
            )
        }
        .expect("a name-change handler");
    }

    fn automation(&self, event: UIA_EVENT_ID, hwnd: Option<isize>) {
        let element = match hwnd {
            Some(hwnd) => self.element(hwnd),
            None => self.root(),
        };
        let handler = handler();
        // SAFETY: live client, element, cache request and handler.
        unsafe {
            self.client.AddAutomationEventHandler(
                event,
                &element,
                TreeScope_Subtree,
                &self.cache,
                &handler,
            )
        }
        .expect("an automation event handler");
    }

    fn notification(&self, hwnd: Option<isize>) {
        let element = match hwnd {
            Some(hwnd) => self.element(hwnd),
            None => self.root(),
        };
        let client: windows::Win32::UI::Accessibility::IUIAutomation5 = self
            .client
            .cast()
            .expect("CUIAutomation8 handles notifications");
        let handler = events::as_notification_handler(&handler());
        // SAFETY: live client, element, cache request and handler.
        unsafe {
            client.AddNotificationEventHandler(&element, TreeScope_Subtree, &self.cache, &handler)
        }
        .expect("a notification handler");
    }

    fn element(&self, hwnd: isize) -> windows::Win32::UI::Accessibility::IUIAutomationElement {
        // SAFETY: a live client; any window handle is accepted.
        unsafe { self.client.ElementFromHandle(HWND(hwnd as *mut _)) }
            .expect("the window's element")
    }

    fn root(&self) -> windows::Win32::UI::Accessibility::IUIAutomationElement {
        // SAFETY: a live client.
        unsafe { self.client.GetRootElement() }.expect("the desktop's element")
    }
}

/// The cache request a spec's `cache=` names: `name` (the default), the
/// name alone; `none`, nothing; `pid`, the process id; or `base`, NVDA's
/// base cache request, thirteen properties and the text pattern.
fn cache(client: &IUIAutomation, kind: &str) -> IUIAutomationCacheRequest {
    use windows::Win32::UI::Accessibility::{
        UIA_AutomationIdPropertyId, UIA_ClassNamePropertyId, UIA_ControlTypePropertyId,
        UIA_FrameworkIdPropertyId, UIA_HasKeyboardFocusPropertyId, UIA_IsContentElementPropertyId,
        UIA_IsControlElementPropertyId, UIA_IsTextPatternAvailablePropertyId,
        UIA_LocalizedControlTypePropertyId, UIA_NativeWindowHandlePropertyId,
        UIA_ProcessIdPropertyId, UIA_ProviderDescriptionPropertyId, UIA_TextPatternId,
    };
    // SAFETY: a live client.
    let request = unsafe { client.CreateCacheRequest() }.expect("a cache request");
    let properties: &[UIA_PROPERTY_ID] = match kind {
        "name" => &[UIA_NamePropertyId],
        "none" => &[],
        "pid" => &[UIA_ProcessIdPropertyId],
        "min" => &[
            UIA_ProcessIdPropertyId,
            UIA_NativeWindowHandlePropertyId,
            UIA_NamePropertyId,
        ],
        "base" => &[
            UIA_NativeWindowHandlePropertyId,
            UIA_FrameworkIdPropertyId,
            UIA_AutomationIdPropertyId,
            UIA_ClassNamePropertyId,
            UIA_ControlTypePropertyId,
            UIA_ProviderDescriptionPropertyId,
            UIA_ProcessIdPropertyId,
            UIA_IsTextPatternAvailablePropertyId,
            UIA_IsContentElementPropertyId,
            UIA_IsControlElementPropertyId,
            UIA_NamePropertyId,
            UIA_LocalizedControlTypePropertyId,
            UIA_HasKeyboardFocusPropertyId,
        ],
        other => panic!("unknown cache {other}"),
    };
    for property in properties {
        // SAFETY: a live cache request.
        unsafe { request.AddProperty(*property) }.expect("a property cached");
    }
    if kind == "base" {
        // SAFETY: as above.
        unsafe { request.AddPattern(UIA_TextPatternId) }.expect("the text pattern cached");
    }
    request
}

/// The `WinEvent` mappings each client object's spec asks to change.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Map {
    Keep,
    Clear,
    NoFocus,
    Verbatim,
}

/// Changes `client`'s proxy factory mapping as `map` says.
fn apply_map(client: &IUIAutomation, map: Map) {
    if map == Map::Keep {
        return;
    }
    // SAFETY: local calls on a live client and the mapping it returns.
    let mapping = unsafe { client.ProxyFactoryMapping() }.expect("the mapping");
    if map == Map::Clear {
        // SAFETY: as above.
        unsafe { mapping.ClearTable() }.expect("the mapping cleared");
        return;
    }
    let removed: Vec<(UIA_EVENT_ID, UIA_PROPERTY_ID)> = if map == Map::NoFocus {
        vec![(UIA_AutomationFocusChangedEventId, UIA_PROPERTY_ID(0))]
    } else {
        verbatims_ignored_events()
    };
    // SAFETY: as above.
    let count = unsafe { mapping.Count() }.expect("the entry count");
    for index in 0..count {
        // SAFETY: as above, with an index below the count.
        let entry = unsafe { mapping.GetEntry(index) }.expect("an entry");
        let mut changed = false;
        for (event, property) in &removed {
            // SAFETY: a local call on a live entry; the array is read and
            // destroyed below.
            let array = unsafe { entry.GetWinEventsForAutomationEvent(*event, *property) }
                .expect("the entry's WinEvents");
            let mapped = !array.is_null()
                // SAFETY: a live array UIA returned.
                && unsafe { (*array).rgsabound[0].cElements } > 0;
            if !array.is_null() {
                // SAFETY: the array UIA returned, owned here.
                let _ = unsafe { windows::Win32::System::Ole::SafeArrayDestroy(array) };
            }
            if mapped {
                // SAFETY: a null array maps no WinEvent.
                unsafe {
                    entry.SetWinEventsForAutomationEvent(*event, *property, std::ptr::null())
                }
                .expect("the WinEvents removed");
                changed = true;
            }
        }
        if changed {
            // SAFETY: the live mapping; the entry goes back where it was, as
            // an entry's change takes effect only once it is put back.
            unsafe { mapping.RemoveEntry(index) }.expect("the entry removed");
            // SAFETY: as above.
            unsafe { mapping.InsertEntry(index, &entry) }.expect("the entry put back");
        }
    }
}

/// The events whose `WinEvent` mappings Verbatim's client removes, copied
/// from `ignored_events` in `crates/verbatim-uia/src/proxy.rs`: the
/// property changes Verbatim follows, and six automation events.
fn verbatims_ignored_events() -> Vec<(UIA_EVENT_ID, UIA_PROPERTY_ID)> {
    let automation_events = [
        UIA_SelectionItem_ElementSelectedEventId,
        UIA_MenuOpenedEventId,
        UIA_NotificationEventId,
        UIA_Text_TextSelectionChangedEventId,
        UIA_Text_TextChangedEventId,
        UIA_ActiveTextPositionChangedEventId,
    ];
    verbatim_uia::FOCUS_PROPERTIES
        .iter()
        .map(|&property| (UIA_AutomationPropertyChangedEventId, property))
        .chain(
            automation_events
                .into_iter()
                .map(|event| (event, UIA_PROPERTY_ID(0))),
        )
        .collect()
}

/// A word's value after `key=`.
fn value<'a>(spec: &'a [String], key: &'a str) -> Option<&'a str> {
    values(spec, key).next()
}

/// Every word's value after `key=`, in order.
fn values<'a>(spec: &'a [String], key: &'a str) -> impl Iterator<Item = &'a str> {
    spec.iter()
        .filter_map(move |word| word.strip_prefix(key)?.strip_prefix('='))
}

fn handle(word: &str) -> isize {
    word.parse().expect("a window handle")
}

/// A new client object from support's [`uia::client`], as the base
/// interface, its mapping changed as `map` says.
fn new_client(map: Map) -> IUIAutomation {
    let client: IUIAutomation = uia::as_interface(&uia::client());
    apply_map(&client, map);
    client
}

/// The listener role: makes the registrations `spec` names, reports
/// `ready`, then reports what it hears and answers commands until the
/// parent closes them. It ends with [`crate::exit_now`].
///
/// # Panics
///
/// When a registration fails.
#[expect(
    clippy::too_many_lines,
    reason = "one registration a spec word, read as a list"
)]
pub fn run(spec: &[String]) -> ! {
    let has = |word: &str| spec.iter().any(|w| w == word);
    let map = if has("clearmap") {
        Map::Clear
    } else if has("nofocusmap") {
        Map::NoFocus
    } else if has("vbmap") {
        Map::Verbatim
    } else {
        Map::Keep
    };
    let cto: u32 = value(spec, "cto").map_or(2_000, |v| v.parse().expect("a timeout"));
    let tto: u32 = value(spec, "tto").map_or(20_000, |v| v.parse().expect("a timeout"));
    let older = has("older");
    if !older {
        uia::set_timeouts(&uia::client(), cto, tto);
    }
    let _hook = value(spec, "hook").map(|pid| {
        let (hook, events) = Hook::start(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_OBJECT_NAMECHANGE,
            pid.parse().expect("a process id"),
        );
        std::thread::spawn(move || {
            for event in events {
                if matches!(
                    event.event,
                    EVENT_OBJECT_FOCUS | EVENT_OBJECT_NAMECHANGE | EVENT_SYSTEM_FOREGROUND
                ) {
                    role::report(&format!("hook {} {} {}", event.event, event.hwnd, event.at));
                }
            }
        });
        hook
    });
    let cache_kind: &'static str = match value(spec, "cache") {
        None | Some("name") => "name",
        Some("none") => "none",
        Some("pid") => "pid",
        Some("min") => "min",
        Some("base") => "base",
        Some(other) => panic!("unknown cache {other}"),
    };
    let owned: Vec<String> = spec.to_vec();
    let first = move || {
        let spec = &owned;
        let has = |word: &str| spec.iter().any(|w| w == word);
        let client: IUIAutomation = if has("older") {
            uia::older_client()
        } else {
            new_client(map)
        };
        let registrations = Registrations {
            client: &client,
            cache: cache(&client, cache_kind),
        };
        if has("desk") {
            registrations.desk();
        }
        if has("desk2") {
            registrations.desk();
        }
        let more: usize = value(spec, "desks").map_or(0, |n| n.parse().expect("a count"));
        for _ in 0..more {
            registrations.desk();
        }
        for hwnd in values(spec, "group") {
            let client6: IUIAutomation6 = client.cast().expect("CUIAutomation8 has groups");
            // SAFETY: a live client.
            let group = unsafe { client6.CreateEventHandlerGroup() }.expect("a group");
            // SAFETY: a live group, cache request and handler.
            unsafe {
                group.AddAutomationEventHandler(
                    UIA_AutomationFocusChangedEventId,
                    TreeScope_Subtree,
                    &registrations.cache,
                    &handler(),
                )
            }
            .expect("the focus event in the group");
            let element = registrations.element(handle(hwnd));
            events::add_group(&client6, &element, &group).expect("the group registered");
        }
        for hwnd in values(spec, "name") {
            registrations.name(handle(hwnd));
        }
        for hwnd in values(spec, "text") {
            registrations.automation(UIA_Text_TextChangedEventId, Some(handle(hwnd)));
        }
        for hwnd in values(spec, "notification") {
            registrations.notification(Some(handle(hwnd)));
        }
        if has("desknotification") {
            registrations.notification(None);
        }
        if has("deskname") {
            let root = registrations.root();
            registrations.name_on(&root);
        }
        client
    };
    let _first = if has("sta") {
        let (made, is_made) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            // SAFETY: the first COM call on this new thread.
            unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
                .ok()
                .expect("an STA thread");
            let client = first();
            let _ = made.send(());
            let _keep = client;
            let mut msg = MSG::default();
            // SAFETY: a live structure for each call.
            while unsafe { GetMessageW(&raw mut msg, None, 0, 0) }.as_bool() {
                // SAFETY: a message GetMessageW filled.
                let _ = unsafe { TranslateMessage(&raw const msg) };
                // SAFETY: as above.
                unsafe { DispatchMessageW(&raw const msg) };
            }
        });
        is_made.recv().expect("the STA thread registers");
        None
    } else {
        Some(first())
    };
    let mut kept = Vec::new();
    if has("deskobj") {
        let client = new_client(map);
        Registrations {
            client: &client,
            cache: cache(&client, cache_kind),
        }
        .desk();
        kept.push(client);
    }
    for (word, desk) in [("deskthr", true), ("namethr", false)] {
        let hwnd = value(spec, word).map(handle);
        if (desk && has(word)) || hwnd.is_some() {
            let (made, is_made) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                uia::mta();
                let client = new_client(map);
                let registrations = Registrations {
                    client: &client,
                    cache: cache(&client, cache_kind),
                };
                match hwnd {
                    Some(hwnd) => registrations.name(hwnd),
                    None => registrations.desk(),
                }
                let _ = made.send(());
                // The client object lives as long as the process.
                let _keep = client;
                loop {
                    std::thread::park();
                }
            });
            is_made.recv().expect("the thread registers");
        }
    }
    role::report("ready");
    for command in role::commands() {
        let words: Vec<&str> = command.split(' ').collect();
        match words.as_slice() {
            ["read", hwnd] => {
                let hwnd = handle(hwnd);
                std::thread::spawn(move || {
                    uia::mta();
                    let client = uia::client();
                    let start = crate::now_us();
                    let outcome = uia::element(&client, hwnd).and_then(|e| uia::name(&e));
                    let end = crate::now_us();
                    role::report(&format!("read {} {start} {end}", uia::outcome(&outcome)));
                });
            }
            ["cpu"] => role::report(&format!("cpu {}", processor_time_us())),
            _ => {}
        }
    }
    crate::exit_now(0)
}

/// This process's processor time, user and kernel, in microseconds.
#[must_use]
pub fn processor_time_us() -> u64 {
    let mut times = [FILETIME::default(); 4];
    let [created, exited, kernel, user] = &mut times;
    // SAFETY: no precondition.
    let process = unsafe { GetCurrentProcess() };
    // SAFETY: the pseudo-handle of this process and four live FILETIMEs.
    let read = unsafe {
        GetProcessTimes(
            process,
            &raw mut *created,
            &raw mut *exited,
            &raw mut *kernel,
            &raw mut *user,
        )
    };
    if read.is_err() {
        return 0;
    }
    let ticks = |t: &FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
    (ticks(&times[2]) + ticks(&times[3])) / 10
}
