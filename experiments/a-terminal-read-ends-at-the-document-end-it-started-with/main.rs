//! Footer omission probe.
//!
//! Usage: footer-omission-probe <conhost|wt> <region|region-slow|plain> <out.tsv> <reads per phase>
//!
//! Opens a terminal running a PowerShell script that draws a footer,
//! "status: busy", on the screen's bottom row with the caret on the row
//! above it, waits for a go file, then floods until a stop file, redrawing
//! the footer every 20 lines, and finally writes "status: done". With
//! `region` and `region-slow` the flood scrolls inside a scroll region over
//! the rows above the footer (crates/verbatim-e2e's footer script);
//! `region-slow` sleeps 30 ms after each 20 lines. With `plain` there is no
//! scroll region: each flood line is written at its own row by moving the
//! cursor there, cycling through the rows above the footer, so nothing
//! scrolls and the caret is usually above other rows with text.
//!
//! It takes the reads of Verbatim's read program, call by call: the
//! document range, the first visible range's text, the text from two
//! screens above the document's end to the document's end in one
//! `GetText`, and the caret's rows to the document's end. Then, for the
//! measurement only, a second document range, compared with the first, and
//! when its end moved the snapshot again to the new end; and the console
//! cursor and window before and after. Three phases of reads: before the
//! flood, during it, and after "status: done" is written.

use std::os::windows::process::CommandExt;
use std::sync::atomic::{AtomicI64, AtomicIsize, AtomicU64, Ordering};
use std::time::Duration;

use windows::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, HWND};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};
use windows::Win32::System::Console::{
    AttachConsole, CONSOLE_SCREEN_BUFFER_INFO, FreeConsole, GetConsoleScreenBufferInfo,
};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    CUIAutomation, HWINEVENTHOOK, IUIAutomation, IUIAutomationTextPattern, IUIAutomationTextRange,
    SetWinEventHook, TextPatternRangeEndpoint_End, TextPatternRangeEndpoint_Start, TextUnit_Line,
    TreeScope_Subtree, UIA_ClassNamePropertyId, UIA_IsTextPatternAvailablePropertyId, UIA_TextPatternId,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, FindWindowW, GetMessageW, MSG, WINEVENT_OUTOFCONTEXT,
};
use windows_core::{HSTRING, PCWSTR, w};

const SCRIPT: &str = r#"param($dir, $mode, $title)
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public static class Vt {
    [DllImport("kernel32.dll")] static extern IntPtr GetStdHandle(int n);
    [DllImport("kernel32.dll")] static extern bool GetConsoleMode(IntPtr h, out int m);
    [DllImport("kernel32.dll")] static extern bool SetConsoleMode(IntPtr h, int m);
    public static void Enable() { var h = GetStdHandle(-11); int m; GetConsoleMode(h, out m); SetConsoleMode(h, m | 4); }
}
"@
[Vt]::Enable()
$Host.UI.RawUI.WindowTitle = $title
$e = [char]27
$rows = [Console]::WindowHeight
if ($mode -eq 'plain') {
    [Console]::Write("$e[2J$e[$rows;1Hstatus: busy$e[$($rows - 1);1H")
} else {
    [Console]::Write("$e[2J$e[1;$($rows - 1)r$e[$rows;1Hstatus: busy$e[$($rows - 1);1H")
}
Set-Content -Path "$dir\pid" -Value $PID
while (-not (Test-Path "$dir\go")) { Start-Sleep -Milliseconds 10 }
$n = 0
while (-not (Test-Path "$dir\stop")) {
    for ($k = 0; $k -lt 20; $k++) {
        $n++
        if ($mode -eq 'plain') {
            $r = (($n - 1) % ($rows - 1)) + 1
            [Console]::Write("$e[$r;1Hflood line $n$e[K")
        } else {
            [Console]::Write("`nflood line $n")
        }
    }
    [Console]::Write("$e[s$e[$rows;1Hstatus: busy$e[u")
    if ($mode -eq 'region-slow') { Start-Sleep -Milliseconds 30 }
}
[Console]::Write("$e[s$e[$rows;1Hstatus: done$e[u")
Set-Content -Path "$dir\done" -Value $n
while (-not (Test-Path "$dir\quit")) { Start-Sleep -Milliseconds 20 }
[Console]::Write("$e[r")
"#;

static TARGET: AtomicIsize = AtomicIsize::new(0);
static LAST_EVENT: AtomicI64 = AtomicI64::new(0);
static EVENTS: AtomicU64 = AtomicU64::new(0);

unsafe extern "system" fn on_event(
    _hook: HWINEVENTHOOK,
    _event: u32,
    hwnd: HWND,
    _object: i32,
    _child: i32,
    _thread: u32,
    _time: u32,
) {
    if hwnd.0 as isize == TARGET.load(Ordering::Relaxed) {
        LAST_EVENT.store(now(), Ordering::Relaxed);
        EVENTS.fetch_add(1, Ordering::Relaxed);
    }
}

fn now() -> i64 {
    let mut t = 0;
    unsafe { QueryPerformanceCounter(&raw mut t) }.unwrap();
    t
}

fn freq() -> f64 {
    let mut f = 0;
    unsafe { QueryPerformanceFrequency(&raw mut f) }.unwrap();
    f as f64
}

fn text(range: &IUIAutomationTextRange) -> String {
    unsafe { range.GetText(-1) }.map(|b| b.to_string()).unwrap_or_default()
}

fn rows(text: &str) -> Vec<String> {
    let mut rows: Vec<String> = text
        .split('\n')
        .map(|row| row.trim_end_matches('\r').trim_end().to_owned())
        .collect();
    if text.ends_with('\n') {
        rows.pop();
    }
    rows
}

fn clean(s: &str) -> String {
    s.replace('\t', " ")
}

struct Read {
    start: i64,
    end: i64,
    snapshot: String,
    screen: String,
    caret_rows_to_end: i32,
    caret_line: String,
    end_moved: i32,
    retry: Option<(String, i32)>,
}

fn read(pattern: &IUIAutomationTextPattern, screen_rows: i32) -> windows_core::Result<Read> {
    let start = now();
    unsafe {
        // Verbatim's read: the document, the screen, the snapshot's one
        // GetText from a screen above the screen's top, and the caret.
        let document = pattern.DocumentRange()?;
        let visible = pattern.GetVisibleRanges()?;
        let screen_range = if visible.Length()? > 0 { visible.GetElement(0)? } else { document.Clone()? };
        let screen = text(&screen_range);
        let begin = document.Clone()?;
        begin.MoveEndpointByRange(TextPatternRangeEndpoint_Start, &document, TextPatternRangeEndpoint_End)?;
        begin.Move(TextUnit_Line, -(2 * screen_rows))?;
        begin.MoveEndpointByRange(TextPatternRangeEndpoint_End, &document, TextPatternRangeEndpoint_End)?;
        let snapshot = text(&begin);
        // The measurement's own calls: whether the document's end moved
        // between the DocumentRange call and the snapshot's GetText, by a
        // second DocumentRange taken at once; and when it did, the same
        // snapshot again to the new end, with a third DocumentRange after it.
        let document2 = pattern.DocumentRange()?;
        let end_moved =
            document2.CompareEndpoints(TextPatternRangeEndpoint_End, &document, TextPatternRangeEndpoint_End)?;
        let retry = if end_moved == 0 {
            None
        } else {
            begin.MoveEndpointByRange(TextPatternRangeEndpoint_End, &document2, TextPatternRangeEndpoint_End)?;
            let again = text(&begin);
            let document3 = pattern.DocumentRange()?;
            let moved = document3.CompareEndpoints(TextPatternRangeEndpoint_End, &document2, TextPatternRangeEndpoint_End)?;
            Some((again, moved))
        };
        let (caret_rows_to_end, caret_line) = match pattern.GetSelection()?.Length()? {
            0 => (-1, String::new()),
            _ => {
                let caret = pattern.GetSelection()?.GetElement(0)?;
                let at = caret.Clone()?;
                at.MoveEndpointByRange(TextPatternRangeEndpoint_End, &caret, TextPatternRangeEndpoint_Start)?;
                let line = at.Clone()?;
                line.ExpandToEnclosingUnit(TextUnit_Line)?;
                let line = text(&line);
                (at.Move(TextUnit_Line, 1_000_000)?, line)
            }
        };
        let end = now();
        Ok(Read { start, end, snapshot, screen, caret_rows_to_end, caret_line, end_moved, retry })
    }
}

fn flood_n(screen_rows: &[String]) -> u64 {
    screen_rows
        .iter()
        .filter_map(|r| r.strip_prefix("flood line ").and_then(|n| n.trim().parse().ok()))
        .max()
        .unwrap_or(0)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (terminal, mode, out, per_phase) =
        (args[1].clone(), args[2].clone(), args[3].clone(), args[4].parse::<usize>().unwrap());
    let dir = std::env::temp_dir().join(format!("footer-probe-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("footer.ps1");
    std::fs::write(&script, SCRIPT).unwrap();
    let title = format!("footer-probe-{}", std::process::id());
    let ps = format!(
        "powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File \"{}\" \"{}\" {mode} {title}",
        script.display(),
        dir.display()
    );
    let mut child = if terminal == "wt" {
        std::process::Command::new("cmd.exe")
            .raw_arg(format!("/c wt.exe -w new {ps}"))
            .spawn()
    } else {
        std::process::Command::new("conhost.exe")
            .raw_arg(&ps)
            .creation_flags(0x10)
            .spawn()
    }
    .unwrap();
    let _ = &mut child;

    let mut hwnd = HWND::default();
    for _ in 0..200 {
        if let Ok(h) = unsafe { FindWindowW(PCWSTR::null(), &HSTRING::from(title.as_str())) } {
            if !h.is_invalid() {
                hwnd = h;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!hwnd.is_invalid(), "no window titled {title}");
    let mut pid = 0u32;
    for _ in 0..200 {
        if let Ok(s) = std::fs::read_to_string(dir.join("pid")) {
            if let Ok(p) = s.trim().parse() {
                pid = p;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(pid != 0, "no pid");
    TARGET.store(hwnd.0 as isize, Ordering::Relaxed);

    std::thread::spawn(|| unsafe {
        let _hook = SetWinEventHook(0x4002, 0x4004, None, Some(on_event), 0, 0, WINEVENT_OUTOFCONTEXT);
        let mut msg = MSG::default();
        while GetMessageW(&raw mut msg, None, 0, 0).as_bool() {
            DispatchMessageW(&raw const msg);
        }
    });

    unsafe {
        let _ = FreeConsole();
        AttachConsole(pid).expect("attach");
    }
    let conout = unsafe {
        CreateFileW(
            w!("CONOUT$"),
            (GENERIC_READ | GENERIC_WRITE).0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )
    }
    .expect("CONOUT$");

    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok().unwrap();
    let uia: IUIAutomation = unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }.unwrap();
    let root = unsafe { uia.ElementFromHandle(hwnd) }.unwrap();
    // Windows Terminal's tab titles have the text pattern too; its terminal
    // is the TermControl.
    let condition = if terminal == "wt" {
        unsafe { uia.CreatePropertyCondition(UIA_ClassNamePropertyId, &VARIANT::from("TermControl")) }.unwrap()
    } else {
        unsafe { uia.CreatePropertyCondition(UIA_IsTextPatternAvailablePropertyId, &VARIANT::from(true)) }.unwrap()
    };
    let element = unsafe { root.FindFirst(TreeScope_Subtree, &condition) }.unwrap();
    let pattern: IUIAutomationTextPattern = unsafe { element.GetCurrentPatternAs(UIA_TextPatternId) }.unwrap();
    std::thread::sleep(Duration::from_millis(500));

    let f = freq();
    let t0 = now();
    let ms = |t: i64| (t - t0) as f64 * 1000.0 / f;
    let mut out_text = format!(
        "# terminal={terminal} mode={mode} window={:#x} shell_pid={pid}\n\
         phase\ti\tt_ms\tread_ms\tsince_event_ms\tevents_during\tflood_n\tscreen_rows\tsnapshot_rows\t\
         caret_rows_to_end\tend_moved\tcut\tretry_cut\tretry_moved\tscreen_footer\twindow_moved\tcursor_row\twindow_height\t\
         doc_last\tcaret_line\tscreen_last\n",
        hwnd.0 as isize
    );
    let mut screen_rows = 0i32;
    for phase in ["before", "flood", "after"] {
        if phase == "flood" {
            std::fs::write(dir.join("go"), "").unwrap();
            std::thread::sleep(Duration::from_millis(100));
        }
        if phase == "after" {
            std::fs::write(dir.join("stop"), "").unwrap();
            while !dir.join("done").exists() {
                std::thread::sleep(Duration::from_millis(10));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        for i in 0..per_phase {
            let events_before = EVENTS.load(Ordering::Relaxed);
            let last = LAST_EVENT.load(Ordering::Relaxed);
            let mut before = CONSOLE_SCREEN_BUFFER_INFO::default();
            let _ = unsafe { GetConsoleScreenBufferInfo(conout, &raw mut before) };
            let Ok(r) = read(&pattern, screen_rows.max(1)) else {
                out_text.push_str(&format!("{phase}\t{i}\terror\n"));
                continue;
            };
            let events_during = EVENTS.load(Ordering::Relaxed) - events_before;
            let mut info = CONSOLE_SCREEN_BUFFER_INFO::default();
            let _ = unsafe { GetConsoleScreenBufferInfo(conout, &raw mut info) };
            let screen = rows(&r.screen);
            if std::env::var_os("DUMP").is_some() && i < 4 {
                use std::io::Write as _;
                let mut d = std::fs::OpenOptions::new().create(true).append(true).open(format!("{out}.dump")).unwrap();
                let _ = writeln!(d, "=== {phase} {i}\n--- screen\n{}\n--- snapshot\n{}\n--- retry\n{:?}", r.screen, r.snapshot, r.retry);
            }
            screen_rows = i32::try_from(screen.len()).unwrap();
            let snapshot = rows(&r.snapshot);
            let ends_footer = |rows: &[String]| rows.iter().rev().find(|s| !s.is_empty()).is_some_and(|s| s.starts_with("status: "));
            let cut = !ends_footer(&snapshot);
            let screen_footer = screen.last().is_some_and(|s| s.starts_with("status: "));
            let (retry_cut, retry_moved) = match &r.retry {
                Some((t, m)) => (i32::from(!ends_footer(&rows(t))), *m),
                None => (-1, 0),
            };
            let moved_window = info.srWindow.Top - before.srWindow.Top;
            let since = if last == 0 { -1.0 } else { (r.start - last) as f64 * 1000.0 / f };
            out_text.push_str(&format!(
                "{phase}\t{i}\t{:.3}\t{:.3}\t{:.3}\t{events_during}\t{}\t{}\t{}\t{}\t{}\t{}\t{retry_cut}\t{retry_moved}\t{}\t{moved_window}\t{}\t{}\t{}\t{}\t{}\n",
                ms(r.start),
                (r.end - r.start) as f64 * 1000.0 / f,
                since,
                flood_n(&screen),
                screen.len(),
                snapshot.len(),
                r.caret_rows_to_end,
                r.end_moved,
                u8::from(cut),
                u8::from(screen_footer),
                info.dwCursorPosition.Y - info.srWindow.Top,
                info.srWindow.Bottom - info.srWindow.Top + 1,
                clean(snapshot.last().map_or("", String::as_str)),
                clean(r.caret_line.trim_end()),
                clean(screen.iter().rev().find(|s| !s.is_empty()).map_or("", String::as_str)),
            ));
        }
    }
    std::fs::write(dir.join("quit"), "").unwrap();
    std::fs::write(&out, out_text).unwrap();
    std::thread::sleep(Duration::from_millis(500));
    let _ = std::fs::remove_dir_all(&dir);
}
