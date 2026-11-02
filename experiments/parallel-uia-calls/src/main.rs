//! puia: does an application serve concurrent UIA and MSAA calls in
//! parallel, and does one stuck call hold the others? See readme.md.
//!
//! Subcommands, each taking key=value arguments:
//! - `windows pid=<pid>`: list the process's top-level and child windows.
//! - `bench`: K calls on one thread against the same K split over N threads.
//! - `pair`: a slow call A in a loop while a cheap call B runs on another thread.
//! - `hung`: block an application's UI thread, then time probes into it.
//! - `serve`: the synthetic provider process (serve.rs).
//! - `resume tid=<tid>`: resume a thread left suspended.
//! - `threads pid=<pid>`: every thread of a process, its start address and cycles.
//! - `parts`: one call split into its parts on one clock, at N client threads.
//! - `abandon`: later calls beside a stuck one, on its thread or on new threads.
//! - `ops`: one client operation at a time, named on stdin (ops.rs).
//! - `xbench`: N threads of one client process against N client processes (xproc.rs).

#![allow(clippy::all, clippy::pedantic, missing_docs)]

mod ops;
mod serve;
mod xproc;

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::Win32::System::Threading::{
    OpenThread, ResumeThread, SuspendThread, THREAD_SUSPEND_RESUME,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, Interface};

pub type Args = HashMap<String, Vec<String>>;

fn parse(args: &[String]) -> Args {
    let mut m: Args = HashMap::new();
    for a in args {
        let (k, v) = a.split_once('=').unwrap_or((a.as_str(), ""));
        m.entry(k.to_string()).or_default().push(v.to_string());
    }
    m
}
fn get<'a>(a: &'a Args, k: &str, d: &'a str) -> &'a str {
    a.get(k).and_then(|v| v.last()).map_or(d, |s| s.as_str())
}
fn num(a: &Args, k: &str, d: usize) -> usize {
    get(a, k, "").parse().unwrap_or(d)
}

fn main() {
    let argv: Vec<String> = std::env::args().collect();
    let a = parse(&argv[2..]);
    match argv[1].as_str() {
        "windows" => list_windows(num(&a, "pid", 0) as u32),
        "bench" => bench(&a),
        "pair" => pair(&a),
        "hung" => hung(&a),
        "serve" => serve::run(&a),
        "threads" => {
            for (tid, start, cycles) in thread_table(num(&a, "pid", 0) as u32) {
                println!("thread tid={tid} start={start} cycles={cycles}");
            }
        }
        "parts" => parts(&a),
        "abandon" => abandon(&a),
        "ops" => ops::run(&a),
        "ett" => ops::ett(&a),
        "xbench" => xproc::xbench(&a),
        "worker" => xproc::worker(&a),
        // elems hwnd=<h> elems=<n>: the held elements, by index, as the
        // other subcommands pick them.
        "elems" => unsafe {
            mta();
            let ctx = Ctx::new(new_client(), hwnd_of(get(&a, "hwnd", "0")), num(&a, "elems", 16), &[]);
            println!("descendants found {}", ctx.found);
            for (i, e) in ctx.elems.iter().enumerate() {
                println!(
                    "{i}: name={} framework={} class={} type={}",
                    e.CurrentName().unwrap_or_default(),
                    e.CurrentFrameworkId().unwrap_or_default(),
                    e.CurrentClassName().unwrap_or_default(),
                    e.CurrentControlType().map(|c| c.0).unwrap_or(0)
                );
            }
        },
        // probe hwnd=<h> op=<op> elem=<i> elems=<n>: a probe in a client
        // process of its own; prints ready, makes the call at "go" on stdin.
        "probe" => {
            mta();
            let op = op_of(get(&a, "op", "name"));
            let ctx = Ctx::new(new_client(), hwnd_of(get(&a, "hwnd", "0")), num(&a, "elems", 16), &[op]);
            let e = num(&a, "elem", 0);
            let _ = ctx.call(op, e);
            println!("ready");
            std::io::stdout().flush().ok();
            let mut l = String::new();
            while std::io::stdin().read_line(&mut l).map(|n| n > 0).unwrap_or(false) {
                l.clear();
                let t = Instant::now();
                let r = ctx.call(op, e).map_err(|x| format!("{:#x}", x.code().0));
                let ms = t.elapsed().as_secs_f64() * 1e3;
                match r { Ok(_) => println!("done ok {ms:.3}"), Err(c) => println!("done error {c} {ms:.3}") }
                std::io::stdout().flush().ok();
            }
        }
        // send hwnd=<h> app=<n> wp=<w> lp=<l>: SendMessage(WM_APP + n), e.g.
        // app=4 to set a serve window's name-read delay.
        "send" => unsafe {
            let r = SendMessageW(
                hwnd_of(get(&a, "hwnd", "0")),
                WM_APP + num(&a, "app", 0) as u32,
                Some(WPARAM(num(&a, "wp", 0))),
                Some(LPARAM(num(&a, "lp", 0) as isize)),
            );
            println!("answer {}", r.0);
        },
        "resume" => {
            let tid = num(&a, "tid", 0) as u32;
            unsafe {
                let h = OpenThread(THREAD_SUSPEND_RESUME, false, tid).unwrap();
                println!("resume -> {}", ResumeThread(h));
                let _ = CloseHandle(h);
            }
        }
        other => panic!("unknown subcommand {other}"),
    }
    // UIAutomationCore's detach code can hang a process that used UIA
    // (docs/uia-bugs.md), so end without running it.
    std::io::stdout().flush().ok();
    unsafe { windows::Win32::System::Threading::TerminateProcess(
        windows::Win32::System::Threading::GetCurrentProcess(), 0).ok() };
}

// ---------------------------------------------------------------- windows

fn class_and_title(h: HWND) -> (String, String) {
    let mut c = [0u16; 256];
    let mut t = [0u16; 256];
    let cl = unsafe { GetClassNameW(h, &mut c) } as usize;
    let tl = unsafe { GetWindowTextW(h, &mut t) } as usize;
    (String::from_utf16_lossy(&c[..cl]), String::from_utf16_lossy(&t[..tl]))
}

fn thread_of(h: HWND) -> (u32, u32) {
    let mut pid = 0u32;
    let tid = unsafe { GetWindowThreadProcessId(h, Some(&mut pid)) };
    (tid, pid)
}

fn list_windows(pid: u32) {
    unsafe extern "system" fn top(h: HWND, l: LPARAM) -> BOOL {
        let v = unsafe { &mut *(l.0 as *mut Vec<HWND>) };
        v.push(h);
        BOOL(1)
    }
    let mut tops: Vec<HWND> = Vec::new();
    unsafe { EnumWindows(Some(top), LPARAM(&mut tops as *mut _ as isize)).ok() };
    for h in tops {
        let (tid, p) = thread_of(h);
        if p != pid {
            continue;
        }
        let (c, t) = class_and_title(h);
        let vis = unsafe { IsWindowVisible(h).as_bool() };
        println!("top {} tid={tid} visible={vis} class={c} title={t}", h.0 as isize);
        let mut kids: Vec<HWND> = Vec::new();
        unsafe { let _ = EnumChildWindows(Some(h), Some(top), LPARAM(&mut kids as *mut _ as isize)); };
        for k in kids.iter().take(40) {
            let (ktid, _) = thread_of(*k);
            let (c, t) = class_and_title(*k);
            println!("  child {} tid={ktid} class={c} title={t}", k.0 as isize);
        }
    }
}

// ---------------------------------------------------------------- client side

/// A COM pointer moved to another thread. UIA's client objects and
/// elements are free-threaded, and an MSAA proxy made in the
/// multithreaded apartment is usable from any thread in it.
struct Sendable<T>(T);
unsafe impl<T> Send for Sendable<T> {}

fn mta() {
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok().unwrap();
}

fn new_client() -> IUIAutomation {
    unsafe { CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER) }.unwrap()
}

fn hwnd_of(s: &str) -> HWND {
    HWND(s.parse::<isize>().expect("hwnd") as *mut _)
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Op {
    Name,
    FindAll,
    FromHandle,
    Text,
    Msaa,
    MsaaNew,
    Parent,
    /// A remote operation reading the element's name: one round trip.
    Rop,
    /// One remote operation that reads the element's bounding rectangle 1,500 times: one
    /// round trip whose provider work is long.
    RopSlow,
    /// The whole document's text in one GetText(-1).
    TextAll,
    /// `accName` alone, and `accChildCount` alone, on the held MSAA object.
    MsaaName,
    MsaaCount,
    /// SendMessageW of WM_GETTEXT, and of WM_NULL, to the window: the window
    /// messages without oleacc.
    WmText,
    WmNull,
    /// The invoke pattern's Invoke on the held element.
    Invoke,
}

fn op_of(s: &str) -> Op {
    match s {
        "name" => Op::Name,
        "findall" => Op::FindAll,
        "fromhandle" => Op::FromHandle,
        "text" => Op::Text,
        "msaa" => Op::Msaa,
        "msaanew" => Op::MsaaNew,
        "parent" => Op::Parent,
        "rop" => Op::Rop,
        "ropslow" => Op::RopSlow,
        "textall" => Op::TextAll,
        "msaaname" => Op::MsaaName,
        "msaacount" => Op::MsaaCount,
        "wmtext" => Op::WmText,
        "wmnull" => Op::WmNull,
        "invoke" => Op::Invoke,
        _ => panic!("op {s}"),
    }
}

/// What one thread holds to make its calls into one window.
struct Ctx {
    client: IUIAutomation,
    hwnd: HWND,
    root: IUIAutomationElement,
    elems: Vec<IUIAutomationElement>,
    cond: IUIAutomationCondition,
    cache: IUIAutomationCacheRequest,
    walker: IUIAutomationTreeWalker,
    text: Option<IUIAutomationTextPattern>,
    acc: Option<IAccessible>,
    rops: Vec<verbatim_uia_rops::Operation>,
    found: usize,
}

fn acc_of(hwnd: HWND) -> windows::core::Result<IAccessible> {
    let mut p: *mut core::ffi::c_void = std::ptr::null_mut();
    unsafe {
        AccessibleObjectFromWindow(hwnd, OBJID_CLIENT.0 as u32, &IAccessible::IID, &mut p)?;
        Ok(IAccessible::from_raw(p))
    }
}

impl Ctx {
    /// `elems`: how many descendants to hold for name reads (0 holds the
    /// root alone). `skip_tree` skips the descendant search entirely.
    fn new(client: IUIAutomation, hwnd: HWND, elems: usize, need: &[Op]) -> Ctx {
        unsafe {
            let root = client.ElementFromHandle(hwnd).expect("ElementFromHandle");
            let cond = client.CreateTrueCondition().unwrap();
            let cache = client.CreateCacheRequest().unwrap();
            cache.AddProperty(UIA_NamePropertyId).unwrap();
            cache.AddProperty(UIA_ControlTypePropertyId).unwrap();
            let walker = client.RawViewWalker().unwrap();
            let mut list = Vec::new();
            let mut found = 0;
            if elems > 0 {
                if let Ok(all) = root.FindAll(TreeScope_Descendants, &cond) {
                    // Leave out the title bar and its buttons: UIA's own
                    // proxies answer them in the client, with no call into
                    // the provider. CompareElements makes no call
                    // (experiments/compareelements-makes-no-call).
                    let tb = client
                        .CreatePropertyCondition(UIA_ControlTypePropertyId, &VARIANT::from(UIA_TitleBarControlTypeId.0))
                        .unwrap();
                    let mut furniture = Vec::new();
                    if let Ok(bars) = root.FindAll(TreeScope_Descendants, &tb) {
                        for b in 0..bars.Length().unwrap_or(0) {
                            if let Ok(sub) = bars.GetElement(b).unwrap().FindAll(TreeScope_Subtree, &cond) {
                                for j in 0..sub.Length().unwrap_or(0) {
                                    furniture.push(sub.GetElement(j).unwrap());
                                }
                            }
                        }
                    }
                    let mut kept = Vec::new();
                    for j in 0..all.Length().unwrap_or(0) {
                        let e = all.GetElement(j).unwrap();
                        if !furniture.iter().any(|f| client.CompareElements(f, &e).map(|b| b.as_bool()).unwrap_or(false)) {
                            kept.push(e);
                        }
                    }
                    let n = kept.len();
                    found = n;
                    let step = (n / elems).max(1);
                    let mut i = 0;
                    while i < n && list.len() < elems {
                        list.push(kept[i].clone());
                        i += step;
                    }
                }
            }
            if list.is_empty() {
                list.push(root.clone());
            }
            let text = if need.contains(&Op::Text) || need.contains(&Op::TextAll) {
                let mut tc = client
                    .CreatePropertyCondition(UIA_IsTextPatternAvailablePropertyId, &VARIANT::from(true))
                    .unwrap();
                // PUIA_TEXT_CLASS: the text element's class name, where the
                // first element with the text pattern is not the one wanted
                // (Windows Terminal's tab title is a TextBlock before its
                // terminal control).
                if let Ok(class) = std::env::var("PUIA_TEXT_CLASS") {
                    let cc = client
                        .CreatePropertyCondition(UIA_ClassNamePropertyId, &VARIANT::from(windows::core::BSTR::from(class.as_str())))
                        .unwrap();
                    tc = client.CreateAndCondition(&tc, &cc).unwrap();
                }
                root.FindFirst(TreeScope_Subtree, &tc)
                    .ok()
                    .and_then(|e| e.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId).ok())
            } else {
                None
            };
            let acc = if need.iter().any(|o| matches!(o, Op::Msaa | Op::MsaaName | Op::MsaaCount)) {
                acc_of(hwnd).ok()
            } else {
                None
            };
            let slow = need.contains(&Op::RopSlow);
            let rops = if need.contains(&Op::Rop) || slow {
                list.iter().map(|e| {
                    let mut b = verbatim_uia_rops::Builder::new();
                    let r = b.import_element(e);
                    if slow {
                        let i = b.new_int(0);
                        let lim = b.int(1500);
                        let one = b.int(1);
                        b.while_(|b| b.compare(i, lim, verbatim_uia_rops::Comparison::LessThan), |b| {
                            let _ = b.property(r, UIA_BoundingRectanglePropertyId.0);
                            b.add_assign(i, one);
                        });
                        b.add_to_results(i);
                    } else {
                        let n = b.property(r, UIA_NamePropertyId.0);
                        b.add_to_results(n);
                    }
                    b.finish()
                }).collect()
            } else {
                Vec::new()
            };
            Ctx { client, hwnd, root, elems: list, cond, cache, walker, text, acc, rops, found }
        }
    }

    /// One call of `op`; `i` picks the element. Returns a small number
    /// from the answer so nothing is optimised away.
    fn call(&self, op: Op, i: usize) -> windows::core::Result<usize> {
        unsafe {
            match op {
                Op::Name => Ok(self.elems[i % self.elems.len()].CurrentName()?.len()),
                Op::FindAll => Ok(self
                    .root
                    .FindAllBuildCache(TreeScope_Subtree, &self.cond, &self.cache)?
                    .Length()? as usize),
                Op::FromHandle => {
                    let e = self.client.ElementFromHandle(self.hwnd)?;
                    Ok(e.CurrentProcessId()? as usize)
                }
                Op::Text => {
                    let t = self.text.as_ref().ok_or_else(windows::core::Error::empty)?;
                    Ok(t.DocumentRange()?.GetText(200)?.len())
                }
                Op::Msaa => {
                    let a = self.acc.as_ref().ok_or_else(windows::core::Error::empty)?;
                    let n = a.get_accName(&VARIANT::from(0i32))?.len();
                    Ok(n + a.accChildCount()? as usize)
                }
                Op::MsaaName => {
                    let a = self.acc.as_ref().ok_or_else(windows::core::Error::empty)?;
                    Ok(a.get_accName(&VARIANT::from(0i32))?.len())
                }
                Op::MsaaCount => {
                    let a = self.acc.as_ref().ok_or_else(windows::core::Error::empty)?;
                    Ok(a.accChildCount()? as usize)
                }
                Op::WmText => {
                    let mut buf = [0u16; 256];
                    let r = SendMessageW(self.hwnd, WM_GETTEXT, Some(WPARAM(buf.len())), Some(LPARAM(buf.as_mut_ptr() as isize)));
                    Ok(r.0 as usize)
                }
                Op::WmNull => Ok(SendMessageW(self.hwnd, WM_NULL, None, None).0 as usize),
                Op::MsaaNew => {
                    let a = acc_of(self.hwnd)?;
                    Ok(a.get_accName(&VARIANT::from(0i32))?.len())
                }
                Op::TextAll => {
                    let t = self.text.as_ref().ok_or_else(windows::core::Error::empty)?;
                    Ok(t.DocumentRange()?.GetText(-1)?.len())
                }
                Op::Rop | Op::RopSlow => {
                    let op = &self.rops[i % self.rops.len()];
                    op.execute().map(|_| 1).map_err(|e| match e {
                        verbatim_uia_rops::Error::Execute(x) | verbatim_uia_rops::Error::Import(x) | verbatim_uia_rops::Error::Unavailable(x) => x,
                        other => { eprintln!("rop: {other:?}"); windows::core::Error::from_hresult(windows::core::HRESULT(0x8000_4005u32 as i32)) }
                    })
                }
                Op::Invoke => {
                    let e = &self.elems[i % self.elems.len()];
                    e.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)?.Invoke()?;
                    Ok(1)
                }
                Op::Parent => {
                    let e = &self.elems[i % self.elems.len()];
                    Ok(self.walker.GetParentElement(e).map(|_| 1)?)
                }
            }
        }
    }
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if v.is_empty() {
        return f64::NAN;
    }
    let idx = ((v.len() as f64 - 1.0) * p).round() as usize;
    v[idx]
}

// ---------------------------------------------------------------- bench

enum Cmd {
    Go(Op, usize, usize),
    Stop,
}

/// bench hwnd=<h> label=<l> ops=name,findall,... k=<per op, e.g. name:64>
/// threads=1,2,4,8 reps=25 client=per|shared elems=16 stats=<serve hwnd>
fn bench(a: &Args) {
    // With mockapp=<exe> fixture=<json> backend=<b>, bench starts mockapp
    // and measures its window.
    let mock = a.get("mockapp").map(|v| {
        Mock::start(&v[0], get(a, "fixture", ""), "puia-mock-bench", get(a, "backend", "uia"))
    });
    let hwnd = if mock.is_some() { find_top("puia-mock-bench") } else { hwnd_of(get(a, "hwnd", "0")) };
    // hwnds=<h1,h2,...>: worker w targets hwnds[w % len] instead of hwnd.
    let hwnds: Vec<HWND> =
        a.get("hwnds").map(|v| v[0].split(',').map(hwnd_of).collect()).unwrap_or_else(|| vec![hwnd]);
    let label = get(a, "label", "target").to_string();
    let ops: Vec<Op> = get(a, "ops", "name").split(',').map(op_of).collect();
    let threads: Vec<usize> =
        get(a, "threads", "1,2,4,8").split(',').map(|s| s.parse().unwrap()).collect();
    let reps = num(a, "reps", 25);
    let elems = num(a, "elems", 16);
    let shared = get(a, "client", "per") == "shared";
    let mut kmap: HashMap<String, usize> = HashMap::new();
    for kv in get(a, "k", "").split(',').filter(|s| !s.is_empty()) {
        let (o, k) = kv.split_once(':').unwrap();
        kmap.insert(o.to_string(), k.parse().unwrap());
    }
    let stats_hwnd = a.get("stats").map(|v| hwnd_of(&v[0]));
    mta();
    let (tid, pid) = thread_of(hwnd);
    let (cls, title) = class_and_title(hwnd);
    println!(
        "TARGET label={label} hwnd={} pid={pid} ui_tid={tid} class={cls} title={title} client={}",
        hwnd.0 as isize,
        if shared { "shared" } else { "per-thread" }
    );
    let shared_client = if shared { Some(new_client()) } else { None };
    let max_n = *threads.iter().max().unwrap();
    // One pool of max_n workers per N, made once, each with its own Ctx.
    let mut pools: Vec<(usize, Vec<mpsc::Sender<Cmd>>, mpsc::Receiver<(f64, usize)>)> = Vec::new();
    let mut handles = Vec::new();
    for &n in &threads {
        let (rtx, rrx) = mpsc::channel();
        let mut txs = Vec::new();
        for w in 0..n {
            let (tx, rx) = mpsc::channel::<Cmd>();
            let rtx = rtx.clone();
            let sc = shared_client.clone().map(Sendable);
            let h = Sendable(hwnds[w % hwnds.len()]);
            let need = ops.clone();
            let ready = Arc::new(Barrier::new(2));
            let r2 = ready.clone();
            handles.push(std::thread::spawn(move || {
                let (h, sc) = (h, sc);
                mta();
                let client = sc.map_or_else(new_client, |s| s.0);
                let ctx = Ctx::new(client, h.0, elems, &need);
                if w == 0 {
                    println!(
                        "CTX n={n} descendants_found={} elems_held={} text={} msaa={}",
                        ctx.found,
                        ctx.elems.len(),
                        ctx.text.is_some(),
                        ctx.acc.is_some()
                    );
                }
                r2.wait();
                while let Ok(Cmd::Go(op, count, offset)) = rx.recv() {
                    let t = Instant::now();
                    let mut errs = 0;
                    for i in 0..count {
                        if ctx.call(op, offset + i).is_err() {
                            errs += 1;
                        }
                    }
                    rtx.send((t.elapsed().as_secs_f64() * 1e3, errs)).unwrap();
                }
            }));
            ready.wait();
            txs.push(tx);
        }
        pools.push((n, txs, rrx));
    }
    let _ = max_n;
    for &op in &ops {
        let opname = format!("{op:?}").to_lowercase();
        let k = *kmap.get(&opname).unwrap_or(&64);
        let mut medians: Vec<(usize, f64)> = Vec::new();
        for (n, txs, rrx) in &pools {
            let n = *n;
            let per = k / n;
            let run = |txs: &Vec<mpsc::Sender<Cmd>>| -> (f64, usize) {
                let t = Instant::now();
                for (w, tx) in txs.iter().enumerate() {
                    tx.send(Cmd::Go(op, per, w * per)).unwrap();
                }
                let mut errs = 0;
                for _ in 0..n {
                    errs += rrx.recv().unwrap().1;
                }
                (t.elapsed().as_secs_f64() * 1e3, errs)
            };
            run(txs); // warm-up
            if let Some(s) = stats_hwnd {
                unsafe { SendMessageW(s, WM_APP + 2, None, None) };
            }
            let mut walls = Vec::new();
            let mut errs = 0;
            for rep in 0..reps {
                let (w, e) = run(txs);
                println!("RAW label={label} op={opname} n={n} rep={rep} wall_ms={w:.3} errors={e}");
                walls.push(w);
                errs += e;
            }
            let stats = stats_hwnd.map(|s| {
                let r = unsafe { SendMessageW(s, WM_APP + 2, None, None) }.0 as u64;
                format!(" provider_max_concurrent={} provider_calls={} provider_off_ui_thread={}",
                    r & 0xffff, (r >> 40) & 0xff_ffff, (r >> 16) & 0xff_ffff)
            });
            let med = pct(&mut walls.clone(), 0.5);
            let p90 = pct(&mut walls.clone(), 0.9);
            medians.push((n, med));
            let base = medians[0].1;
            println!(
                "RESULT label={label} op={opname} n={n} k={} median_ms={med:.2} p90_ms={p90:.2} per_call_us={:.1} speedup={:.2} errors={errs}{}",
                per * n,
                med * 1e3 / (per * n) as f64,
                base / med,
                stats.unwrap_or_default()
            );
        }
    }
    for (_, txs, _) in &pools {
        for tx in txs {
            let _ = tx.send(Cmd::Stop);
        }
    }
    if let Some(mut m) = mock {
        m.send("quit");
        let _ = m.child.wait();
    }
}

// ---------------------------------------------------------------- pair

/// pair hwnd_a=<h> op_a=findall [elem_a=0] hwnd_b=<h> op_b=name [elem_b=1]
/// iters_a=10 elems=16 gap_ms=2 label=<l>
/// A runs iters_a times on one thread; B runs back to back (gap_ms apart)
/// on another thread for as long as A runs. Then each alone for the same counts.
fn pair(a: &Args) {
    let label = get(a, "label", "pair").to_string();
    let ha = hwnd_of(get(a, "hwnd_a", "0"));
    let hb = hwnd_of(get(a, "hwnd_b", get(a, "hwnd_a", "0")));
    let opa = op_of(get(a, "op_a", "findall"));
    let opb = op_of(get(a, "op_b", "name"));
    let ea = num(a, "elem_a", 0);
    let eb = num(a, "elem_b", 1);
    let iters_a = num(a, "iters_a", 10);
    let elems = num(a, "elems", 16);
    let gap = Duration::from_millis(num(a, "gap_ms", 2) as u64);
    mta();
    let ctx_a = Sendable(Ctx::new(new_client(), ha, elems, &[opa]));
    let xb = get(a, "xb", "0") == "1";
    let ctx_b = Sendable(BSide::new(xb, hb, opb, eb, elems));
    println!(
        "PAIR label={label} a={opa:?}@{} elem {ea} (of {} held, {} found) b={opb:?}@{} elem {eb} (of {} held)",
        ha.0 as isize, ctx_a.0.elems.len(), ctx_a.0.found, hb.0 as isize, if xb { 0 } else { 16 }
    );
    println!("PAIR b_in_other_process={xb}");
    // A alone.
    let mut a_alone = Vec::new();
    for _ in 0..iters_a {
        let t = Instant::now();
        let r = ctx_a.0.call(opa, ea);
        a_alone.push(t.elapsed().as_secs_f64() * 1e3);
        if let Err(e) = r { println!("A error {e}"); }
    }
    // B alone, the same number of calls B makes below, measured after.
    let (mut ctx_b, b_with, a_with) = {
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let d2 = done.clone();
        let start = Arc::new(Barrier::new(2));
        let s2 = start.clone();
        let bt = std::thread::spawn(move || {
            mta();
            let mut c = ctx_b;
            let mut lat = Vec::new();
            s2.wait();
            while !d2.load(std::sync::atomic::Ordering::Relaxed) {
                lat.push(c.0.call());
                std::thread::sleep(gap);
            }
            (c, lat)
        });
        start.wait();
        let mut aw = Vec::new();
        for _ in 0..iters_a {
            let t = Instant::now();
            let _ = ctx_a.0.call(opa, ea);
            aw.push(t.elapsed().as_secs_f64() * 1e3);
        }
        done.store(true, std::sync::atomic::Ordering::Relaxed);
        let (c, lat) = bt.join().unwrap();
        (c, lat, aw)
    };
    let mut b_alone = Vec::new();
    for _ in 0..b_with.len().max(20) {
        b_alone.push(ctx_b.0.call());
        std::thread::sleep(gap);
    }
    let s = |name: &str, v: &Vec<f64>| {
        let mut c = v.clone();
        println!(
            "RESULT label={label} {name} count={} median_ms={:.2} p90_ms={:.2} max_ms={:.2}",
            c.len(), pct(&mut c, 0.5), pct(&mut c, 0.9), pct(&mut c, 1.0)
        );
    };
    s("a_alone", &a_alone);
    s("a_with_b", &a_with);
    s("b_alone", &b_alone);
    s("b_during_a", &b_with);
}

/// Call B, made on this thread or by a probe process of its own.
enum BSide {
    Local(Ctx, Op, usize),
    Remote(Child, ChildStdin, BufReader<std::process::ChildStdout>),
}

impl BSide {
    fn new(remote: bool, h: HWND, op: Op, e: usize, elems: usize) -> BSide {
        if !remote {
            return BSide::Local(Ctx::new(new_client(), h, elems, &[op]), op, e);
        }
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["probe", &format!("hwnd={}", h.0 as isize), &format!("op={}", format!("{op:?}").to_lowercase()), &format!("elem={e}"), &format!("elems={elems}")])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut out = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        out.read_line(&mut line).unwrap();
        assert!(line.starts_with("ready"), "probe: {line}");
        let stdin = child.stdin.take().unwrap();
        BSide::Remote(child, stdin, out)
    }
    /// One call; its duration in ms, as the calling process timed it.
    fn call(&mut self) -> f64 {
        match self {
            BSide::Local(c, op, e) => {
                let t = Instant::now();
                let _ = c.call(*op, *e);
                t.elapsed().as_secs_f64() * 1e3
            }
            BSide::Remote(_, stdin, out) => {
                writeln!(stdin, "go").unwrap();
                stdin.flush().unwrap();
                let mut line = String::new();
                out.read_line(&mut line).unwrap();
                line.split_whitespace().last().and_then(|s| s.parse().ok()).unwrap_or(f64::NAN)
            }
        }
    }
}

// ---------------------------------------------------------------- hung

struct Mock {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<(Instant, String)>,
}

impl Mock {
    fn start(exe: &str, fixture: &str, title: &str, backend: &str) -> Mock {
        let mut child = Command::new(exe)
            .args(["--fixture", fixture, "--backend", backend, "--title", title])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("mockapp");
        let stdin = child.stdin.take().unwrap();
        let out = child.stdout.take().unwrap();
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for l in BufReader::new(out).lines().map_while(Result::ok) {
                let _ = tx.send((Instant::now(), l));
            }
        });
        let m = Mock { child, stdin, lines };
        m.expect("ready");
        m
    }
    fn send(&mut self, cmd: &str) {
        writeln!(self.stdin, "{cmd}").unwrap();
        self.stdin.flush().unwrap();
    }
    fn expect(&self, prefix: &str) -> (Instant, String) {
        loop {
            let (t, l) = self.lines.recv_timeout(Duration::from_secs(30)).expect(prefix);
            if l.starts_with(prefix) {
                return (t, l);
            }
            println!("mockapp: {l}");
        }
    }
}

fn find_top(title: &str) -> HWND {
    let w: Vec<u16> = title.encode_utf16().chain([0]).collect();
    unsafe { FindWindowW(None, windows::core::PCWSTR(w.as_ptr())) }.expect("FindWindow")
}

struct Suspended(windows::Win32::Foundation::HANDLE, u32);
impl Drop for Suspended {
    fn drop(&mut self) {
        unsafe {
            ResumeThread(self.0);
            let _ = CloseHandle(self.0);
        }
        println!("resumed tid={}", self.1);
    }
}

/// hung block=<suspend:<hwnd> | busy:<hwnd>:<ms> | slow:<hwnd>:<ms>:<sleep|pump> |
///   mock-hold | mock-stall:<ms> | mock-slow-parent:<id>:<ms> | none>
///   [mockapp=<exe> fixture=<json> title=<t> backend=uia child=1]
///   [stuck=<hwnd|main|child>:<op>:<elem>] probe=<label>:<hwnd|main|child>:<op>:<elem> ...
///   observe_ms=6000 elems=16 trials=3 label=<l>
fn hung(a: &Args) {
    let label = get(a, "label", "hung").to_string();
    let trials = num(a, "trials", 3);
    let observe = Duration::from_millis(num(a, "observe_ms", 6000) as u64);
    let elems = num(a, "elems", 16);
    let block = get(a, "block", "none").to_string();
    mta();
    let mut mock = a.get("mockapp").map(|v| {
        Mock::start(&v[0], get(a, "fixture", ""), get(a, "title", "puia-mock"), get(a, "backend", "uia"))
    });
    let mut names: HashMap<String, HWND> = HashMap::new();
    if let Some(m) = mock.as_mut() {
        let main = find_top(get(a, "title", "puia-mock"));
        names.insert("main".into(), main);
        if get(a, "child", "0") == "1" {
            m.send("child-window");
            m.expect("applied");
            let c = unsafe { FindWindowExW(Some(main), None, windows::core::PCWSTR::null(), windows::core::PCWSTR::null()) }.unwrap();
            names.insert("child".into(), c);
        }
    }
    if get(a, "list", "0") == "1" {
        let ctx = Ctx::new(new_client(), names["main"], elems, &[]);
        for (i, e) in ctx.elems.iter().enumerate() {
            unsafe {
                println!(
                    "ELEM {i}: name={} framework={} type={}",
                    e.CurrentName().unwrap_or_default(),
                    e.CurrentFrameworkId().unwrap_or_default(),
                    e.CurrentControlType().map(|c| c.0).unwrap_or(0)
                );
            }
        }
    }
    let resolve = |s: &str| -> HWND { names.get(s).copied().unwrap_or_else(|| hwnd_of(s)) };
    let spec = |s: &str| -> (String, HWND, Op, usize) {
        let p: Vec<&str> = s.split(':').collect();
        (p[0].to_string(), resolve(p[1]), op_of(p[2]), p.get(3).map_or(0, |x| x.parse().unwrap()))
    };
    let probes: Vec<(String, HWND, Op, usize)> =
        a.get("probe").map(|v| v.iter().map(|s| spec(s)).collect()).unwrap_or_default();
    let stuck = a.get("stuck").map(|v| spec(&format!("stuck:{}", v[0])));
    for (l, h, op, e) in probes.iter().chain(stuck.iter()) {
        let (tid, pid) = thread_of(*h);
        let (c, t) = class_and_title(*h);
        println!("CALL {l} hwnd={} pid={pid} ui_tid={tid} op={op:?} elem={e} class={c} title={t}", h.0 as isize);
    }
    for trial in 0..trials {
        // Every probe thread gets its client and elements before the block.
        let go = Arc::new(Barrier::new(probes.len() + 1));
        let (rtx, rrx) = mpsc::channel::<(String, Instant, Result<usize, String>, f64)>();
        let mut joins = Vec::new();
        for (l, h, op, e) in probes.clone() {
            let go = go.clone();
            let rtx = rtx.clone();
            let h = Sendable(h);
            let ready = Arc::new(Barrier::new(2));
            let r2 = ready.clone();
            joins.push(std::thread::spawn(move || {
                let h = h;
                mta();
                let ctx = Ctx::new(new_client(), h.0, elems, &[op]);
                // Baseline before the block.
                let t = Instant::now();
                let _ = ctx.call(op, e);
                let base = t.elapsed().as_secs_f64() * 1e3;
                r2.wait();
                go.wait();
                let r = ctx.call(op, e).map_err(|x| format!("{:#x}", x.code().0));
                rtx.send((l, Instant::now(), r, base)).unwrap();
            }));
            ready.wait();
        }
        // Probes in client processes of their own.
        let mut xprocs = Vec::new();
        for s in a.get("xprobe").cloned().unwrap_or_default() {
            let (l, h, op, e) = spec(&s);
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args(["probe", &format!("hwnd={}", h.0 as isize), &format!("op={}", format!("{op:?}").to_lowercase()), &format!("elem={e}"), &format!("elems={elems}")])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            let mut out = BufReader::new(child.stdout.take().unwrap());
            let mut line = String::new();
            out.read_line(&mut line).unwrap();
            assert!(line.starts_with("ready"), "probe: {line}");
            xprocs.push((format!("xproc-{l}"), child, out));
        }
        let stuck_ctx = stuck.as_ref().map(|(_, h, op, _)| Sendable(Ctx::new(new_client(), *h, elems, &[*op])));
        // Block.
        let mut susp = None;
        let parts: Vec<&str> = block.split(':').collect();
        let t_block = Instant::now();
        match parts[0] {
            "suspend" => unsafe {
                let (tid, _) = thread_of(hwnd_of(parts[1]));
                let h = OpenThread(THREAD_SUSPEND_RESUME, false, tid).unwrap();
                SuspendThread(h);
                susp = Some(Suspended(h, tid));
                println!("suspended tid={tid}");
            },
            "busy" => unsafe {
                PostMessageW(Some(hwnd_of(parts[1])), WM_APP + 1, WPARAM(parts[2].parse().unwrap()), LPARAM(0)).unwrap();
            },
            "slow" => unsafe {
                let kind = if parts[3] == "pump" { 1 } else { 0 };
                SendMessageW(hwnd_of(parts[1]), WM_APP + 4, Some(WPARAM(parts[2].parse().unwrap())), Some(LPARAM(kind)));
            },
            "mock-hold" => {
                let m = mock.as_mut().unwrap();
                m.send("hold");
                m.expect("applied");
            }
            "mock-stall" => {
                let m = mock.as_mut().unwrap();
                m.send(&format!("stall {}", parts[1]));
                m.expect("stall started");
            }
            "mock-slow-parent" => {
                let m = mock.as_mut().unwrap();
                m.send(&format!("slow-parent {} {}", parts[1], parts[2]));
                m.expect("applied");
            }
            _ => {}
        }
        // The stuck call, on a thread of its own; evidence that it is in progress.
        let t_stuck = Instant::now();
        let stuck_join = stuck_ctx.map(|c| {
            let (_, _, op, e) = stuck.clone().unwrap();
            let rtx = rtx.clone();
            std::thread::spawn(move || {
                let c = c;
                mta();
                let r = c.0.call(op, e).map_err(|x| format!("{:#x}", x.code().0));
                rtx.send(("stuck".into(), Instant::now(), r, 0.0)).unwrap();
            })
        });
        let mut evidence = String::from("200 ms wait");
        match (parts[0], mock.as_ref()) {
            ("mock-hold", Some(m)) if stuck_join.is_some() => {
                let (t, l) = m.expect("held");
                evidence = format!("{l} at {:.1} ms", (t - t_stuck).as_secs_f64() * 1e3);
            }
            ("mock-slow-parent", Some(m)) if stuck_join.is_some() => {
                let (t, l) = m.expect("slow-parent started");
                evidence = format!("{} at {:.1} ms", &l[..19], (t - t_stuck).as_secs_f64() * 1e3);
            }
            _ => std::thread::sleep(Duration::from_millis(200)),
        }
        // Probes start together.
        let t_go = Instant::now();
        go.wait();
        // A marker for a debugger script that takes stacks while calls wait.
        println!("PROBES_STARTED trial={trial}");
        std::io::stdout().flush().ok();
        let nx = xprocs.len();
        for (l, mut child, mut out) in xprocs {
            let rtx = rtx.clone();
            let mut stdin = child.stdin.take().unwrap();
            writeln!(stdin, "go").unwrap();
            stdin.flush().unwrap();
            std::thread::spawn(move || {
                let mut line = String::new();
                out.read_line(&mut line).unwrap();
                let t = Instant::now();
                let r = if line.starts_with("done ok") { Ok(0) } else { Err(line.trim().to_string()) };
                rtx.send((l, t, r, f64::NAN)).unwrap();
                let _ = child.wait();
            });
        }
        let mut got: Vec<(String, f64, Result<usize, String>, f64)> = Vec::new();
        let expected = probes.len() + nx + usize::from(stuck_join.is_some());
        let deadline = t_go + observe;
        while got.len() < expected {
            let left = deadline.saturating_duration_since(Instant::now());
            match rrx.recv_timeout(left) {
                Ok((l, t, r, b)) => got.push((l, (t - t_go).as_secs_f64() * 1e3, r, b)),
                Err(_) => break,
            }
        }
        // Unblock.
        let t_unblock = Instant::now();
        drop(susp);
        match parts[0] {
            "mock-hold" => mock.as_mut().unwrap().send("release"),
            "mock-slow-parent" => {
                let m = mock.as_mut().unwrap();
                m.send(&format!("slow-parent {} 0", parts[1]));
            }
            "slow" => unsafe {
                SendMessageW(hwnd_of(parts[1]), WM_APP + 4, Some(WPARAM(0)), Some(LPARAM(0)));
            },
            _ => {}
        }
        let late_deadline = Instant::now() + Duration::from_secs(40);
        let in_window = got.len();
        while got.len() < expected {
            match rrx.recv_timeout(late_deadline.saturating_duration_since(Instant::now())) {
                Ok((l, t, r, b)) => got.push((l, (t - t_go).as_secs_f64() * 1e3, r, b)),
                Err(_) => break,
            }
        }
        let unblock_ms = (t_unblock - t_go).as_secs_f64() * 1e3;
        println!(
            "TRIAL label={label} trial={trial} block={block} evidence=\"{evidence}\" probes_started_ms_after_block={:.1} unblocked_at_ms={unblock_ms:.1}",
            (t_go - t_block).as_secs_f64() * 1e3
        );
        for (i, (l, ms, r, b)) in got.iter().enumerate() {
            let when = if i < in_window { "before_unblock" } else { "after_unblock" };
            let r = match r { Ok(_) => "ok".to_string(), Err(c) => format!("error {c}") };
            println!("RESULT label={label} trial={trial} call={l} returned_ms={ms:.1} {when} result={r} idle_baseline_ms={b:.2}");
        }
        if got.len() < expected {
            println!("RESULT label={label} trial={trial} {} calls still pending 40 s after unblock", expected - got.len());
        }
        for j in joins { let _ = j.join(); }
        if let Some(j) = stuck_join { let _ = j.join(); }
        // Drain mockapp's lines (the release's ack and slow-parent's lines).
        if let Some(m) = mock.as_ref() {
            std::thread::sleep(Duration::from_millis(300));
            while let Ok((_, l)) = m.lines.try_recv() { println!("mockapp: {l}"); }
        }
        // A busy period ends on its own; let it end before the next setup.
        let rest = if parts[0] == "busy" { parts[2].parse().unwrap_or(0) } else { 0 };
        std::thread::sleep(Duration::from_millis(500.max(rest)));
    }
    if let Some(mut m) = mock {
        m.send("quit");
        let _ = m.child.wait();
    }
}

// ---------------------------------------------------------------- clocks and threads

fn qpc() -> i64 {
    let mut v = 0i64;
    let _ = unsafe { windows::Win32::System::Performance::QueryPerformanceCounter(&mut v) };
    v
}

fn qpf() -> f64 {
    let mut v = 0i64;
    let _ = unsafe { windows::Win32::System::Performance::QueryPerformanceFrequency(&mut v) };
    v as f64
}

fn own_cycles() -> u64 {
    let mut c = 0u64;
    let _ = unsafe {
        windows::Win32::System::WindowsProgramming::QueryThreadCycleTime(
            windows::Win32::System::Threading::GetCurrentThread(),
            &mut c,
        )
    };
    c
}

/// The time of day in UTC to the microsecond, as the trace reader takes it.
fn utc_now() -> String {
    let ft = unsafe { windows::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime() };
    let t = ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64;
    let us_of_day = (t / 10) % 86_400_000_000;
    let (h, m, s, us) = (us_of_day / 3_600_000_000, us_of_day / 60_000_000 % 60, us_of_day / 1_000_000 % 60, us_of_day % 1_000_000);
    format!("{h:02}:{m:02}:{s:02}.{us:06}")
}

/// A process's modules: base, size and name.
fn modules(pid: u32) -> Vec<(usize, usize, String)> {
    use windows::Win32::System::Diagnostics::ToolHelp::*;
    let mut out = Vec::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid) else { return out };
        let mut me = MODULEENTRY32W { dwSize: std::mem::size_of::<MODULEENTRY32W>() as u32, ..Default::default() };
        let mut ok = Module32FirstW(snap, &mut me).is_ok();
        while ok {
            let n = me.szModule.iter().position(|c| *c == 0).unwrap_or(me.szModule.len());
            out.push((me.modBaseAddr as usize, me.modBaseSize as usize, String::from_utf16_lossy(&me.szModule[..n])));
            ok = Module32NextW(snap, &mut me).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    out
}

/// Every thread of `pid`: its id, its start address as module+offset, and
/// the processor cycles it has used.
fn thread_table(pid: u32) -> Vec<(u32, String, u64)> {
    use windows::Win32::System::Diagnostics::ToolHelp::*;
    use windows::Win32::System::Threading::THREAD_QUERY_INFORMATION;
    let mods = modules(pid);
    let mut out = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0).unwrap();
        let mut te = THREADENTRY32 { dwSize: std::mem::size_of::<THREADENTRY32>() as u32, ..Default::default() };
        let mut ok = Thread32First(snap, &mut te).is_ok();
        while ok {
            if te.th32OwnerProcessID == pid {
                if let Ok(h) = OpenThread(THREAD_QUERY_INFORMATION, false, te.th32ThreadID) {
                    let mut c = 0u64;
                    let _ = windows::Win32::System::WindowsProgramming::QueryThreadCycleTime(h, &mut c);
                    let mut start: usize = 0;
                    let _ = windows::Wdk::System::Threading::NtQueryInformationThread(
                        h,
                        windows::Wdk::System::Threading::THREADINFOCLASS(9), // ThreadQuerySetWin32StartAddress
                        &mut start as *mut usize as *mut _,
                        std::mem::size_of::<usize>() as u32,
                        std::ptr::null_mut(),
                    );
                    let label = mods
                        .iter()
                        .find(|(b, n, _)| start >= *b && start < b + n)
                        .map_or(format!("{start:#x}"), |(b, _, m)| format!("{m}+{:#x}", start - b));
                    out.push((te.th32ThreadID, label, c));
                    let _ = CloseHandle(h);
                }
            }
            ok = Thread32Next(snap, &mut te).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    out
}

fn median(v: &[f64]) -> f64 {
    let mut c = v.to_vec();
    pct(&mut c, 0.5)
}

// ---------------------------------------------------------------- parts

/// parts hwnd=<h> op=name k=64 threads=1,2,4,8 reps=15 elems=0 [serve_log=<path>]
///   [isolated=<count>]
/// One call split into its parts, on QueryPerformanceCounter, which every
/// process shares: the client's time per call, each thread of the target's
/// processor time per call (cycles converted to time), and with the
/// synthetic provider (serve_log, the file its `log=` names) the provider
/// call's entry and exit, so the time before and after the provider. At N
/// threads, the interval between provider entries is the serial time per
/// call. `isolated` adds that many single calls 20 ms apart with their UTC
/// times, to find in a context-switch trace.
fn parts(a: &Args) {
    let hwnd = hwnd_of(get(a, "hwnd", "0"));
    let (ui_tid, pid) = thread_of(hwnd);
    let op = op_of(get(a, "op", "name"));
    let k = num(a, "k", 64);
    let reps = num(a, "reps", 15);
    let elems = num(a, "elems", 0);
    let label = get(a, "label", "parts").to_string();
    let threads: Vec<usize> = get(a, "threads", "1,2,4,8").split(',').map(|s| s.parse().unwrap()).collect();
    let serve_log = a.get("serve_log").map(|v| v[0].clone());
    mta();
    let f = qpf();
    let (c0, q0) = (own_cycles(), qpc());
    while ((qpc() - q0) as f64) / f < 0.3 {}
    let cyc_per_us = (own_cycles() - c0) as f64 / (((qpc() - q0) as f64) / f * 1e6);
    println!("PARTS label={label} hwnd={} pid={pid} ui_tid={ui_tid} op={op:?} k={k} cycles_per_us={cyc_per_us:.1} qpf={f}", hwnd.0 as isize);
    for (tid, start, _) in thread_table(pid) {
        println!("THREAD tid={tid} start={start}{}", if tid == ui_tid { " ui" } else { "" });
    }
    let us = |ticks: i64| ticks as f64 / f * 1e6;
    let read_log = |clear_only: bool| -> Vec<(i64, i64, u32)> {
        let r = unsafe { SendMessageW(hwnd, WM_APP + 5, Some(WPARAM(usize::from(!clear_only))), None) };
        let _ = r;
        if clear_only {
            return Vec::new();
        }
        let text = std::fs::read_to_string(serve_log.as_ref().unwrap()).unwrap();
        text.lines()
            .map(|l| {
                let p: Vec<&str> = l.split(' ').collect();
                (p[0].parse().unwrap(), p[1].parse().unwrap(), p[2].parse().unwrap())
            })
            .collect()
    };
    for &n in &threads {
        // Workers: each records each call's start and end, and its cycles.
        let (rtx, rrx) = mpsc::channel::<(Vec<(i64, i64)>, u64)>();
        let mut txs = Vec::new();
        for _ in 0..n {
            let (tx, rx) = mpsc::channel::<usize>();
            let rtx = rtx.clone();
            let h = Sendable(hwnd);
            let ready = Arc::new(Barrier::new(2));
            let r2 = ready.clone();
            std::thread::spawn(move || {
                let h = h;
                mta();
                let ctx = Ctx::new(new_client(), h.0, elems, &[op]);
                r2.wait();
                while let Ok(count) = rx.recv() {
                    let mut v = Vec::with_capacity(count);
                    let c = own_cycles();
                    for i in 0..count {
                        let t0 = qpc();
                        let _ = ctx.call(op, i);
                        v.push((t0, qpc()));
                    }
                    rtx.send((v, own_cycles() - c)).unwrap();
                }
            });
            ready.wait();
            txs.push(tx);
        }
        let per = k / n;
        let run = || -> (f64, Vec<(i64, i64)>, u64) {
            let t = qpc();
            for tx in &txs {
                tx.send(per).unwrap();
            }
            let mut calls = Vec::new();
            let mut cyc = 0;
            for _ in 0..n {
                let (v, c) = rrx.recv().unwrap();
                calls.extend(v);
                cyc += c;
            }
            (us(qpc() - t), calls, cyc)
        };
        run();
        let mut rows: Vec<HashMap<&'static str, f64>> = Vec::new();
        let mut thread_us: HashMap<String, Vec<f64>> = HashMap::new();
        for rep in 0..reps {
            if serve_log.is_some() {
                read_log(true);
            }
            let me = std::process::id();
            let own_io = || -> u64 { thread_table(me).iter().filter(|t| t.1.starts_with("uiautomationcore")).map(|t| t.2).sum() };
            let o0 = own_io();
            let s0: HashMap<u32, (String, u64)> = thread_table(pid).into_iter().map(|(t, s, c)| (t, (s, c))).collect();
            let (wall, mut calls, cyc) = run();
            let s1 = thread_table(pid);
            let o1 = own_io();
            let mut m: HashMap<&'static str, f64> = HashMap::new();
            m.insert("wall_us", wall);
            let rt: Vec<f64> = calls.iter().map(|(a, b)| us(b - a)).collect();
            m.insert("call_us", median(&rt));
            m.insert("client_cpu_us", cyc as f64 / cyc_per_us / (per * n) as f64);
            // The client process's own UIA threads, which read the replies.
            m.insert("client_uia_threads_cpu_us", (o1 - o0) as f64 / cyc_per_us / (per * n) as f64);
            let mut line = String::new();
            for (tid, start, c) in &s1 {
                let Some((_, c0)) = s0.get(tid) else { continue };
                let d = (c - c0) as f64 / cyc_per_us / (per * n) as f64;
                if d < 0.05 {
                    continue;
                }
                let name = format!("{tid}:{start}{}", if *tid == ui_tid { ":ui" } else { "" });
                line.push_str(&format!(" cpu[{name}]={d:.1}"));
                thread_us.entry(name).or_default().push(d);
            }
            if serve_log.is_some() {
                let mut log = read_log(false);
                log.sort();
                let prov: Vec<f64> = log.iter().map(|(a, b, _)| us(b - a)).collect();
                m.insert("provider_us", median(&prov));
                let iv: Vec<f64> = log.windows(2).map(|w| us(w[1].0 - w[0].0)).collect();
                m.insert("entry_interval_us", median(&iv));
                if log.len() > 1 {
                    m.insert("entry_interval_mean_us", us(log[log.len() - 1].0 - log[0].0) / (log.len() - 1) as f64);
                }
                let gap: Vec<f64> = log.windows(2).map(|w| us(w[1].0 - w[0].1)).collect();
                m.insert("exit_to_next_entry_us", median(&gap));
                m.insert("provider_on_ui_thread", log.iter().filter(|x| x.2 == ui_tid).count() as f64);
                m.insert("provider_calls", log.len() as f64);
                if n == 1 && log.len() == calls.len() {
                    calls.sort();
                    let before: Vec<f64> = calls.iter().zip(&log).map(|(c, l)| us(l.0 - c.0)).collect();
                    let after: Vec<f64> = calls.iter().zip(&log).map(|(c, l)| us(c.1 - l.1)).collect();
                    m.insert("before_provider_us", median(&before));
                    m.insert("after_provider_us", median(&after));
                }
            }
            let mut keys: Vec<_> = m.keys().copied().collect();
            keys.sort();
            let fields: String = keys.iter().map(|k| format!(" {k}={:.1}", m[k])).collect();
            println!("RAW label={label} n={n} rep={rep}{fields}{line}");
            rows.push(m);
        }
        let mut keys: Vec<_> = rows[0].keys().copied().collect();
        keys.sort();
        let fields: String = keys
            .iter()
            .map(|key| {
                let v: Vec<f64> = rows.iter().filter_map(|r| r.get(key).copied()).collect();
                format!(" {key}={:.1}", median(&v))
            })
            .collect();
        let mut tk: Vec<_> = thread_us.keys().cloned().collect();
        tk.sort();
        let tl: String = tk.iter().map(|t| format!(" cpu[{t}]={:.1}", median(&thread_us[t]))).collect();
        println!("RESULT label={label} n={n} k={}{fields}{tl}", per * n);
        drop(txs);
    }
    let iso = num(a, "isolated", 0);
    if iso > 0 {
        let ctx = Ctx::new(new_client(), hwnd, elems, &[op]);
        if serve_log.is_some() {
            read_log(true);
        }
        let mut v = Vec::new();
        for i in 0..iso {
            std::thread::sleep(Duration::from_millis(20));
            let u0 = utc_now();
            let t0 = qpc();
            let _ = ctx.call(op, i);
            let t1 = qpc();
            let u1 = utc_now();
            v.push((u0, t0, t1, u1));
        }
        let log = if serve_log.is_some() { read_log(false) } else { Vec::new() };
        println!("ISOLATED client_tid={}", unsafe { windows::Win32::System::Threading::GetCurrentThreadId() });
        for (i, (u0, t0, t1, u1)) in v.iter().enumerate() {
            let p = log.get(i).map_or(String::new(), |(e, x, t)| {
                format!(" before_provider_us={:.1} provider_us={:.1} after_provider_us={:.1} provider_tid={t}", us(e - t0), us(x - e), us(t1 - x))
            });
            println!("ISOLATED i={i} utc_start={u0} utc_end={u1} call_us={:.1}{p}", us(t1 - t0));
        }
    }
}

// ---------------------------------------------------------------- abandon

/// abandon label=<l> mode=same|new trials=2
///   block=<busy:<hwnd>:<ms> | slow:<hwnd>:<ms> | mock-hold:<release ms> | suspend:<hwnd>:<release ms> | none>
///   [mockapp=<exe> fixture=<json> title=<t> backend=uia]
///   stuck=<hwnd|main>:<op>:<elem> call=<at ms>:<label>:<hwnd|main>:<op>:<elem> ... tail_ms=3000
/// A stuck call, then later calls at chosen times after the block. With
/// mode=same every call is made on the stuck call's thread, each when the
/// one before it has returned and not before its time; with mode=new the
/// stuck call is left on its thread and each later call is made at its
/// time on a new thread. Every element and client is made before the
/// block; one client serves every call, so only the thread differs.
fn abandon(a: &Args) {
    let label = get(a, "label", "abandon").to_string();
    let same = get(a, "mode", "same") == "same";
    let trials = num(a, "trials", 2);
    let elems = num(a, "elems", 16);
    let tail = Duration::from_millis(num(a, "tail_ms", 3000) as u64);
    let block = get(a, "block", "none").to_string();
    let parts: Vec<String> = block.split(':').map(str::to_string).collect();
    mta();
    let mut mock = a.get("mockapp").map(|v| {
        Mock::start(&v[0], get(a, "fixture", ""), get(a, "title", "puia-mock"), get(a, "backend", "uia"))
    });
    let main_hwnd = mock.as_ref().map(|_| find_top(get(a, "title", "puia-mock")));
    let resolve = |s: &str| -> HWND { if s == "main" { main_hwnd.unwrap() } else { hwnd_of(s) } };
    let stuck_spec: Vec<String> = get(a, "stuck", "").split(':').map(str::to_string).collect();
    let calls: Vec<(u64, String, HWND, Op, usize)> = a
        .get("call")
        .map(|v| {
            v.iter()
                .map(|s| {
                    let p: Vec<&str> = s.split(':').collect();
                    (p[0].parse().unwrap(), p[1].to_string(), resolve(p[2]), op_of(p[3]), p.get(4).map_or(0, |x| x.parse().unwrap()))
                })
                .collect()
        })
        .unwrap_or_default();
    let stuck_h = resolve(&stuck_spec[0]);
    let stuck_op = op_of(&stuck_spec[1]);
    let stuck_e: usize = stuck_spec.get(2).map_or(0, |x| x.parse().unwrap());
    println!(
        "ABANDON label={label} mode={} block={block} stuck={:?}@{} elem {stuck_e}",
        if same { "same" } else { "new" },
        stuck_op,
        stuck_h.0 as isize
    );
    for (at, l, h, op, e) in &calls {
        let (tid, pid) = thread_of(*h);
        println!("CALL {l} at_ms={at} hwnd={} pid={pid} ui_tid={tid} op={op:?} elem={e}", h.0 as isize);
    }
    for trial in 0..trials {
        let client = new_client();
        let stuck_ctx = Sendable(Ctx::new(client.clone(), stuck_h, elems, &[stuck_op]));
        let ctxs: Vec<Sendable<Ctx>> = calls.iter().map(|(_, _, h, op, _)| Sendable(Ctx::new(client.clone(), *h, elems, &[*op]))).collect();
        // Baselines before the block, each on its own context.
        let _ = stuck_ctx.0.call(stuck_op, stuck_e);
        for (c, (_, _, _, op, e)) in ctxs.iter().zip(&calls) {
            let _ = c.0.call(*op, *e);
        }
        let (rtx, rrx) = mpsc::channel::<(String, f64, f64, String)>();
        let (thread_ctxs, mut ctxs): (Option<Vec<Sendable<Ctx>>>, Vec<Option<Sendable<Ctx>>>) =
            if same { (Some(ctxs), Vec::new()) } else { (None, ctxs.into_iter().map(Some).collect()) };
        let mut susp = None;
        let block_utc = utc_now();
        let t_block = Instant::now();
        let ms_since = |t: Instant| (t - t_block).as_secs_f64() * 1e3;
        let mut release_at: Option<f64> = None;
        match parts[0].as_str() {
            "busy" => unsafe {
                PostMessageW(Some(hwnd_of(&parts[1])), WM_APP + 1, WPARAM(parts[2].parse().unwrap()), LPARAM(0)).unwrap();
                release_at = Some(parts[2].parse().unwrap());
            },
            "slow" => unsafe {
                // One-shot: only the stuck call's name read sleeps.
                SendMessageW(hwnd_of(&parts[1]), WM_APP + 4, Some(WPARAM(parts[2].parse().unwrap())), Some(LPARAM(2)));
            },
            "suspend" => unsafe {
                let (tid, _) = thread_of(hwnd_of(&parts[1]));
                let h = OpenThread(THREAD_SUSPEND_RESUME, false, tid).unwrap();
                SuspendThread(h);
                susp = Some(Suspended(h, tid));
            },
            "mock-hold" => {
                let m = mock.as_mut().unwrap();
                m.send("hold");
                m.expect("applied");
            }
            _ => {}
        }
        // The stuck call, and in mode=same the later calls after it on its thread.
        let t_stuck = Instant::now();
        let stuck_join = {
            let rtx = rtx.clone();
            let later: Vec<(u64, String, Op, usize)> = if same {
                calls.iter().map(|(at, l, _, op, e)| (*at, l.clone(), *op, *e)).collect()
            } else {
                Vec::new()
            };
            let mine = thread_ctxs;
            let c = stuck_ctx;
            std::thread::spawn(move || {
                let (c, mine) = (c, mine);
                mta();
                let t0 = Instant::now();
                let r = c.0.call(stuck_op, stuck_e).map(|_| "ok".to_string()).unwrap_or_else(|x| format!("error {:#x}", x.code().0));
                rtx.send(("stuck".into(), (t0 - t_block).as_secs_f64() * 1e3, (Instant::now() - t_block).as_secs_f64() * 1e3, r)).unwrap();
                if let Some(mine) = mine {
                    for ((at, l, op, e), cx) in later.into_iter().zip(mine) {
                        let due = t_block + Duration::from_millis(at);
                        let now = Instant::now();
                        if now < due {
                            std::thread::sleep(due - now);
                        }
                        let t0 = Instant::now();
                        let r = cx.0.call(op, e).map(|_| "ok".to_string()).unwrap_or_else(|x| format!("error {:#x}", x.code().0));
                        rtx.send((l, (t0 - t_block).as_secs_f64() * 1e3, (Instant::now() - t_block).as_secs_f64() * 1e3, r)).unwrap();
                    }
                }
            })
        };
        let mut evidence = String::from("none");
        if parts[0] == "mock-hold" {
            let (t, l) = mock.as_ref().unwrap().expect("held");
            evidence = format!("{l} at {:.1} ms", (t - t_stuck).as_secs_f64() * 1e3);
        }
        if parts[0] == "slow" {
            release_at = Some(ms_since(t_stuck) + parts[2].parse::<f64>().unwrap());
        }
        // mode=new: each later call on a new thread at its time; the
        // release on this thread at its time.
        let mut events: Vec<(f64, usize)> = Vec::new();
        if !same {
            for (i, (at, ..)) in calls.iter().enumerate() {
                events.push((*at as f64, i));
            }
        }
        let release_ms: Option<f64> = match parts[0].as_str() {
            "mock-hold" | "suspend" => Some(parts.last().unwrap().parse().unwrap()),
            _ => None,
        };
        if let Some(r) = release_ms {
            events.push((r, usize::MAX));
            release_at = Some(r);
        }
        events.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap());
        let mut joins = Vec::new();
        for (at, i) in events {
            let due = t_block + Duration::from_secs_f64(at / 1e3);
            let now = Instant::now();
            if now < due {
                std::thread::sleep(due - now);
            }
            if i == usize::MAX {
                if parts[0] == "mock-hold" {
                    mock.as_mut().unwrap().send("release");
                }
                drop(susp.take());
                release_at = Some(ms_since(Instant::now()));
                continue;
            }
            let c = ctxs[i].take().unwrap();
            let (_, l, _, op, e) = calls[i].clone();
            let rtx = rtx.clone();
            joins.push(std::thread::spawn(move || {
                let c = c;
                mta();
                let t0 = Instant::now();
                let r = c.0.call(op, e).map(|_| "ok".to_string()).unwrap_or_else(|x| format!("error {:#x}", x.code().0));
                rtx.send((l, (t0 - t_block).as_secs_f64() * 1e3, (Instant::now() - t_block).as_secs_f64() * 1e3, r)).unwrap();
            }));
        }
        drop(rtx);
        // Every call answers or fails within UIA's own timeouts; wait for all.
        let mut got = Vec::new();
        while let Ok(x) = rrx.recv() {
            got.push(x);
        }
        let _ = stuck_join.join();
        for j in joins {
            let _ = j.join();
        }
        let rel = release_at.unwrap_or(f64::NAN);
        println!("TRIAL label={label} trial={trial} mode={} block={block} block_utc={block_utc} evidence=\"{evidence}\" release_ms={rel:.1}", if same { "same" } else { "new" });
        got.sort_by(|x, y| x.2.partial_cmp(&y.2).unwrap());
        for (l, s, e, r) in &got {
            println!(
                "RESULT label={label} trial={trial} call={l} started_ms={s:.1} returned_ms={e:.1} took_ms={:.1} after_release_ms={:.1} result={r}",
                e - s,
                e - rel
            );
        }
        if let Some(m) = mock.as_ref() {
            std::thread::sleep(Duration::from_millis(300));
            while let Ok((_, l)) = m.lines.try_recv() {
                println!("mockapp: {l}");
            }
        }
        std::thread::sleep(tail);
    }
    if let Some(mut m) = mock {
        m.send("quit");
        let _ = m.child.wait();
    }
}
