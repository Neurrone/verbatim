//! `e38-tooltip`, ported from the collide tool of `collide-tool-e38.patch`
//! (in git at fdfb63ae, applied onto f9fb32f7) step for step: its client
//! process, its orchestrator's `fgright` and `console` steps, and its trial
//! loop. The script it runs:
//!
//! 1. T, a client: `init` (a `CUIAutomation8` client object as
//!    `IUIAutomation`, no setting changed), `root`, `events` (one
//!    `ToolTipOpened` handler over the desktop root's subtree, with no
//!    cache request), from its main thread in the multithreaded apartment.
//! 2. U and R, clients: `init`.
//! 3. `fgright`: a zero mouse move injected, 30 ms, and
//!    `AllowSetForegroundWindow(ASFW_ANY)`, so that the console may take
//!    the foreground.
//! 4. `console`: `conhost.exe cmd.exe /d /k`, titled by `lpTitle`, created
//!    suspended, put in a job that kills it on close, resumed; then a spin
//!    on `FindWindowW` for its `ConsoleWindowClass` window and title.
//! 5. At once: R `target`, U `target`, R `rawprobe`, U `probe`, R
//!    `efhbcd` and U `efhbcd` (`ElementFromHandleBuildCache` with the
//!    name, process id and provider description cached), written in that
//!    order; every answer awaited, then 1.5 s.
//! 6. The console closed, and 0.5 s before the next trial.

use std::ffi::c_void;
use std::io::BufRead;
use std::time::Duration;

use uia_facts_support::role::{self, Role};
use uia_facts_support::{exit_now, now_us};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject,
};
use windows::Win32::System::Threading::{
    CREATE_SUSPENDED, CreateProcessW, PROCESS_INFORMATION, ResumeThread, STARTUPINFOW,
    WaitForSingleObject,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation8, IUIAutomation, IUIAutomationElement, IUIAutomationEventHandler,
    TreeScope_Subtree, UIA_NamePropertyId, UIA_ProcessIdPropertyId,
    UIA_ProviderDescriptionPropertyId, UIA_ToolTipOpenedEventId, UiaHasServerSideProvider,
    UiaRootObjectId,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_MOVE, MOUSEINPUT, SendInput,
};
use windows::Win32::UI::WindowsAndMessaging::{
    ASFW_ANY, AllowSetForegroundWindow, FindWindowW, SMTO_ABORTIFHUNG, SendMessageTimeoutW,
    WM_GETOBJECT,
};
use windows::core::{PCWSTR, PWSTR, w};

/// The role name of the ported client.
pub const ROLE: &str = "e38-client";

/// The ported client: commands on its standard input, each answered with
/// `done <command> <detail>`, as the collide client answered them.
#[expect(
    clippy::too_many_lines,
    reason = "one arm per command, as the collide client had"
)]
pub fn client_role() -> ! {
    let mut uia: Option<IUIAutomation> = None;
    let mut element: Option<IUIAutomationElement> = None;
    let mut hwnd = HWND(std::ptr::null_mut());
    let mut handlers: Vec<IUIAutomationEventHandler> = Vec::new();
    role::report("ready");
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim().to_owned();
        if let Some(target) = line.strip_prefix("target ") {
            hwnd = HWND(target.parse::<isize>().expect("a window") as *mut c_void);
            element = None;
            role::report("done target");
            continue;
        }
        let start = now_us();
        let detail = match line.as_str() {
            // SAFETY: an in-process class.
            "init" => match unsafe {
                CoCreateInstance::<_, IUIAutomation>(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)
            } {
                Ok(made) => {
                    uia = Some(made);
                    "ok".to_owned()
                }
                Err(e) => format!("err {e:?}"),
            },
            "root" => {
                let client = uia.as_ref().expect("init first");
                // SAFETY: a live client.
                match unsafe { client.GetRootElement() } {
                    Ok(root) => {
                        element = Some(root);
                        "ok".to_owned()
                    }
                    Err(e) => format!("err {:#x}", e.code().0),
                }
            }
            "events" => {
                let client = uia.as_ref().expect("init first");
                let root = element.as_ref().expect("root first");
                let (handler, _reports) = uia_facts_support::events::Handler::make();
                // SAFETY: live client, element and handler; no cache
                // request, as the original passed none.
                match unsafe {
                    client.AddAutomationEventHandler(
                        UIA_ToolTipOpenedEventId,
                        root,
                        TreeScope_Subtree,
                        None,
                        &handler,
                    )
                } {
                    Ok(()) => {
                        handlers.push(handler);
                        "ok".to_owned()
                    }
                    Err(e) => format!("err {:#x}", e.code().0),
                }
            }
            "rawprobe" => {
                let mut answer = 0usize;
                // SAFETY: a plain message with a live out value.
                let sent = unsafe {
                    SendMessageTimeoutW(
                        hwnd,
                        WM_GETOBJECT,
                        WPARAM(0),
                        LPARAM(UiaRootObjectId as isize),
                        SMTO_ABORTIFHUNG,
                        3000,
                        Some(&raw mut answer),
                    )
                };
                format!("sent={} lresult={answer:#x}", sent.0)
            }
            "efhbcd" => {
                let client = uia.as_ref().expect("init first");
                let cached = (|| {
                    // SAFETY: a live client.
                    let request = unsafe { client.CreateCacheRequest() }?;
                    for property in [
                        UIA_NamePropertyId,
                        UIA_ProcessIdPropertyId,
                        UIA_ProviderDescriptionPropertyId,
                    ] {
                        // SAFETY: a live cache request.
                        unsafe { request.AddProperty(property) }?;
                    }
                    // SAFETY: a live client and cache request; any window.
                    unsafe { client.ElementFromHandleBuildCache(hwnd, &request) }
                })();
                match cached {
                    Ok(found) => {
                        element = Some(found);
                        "ok".to_owned()
                    }
                    Err(e) => format!("err {:#x}", e.code().0),
                }
            }
            "probe" => {
                // SAFETY: any window handle is tolerated.
                let yes = unsafe { UiaHasServerSideProvider(hwnd) }.as_bool();
                format!("result={yes}")
            }
            other => format!("unknown {other}"),
        };
        let ms = uia_facts_support::stats::us_to_ms(now_us() - start);
        role::report(&format!("done {line} ms={ms:.1} {detail}"));
    }
    drop((handlers, element, uia));
    exit_now(0)
}

/// Starts a ported client and waits for its readiness.
fn client() -> Role {
    let role = Role::start(ROLE, &[]);
    assert_eq!(role.expect_line(), "ready", "the client is ready");
    role
}

/// Sends `command` and waits for its `done` line; answers the line.
fn call(role: &mut Role, command: &str) -> String {
    role.send(command);
    let line = role
        .line(Duration::from_secs(30))
        .unwrap_or_else(|| panic!("the client answers {command}"));
    assert!(line.starts_with("done "), "the client answered {line}");
    line
}

/// The orchestrator's `fgright` step.
fn fgright() {
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                dwFlags: MOUSEEVENTF_MOVE,
                ..Default::default()
            },
        },
    };
    let size = i32::try_from(std::mem::size_of::<INPUT>()).expect("a small structure");
    // SAFETY: one well-formed input.
    let _ = unsafe { SendInput(&[input], size) };
    std::thread::sleep(Duration::from_millis(30));
    // SAFETY: a plain call.
    let _ = unsafe { AllowSetForegroundWindow(ASFW_ANY) };
}

/// A console started as the orchestrator's `console` step started it.
struct Console {
    job: HANDLE,
    process: HANDLE,
    hwnd: isize,
}

impl Console {
    fn start(title: &str) -> Self {
        let mut title: Vec<u16> = title.encode_utf16().chain(Some(0)).collect();
        let mut command: Vec<u16> = "conhost.exe cmd.exe /d /k"
            .encode_utf16()
            .chain(Some(0))
            .collect();
        // SAFETY: a new job with no name.
        let job = unsafe { CreateJobObjectW(None, PCWSTR::null()) }.expect("a job");
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: the job made above and a live structure of the size given.
        unsafe {
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                u32::try_from(std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
                    .expect("a small structure"),
            )
        }
        .expect("the job's limits");
        let startup = STARTUPINFOW {
            cb: u32::try_from(std::mem::size_of::<STARTUPINFOW>()).expect("a small structure"),
            lpTitle: PWSTR(title.as_mut_ptr()),
            ..Default::default()
        };
        let mut info = PROCESS_INFORMATION::default();
        // SAFETY: writable, terminated buffers that outlive the call.
        unsafe {
            CreateProcessW(
                PCWSTR::null(),
                Some(PWSTR(command.as_mut_ptr())),
                None,
                None,
                false,
                CREATE_SUSPENDED,
                None,
                PCWSTR::null(),
                &raw const startup,
                &raw mut info,
            )
        }
        .expect("conhost starts");
        // SAFETY: the job and the process made above.
        unsafe { AssignProcessToJobObject(job, info.hProcess) }.expect("the job holds it");
        let launched = now_us();
        // SAFETY: the suspended thread just made.
        let _ = unsafe { ResumeThread(info.hThread) };
        let hwnd = loop {
            // SAFETY: a valid class and a live, terminated title.
            let found = unsafe { FindWindowW(w!("ConsoleWindowClass"), PCWSTR(title.as_ptr())) };
            if let Ok(hwnd) = found {
                break hwnd.0 as isize;
            }
            assert!(now_us() - launched < 10_000_000, "no console window");
        };
        // SAFETY: the thread handle is not used again.
        let _ = unsafe { CloseHandle(info.hThread) };
        Self {
            job,
            process: info.hProcess,
            hwnd,
        }
    }

    /// The orchestrator's `closeconsole`.
    fn close(self) {
        // SAFETY: handles made by `start`, each closed once.
        let _ = unsafe { CloseHandle(self.job) };
        // SAFETY: as above.
        let _ = unsafe { WaitForSingleObject(self.process, 5000) };
        // SAFETY: as above.
        let _ = unsafe { CloseHandle(self.process) };
    }
}

/// One trial; answers U's probe line.
fn trial(n: usize) -> (String, String) {
    let mut t = client();
    call(&mut t, "init");
    call(&mut t, "root");
    let events = call(&mut t, "events");
    assert!(events.ends_with(" ok"), "T's handler registers: {events}");
    let mut u = client();
    call(&mut u, "init");
    let mut r = client();
    call(&mut r, "init");
    fgright();
    let console = Console::start(&format!("e38-tooltip-{n}-{}", std::process::id()));
    let target = format!("target {}", console.hwnd);
    r.send(&target);
    u.send(&target);
    r.send("rawprobe");
    u.send("probe");
    r.send("efhbcd");
    u.send("efhbcd");
    let mut probe = String::new();
    let mut raw = String::new();
    while probe.is_empty() {
        let line = u
            .line(Duration::from_secs(20))
            .expect("U answers its probe");
        if line.starts_with("done probe") {
            probe = line;
        }
    }
    while raw.is_empty() {
        let line = r.line(Duration::from_secs(20)).expect("R answers");
        if line.starts_with("done rawprobe") {
            raw = line;
        }
    }
    // `wait U done efhbcd 20000`, then `drain 1500`, as the script ends.
    for client in [&u, &r] {
        loop {
            let line = client
                .line(Duration::from_secs(20))
                .expect("the lookup answers");
            if line.starts_with("done efhbcd") {
                break;
            }
        }
    }
    std::thread::sleep(Duration::from_millis(1500));
    console.close();
    drop((t, u, r));
    std::thread::sleep(Duration::from_millis(500));
    (probe, raw)
}

/// `n` trials of the ported `e38-tooltip`: each trial's U probe line and
/// R raw probe line.
pub fn run(n: usize) -> Vec<(String, String)> {
    (0..n).map(trial).collect()
}
