# verbatim-supervisor

Core's side of starting, watching and ending its children
(architecture section 1, decisions D9 and D13): the outposts
([verbatim-outpost](verbatim-outpost.md)) and the focus listener
([verbatim-listener](verbatim-listener.md)), a library `verbatim-app`
uses. It routes the listener's facts to the right outpost, keeps two spare
outposts waiting, and ends children that crash, hang, or are no longer
needed. Its pure decisions, the generations' typestates and the policy,
are `verbatim_core::supervisor`'s ([verbatim-core](verbatim-core.md)); the
message layer, the launch arguments and the held processes are
[verbatim-process](verbatim-process.md)'s.

## Public API

`Supervisor`:

- `new(events)` starts the relay, which starts the listener at once,
  launching each role from its executable beside the current one
  (`Executables::beside_current`: `verbatim-outpost.exe` and
  `verbatim-listener.exe`); a missing one fails that role's launch.
  `with_executables` takes the `Executables` and a shutdown limit, for
  tests, whose stand-in is both (`Executables::both`), telling the roles
  apart by the listener's first argument, `--listener`. Both take the
  `OutpostOptions` every outpost gets.
- `shutdown()` shuts every child down and returns a `ShutdownSummary`: how
  many children ended cleanly, had exited, exited with an error, were killed
  at their time limit, or were killed because ending them failed.
- `ensure_spawned(pid)` starts an outpost, from a spare when one waits.
- `send_to_outpost(outpost, command)` queues a command without waiting,
  failing at once with a `QueueError` when the generation ended or its
  queue is full.
- `note_views(attention, holding)`, `leave_nodes_held` (left in the
  writer's slot for the next command), `send_nodes_held` (written behind
  what waits, in place of a list left), `query_idle(round)`, `is_ignored`,
  and `frames`.
- `SPARES` is two; Verbatim's readiness waits for them.

The app hears through `OutpostMessage`: `ListenerReady`, `SpareReady`,
`Regenerated`, `IdleQueried`, `NotWatched`, `ForegroundWindow`, and per
generation `Started`, its messages as unparsed `Frame`s, and `Ended` with a
reason (exited, killed, retired, target exited) and whether a next
generation follows (`follows`), the supervisor's decision as it ended it.
Every removal of a record, at a retirement or a target exit, one waiting
for a failure's backoff among them, sends `NotWatched` after the `Ended` of
every generation the app heard started, so nothing waits for an outpost
that will not come. The lifecycle notices (`Started`, `Ended`,
`Regenerated`, `NotWatched`, `SpareReady`, `ListenerReady`,
`IdleQueried`) carry a `Sealed` that only the supervisor makes, so no other
code announces a generation: the generations' typestates, in
`verbatim_core::supervisor::generation`, return a plain `Lifecycle` notice,
which only the owner turns into its sealed message; the `lifecycle-for-tests` feature adds
`Sealed::for_tests` for the app's tests. `Frames::arrived` (from
`Supervisor::frames()`, on the reducer's thread) turns a frame into
`Event(pid, outpost_id, position, message)`, the payload boxed: it parses
each frame, stamps node ids with the outpost's id, records when the relay
read it, and numbers the messages that carry node ids
(`docs/design/communication.md`, section 4.8). A frame that is not a message
ends its outpost: `Frames` drops that outpost's later frames at once,
handing on only its end, and the relay drops every frame from an outpost
it has already ended. Messages go through the `EventSink` given to `new` (a
channel converts into one; `EventSink::new` takes the app's posting
function). `Undelivered` says why one was not: `Gone`, or `Refused`, when
the relay keeps it and everything after it and delivers them again in order
(`crates/verbatim-supervisor/tests/target_gone.rs`).
`EventSink::watched(progress, stalled)` reports a `ReducerStall` once the
reducer thread has taken nothing for three heartbeats while inputs waited;
nothing is ended for it (`docs/design/communication.md`, section 6.8).

How it works:

- The owner and relay (`owner`, `relay`) is one thread, `verbatim-relay`,
  that makes every lifecycle decision and owns the records
  (`docs/design/communication.md`, section 6.2). It waits only on one
  completion port, timing out at the next heartbeat (3 s) or sweep (30 s).
  It reads every child's pipe (`transport::PortReader`) and handles each
  frame in the pass that read it: a listener's frame decodes as a
  `ListenerToCore`, matched exhaustively, and its fact is routed and
  written to its outpost there and then; an outpost's frame is read only
  for its kind (`verbatim_process::message::peek`), a pong and `TargetExited` handled,
  `Ready` handled and passed on, anything else handed on whole
  (`transport::FrameBody::Owned` or a copy). Helpers report through `relay::Mailbox`. It never blocks:
  launches, retirements, and hang checks run on threads of their own.
- Each application's record keeps what belongs to the application (its
  held facts, held idle round, attention time and backoff) and its
  generation's state (`verbatim_core::supervisor::generation::Generation`): `Launching`,
  `SpareGiven`, `Starting`, `Ready`, or `Waiting` for a failure's backoff
  with no generation, the one that failed before its `Ready` kept to be
  replaced. Each transition (`given`, `launched`, `assigned`, `ready`,
  `failed`, `removed`) consumes the state and returns the next with its
  notices in order, which the owner applies (`Owner::apply`): the
  listener's hand-over messages, a spare's `Assign`, a launch, and the
  app's messages. Leaving `Ready` always removes the listener's hook in the
  same step, and the listener's "forwarding ended" is relayed only to the
  `Ready` generation it names. The listener and the spares waiting, with
  no generations, are `Run::Launching` or `Run::Running`.
- Each child's commands go through a `transport::Sender` (`writer`), of
  the commands its kind of child takes (`WriterHandle<M>` over
  `ToChild`, whose routed fact for the listener is uninhabited, so the
  listener's writer cannot be given one): the
  sending thread writes when nothing is in flight or waiting, and otherwise
  the command waits in a queue of 64, written by the relay as writes
  complete (`relay::CommandPipes`). When the queue is full a ping, the
  hand-over's messages, and a UIA focus's parts still get through, never
  merged (`Outgoing::delivered`); a routed fact replaces a waiting one for
  the same object and kind (and thread, for an MSAA event but a menu
  opening) or the oldest fact makes room; anything else fails at once.
- Facts for a starting outpost are held, merged by the same rule, and
  released on `Ready` (`verbatim_core::supervisor::policy::HeldFacts`, which
  reads the clock, through the owner, only for an application's first held
  fact); then the listener gets `HookInstalled`,
  and later `HookRemoved`, through `Owner::tell_listener`.
- Each application is identified by a process held open
  (`verbatim_process::held::Target`)
  while it has a record. Nothing is started for an application that exited
  or cannot be opened; the app hears `NotWatched`.
- An outpost whose pipe closes is ended once everything it wrote is handed
  on: with its target if the application exited, otherwise by the failure
  path (`crates/verbatim-supervisor/tests/spares.rs`). Every child is pinged
  every three seconds, and nine seconds without a pong ends it; eight
  abandoned workers end an outpost unless its application's windows are
  reported hung (`policy::wedge_decision`). An outpost whose application has not
  held attention for two minutes, is not Core's own, and holds no nodes of
  the reducer's is retired by the sweep (`policy::retirement_decision`). An
  outpost says `TargetExited` when its application exits, and the heartbeat
  also checks every held application.
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
- The relay stamps each fact as it reads it from the listener
  (`verbatim_core::supervisor::Received`, read on both clocks by
  `writer::received_now`), and its relay times as it writes it
  (`writer::stamp`).

## Ignored processes

The processes Verbatim ignores are `verbatim_process::held::IgnoredProcesses`
([verbatim-process](verbatim-process.md)): no outpost starts for one
(`ensure_spawned` answers `NotWatched`), none of its facts is routed, and
the listener, given `--ignore-pids`, drops them before queuing.
`Supervisor::is_ignored` lets the app pass them over.

## Implementation notes

### Spawning

Each child is created suspended with two unnamed pipes
(`verbatim_process::overlapped::pipe`, 64 KiB each way, verbatim.exe's ends
overlapped), inheriting only its pipe ends and log handle
(`PROC_THREAD_ATTRIBUTE_HANDLE_LIST`), placed in a kill-on-close job with a
200 MB memory cap, and resumed (`docs/architecture.md`, section 1). Command
lines carry `--target-pid`, `--spare <launch>`, or `--listener`, and
`--classic-uia` when `uia.remote_operations` is off in `settings.toml`. Each
child's standard error and output go to an append-mode file in this launch's
log directory: `logs\<Verbatim's pid>\outpost-<target image>-<target
pid>.log`, `outpost-spare-<launch>.log`, or `listener.log`, beside the
executables. `verbatim_process::prepare_launch_logs`, called before
any child starts, empties a directory an earlier process with the same pid
left and keeps the newest ten. A failed log open leaves the child
unredirected, never unspawned.
