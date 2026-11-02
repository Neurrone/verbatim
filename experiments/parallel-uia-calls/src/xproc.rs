//! `xbench`: N concurrent callers as N threads of one client process against
//! N client processes of one thread each, measured the same way (readme,
//! "Threads of one client against client processes").
//!
//! xbench hwnd=<h> op=<op> k=<calls> reps=<r> ns=2,4,8 elems=<n> label=<l>
//! - Every run is K calls split evenly over the callers; its time is from
//!   the parent's first `go` to the last worker's `done`, so both forms pay
//!   the same hand-off. N = 1 is one process with one thread.
//! - Each worker also reports the processor cycles its own UIA I/O thread
//!   (the one starting at `OverlappedIOManager::StaticIoThreadProc`) used
//!   in the run.
//!
//! worker hwnd=<h> op=<op> threads=<t> elems=<n>: one client process with t
//! calling threads, each with its own client and elements; `go <k>` splits
//! k calls over them and answers `done <ms> errs=<e> io_cycles=<c>`.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Instant;

use crate::{Args, Ctx, get, hwnd_of, mta, new_client, num, op_of, pct, thread_table};

/// UIA's I/O thread starts here on this build (`threads` in the readme).
const IO_THREAD_START: &str = "+0x162f60";

fn io_cycles() -> u64 {
    thread_table(std::process::id())
        .iter()
        .filter(|(_, l, _)| l.to_lowercase().starts_with("uiautomationcore") && l.ends_with(IO_THREAD_START))
        .map(|(_, _, c)| *c)
        .sum()
}

struct Send2<T>(T);
unsafe impl<T> Send for Send2<T> {}

pub fn worker(a: &Args) {
    mta();
    let op = op_of(get(a, "op", "name"));
    let n = num(a, "threads", 1);
    let elems = num(a, "elems", 16);
    let hwnd = Send2(hwnd_of(get(a, "hwnd", "0")));
    let (rtx, rrx) = mpsc::channel::<usize>();
    let mut txs = Vec::new();
    for w in 0..n {
        let (tx, rx) = mpsc::channel::<(usize, usize)>();
        let rtx = rtx.clone();
        let h = Send2(hwnd.0);
        let (ready_tx, ready_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let h = h;
            mta();
            let ctx = Ctx::new(new_client(), h.0, elems, &[op]);
            let _ = ctx.call(op, w);
            ready_tx.send(()).unwrap();
            while let Ok((count, offset)) = rx.recv() {
                let mut errs = 0;
                for i in 0..count {
                    if ctx.call(op, offset + i).is_err() {
                        errs += 1;
                    }
                }
                rtx.send(errs).unwrap();
            }
        });
        ready_rx.recv().unwrap();
        txs.push(tx);
    }
    println!("ready");
    std::io::stdout().flush().ok();
    let stdin = std::io::stdin();
    for l in stdin.lock().lines().map_while(Result::ok) {
        let Some(k) = l.trim_start_matches('\u{feff}').strip_prefix("go ").and_then(|s| s.trim().parse::<usize>().ok()) else { continue };
        let before = io_cycles();
        let t = Instant::now();
        let per = k / n;
        for (w, tx) in txs.iter().enumerate() {
            tx.send((per, w * per)).unwrap();
        }
        let errs: usize = (0..n).map(|_| rrx.recv().unwrap()).sum();
        let ms = t.elapsed().as_secs_f64() * 1e3;
        println!("done {ms:.3} errs={errs} io_cycles={}", io_cycles() - before);
        std::io::stdout().flush().ok();
    }
}

struct Worker {
    child: std::process::Child,
    out: BufReader<std::process::ChildStdout>,
}

fn spawn(a: &Args, threads: usize) -> Worker {
    let exe = std::env::current_exe().unwrap();
    let mut child = Command::new(exe)
        .args([
            "worker".to_string(),
            format!("hwnd={}", get(a, "hwnd", "0")),
            format!("op={}", get(a, "op", "name")),
            format!("threads={threads}"),
            format!("elems={}", get(a, "elems", "16")),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut l = String::new();
    out.read_line(&mut l).unwrap();
    assert!(l.starts_with("ready"), "worker: {l}");
    Worker { child, out }
}

/// One run: `go` to every worker, then every `done`. Returns the wall
/// time, the errors and the I/O threads' cycles.
fn run(ws: &mut [Worker], each: usize) -> (f64, usize, u64) {
    let t = Instant::now();
    for w in ws.iter_mut() {
        let i = w.child.stdin.as_mut().unwrap();
        writeln!(i, "go {each}").unwrap();
        i.flush().unwrap();
    }
    let (mut errs, mut cyc) = (0, 0);
    for w in ws.iter_mut() {
        let mut l = String::new();
        w.out.read_line(&mut l).unwrap();
        let f: Vec<&str> = l.split_whitespace().collect();
        errs += f[2].trim_start_matches("errs=").parse::<usize>().unwrap();
        cyc += f[3].trim_start_matches("io_cycles=").parse::<u64>().unwrap();
    }
    (t.elapsed().as_secs_f64() * 1e3, errs, cyc)
}

pub fn xbench(a: &Args) {
    let k = num(a, "k", 256);
    let reps = num(a, "reps", 15);
    let label = get(a, "label", "x");
    let ns: Vec<usize> = get(a, "ns", "2,4,8").split(',').map(|s| s.parse().unwrap()).collect();
    let mut forms: Vec<(String, usize, usize)> = vec![("one".into(), 1, 1)];
    for &n in &ns {
        forms.push((format!("threads{n}"), 1, n));
        forms.push((format!("procs{n}"), n, 1));
    }
    let mut base = 0.0;
    for (name, procs, threads) in forms {
        let mut ws: Vec<Worker> = (0..procs).map(|_| spawn(a, threads)).collect();
        let each = k / procs;
        let _ = run(&mut ws, each); // warm-up
        let mut walls = Vec::new();
        let (mut errs, mut cyc) = (0, 0u64);
        for _ in 0..reps {
            let (ms, e, c) = run(&mut ws, each);
            walls.push(ms);
            errs += e;
            cyc += c;
        }
        let med = pct(&mut walls.clone(), 0.5);
        let p90 = pct(&mut walls, 0.9);
        if name == "one" {
            base = med;
        }
        println!(
            "XBENCH label={label} op={} form={name} processes={procs} threads_each={threads} k={k} reps={reps} median_ms={med:.3} p90_ms={p90:.3} per_call_us={:.1} speedup={:.2} errors={errs} client_io_cycles_per_call={:.0}",
            get(a, "op", "name"),
            med * 1e3 / k as f64,
            base / med,
            cyc as f64 / (reps * k) as f64
        );
        std::io::stdout().flush().ok();
        for mut w in ws {
            drop(w.child.stdin.take());
            let _ = w.child.wait();
        }
    }
}
