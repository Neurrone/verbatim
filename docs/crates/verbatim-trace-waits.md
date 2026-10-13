# verbatim-trace-waits

How long threads waited to run, read from a Windows Performance Recorder
trace of the CPU's scheduling, a binary run by `cargo xtask trace waits
<trace.etl> [process.exe ...]` (`docs/tooling.md`, "Tracing the CPU's
scheduling"). `docs/design/communication.md` uses it to attribute a slow
hand-off to a thread that was ready and not run, and to check that raised
thread priorities never keep Verbatim's GUI thread, or the foreground
application's, waiting (section 6.1, and section 11 step 3's starvation
check).

It has no library API. It reads the trace with Windows' event consumer
(`OpenTraceW` and `ProcessTrace`), which needs no elevation for a trace
file, and takes four kinds of kernel event:

- a ready thread (`ReadyThread`): the thread named becomes ready, unless it
  already is;
- a context switch (`CSwitch`): the thread switched in has waited since it
  became ready, one ready wait; the thread switched out, if it is still
  ready (pre-empted), is ready from now;
- a thread's start, or a thread running when the trace began, and a
  thread's new name (`SetThreadDescription`, which Rust's named threads
  set): its process and its name;
- an image loaded in a process: the process is named by its `.exe`.

For each thread of the processes named (by default `verbatim.exe`,
`verbatim-outpost.exe` and `verbatim-synth-host.exe`) it prints the
process's image and id, the thread's id and name, how many times it was
woken (each ready event naming it), its time on a processor (summed from
the context switch giving it one to the next taking it away; a thread
already running when the trace began counts from its first switch), how
many ready waits it had, their median, 99.9th percentile and maximum, and how many were over 1,
10 and 100 ms. A Rust program's main thread is named `main`, which in
verbatim.exe is the GUI thread.

The event layouts it reads are the kernel's documented ones on 64-bit
Windows: a context switch's new and old thread ids at offsets 0 and 4 and
the old thread's state at 14; a ready thread's id at 0; a thread event's
process and thread ids at 0 and 4 and its name at 72; a name change's
process and thread ids at 0 and 4 and its name at 8; an image event's
process id at 16 and its file name at 56.

A trace of WPR's CPU profile grows by about 20 MB a second, so a trace is
kept to the run being measured, and deleted once read.
