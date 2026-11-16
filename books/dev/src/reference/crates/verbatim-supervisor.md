# verbatim-supervisor

Core's side of starting, watching and ending its children
(architecture section 1, decisions D9 and D13): the outposts
([verbatim-outpost](verbatim-outpost.md)) and the focus listener
([verbatim-listener](verbatim-listener.md)), a library `verbatim-app`
uses. It is reduced to process facts and their effects
(`docs/design/readers-tell-core-asks-build1.md`, "The outpost generation
record"): it launches children, watches their liveness by pings, sees
pipes close and processes exit, reports these to Core, and carries out
Core's start, end and shutdown effects. Core's reducer owns the outpost
generation record and judges every generation, its replacement, backoff
and missed pings among it (`verbatim_core::supervisor`,
[verbatim-core](verbatim-core.md)). It keeps two spare outposts waiting.
The message layer, the launch arguments and the held processes are
[verbatim-process](verbatim-process.md)'s.

## Public API

`Supervisor`:

- `new(events, options, ignored)` starts the relay, which
  starts the listener at once, launching each role from its executable
  beside the current one (`Executables::beside_current`:
  `verbatim-outpost.exe` and `verbatim-listener.exe`); a missing one fails
  that role's launch. `with_executables` takes the `Executables` and a
  shutdown limit, for tests, whose stand-in is both
  (`Executables::both`), telling the roles apart by the listener's first
  argument, `--listener`. Both take the `OutpostOptions` every outpost
  gets.
- `Timings` holds the ping interval (3 seconds), the pings the listener
  and a spare may miss (three) and the spares' backoff's first wait and
  limit (1 and 30 seconds). `Timings::default` is Verbatim's; `new` and
  `with_executables` use it unchanged, since a stand-in judged by a
  shorter ping interval is ended as stuck when its start is slow; a test
  of the spares' backoff moves the clock it is judged by instead
  (`set_backoff_clock`). No setting reaches them.
- `shutdown()` shuts every child down and returns a `ShutdownSummary`: how
  many children ended cleanly, had exited, exited with an error, were killed
  at their time limit, or were killed because ending them failed.
- `start_outpost(pid)` carries out Core's effect
  (`QueueEffect::StartOutpost`): it gives the application the oldest spare
  waiting, or launches one for it; asked for an application whose
  generation already runs, it reports that one (`Running`), so Core's
  record takes it.
- `end_outpost(outpost)` ends a generation as Core's record asks: it
  missed its pings, or its application exited before it was ready. No
  outpost is ended for being idle (Dickson, 2026-11-10 and 2026-11-16).
- `send_to_listener(command)` sends the listener a request or cancel of
  Core's event queue, behind every command sent to it before.
- `send_to_outpost(outpost, command)` queues a command without waiting,
  failing at once with a `QueueError` when the generation ended or its
  queue is full.
- `leave_nodes_held` (left in the
  writer's slot for the next command), `send_nodes_held` (written behind
  what waits, in place of a list left), `query_idle(round)`, `is_ignored`,
  and `frames`.
- `set_backoff_clock(clock)` has the spares' backoff set and ended by a
  test's clock in place of the run's shared one, so a test decides when a
  backoff ends, after what it waits for has happened.
- `SPARES` is two; Verbatim's readiness waits for them.

The app hears through `OutpostMessage`: `ListenerStarted` (before the
listener's launch, so Core keeps its first tells), `ListenerReady`,
`ListenerEnded`, `Listener` (the current listener's tells and answers,
which Core's event queue judges), `Pong` (each child's answer to a ping,
for Core's liveness), `SpareReady`, `IdleQueried`, `Foreground`,
`Progress` (a progress bar's change from the listener), `Running`,
`LaunchFailed` (Core asked for an outpost and none started, saying
whether the application is gone: exited, not holdable, or ignored),
`ApplicationExited` (a held process exited, after its outpost's `Ended`
when one ran), and per generation `Started` (once its process runs), its
messages as unparsed `Frame`s, and `Ended` with a reason (exited, killed,
target exited). What follows an end is Core's decision. The lifecycle
notices (`Started`, `Ended`, `LaunchFailed`, `ApplicationExited`,
`SpareReady`, `ListenerReady`, `IdleQueried`) carry a `Sealed` that only
the supervisor makes, so no other code announces a generation; the
`lifecycle-for-tests` feature adds `Sealed::for_tests` for the app's
tests. `Frames::arrived` (from
`Supervisor::frames()`, on the reducer's thread) turns a frame into
`Event(pid, outpost_id, position, message)`, the payload boxed: it parses
each frame, stamps node ids with the outpost's id, records when the relay
read it, and numbers the messages that carry node ids
(`docs/design/communication.md`, section 4.8), the position `NodesHeld`
acknowledges. A frame that is not a message
ends its outpost: `Frames` drops that outpost's later frames at once,
handing on only its end, and the relay drops every frame from an outpost
it has already ended. Messages go through the `EventSink` given to `new` (a
channel converts into one; `EventSink::new` takes a delivering function,
and `EventSink::posting(poster, wrap, unwrap)` posts each message to a
completion port's queue, as the app's reducer takes it, mapping an item
given back while the queue is open to `Refused` and one given back after
to `Gone`). `Undelivered` says why one was not: `Gone`, or `Refused`, when
the relay keeps it and everything after it and delivers them again in order
at its next send and at every heartbeat
(`crates/verbatim-supervisor/tests/target_gone.rs`, and, through the port
queue's fault point, `crates/verbatim-supervisor/tests/fault_paths.rs`).
With the `faults` feature, `refuse_shutdown_wake()` arms the mailbox's
fault point, `RelayWake`, to refuse the wake of `shutdown`'s message once,
since its subject is private to the crate ([verbatim-faults](verbatim-faults.md));
`fault_paths` checks that such a shutdown still ends, at the next
heartbeat.

How it works:

- The owner and relay (`owner`, `relay`) is one thread, `verbatim-relay`,
  that launches and ends every child (`docs/design/communication.md`,
  section 6.2). It waits only on one completion port, timing out at the
  next heartbeat (3 s) or the end of the spares' backoff. It reads every
  child's pipe (`transport::PortReader`) and handles each frame in the
  pass that read it: a listener's frame decodes as a `ListenerToCore`,
  matched exhaustively, every frame of a listener other than the current
  one dropped, and its tells and answers handed on; an outpost's frame is
  read only for its kind (`verbatim_process::message::peek`), a pong and
  `TargetExited` handled, `Ready` handled and passed on, anything else
  handed on whole (a copy of its lent parts, `transport::Parts::into_owned`).
  Helpers report through `relay::Mailbox`: a queue
  and a `closed` mark under one lock, where a send into an empty queue
  posts the relay's wake after unlocking, the relay takes the whole queue
  at each wake and each heartbeat, and the `Inbox`'s drop, however the
  owner thread ends, sets `closed` so a later send gets `OwnerGone`. A
  wake the port refuses leaves the item for the heartbeat's take. It never blocks:
  launches and retirements run on threads of their own.
- Each application's generation, from Core's start effect until it ends,
  is one state (`owner::Generation`): `Launching` (a launch at work, or a
  spare given whose launch has not reported), `Starting` (its process
  runs, with a given spare's last fault and the idle round held for its
  `Ready`), or `Ready`, each with the workers its last pong reported
  parked. The listener and the spares waiting are `Run::Launching` or
  `Run::Running`.
- Each child's commands go through a `transport::Sender` (`writer`), of
  the commands its kind of child takes (`WriterHandle<M>` over
  `ToChild`): the sending thread writes when nothing is in flight or
  waiting, and otherwise the command waits in a queue of 64, written by
  the relay as writes complete (`relay::CommandPipes`). When the queue is
  full a ping, the shutdown message, a spare's `Assign`, every command to
  the listener, and the idle round's query still get through, behind what
  waits; anything else fails at once.
- Each application is identified by a process held open
  (`verbatim_process::held::Target`) from the first tell that names it
  (a lookup per tell) or Core's start of its outpost, until it exits.
  Nothing is started for an application that exited or cannot be opened;
  Core hears `LaunchFailed`. The heartbeat checks every held process: one
  that exited ends its outpost, if one runs, as its exit, and Core hears
  `ApplicationExited`; the listener is told (`ApplicationExited`) and the
  handle stays in the listener's record (`exits_told`) until
  `ExitHandled`, so no process can take the id before the listener has
  moved its record. With no listener running, or one still launching, the
  handle closes at once and nothing is sent; a listener that ends drops
  the map with its record.
- An outpost whose pipe closes is ended once everything it wrote is handed
  on: as its application's exit if the application exited, otherwise as
  exited (`crates/verbatim-supervisor/tests/spares.rs`). Every child is
  pinged every three seconds. Core judges an outpost's missed pings by its
  own due time and ends it through `end_outpost`; the owner judges the
  listener's and the spares' by their `Liveness`, nine seconds without a
  pong ending one, and ends an outpost whose pong reports eight workers
  parked on windows the outpost did not judge hung (its `parked_on_hung`
  never counts), all on the run's shared clock. An outpost says
  `TargetExited` when its application exits.
- Every ending is a clean shutdown (`retire`, under "Shutting down"); an
  outpost ended for any reason but its pipe's end has `Ended` sent at once,
  and the app drops what it wrote after.
- The listener is supervised the same way; a replacement's `Ready` makes
  Core ask the attention application for its focus.
- `process` holds the roles' log names and the local query whether an
  application is hung; the command lines are
  `verbatim_process::arguments`' `OutpostArguments` and
  `ListenerArguments`, and launching is
  [verbatim-process](verbatim-process.md)'s.

## Ignored processes

The processes Verbatim ignores are `verbatim_process::held::IgnoredProcesses`
([verbatim-process](verbatim-process.md)): no outpost starts for one
(`start_outpost` answers `LaunchFailed`, the application counted as
gone), no tell of one holds its process, and the listener, given
`--ignore-pids`, drops their events before queuing.
`Supervisor::is_ignored` lets the app pass them over.

## Implementation notes

### Spawning

Each child is created suspended with two unnamed pipes
(`verbatim_process::overlapped::pipe`, 64 KiB each way, verbatim.exe's ends
overlapped), inheriting only its pipe ends and log handle
(`PROC_THREAD_ATTRIBUTE_HANDLE_LIST`), placed in a kill-on-close job with a
200 MB memory cap, and resumed (`books/dev/src/explanation/architecture.md`, section 1). Command
lines carry `--target-pid`, `--spare <launch>`, or `--listener`, and
`--classic-uia` when `uia.remote_operations` is off in `settings.toml`. Each
child's standard error and output go to an append-mode file in this launch's
log directory: `logs\<Verbatim's pid>\outpost-<target image>-<target
pid>.log`, `outpost-spare-<launch>.log`, or `listener.log`, beside the
executables. `verbatim_process::prepare_launch_logs`, called before
any child starts, empties a directory an earlier process with the same pid
left and keeps the newest ten. A failed log open leaves the child
unredirected, never unspawned.
