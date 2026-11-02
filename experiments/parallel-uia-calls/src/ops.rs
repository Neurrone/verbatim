//! `ops`: one client operation at a time, named on stdin, for counting
//! each operation's requests and provider calls (readme, "Requests per
//! client operation"). Every operation runs on the main thread, an MTA
//! thread, except `async`, which runs one on a new thread.
//!
//! ops hwnd=<h> target=mock|serve
//! - Prints `ready tid=<main thread>` once its elements are held.
//! - Before each operation it writes `PUIA_OP <line>` with
//!   `OutputDebugString`, so a debugger's log marks it, and, for mockapp,
//!   resets mockapp's hit counters (`WM_HITS_RESET`, a window message to
//!   its window thread, which a debugger on mockapp marks as a fence);
//!   after it, `PUIA_END`, a 400 ms wait for anything sent late, and the
//!   nonzero counters (`WM_HITS_READ`).
//! - `async <tag> <op>`: runs the operation on a new thread and returns at
//!   once; the thread prints `async <tag> ...` with its start and end in
//!   milliseconds from the process's start.
//! - `transaction <ms>` and `connection <ms>`: UIA's timeouts, process-wide.
//! - `hits`: the counters, not reset.
//! - `wait=1`: connect only after a first line on stdin.
//! - `recovery 0|1` and `coalesce 0|1`: the client's connection recovery
//!   and event coalescing.
//!
//! ett hwnd=<h> trans=<ms> conn=<ms> [conn0=<ms>] [conn1=<ms>] [recovery=0|1]
//! op=ett_find|stall_find: one search on a new client ([`ett`]).

use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::System::Diagnostics::Debug::OutputDebugStringW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BSTR, HSTRING, Interface, PCWSTR, Ref, Result};
use windows_core::implement;

use crate::{Args, get, hwnd_of, mta, new_client};

const HIT_NAMES: [&str; 75] = ["WM_GETOBJECT", "ProviderOptions", "GetPatternProvider", "GetPropertyValue", "HostRawElementProvider", "Navigate", "GetRuntimeId", "BoundingRectangle", "GetEmbeddedFragmentRoots", "SetFocus", "FragmentRoot", "ElementProviderFromPoint", "GetFocus", "Toggle", "ToggleState", "Expand", "Collapse", "ExpandCollapseState", "SetValue", "Value", "IsReadOnly", "Select", "AddToSelection", "RemoveFromSelection", "IsSelected", "SelectionContainer", "GetSelection", "CanSelectMultiple", "IsSelectionRequired", "FirstSelectedItem", "LastSelectedItem", "CurrentSelectedItem", "ItemCount", "accParent", "accChildCount", "get_accChild", "get_accName", "get_accValue", "get_accDescription", "get_accRole", "get_accState", "get_accHelp", "get_accHelpTopic", "get_accKeyboardShortcut", "accFocus", "accSelection", "get_accDefaultAction", "accSelect", "accLocation", "accNavigate", "accHitTest", "accDoDefaultAction", "put_accName", "put_accValue", "GetTypeInfoCount", "GetTypeInfo", "GetIDsOfNames", "IDispatch::Invoke", "ITextProvider::GetSelection", "GetVisibleRanges", "DocumentRange", "GetCaretRange", "Clone", "Compare", "CompareEndpoints", "ExpandToEnclosingUnit", "FindText", "GetAttributeValue", "GetBoundingRectangles", "GetText", "Move", "MoveEndpointByUnit", "MoveEndpointByRange", "ITextRangeProvider::Select", "WM_GETTITLEBARINFOEX"];

fn origin() -> Instant {
    static O: OnceLock<Instant> = OnceLock::new();
    *O.get_or_init(Instant::now)
}
fn since() -> f64 {
    origin().elapsed().as_secs_f64() * 1e3
}
fn tid() -> u32 {
    unsafe { GetCurrentThreadId() }
}
fn dbg(s: &str) {
    let w = HSTRING::from(format!("{s}\n"));
    unsafe { OutputDebugStringW(PCWSTR(w.as_ptr())) };
}
fn say(s: &str) {
    let mut o = std::io::stdout().lock();
    let _ = writeln!(o, "{s}");
    let _ = o.flush();
}

#[implement(IUIAutomationFocusChangedEventHandler)]
struct Focus;
impl IUIAutomationFocusChangedEventHandler_Impl for Focus_Impl {
    fn HandleFocusChangedEvent(&self, sender: Ref<IUIAutomationElement>) -> Result<()> {
        let n = sender.ok().ok().and_then(|e| unsafe { e.CachedName() }.ok()).unwrap_or_default();
        dbg(&format!("PUIA_EVENT focus tid={}", tid()));
        say(&format!("event focus tid={} at={:.1} cached_name={n}", tid(), since()));
        Ok(())
    }
}
#[implement(IUIAutomationEventHandler)]
struct Auto;
impl IUIAutomationEventHandler_Impl for Auto_Impl {
    fn HandleAutomationEvent(&self, _sender: Ref<IUIAutomationElement>, id: UIA_EVENT_ID) -> Result<()> {
        dbg(&format!("PUIA_EVENT auto tid={}", tid()));
        say(&format!("event auto {} tid={} at={:.1}", id.0, tid(), since()));
        Ok(())
    }
}
#[implement(IUIAutomationPropertyChangedEventHandler)]
struct Prop;
impl IUIAutomationPropertyChangedEventHandler_Impl for Prop_Impl {
    fn HandlePropertyChangedEvent(&self, _sender: Ref<IUIAutomationElement>, id: UIA_PROPERTY_ID, _v: &VARIANT) -> Result<()> {
        dbg(&format!("PUIA_EVENT prop tid={}", tid()));
        say(&format!("event prop {} tid={} at={:.1}", id.0, tid(), since()));
        Ok(())
    }
}

struct St {
    c: IUIAutomation,
    hwnd: HWND,
    mock: bool,
    root: IUIAutomationElement,
    x: IUIAutomationElement,
    x2: IUIAutomationElement,
    xc: IUIAutomationElement,
    raw: IUIAutomationTreeWalker,
    ctl: IUIAutomationTreeWalker,
    cr: IUIAutomationCacheRequest,
    cr6: IUIAutomationCacheRequest,
    crsub: IUIAutomationCacheRequest,
    deep: IUIAutomationCondition,
    yes: IUIAutomationCondition,
    text: Option<IUIAutomationTextPattern>,
    range: Option<IUIAutomationTextRange>,
    textel: Option<IUIAutomationElement>,
    action: Option<IUIAutomationElement>,
    toggle: Option<IUIAutomationTogglePattern>,
    invoke: Option<IUIAutomationInvokePattern>,
    held: Mutex<Vec<IUIAutomationElement>>,
    focus: IUIAutomationFocusChangedEventHandler,
    auto: IUIAutomationEventHandler,
    prop: IUIAutomationPropertyChangedEventHandler,
}
unsafe impl Send for St {}
unsafe impl Sync for St {}

fn by_id(c: &IUIAutomation, id: &str) -> IUIAutomationCondition {
    unsafe { c.CreatePropertyCondition(UIA_AutomationIdPropertyId, &VARIANT::from(BSTR::from(id))) }.unwrap()
}

impl St {
    fn new(hwnd: HWND, mock: bool) -> St {
        unsafe {
            let c = new_client();
            let root = c.ElementFromHandle(hwnd).expect("ElementFromHandle");
            let yes = c.CreateTrueCondition().unwrap();
            let cr = c.CreateCacheRequest().unwrap();
            cr.AddProperty(UIA_NamePropertyId).unwrap();
            cr.AddProperty(UIA_ControlTypePropertyId).unwrap();
            let cr6 = c.CreateCacheRequest().unwrap();
            for p in [UIA_NamePropertyId, UIA_ControlTypePropertyId, UIA_AutomationIdPropertyId, UIA_IsEnabledPropertyId, UIA_HasKeyboardFocusPropertyId, UIA_BoundingRectanglePropertyId] {
                cr6.AddProperty(p).unwrap();
            }
            let crsub = c.CreateCacheRequest().unwrap();
            crsub.AddProperty(UIA_NamePropertyId).unwrap();
            crsub.AddProperty(UIA_ControlTypePropertyId).unwrap();
            crsub.SetTreeScope(TreeScope_Subtree).unwrap();
            let (x, x2, deep, textel, action) = if mock {
                let l = by_id(&c, "list1");
                (
                    root.FindFirst(TreeScope_Children, &l).expect("list1"),
                    root.FindFirst(TreeScope_Children, &l).expect("list1"),
                    by_id(&c, "tab2"),
                    root.FindFirst(TreeScope_Children, &by_id(&c, "doc")).ok(),
                    root.FindFirst(TreeScope_Descendants, &by_id(&c, "chk_enable")).ok(),
                )
            } else {
                (root.clone(), c.ElementFromHandle(hwnd).unwrap(), by_id(&c, "none"), None, Some(root.clone()))
            };
            let xc = x.BuildUpdatedCache(&cr).unwrap();
            let text = textel.as_ref().and_then(|e| e.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId).ok());
            let range = text.as_ref().and_then(|t| t.DocumentRange().ok());
            let toggle = if mock { action.as_ref().and_then(|e| e.GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId).ok()) } else { None };
            let invoke = if mock { None } else { action.as_ref().and_then(|e| e.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId).ok()) };
            St {
                raw: c.RawViewWalker().unwrap(),
                ctl: c.ControlViewWalker().unwrap(),
                c, hwnd, mock, root, x, x2, xc, cr, cr6, crsub, deep, yes, text, range, textel, action, toggle, invoke,
                held: Mutex::new(Vec::new()),
                focus: Focus.into(),
                auto: Auto.into(),
                prop: Prop.into(),
            }
        }
    }

    fn hits(&self) -> String {
        let mut s = String::new();
        for (i, n) in HIT_NAMES.iter().enumerate() {
            let v = unsafe { SendMessageW(self.hwnd, WM_APP + 2, Some(WPARAM(i)), None) }.0;
            if v != 0 {
                s += &format!(" {n}={v}");
            }
        }
        for (name, ev) in [("advised_focus", UIA_AutomationFocusChangedEventId.0), ("advised_selected", UIA_SelectionItem_ElementSelectedEventId.0), ("advised_property", UIA_AutomationPropertyChangedEventId.0)] {
            let v = unsafe { SendMessageW(self.hwnd, WM_APP + 4, Some(WPARAM(ev as usize)), None) }.0;
            if v != 0 {
                s += &format!(" {name}={v}");
            }
        }
        s
    }

    /// One operation; the answer, short.
    fn op(&self, w: &[&str]) -> Result<String> {
        unsafe {
            let e = |r: Result<IUIAutomationElement>| r.map(|e| e.CurrentProcessId().map(|_| "element".to_string()).unwrap_or("element".into()));
            let none = || windows::core::Error::empty();
            Ok(match w[0] {
                "raw_first" => { let _ = self.raw.GetFirstChildElement(&self.x)?; "first".into() }
                "raw_next" => { let _ = self.raw.GetNextSiblingElement(&self.x)?; "next".into() }
                "raw_parent" => { let _ = self.raw.GetParentElement(&self.x)?; "parent".into() }
                "ctl_first" => { let _ = self.ctl.GetFirstChildElement(&self.x)?; "first".into() }
                "ctl_next" => { let _ = self.ctl.GetNextSiblingElement(&self.x)?; "next".into() }
                "ctl_parent" => { let _ = self.ctl.GetParentElement(&self.x)?; "parent".into() }
                "raw_first_cache" => self.raw.GetFirstChildElementBuildCache(&self.x, &self.cr)?.CachedName()?.to_string(),
                "raw_next_cache" => self.raw.GetNextSiblingElementBuildCache(&self.x, &self.cr)?.CachedName()?.to_string(),
                "raw_parent_cache" => self.raw.GetParentElementBuildCache(&self.x, &self.cr)?.CachedName()?.to_string(),
                "ctl_first_cache" => self.ctl.GetFirstChildElementBuildCache(&self.x, &self.cr)?.CachedName()?.to_string(),
                "ctl_next_cache" => self.ctl.GetNextSiblingElementBuildCache(&self.x, &self.cr)?.CachedName()?.to_string(),
                "ctl_parent_cache" => self.ctl.GetParentElementBuildCache(&self.x, &self.cr)?.CachedName()?.to_string(),
                "findfirst" => { let _ = self.root.FindFirst(TreeScope_Descendants, &self.deep)?; "found".into() }
                "findfirst_cache" => self.root.FindFirstBuildCache(TreeScope_Descendants, &self.deep, &self.cr)?.CachedName()?.to_string(),
                // E-TT's search (small.json): FindFirstBuildCache over the descendants, Name "First", Name cached.
                "ett_find" => {
                    let cond = self.c.CreatePropertyCondition(UIA_NamePropertyId, &VARIANT::from(BSTR::from("First")))?;
                    let cr = self.c.CreateCacheRequest()?;
                    cr.AddProperty(UIA_NamePropertyId)?;
                    match self.root.FindFirstBuildCache(TreeScope_Descendants, &cond, &cr) { Ok(f) => f.CachedName()?.to_string(), Err(x) if x.code().is_ok() => "none".into(), Err(x) => return Err(x) }
                }
                // The removed stalled-thread test's search: FindFirst over the descendants, Name "Second".
                "stall_find" => {
                    let cond = self.c.CreatePropertyCondition(UIA_NamePropertyId, &VARIANT::from(BSTR::from("Second")))?;
                    match self.root.FindFirst(TreeScope_Descendants, &cond) { Ok(_) => "found".into(), Err(x) if x.code().is_ok() => "none".into(), Err(x) => return Err(x) }
                }
                "findall" => self.root.FindAll(TreeScope_Subtree, &self.yes)?.Length()?.to_string(),
                "findall_cache" => self.root.FindAllBuildCache(TreeScope_Subtree, &self.yes, &self.cr)?.Length()?.to_string(),
                "build_props" => self.x.BuildUpdatedCache(&self.cr6)?.CachedName()?.to_string(),
                "build_subtree" => self.x.BuildUpdatedCache(&self.crsub)?.GetCachedChildren()?.Length()?.to_string(),
                "cur_name" => self.x.CurrentName()?.to_string(),
                "cached_name" => self.xc.CachedName()?.to_string(),
                "runtime_id" => { let a = self.x.GetRuntimeId()?; let _ = windows::Win32::System::Ole::SafeArrayDestroy(a); "id".into() }
                "compare" => self.c.CompareElements(&self.x, &self.x2)?.as_bool().to_string(),
                "fromhandle" => e(self.c.ElementFromHandle(self.hwnd))?,
                "frompoint" => {
                    let mut r = RECT::default();
                    GetWindowRect(self.hwnd, &mut r)?;
                    let p = POINT { x: (r.left + r.right) / 2, y: (r.top + r.bottom) / 2 };
                    let el = self.c.ElementFromPoint(p)?;
                    format!("pid={} name={}", el.CurrentProcessId()?, el.CurrentName()?)
                }
                "focused" => {
                    let el = self.c.GetFocusedElement()?;
                    format!("element (pid read after: {})", el.CurrentProcessId()?)
                }
                "foreground" => {
                    // An Alt press first, so this process may set the foreground window.
                    use windows::Win32::UI::Input::KeyboardAndMouse::{KEYEVENTF_KEYUP, VK_MENU, keybd_event};
                    keybd_event(VK_MENU.0 as u8, 0, Default::default(), 0);
                    keybd_event(VK_MENU.0 as u8, 0, KEYEVENTF_KEYUP, 0);
                    let ok = SetForegroundWindow(self.hwnd).as_bool();
                    format!("set={ok} foreground_is_target={}", GetForegroundWindow() == self.hwnd)
                }
                "waitonaddress" => {
                    // A bare WaitOnAddress with a timeout, nothing waking it: how long the
                    // system's wait takes in this process.
                    let ms: u32 = w.get(1).and_then(|s| s.parse().ok()).unwrap_or(2000);
                    let v = 0u32;
                    let cmp = 0u32;
                    let t = Instant::now();
                    let _ = windows::Win32::System::Threading::WaitOnAddress(&v as *const u32 as *const _, &cmp as *const u32 as *const _, 4, Some(ms));
                    format!("waited {:.1} ms for {ms}", t.elapsed().as_secs_f64() * 1e3)
                }
                "focused_only" => { let _ = self.c.GetFocusedElement()?; "element".into() }
                "getpattern" => {
                    if self.mock {
                        let _ = self.textel.as_ref().ok_or_else(none)?.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)?;
                    } else {
                        let _ = self.action.as_ref().ok_or_else(none)?.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)?;
                    }
                    "pattern".into()
                }
                "action" => {
                    if let Some(t) = &self.toggle { t.Toggle()?; "toggled".into() } else { self.invoke.as_ref().ok_or_else(none)?.Invoke()?; "invoked".into() }
                }
                "docrange" => { let _ = self.text.as_ref().ok_or_else(none)?.DocumentRange()?; "range".into() }
                "gettext" => self.range.as_ref().ok_or_else(none)?.GetText(-1)?.len().to_string(),
                "move" => {
                    let r = self.range.as_ref().ok_or_else(none)?.Clone()?;
                    let n = r.Move(TextUnit_Word, 1)?;
                    format!("moved {n} (Clone first)")
                }
                "move_held" => self.range.as_ref().ok_or_else(none)?.Move(TextUnit_Word, 1)?.to_string(),
                "expand" => { self.range.as_ref().ok_or_else(none)?.ExpandToEnclosingUnit(TextUnit_Line)?; "expanded".into() }
                "add_focus" => { self.c.AddFocusChangedEventHandler(&self.cr, &self.focus)?; "added".into() }
                "remove_focus" => { self.c.RemoveFocusChangedEventHandler(&self.focus)?; "removed".into() }
                "add_auto" => {
                    let id: i32 = w.get(1).and_then(|s| s.parse().ok()).unwrap_or(UIA_SelectionItem_ElementSelectedEventId.0);
                    self.c.AddAutomationEventHandler(UIA_EVENT_ID(id), &self.root, TreeScope_Subtree, &self.cr, &self.auto)?;
                    "added".into()
                }
                "remove_auto" => {
                    let id: i32 = w.get(1).and_then(|s| s.parse().ok()).unwrap_or(UIA_SelectionItem_ElementSelectedEventId.0);
                    self.c.RemoveAutomationEventHandler(UIA_EVENT_ID(id), &self.root, &self.auto)?;
                    "removed".into()
                }
                "add_prop" => { self.c.AddPropertyChangedEventHandlerNativeArray(&self.root, TreeScope_Subtree, &self.cr, &self.prop, &[UIA_NamePropertyId])?; "added".into() }
                "remove_prop" => { self.c.RemovePropertyChangedEventHandler(&self.root, &self.prop)?; "removed".into() }
                "hold_elems" => {
                    let all = self.root.FindAll(TreeScope_Descendants, &self.yes)?;
                    let mut h = self.held.lock().unwrap();
                    for i in 0..all.Length()? { h.push(all.GetElement(i)?); }
                    h.len().to_string()
                }
                "drop_elems" => { let mut h = self.held.lock().unwrap(); let n = h.len(); h.clear(); n.to_string() }
                "drop_one" => { let mut h = self.held.lock().unwrap(); h.pop(); h.len().to_string() }
                other => return Err(windows::core::Error::new(windows::core::HRESULT(0x80070057u32 as i32), format!("op {other}"))),
            })
        }
    }
}

pub fn run(a: &Args) {
    let _ = origin();
    mta();
    let mock = get(a, "target", "mock") == "mock";
    // wait=1: connect only after a first line on stdin, so a debugger can
    // attach before the connection is made.
    if get(a, "wait", "0") == "1" {
        say(&format!("waiting pid={}", std::process::id()));
        let mut l = String::new();
        let _ = std::io::stdin().read_line(&mut l);
    }
    let st = Arc::new(St::new(hwnd_of(get(a, "hwnd", "0")), mock));
    say(&format!("ready tid={}", tid()));
    let stdin = std::io::stdin();
    for l in stdin.lock().lines().map_while(std::result::Result::ok) {
        let w: Vec<&str> = l.split_whitespace().collect();
        if w.is_empty() {
            continue;
        }
        match w[0] {
            "quit" => break,
            "transaction" | "connection" => {
                let c2: IUIAutomation2 = st.c.cast().unwrap();
                let ms: u32 = w[1].parse().unwrap();
                let r = unsafe { if w[0] == "transaction" { c2.SetTransactionTimeout(ms) } else { c2.SetConnectionTimeout(ms) } };
                let (t, cn) = unsafe { (c2.TransactionTimeout().unwrap(), c2.ConnectionTimeout().unwrap()) };
                say(&format!("set {} {ms} -> {r:?}; transaction={t} connection={cn}", w[0]));
            }
            // E-TT's client options (verbatim-uia's create_client at b0a19893): recovery and coalescing.
            "recovery" | "coalesce" => {
                let c6: IUIAutomation6 = st.c.cast().unwrap();
                let on = w[1] == "1";
                let r = unsafe { if w[0] == "recovery" { c6.SetConnectionRecoveryBehavior(if on { ConnectionRecoveryBehaviorOptions_Enabled } else { ConnectionRecoveryBehaviorOptions_Disabled }) } else { c6.SetCoalesceEvents(if on { CoalesceEventsOptions_Enabled } else { CoalesceEventsOptions_Disabled }) } };
                let (a, b) = unsafe { (c6.ConnectionRecoveryBehavior().unwrap(), c6.CoalesceEvents().unwrap()) };
                say(&format!("set {} {on} -> {r:?}; recovery={} coalesce={}", w[0], a.0, b.0));
            }
            "hits" => say(&format!("hits{}", st.hits())),
            "async" => {
                let tag = w[1].to_string();
                let rest: Vec<String> = w[2..].iter().map(|s| s.to_string()).collect();
                let st = st.clone();
                say(&format!("async {tag} launched"));
                std::thread::spawn(move || {
                    mta();
                    let rest: Vec<&str> = rest.iter().map(|s| s.as_str()).collect();
                    let start = since();
                    dbg(&format!("PUIA_ASYNC {tag} tid={}", tid()));
                    say(&format!("async {tag} started={start:.1} tid={}", tid()));
                    let r = st.op(&rest);
                    let end = since();
                    let r = match r { Ok(s) => format!("ok {s}"), Err(x) => format!("error {:#x}", x.code().0) };
                    say(&format!("async {tag} {} {r} started={start:.1} ended={end:.1} took={:.1}", rest.join(" "), end - start));
                });
            }
            _ => {
                dbg(&format!("PUIA_OP {l}"));
                // mockapp's hit-counter reset; the synthetic provider ignores it.
                unsafe { SendMessageW(st.hwnd, WM_APP + 3, None, None) };
                let t = Instant::now();
                let r = st.op(&w);
                let ms = t.elapsed().as_secs_f64() * 1e3;
                dbg(&format!("PUIA_END {l}"));
                std::thread::sleep(Duration::from_millis(400));
                let h = if st.mock { st.hits() } else { String::new() };
                let r = match r { Ok(s) => format!("ok {s}"), Err(x) => format!("error {:#x} {}", x.code().0, x.message()) };
                say(&format!("op {l} {r} {ms:.3} ms; provider{h}"));
            }
        }
    }
    let _ = LPARAM(0);
}

/// E-TT's client as `held_transaction` made it at b0a19893: a new MTA thread
/// with its own CUIAutomation8, connection timeout `conn0` (10 s), recovery
/// and coalescing on (`recovery=0` turns both off), the transaction timeout
/// `trans` set, the connection timeout set to `conn1` when given, then the
/// root fetched with ProcessId cached. Prints `ready`; on a `go` line it
/// sets the connection timeout to `conn` (E-TT's `within(1 s)`) and runs
/// `op` (`ett_find` or `stall_find`), then prints its result and time.
pub fn ett(a: &Args) {
    let _ = origin();
    let hwnd_s = get(a, "hwnd", "0").to_string();
    let trans: u32 = get(a, "trans", "1000").parse().unwrap();
    let conn: u32 = get(a, "conn", "1000").parse().unwrap();
    let rec = get(a, "recovery", "1") == "1";
    let conn0: u32 = get(a, "conn0", "10000").parse().unwrap();
    let conn1: u32 = get(a, "conn1", "0").parse().unwrap();
    let op = get(a, "op", "ett_find").to_string();
    let t = std::thread::spawn(move || unsafe {
        mta();
        let hwnd = hwnd_of(&hwnd_s);
        let c = new_client();
        let c2: IUIAutomation2 = c.cast().unwrap();
        c2.SetConnectionTimeout(conn0).unwrap();
        if rec {
            let c6: IUIAutomation6 = c.cast().unwrap();
            c6.SetCoalesceEvents(CoalesceEventsOptions_Enabled).unwrap();
            c6.SetConnectionRecoveryBehavior(ConnectionRecoveryBehaviorOptions_Enabled).unwrap();
        }
        c2.SetTransactionTimeout(trans).unwrap();
        let cr = c.CreateCacheRequest().unwrap();
        cr.AddProperty(UIA_ProcessIdPropertyId).unwrap();
        if conn1 > 0 { c2.SetConnectionTimeout(conn1).unwrap(); }
        let root = c.ElementFromHandleBuildCache(hwnd, &cr).expect("root");
        say(&format!("ready tid={} pid={}", tid(), std::process::id()));
        let mut l = String::new();
        loop {
            l.clear();
            if std::io::stdin().read_line(&mut l).unwrap() == 0 { return; }
            if l.trim() == "go" { break; }
        }
        c2.SetConnectionTimeout(conn).unwrap();
        dbg("PUIA_OP ett");
        let start = since();
        let r: Result<String> = (|| {
            if op == "ett_find" {
                let cond = c.CreatePropertyCondition(UIA_NamePropertyId, &VARIANT::from(BSTR::from("First")))?;
                let cr = c.CreateCacheRequest()?;
                cr.AddProperty(UIA_NamePropertyId)?;
                match root.FindFirstBuildCache(TreeScope_Descendants, &cond, &cr) { Ok(f) => Ok(f.CachedName()?.to_string()), Err(x) if x.code().is_ok() => Ok("none".into()), Err(x) => Err(x) }
            } else {
                let cond = c.CreatePropertyCondition(UIA_NamePropertyId, &VARIANT::from(BSTR::from("Second")))?;
                match root.FindFirst(TreeScope_Descendants, &cond) { Ok(_) => Ok("found".into()), Err(x) if x.code().is_ok() => Ok("none".into()), Err(x) => Err(x) }
            }
        })();
        let end = since();
        dbg("PUIA_END ett");
        let r = match r { Ok(s) => format!("ok {s}"), Err(x) => format!("error {:#x}", x.code().0) };
        say(&format!("ett {op} {r} started={start:.1} ended={end:.1} took={:.1}", end - start));
        c2.SetConnectionTimeout(10_000).unwrap();
        loop {
            l.clear();
            if std::io::stdin().read_line(&mut l).unwrap() == 0 { return; }
            if l.trim() == "quit" { return; }
        }
    });
    let _ = t.join();
}
