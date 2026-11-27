# verbatim-listener-logic

The focus listener's pure logic (decision D13; ADR 0003, "Readers tell,
Core asks"): what its check threads, its reading threads and its
foreground watch decide, with no Windows type, no call and no clock, so
every ordering of inputs is a replay test. It is a logic crate: in the
GPL tier, with no Windows dependency, so it builds and is tested on any
host. It depends on `verbatim-model`, on
[verbatim-reader-rules](verbatim-reader-rules.md) for the class verdict
and the limiter's count per thread, and on `tracing`, which the schedule
logs through. The listener ([verbatim-listener](verbatim-listener.md))
runs it on its threads and makes every call; that guide's "Tells, checks
and Core's requests" describes the threads around it. Its API is in its
rustdoc (`cargo doc -p verbatim-listener-logic --no-deps --open`).

## The reading threads' schedule

`schedule`: the reading threads' schedule, as methods on
`Schedule<J, W>`, each returning the `Effects` the shell applies in
order under its lock: threads to start, threads to wake, records let
go, jobs ended unrun (`Ended`, with its `Ending`), requests abandoned,
and idle waiters to run. One record per application process
generation (`AppKey`): `running`, one per process id, which a job looks
in alone, and `exited`, by `AppKey`, for records with a thread still in
charge or abandoned. A record's reader is one value: `Idle` with at
least one abandoned thread out, `InCharge` with its `Charge` (its thread
number, its abandoned count, the job running, when it was last idle and
the jobs waiting), or `Starved` at `ABANDONED_LIMIT` (two) with the jobs
waiting. A job (`JobKind`) is Core's request or a release of kept
elements; requests wait in a lane ahead of releases. `push` queues a job, starting a thread when none is in
charge and fewer than two are abandoned; `next` hands the thread its
next job, or has it wait for the idle time the check threads use
(`IDLE_US`, 30 seconds), then lets its record go, at once for an exited
application; `returned` says whether a job's outcome is sent, a job
abandoned while it ran discarded, and the thread number it goes on as,
an abandoned thread's return at the limit reused for what waits;
`cancel` ends a queued request cancelled for any reason, abandons a
running one for the foreground with a fresh thread for what waits, and
leaves it alone for any other reason; `application_exited`
moves the record to `exited`, a starved record's jobs ended "not
there"; `spawn_failed`, `when_idle` and `stop` end the rest. An idle
waiter runs once every job queued before it is no longer waiting or
running; abandoned threads do not count.

## The foreground watch

`foreground`: what the listener learns of each window that came to
the foreground, for Core. `ForegroundWatch` takes the window in front at
the start (`start`), says whether it has started (`started`), hears of
each window reported (`sent`), which ends any wait for a console's
thread, and takes each console event (`console_event`), answering a
`Step`: the window to report, with when it was observed, and whether to
hook or unhook the console's events. `Pending` is the one pending
foreground, under the lock every tell is placed under: `Clear`,
`Holding` the MSAA items placed behind a foreground event whose window
`GetForegroundWindow` does not name until it does or
`FOREGROUND_HOLD_US` (20 ms) pass, or `Awaiting` that window once its
hold is over. Only the `WinEvent` thread steps it (`step`, given a
`Seen`: a foreground event, the shell's foreground (`Seen::Shell`;
Claude, 2026-11-21), any other event, an invalid window's foreground
event among them (Claude, 2026-11-21), or the hold's timer), and each step answers `Placed`: the window found
in front to report, before what it releases, in order, and how the
hold's one timer changes; a newer foreground event replaces the pending
one and counts its time again, a dropped foreground's window is still
awaited, the shell's foreground holds nothing, and a stale `WM_TIMER`
with no hold does nothing. Before the watch's start nothing is
reported. An item placed from another thread, the idle barrier's
answer, is held behind a hold in force and releases nothing
(`place_behind`); a UI Automation tell passes it by, sent at once. None
sent replaces another.

## The check

`check`: the calls a UI Automation event's check makes at delivery on
its application's check thread, NVDA's handler for its kind in its
order, as one straight-line function, `check`, over the `CheckCalls`
the thread answers on the event's element: the walk to the nearest
window, the window's classes, the probe, each kept element's keyboard
focus, the window's acceptance facts, the previous sibling, the
parent's automation id, the element's keyboard focus, each named to a
test's observer as a `Call` before it is made. `CheckFacts` holds what
the check is made of, from the event's kind and cache, the switcher's
item read by `reading_parent`. `Handler::of` maps a kind to its
handler: a focus or menu opening makes every call, a walk that finds no
window ending it; an element selected, a window opened, a property
change and a progress bar's change walk and probe, then read the
acceptance facts, where NVDA's handler judges acceptance; a
notification walks alone, a failed walk telling no window. The probe
the tell carries, and the record keeps, is the raw probe's answer with
NVDA's checks after it (`listener_provider`, judged with the classes
read): a list view's provider counts only from Windows Forms. A window
on one of NVDA's lists is not probed, nor one whose probe in the
thread's `ProbeRecord` began within `PROBE_RECORD_US` (half a second),
the record keeping whether a provider answered: the record only spares
a probe call, since Core recorded the same probe from the earlier tell.
A window read through MSAA, by a list, the probe or the record, does
not end the check: Core's backend cache is the one owner of a window's
verdict and judges NVDA's native-element test, so the keyboard-focus
reads are made whatever was found. A nameless `NetUITWMenuItem` menu
item has its previous sibling read, and with none, or none read, its
keyboard focus is not, as NVDA's placeholder allows its focus. A focus
whose cache names a list item of `SWITCHER_ITEM_CLASS` (`ListViewItem`),
the task switcher's item, has its raw-view parent's automation id read
with one call, before its keyboard focus. `check` answers `Found`: what
the walk found (`Walked`), the `Checked` the tell carries, and the
parent's automation id, which the tell carries among its cached
properties (`PropertyId::ParentAutomationId`).

## The check threads

`check_threads`: the check threads' records (`CheckThreads`), one per
application, keyed by its process id, under the listener's one lock:
`push` queues an event, starting a thread when there is no record or
waking an idle one; `take` hands the thread its next event, or has it
wait, or exit by removing its record once idle for `IDLE_US`
(30 seconds); `checked` says whether its tell is placed, only while the
record is its own; `ended` drops an ended application's queue untold;
`stop` drops every queue and names the threads it abandons inside a
check, for the listener's log; and `when_idle` holds the idle barrier's waiter until every event pushed
before it is told or dropped.

## Kept elements

`kept`: one application's kept elements (`KeptElements`): told, with whether its event was a focus or a menu
opening, in read for a request, a focus read or not, read, and
released, each kept until Core's held view (`View`) leaves it out.
`with_runtime_id` lists those a check reads the keyboard focus of, the
ones that can be the readers' last focus: told by a focus or a menu
opening, in a focus read, and read. `view` lets go of every told or
read element a view rules out, one in read left for its read's end;
`read_ended` keeps a focus read's element as read unless the last view
rules it out; `read_abandoned` hands back the kept reference of the
element a request cancelled for the foreground was reading; `ended`
lets go of every kept reference, a read in flight releasing its own as
it ends. `Reported` is the writer's numbering of the messages carrying
tokens, in pipe order, and where each token's tell was written: the
numbering both readers share (`verbatim_reader_rules::Numbered` over
tokens), with its `too_early` rule. `ListenerKept` is the listener's whole
judgement: every application's `KeptElements` and Core's last view,
each call given the writer's `Reported`, whose reports of released
tokens it forgets; a read whose answer was not sent is released at its
end. `focus_kind` is the one rule of which kinds a check reads a told
element for, a focus or a menu opening. Every element let go is handed
back for release on a thread that may block.

## Tells

`tell`: what the listener decides of a `WinEvent` before it tells
Core of it: `tell_window`, NVDA's mapping of a window of 0 to the
desktop window, and of an invalid one to it for the end of a menu, a
popup menu or the task switcher, dropping any other event of an invalid
window, and a window's destruction told only for the window's own
object; `dropped_by_class`, Windows Live Messenger's hidden window and a
show of neither a client nor a custom object, and a name change of the
mouse pointer (`OBJID_CURSOR`), whose name is its shape; `hang_tested`,
whether a hung or ghosted window drops an event of a kind, true only
for the kinds NVDA's `winEventToNVDAEvent` tests, so a window's
destruction, the end of a menu, a popup menu or the task switcher, the
system caret and the desktop switch are told from a hung window and
Core's shell refuses its request there.

## Tests

`cargo xtask ci` reads `schedule.rs` by the pure rule
(`xtask/src/ownership.rs`).

Tests: each module's unit tests; the schedule's, one per rule, a replay of calls with exact effects
(`crates/verbatim-listener-logic/src/schedule/tests.rs`). The pending
foreground has one test per cell of its table that changes, the timer
releasing with the window in front among them, its expectation taken
from NVDA's deferral (`crates/verbatim-listener-logic/src/foreground.rs`).
