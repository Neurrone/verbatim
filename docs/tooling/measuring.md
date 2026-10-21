# Measuring

How to measure Verbatim's latency, wake-ups and processor time on this machine, and the platform's floor for crossing processes.

## Measuring performance

Verbatim is judged on latency, wake-ups and processor time together
(`docs/design/communication.md`, section 18; `docs/design/wake-ups.md`).
Two commands measure them on this machine against a locally running
agent, which each starts and ends itself. Both take over the desktop as
every end-to-end run does, so run them on the console (`cargo xtask
park`) or when the desktop is free, never during a build, and never with
another run going. Neither builds Verbatim: build what the run stages
first (the debug binaries, or the release ones for `--release`, as "The
measure set" in [the end-to-end guide](e2e.md) says). Both write under `target\perf\<label>`, which
each run replaces when the label is the same.

Every measurement reported for Verbatim gives the same figures, so that
runs compare (Dickson, 2026-10-13): latency per stage and end to end as
the minimum, P50, P95, P99, and maximum observed, never other
percentiles; the latency of each cross-process hop on its own (outpost to
verbatim.exe, listener to relay, verbatim.exe to an outpost, the
synthesizer host), with the same five figures, split into the hop itself
and the work on each side; and wake-ups and processor time for every
workload, with the top wakers and readying stacks across all threads, not
only the path to speech. User-cost measurements trace at info level
(`VERBATIM_E2E_RUST_LOG=info`; the harness defaults the outpost to debug,
about a fifth of the event thread in a flood) and on default-mode
workloads. Compare with the earlier figures recorded in the design
records rather than rebuilding old commits, except where no earlier
figure exists. No per-process energy estimates: they are impractical.

### Latency: cargo xtask measure

`cargo xtask measure [--release | --verbatim-exe <path>] [--audible]
[--scenario <name>]... [--label <label>]` runs the measure set (the eight `measure_` scenarios a
silent run takes; `measure_speech_real` is named with `--scenario` in an
audible run), keeps each run's `stderr.log`, and writes `stages.txt`: for
every stage of the latency line, over every announcement, the count, the
median, the 99th and 99.9th percentiles, the maximum, and the share over
1 and 2 ms, in milliseconds. It ends with two totals summed per
announcement: verbatim.exe's own stage with its hop to the outpost, the
outpost's outbound queue and its pipe back, and the wait for the reducer;
and a key's way to its speech queued. The stages are those the latency
line names, listed with it under "Driving a running Verbatim with
verbatim-inspect" ([inspect.md](inspect.md)) (`docs/design/communication.md`, section 10.1, gives
each one's stamps): the listener's way to verbatim.exe, for instance,
shows as listener handling, listener queue, and listener's pipe to
verbatim.exe, followed by verbatim.exe's own stage. A total is what the
user waits on; a stage that shrinks while another grows by as much has
moved the wait, not removed it.

Two runs of the same build differ: a stage's share over 2 ms moved from
3.0 to 0.9 percent between two debug runs on 2026-10-13, with about 400
samples for the key stages. Compare runs of one build against each other
before believing a difference, and give more weight to a change in the
maximum or the 99.9th percentile that holds across runs.

`cargo xtask measure` compiles and runs the working tree as it is,
uncommitted edits included, so a "before" baseline is measured from a
clean worktree of the commit being measured, never from a tree that
already holds the change: run there, or built there and named with
`--verbatim-exe`. It chooses the build it stages as `cargo xtask trace
scenario` does, below: this tree's debug build, its release build with
`--release`, or another with `--verbatim-exe <path>`, under the same
checks, and it refuses to start while `VERBATIM_E2E_VERBATIM_EXE` is set.

### Wake-ups and processor time: cargo xtask trace scenario

`cargo xtask trace scenario <name> --after <seconds> --seconds <seconds>
[--release | --verbatim-exe <path>] [--audible] [--wakers <thread>]...
[--stacks <thread>]... [--samples <thread>]... [--symbol-server] [--keep]
[--label <label>]` runs one scenario,
starts a CPU trace `--after` seconds in, stops it `--seconds` later, and
reads it with `verbatim-trace-waits` (`docs/crates/verbatim-trace-waits.md`):

- `waits.txt`: every thread of Verbatim's processes, with how many times
  it was woken, its time on a processor, and its ready waits.
- `by-name.txt`: the same totalled by process and thread name, the most
  woken first, after the totals for each process and for all.
- `wakers-<thread>.txt` for each `--wakers`: which threads woke the threads
  of that name after a blocked wait.
- `stacks-<thread>.txt` for each `--stacks`: where they were readied from.
- `samples-<thread>.txt` for each `--samples`: where their processor time
  went, by module and by function.

The command chooses the build it stages: this tree's debug build, its
release build with `--release`, or with `--verbatim-exe <path>` another
build, such as a worktree of an older commit, to compare against. It
passes that path to the scenario as `VERBATIM_E2E_VERBATIM_EXE`, cannot
be combined with `--release`, and refuses to start unless the path's
folder holds everything a run stages: `verbatim.exe`,
`verbatim-outpost.exe`, `verbatim-synth-host.exe`, `mockapp.exe`,
`espeak-ng-data` and `sounds`, naming each one missing. It also refuses
to start while `VERBATIM_E2E_VERBATIM_EXE` is set in its environment,
since it sets that variable itself.

Stacks and samples name Verbatim's own functions from the build's PDBs
(`target\debug`, `target\release` with `--release`, or the named build's
folder with `--verbatim-exe`), and system and
kernel code by its exported functions. `--symbol-server` passes
`--symbol-server` to the reader for those reports, so system and kernel
code is named by its real functions from PDBs fetched from Microsoft's
public symbol server and kept in `target\symbols`; it needs network
access.

A thread is named as the reader takes it: a name (`verbatim-worker`), or
`<process.exe>:<name>` for that process's threads of that name, an empty
name meaning threads without one (`verbatim-outpost.exe:` for UI
Automation's own threads in the outposts), or `<process.exe>:#<thread id>`
for one thread by its id, such as an unnamed waker a wakers report named. The trace is deleted once read
unless `--keep` is given; a kept trace is about 20 MB a second, so delete
it from `C:\ProgramData\Verbatim\Traces`, with its `.NGENPDB` folder, as
soon as it has been read again (`cargo xtask trace waits <trace.etl>
...`).

The window must lie inside the scenario, away from its start and its end:
after a trace that covered a scenario's start or end, File Explorer has
three times kept putting the focus on the taskbar's Start button whenever
the desktop was brought forward, and every scenario then failed at its
start with "Start toggle button not pressed" spoken first. Restarting
File Explorer clears it. A scenario's length is in its log
(`finished in`); the windows the 2026-10-13 baseline used, debug build:

- `measure_keys_text_box` (39 seconds): `--after 15 --seconds 8`.
- `measure_focus_explorer`: `--after 10 --seconds 8`. The baseline used it when the scenario ran 17 seconds, and it then lay in a different part of the scenario. The scenario now waits for each new window's announcement and takes about 40 seconds, its ten new windows from about 4 to 30 seconds in and the keys after them, so the same window falls inside the new windows and a trace of it is not comparable with the baseline's. Its latency samples are reported by kind in the run's `measure-summary.txt` ([the end-to-end guide](e2e.md), "The measure set"); runs before 3de02d37 carried few or no new-window samples, none waited for, so their P50 and P95 compare with the keys' figures, while their P99 and maximum can hold a new window.
- `conhost_flood` (115 seconds): `--after 8 --seconds 6`.
- `measure_speech_real` (43 seconds, `--audible`): `--after 20 --seconds 8`.
- `measure_focus_text_box` is over in 5 seconds and has no room for one.

A measurement of what a user pays traces at info level, as a user runs
Verbatim: set `VERBATIM_E2E_RUST_LOG=info` for the run. The suite's
default filter (`DEFAULT_RUST_LOG` in `crates/verbatim-e2e/src/scenario.rs`)
logs the outposts and the hook at debug level, about 111,000 lines in a
`conhost_flood` run and about a fifth of the outpost's event thread's
processor time, where info level logs 2 lines for the flood. It traces the
default floods too: `conhost_flood`'s floods 1 and 4 run with "Report new
output" on, as a user has it, while floods 2 and 3 run with it off, when
Core never holds the outpost and it reads on every change, so a window
over them measures that mode, not the default (`docs/design/wake-ups.md`,
"Measurements after the terminal and typing changes").

Idle Verbatim, started directly with nothing happening, is traced by hand:
start `target\debug\verbatim.exe`, wait 12 seconds, `cargo xtask trace
start`, wait 8, `cargo xtask trace stop`, end Verbatim by its process id,
then `cargo xtask trace waits <trace.etl> --by-name` and delete the trace.
An idle period is time, which the suite's scenarios may not wait on.

Counts in one window depend on what part of the scenario it caught, so
compare per unit of work (per key event, per announcement, per message)
or, for a scenario with a fixed workload, totals over the whole scenario
from the outposts' debug logs (`intake pushed` and `worker: handled` lines
in `target\e2e-artifacts\<scenario>\outpost-*.log`). Threads that exist
only for the harness (the WAV recorder, the control plane's readers and
writers) are reported with the rest and left out of comparisons. In test
runs the outposts log at debug, so a fifth or more of a busy outpost
thread's processor time can be writing its log; a user's outposts do not.

### The trace commands themselves

`cargo xtask trace start` and `cargo xtask trace stop` record a Windows
Performance Recorder trace with WPR's built-in CPU profile (context
switches, ready threads, and a CPU sample of every processor each
millisecond, with stacks). WPR needs administrator rights, so Dickson runs
`vm\scripts\Register-VerbatimTraceTask.ps1` once from an elevated
PowerShell; it registers two tasks that run WPR as SYSTEM and that his
account may start without elevation, as the park task does. `stop` writes
the trace to `C:\ProgramData\Verbatim\Traces\verbatim-<time>.etl` and
names it in `last-trace.txt` there; that folder is readable by his account,
and he may delete what is in it. `cargo xtask trace waits <trace.etl>
[report] [process.exe ...]` reads a trace with `verbatim-trace-waits`, any
of its reports (`--by-name`, `--wakers`, `--stacks`, `--samples`,
`--blocked`, `--timeline`, with `--symbols <folder>` for the PDBs of binaries run from a copy, such as the
end-to-end harness's `target\e2e-stage`). Stacks and samples name kernel
frames by the kernel's exported functions; add `--symbol-server` to fetch
the PDBs of the system images they pass through from Microsoft's public
symbol server, which names system and kernel code by its real functions.
The PDBs are kept in `target\symbols` (`--symbol-cache <folder>` for
another folder) and reused by later runs; the reader prints what it
fetched, found in the cache, or could not get, and an image without a PDB
falls back to its exports. Windows' own `tracerpt` does not
decode context switches, and the Windows Performance Analyzer is not
installed.

`--blocked <process.exe>:*` lists the long blocked waits of any
process's threads, an application's as well as Verbatim's, with the
stacks each waited on and was readied from: what an application's UI
thread, or a Verbatim thread waiting on it, was doing while it stalled.
A stall at an application's start falls outside the window `cargo xtask
trace scenario` allows, so trace it by hand: `cargo xtask trace start`,
run the scenario, `cargo xtask trace stop`.

`--timeline <thread> <from> <to>` shows what a thread did in a window of
a few milliseconds, taken from the logs' times: every blocked wait in it,
however short, with its stacks, and every CPU sample, with its stack. Use
it on an application's UI thread and on UI Automation's I/O thread in the
application (the unnamed thread whose stacks hold
`OverlappedIOManager::IoThreadProc`) to see how a UIA call from Verbatim
was served there, and on the Verbatim thread that made the call.

## Measuring the platform floor

`cargo xtask relay-floor` builds `verbatim-relay-floor` in release and
measures how long a message takes to cross processes on this machine,
under its own power plan, with Verbatim's pipes, so verbatim.exe's stages
in the latency line can be judged against what the platform allows
(`docs/archive/relay.md`, sections 6 and 7; `docs/design/communication.md`,
section 11, step 2). It runs headless and does not take over the desktop,
but it loads the processors when asked to, so run it when nothing else is
being measured. Each variant sends 20,000 frames by default, a burst of
one every 20 ms on average (exponential gaps clipped to 1 to 100 ms,
waited out on a high-resolution waitable timer, so every thread is idle
when a frame arrives, as with events), which is about 7 minutes per
variant.

The variants, all with every process opted out of power throttling:

- F0: two threads in one process and a condition variable: one wake-up.
- F1: a thread writes straight to a receiver process: one hop.
- F2: sender process, a relay process that reads synchronously and writes
  at once, receiver process: the floor for any third process.
- F3: the same with the relay on a completion port, its own ends
  overlapped and the children's synchronous (`relay.md` 4.1).
- F4: the same with the relay built as verbatim.exe's was before stage A
  step 9: reader thread, channel, owner thread, condition variable, writer
  thread.
- F5: F3's relay with the children's ends overlapped too: the sender's
  producing threads write their own frames, an I/O thread finishing only
  writes that pended, and the receiver reads on its I/O thread's port
  (`communication.md` 4.3 and 4.6).
- F6: a synthesizer host's round trip: Core's thread sends a command, the
  host burns a fixed burst of CPU standing in for a first chunk, and
  replies with more bytes than its 4 KiB pipe holds, which paces it; Core
  waits on its port with the reply's read already waiting.
- `frames`: how many reads a 1, 2 and 32 MB frame takes over a 64 KiB
  event pipe, with the read waiting first and with the data arriving
  first, and a child that writes 2 MB and exits only once that write has
  completed, which must arrive whole.

The options, each followed by its value:

- `--variant`: `f0` to `f6`, `frames`, or `all` (the default).
- `--messages`: frames per variant (default 20,000; the design asks for
  100,000 for the chosen design, idle and loaded).
- `--priority`: `normal`, `above-normal`, `highest` (the default), or
  `time-critical`, for every thread on the measured path: the senders'
  producing threads, the relay's threads, the receiver's reading thread,
  the host's thread, and every I/O thread.
- `--load`: `idle` (the default), `all` (a spinning normal-priority
  thread per logical processor), or `all-but-one`.
- `--burst`: frames sent back to back after each gap (default 1), for
  back-to-back frames and the send call's time during a flood.
- `--producers`: the sender's producing threads, all writing to one pipe
  (default 1).
- `--writes`: `inline` (the default), where F5's producing thread writes
  its own frame when no write is in flight, or `io-thread`, where it hands
  every frame to the I/O thread, the simpler form the design compares
  with.
- `--burst-us` and `--reply-bytes`: F6's burst of CPU (default 1,000
  microseconds) and reply size (default 8,192 bytes).
- `--mean-gap-us` and `--seed`: the mean gap and the gap sequence's seed,
  which the run prints so it can be repeated.

For example, `cargo xtask relay-floor --variant f5 --burst 8 --producers
4 --writes io-thread`. Each variant prints one line per hop (for F2 to
F5: sender to relay, relay, relay to receiver, end to end, and the
sender's send call): the sample count, the median, 99th, 99.9th and
99.99th percentiles, the maximum, and how many samples were over 0.5, 1
and 2 ms, with the share over 1 and 2 ms. Then come the counts: each
process's heap allocations per frame, and how many overlapped writes
pended rather than completing at once. A frame's time starts when its
producer begins to encode it. The stamps are kept in memory until the run
ends; times are read with `GetSystemTimePreciseAsFileTime`, which is
derived from the performance counter and the same in every process.

Conditions the command does not set up itself, done by hand:

- Under a real end-to-end flood: run a flood scenario ("Running the E2E
  suite runner-direct" in [the end-to-end guide](e2e.md)) in another console while the benchmark runs.
- Once, for attribution only: under the High performance plan, with
  `timeBeginPeriod(1)` in effect (any program that raises the timer
  resolution while it runs), and on bare metal, so the VM's share and
  Windows' can be told apart.
- Attribution of each slow sample (a manual step, which needs an elevated
  prompt): record the run with `wpr -start CPU -filemode` and `wpr -stop
  relay-floor.etl`, the CPU usage profile with context switches and
  ready-thread events, and open it in Windows Performance Analyzer. For
  every sample over 1 ms, find each involved thread's ready-to-running
  delay in the CPU Usage (Precise) table and what ran instead: another
  thread (pre-emption or priority), nothing in the guest (a vCPU stall), or
  an idle processor waking.
