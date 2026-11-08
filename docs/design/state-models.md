# Three state models

Status: approved by Dickson on 2026-11-09. It covers the listener's record of an application, the outpost's record of hung windows, and the outpost's followed focus (`docs/roadmap.md`, "State models"). Core's held-key line is built with the caret and selection rework. Each model below replaces facts kept in several places, or as combinations of flags, with one state value and one writer. A table cell marked "cannot occur" is a logic error returned as one, never a silent default.

## Terms

- An application generation is one run of one process: a process id reused by a new process is a new generation.
- A reader is the listener's thread reading one application's UI Automation events. An abandoned reader is one whose read passed its deadline or whose user moved on, and which has not returned.
- A program is one piece of outpost work run on its worker; a commit is the one change it asks the reducer to apply.
- Telling a hang is sending Core `NotResponding`. Core plays the "not responding" sound only if the application holds attention then, so a hang met in the background is never sounded, even after the user returns (Dickson, 2026-11-06).
- The followed focus is the last focus this outpost reported and adopted: the one whose caret, text and subscription it tracks.

## The listener's record of an application

### State

```rust
struct AppKey { pid: Pid, generation: u64 }

/// A running application, one per process id.
struct Running<J> { generation: u64, life: Life<J>, app: App }

enum Life<J> {
    Live(Reader<J>),
    Ending(Reader<J>),
}

/// A process that has exited: no event can reach the record, and only its readers are left.
struct Exited<J> { draining: Draining<J>, app: App }

enum Reader<J> {
    /// No thread in charge, nothing queued, `abandoned` readers still out, at least one.
    Idle { abandoned: NonZeroUsize },
    InCharge(Charge<J>),
    /// The limit abandoned since `since`, no thread in charge, events may wait.
    Starved { since: u64, queue: VecDeque<Queued<J>> },
}

enum Draining<J> {
    Idle { abandoned: NonZeroUsize },
    InCharge(Charge<J>),
}

/// A thread in charge, fewer than the limit abandoned.
struct Charge<J> {
    abandoned: usize,
    /// The read under way and the generation in charge.
    flight: InFlight<Running>,
    queue: VecDeque<Queued<J>>,
}
```

`Schedule` holds `running: BTreeMap<Pid, Running<J>>` and `exited: BTreeMap<AppKey, Exited<J>>`. An event looks in `running` alone, so it cannot reach an exited record and no list of current processes has to agree with the records. `App` keeps `left_at`, `last_top` and `counters`: `counters` is the generation and request numbers a new `Charge`'s work in flight continues from, read only when one starts, so a thread that has lost charge can never match a later one of its record. Every schedule input, effect and job that names an application names its `AppKey`, and so do the reading threads' shell records (`Spawn`, `Job`, `Next`, `Returned`, `Learned`, `Placed`, `Judged`, `SpawnFailed`, `Wake`, `Unwatch`, `Forget` and `AppShell`, which are keyed by process id today), so an exited generation's returning thread cannot wake, forget or match a new generation with the same id. Which generation is in charge is `Charge.flight`'s alone. Neither map grows with every process, since a record is dropped once nothing is left in it. The count of abandoned readers is a number checked against `ABANDONED_LIMIT` at the one place it rises, because a type for that range would add nothing.

Rules, each with its reason:

- A record with no thread in charge, no reader out and nothing queued does not exist. It is dropped at the transition that reaches it. Nothing it held can be needed: `left_at` and `last_top` serve only a thread's last read, and no thread is left.
- `Starved` carries no count, since it means exactly the limit. `ABANDONED_LIMIT` is reached only through `Starved`, so a live `Idle` or `InCharge` is below it.
- `Ending` is the supervisor's request to let go of the reader when it has nothing to do. Any event of a current key turns it back to `Live`, because the application is running. That event can be a stale one in the pipe; the cost is a reader kept until the process exits, which the exit below ends.
- A process's exit moves the record from `running` to `exited` under its `AppKey`, so no later event reaches that record, and a new event starts a new generation with no abandoned readers. `Exited` has no `Starved`, because its waiting events are dropped at the exit and nothing can wait again; a starved record becomes `Idle` at the limit. An outpost that crashed or retired with its application running keeps the record and its count, because the application's hang is still there.
- The two top-level windows become one. A new `ScheduleInput::Placed`, sent once per read when the read finds its top-level window, carries it into `Running.top`, since a read of a known window never sends `Learned`; a successful `Returned` copies it to `last_top`. `Watched.top_level` and `Foreground.running_tops` go. `FocusReader.last` holds the element's own window, which is another fact, and stays.

### How the listener learns of an exit

Today it cannot: `CoreToListener::HookRemoved { pid }` says only that no outpost's hook is in. The supervisor has the fact. `Supervisor::forget_app` (`crates/verbatim-supervisor/src/owner.rs`) is the single place it learns a held process exited, including a retired outpost's application, found by the 30-second sweep. It sends a new `CoreToListener::ApplicationExited { pid }` from `forget_app`'s branch that found the process, never for one it did not hold, as `release_target` can ask. It keeps the process's handle in a map of exits being told until the listener answers `ListenerToCore::ExitHandled { pid }`, because Windows reuses no process id while a handle is open: no process with that id can exist until the listener has moved the record, so a new process never joins the old key. Closing the handle when the message is written would not give this, since the listener may not yet have read it. With no listener running, or one still launching, the handle closes at once and nothing is sent, since a new listener has no record of the process; a listener that ends drops the whole map for the same reason.

Every process a listener has a reader for is held. An application that sends only facts that start no outpost, such as background selections, has no outpost record, so `route_fact`'s branch for that case holds the process without a record, and the sweep then finds its exit as it does a retired outpost's application. If the process cannot be opened, the branch sends `HookRemoved`, as for any process the supervisor cannot hold; a stale fact of an exited process takes that path too.

A process the supervisor cannot hold never gets this message; `HookRemoved`, sent for it from `not_watched`, ends its reader as today, so only the record's identity across reuse is lost.

### States against rules

| Rule | Live Idle n | Live InCharge n | Live Starved | Ending Idle n | Ending InCharge n | Ending Starved |
| --- | --- | --- | --- | --- | --- | --- |
| Event pushed, key current | InCharge n, a new thread | Queued, its thread woken | Queued; past since plus deadline, every waiting event dropped | Live, then as Live Idle | Live, then as Live InCharge | Live, then as Live Starved |
| Thread asks next, queue has an event | Not in charge, nothing | Job given | Not in charge, nothing | Not in charge, nothing | Job given | Not in charge, nothing |
| Thread asks next, queue empty | Not in charge, nothing | Thread waits | Not in charge, nothing | Not in charge, nothing | n is 0, record dropped; otherwise Ending Idle n | Not in charge, nothing |
| In-charge read returns | Cannot occur | Outcome sent, `last_top` set, stays | Cannot occur | Cannot occur | Outcome sent, `last_top` set, stays | Cannot occur |
| Abandoned read returns | n less one; at 0 dropped | InCharge n less one | Queue empty, Idle at limit less one; otherwise InCharge at limit less one, reusing the returned thread | n less one; at 0 dropped | Ending InCharge n less one | As Live Starved, staying Ending |
| Read abandoned | Cannot occur | n plus one: at the limit, Starved at now; otherwise queue waiting, InCharge n plus one with a new thread, queue empty, Idle n plus one | Cannot occur | Cannot occur | As Live InCharge, staying Ending | Cannot occur |
| Alarm at since plus deadline | Nothing | Nothing | Waiting events dropped, stays | Nothing | Nothing | Waiting events dropped, stays |
| Thread failed to start | Not in charge, nothing | Queue dropped; n is 0, dropped; otherwise Idle n | Not in charge, nothing | Not in charge, nothing | As Live InCharge, staying Ending | Not in charge, nothing |
| Outpost ended | Ending Idle n | Ending InCharge n, thread woken | Ending Starved | Nothing | Nothing | Nothing |
| Application exited | Exited Idle n | Exited InCharge n, thread woken | Queue dropped at once, Exited Idle at the limit | Exited Idle n | Exited InCharge n | Queue dropped at once, Exited Idle at the limit |
| Foreground or another application's focus | Marks `left_at` | Marks `left_at` | Marks `left_at` | Marks `left_at` | Marks `left_at` | Marks `left_at` |

"Cannot occur" cells follow from a read existing only under a thread in charge. An `Exited` record follows the Ending column of the same reader, with three differences: an event cannot reach it, the outpost's end and the exit change nothing, and a read abandoned to reach the limit leaves it `Idle` at the limit with its queue dropped, rather than `Starved`. An abandoned read of an exited generation decrements that record, never the new generation's.

### Removed

`App::{threaded, abandoned, limit_reached, ending}`, `App::spent`, the record's key by process id alone, `Watched.top_level`, `Foreground.running_tops` and the `Returned.top` field.

### Costs

Two pipe writes, the exit and its answer, and a wake of each side per held process's exit. One process open per application first seen through a fact that starts no outpost, and one failed open per such fact for a process that cannot be opened. One more take of the listener's global schedule lock per read, `Placed` in place of an atomic store, which is on the event path and is recorded in `books/dev/src/reference/performance.md`. A read already costs far more. No new timer and no call into another process. Foreground events stop reading each record's atomic. Only the caret build measures wake-ups and processor time before and after (Dickson, 2026-11-09).

### Copies kept, and why

The supervisor's `apps` and the listener's `running` both know which processes live. The supervisor owns the fact, and `running` is what it has told the listener, because the listener stamps every event with a generation and cannot ask across a pipe for each one.

### Limits

A process the supervisor cannot hold has no generation boundary, so a reused id could inherit its record only while a reader is still out; the record ends once those return. An application whose outpost retired is learned to have exited at the next sweep, up to 30 seconds later, and its reader is idle meanwhile. Waiting events of an exited process are dropped as soon as the exit is known rather than at their bound, with the same outcome.

### Test

The generated test `crates/verbatim-listener-logic/src/schedule/generated.rs` is replaced. Its world is real processes and threads: applications that start, exit and reuse an id, an id being reused only once the listener has answered that exit, as Windows reuses no id while the supervisor's handle is open; events of each generation; threads whose reads return, hang or are slow; outposts that crash, retire or end with the process. The reference model is written from `docs/parity/focus-events.md` ("Reads held behind an application's abandoned reads") and the recorded decisions in `docs/roadmap.md`'s entry, not from the table above. It keeps per generation the set of threads with what each does, and the events waiting in push order. It predicts, and does not follow, which thread is told what. It checks after every step: every event has exactly one outcome; at most one thread is in charge and at most the limit abandoned per generation; a new generation's events never wait behind an older generation's readers; an application at the limit reads nothing until a reader returns and drops its waiting events at the bound; a crash with the application running keeps the count; after exit and every reader's return nothing is held for the generation.

## The outpost's record of hung windows

### State

```rust
enum Hang {
    /// No hang is being told.
    None,
    /// `window`'s hang was told to Core.
    Told { window: Window },
}
```

One value per outpost, in the reducer. It ends with the outpost generation, as all reducer state does.

Rules, each with its reason:

- One hang is told per application: seeing a second window hung while the told window is still hung is the same hang, and windows on one thread hang together (Dickson, 2026-11-07).
- A later hang in another window is told again, plays the sound if attended, and replaces the told window (Dickson, 2026-11-08). A window still hung when the told window recovers is such a hang, sounded once when next seen if the application holds attention (Dickson, 2026-11-08).
- While a hang is told, the told window is judged live, by the outpost's one judge of whether a window is hung (`Outpost::readability`, a local call that counts a ghosted window as hung and that tests inject), at most once per reducer input, on an input where an entry is taken or a commit applied (`Effect::JudgeTold`, answered before the shell takes another message). The effect names the window judged and the window waiting on the answer, if any, and the answer brings both back, so `Hang` needs no third state. An answer for a window no longer told changes nothing, because `commit` asks its other checks first and their answers can replace the told window before this one arrives. A not-hung answer ends the hang, so a window that recovers and later freezes again is a new hang (Dickson, 2026-11-09). This replaces a half-second check of the told window, which would need a timer of its own, because the existing half-second checks run only while a program or a marker is in flight. An answered commit is therefore never taken at its word: its call may have begun before the hang and failed after it.
- Every commit that names a window hung is confirmed live before it counts (`Effect::CheckHung`, answered by `Input::HangSeen` with no request). A program begun before a window's destruction can therefore no longer record it, since a destroyed handle answers not hung, and `Current.destroyed` goes.
- Whether a program's worker is parked on a hung window stays `Current.hang_mark`, one fact of that program written once, since a program whose worker names its window has no window to compare at abandonment.
- The two counts reach the pong as one value: `Lock<Parked>`, written together when a worker is parked or returns and read once per ping. The lock is held only for that assignment. Each parked worker is then counted once in a pong. The supervisor's `Parked` is the value each pong replaces.

### States against rules

| Rule | Nothing told | Told of this window | Told of another window |
| --- | --- | --- | --- |
| `HangSeen` of this window | Told of it; `NotResponding` sent | Nothing | `Effect::JudgeTold` for the told window, this window waiting on the answer |
| An entry taken, or any commit, the first on this input | Nothing | `Effect::JudgeTold` | `Effect::JudgeTold` |
| This window judged still hung | Nothing | Nothing: the same hang | Nothing: the hang it judged has ended |
| This window judged not hung, no window waiting | Nothing | Nothing told | Nothing: the hang it judged has ended |
| This window judged not hung, another window waiting | Nothing | Told of the waiting window; `NotResponding` sent | Nothing: the hang it judged has ended |
| Commit names this window hung | `Effect::CheckHung` | `Effect::CheckHung`, its `HangSeen` then changing nothing | `Effect::CheckHung` |
| Commit names this window answered | Nothing | As any commit: judged, never ended by the answer itself | As any commit |
| Window in front readable (`FrontReadable`) | Nothing | Nothing told | Nothing |
| Window's destroy event | Nothing | Nothing told | Nothing |
| Program's check finds its window hung (its own request) | As `HangSeen`, and the program's `hang_mark` set | As `HangSeen`, `hang_mark` set | As `HangSeen`, `hang_mark` set |
| Worker abandoned | Parked as `hang_mark` says | Parked as `hang_mark` says | Parked as `hang_mark` says |

### Removed

`Reducer::hung`, `Current::destroyed`, and the two atomics `abandoned` and `abandoned_on_hung`.

### Costs

One local window call per reducer input while a hang is told, and none otherwise, so nothing on the normal path. A commit naming a window hung adds one to confirm it, and when another window is told a third, to judge the told window with the named window waiting. No new timer, no new wake-up and no call into the application. One lock per ping and per parked worker.

### Copies kept, and why

The shell's `parked` map, of each parked worker and whether it is on a hung window, is the writer's own: it says what to take away when a worker returns. `Lock<Parked>` is the total the pipe reader needs, which cannot touch shell state, and both are written in one step. The supervisor's `Parked` is what the last pong said, replaced whole.

### Limits

With no event or program between a window's recovery and its next freeze, the two are indistinguishable and count as one hang, since telling them apart needs a periodic check, which is not added. A new outpost generation tells a continuing hang again, so the sound can play a second time if an outpost restarts during a hang, since nothing outlives the outpost to remember it (Dickson, 2026-11-09).

### Test

The model in `Run::hangs`, the hang check of the reducer's generated test (`crates/verbatim-outpost-logic/src/tests/reducer.rs`), is rewritten, and that test gains the world below. Its world holds each window's true condition: responsive, hung or destroyed, changing with no event. Its steps are windows hanging, recovering unseen, being destroyed; programs checking, committing answered or hung with evidence from an earlier moment; the front check; workers abandoned and returning. The reference model is written from the recorded decisions on hangs (2026-11-06, 2026-11-07 and 2026-11-08) and ADR 0001, independent of the record. It says when a hang must be told: on evidence of a window truly hung, when no hang is told or the told window was not hung in the world at some step since it was told. It checks that `NotResponding` is sent exactly when predicted, never for a destroyed window, and that a pong counts each parked worker once, on hung exactly if a check found its window hung before it was abandoned.

## The outpost's followed focus

### What survives the focus queue

Core's queue changes what Core does with reports, not what the outpost needs to read and plan its own events, so every fact has a reader at this commit:

- `Marks.focused`: planning a batch and placing a focus's read after the old focus's events (`Intake::push`, `Intake::next`), and the redirect check.
- `Marks.terminal`: planning a terminal's caret event after its text changes.
- `Marks.focus_redirected`: dropping the control's own later event, already reported by the redirect.
- `Marks.absorbed`: dropping a held console's updates at the mailbox.
- `Marks.system_caret_unused`: dropping the system caret's events at the mailbox.
- `Tracking`'s focus, role, window, chain, titles and MSAA report, and `FocusPath`: the ancestor walks, the live re-read, the worker's element lookups.

Each of the five `Marks` fields is set at adoption, ended by the next adoption, and describes the followed focus, so they move into its record.

### State

```rust
/// Replaced whole when a focus is adopted.
struct Followed {
    node: NodeId,
    role: Role,
    object: Option<Object>,
    window: Option<isize>,
    chain: Vec<NodeSnapshot>,
    titles: Vec<SavedTitle>,
    origin: Origin,
    terminal: TerminalMark,
    redirected: bool,
    system_caret_unused: bool,
    absorbed: Option<isize>,
    subscription: Option<Subscribed>,
    owed: Option<Owed>,
}

enum Origin {
    /// The listener's read of a UI Automation focus.
    Listener(FocusPath),
    /// An MSAA focus, which a live re-read finds again.
    Msaa(MsaaReport),
    /// A focus-now answer.
    Answer,
    /// An MSAA menu opening, whose chain is at most Core's hidden frame.
    MenuOpening,
}

struct Subscribed { group: Group, element: u64, slider: Option<(NodeId, u64)> }

struct LastReported { role: Role, batch: u64 }
```

`FollowState` becomes `followed: Option<Followed>`, `last_reported: Option<LastReported>`, `dialog: DialogMemo`, `kept_groups`, and `watch` and `caret_read` as today. `Facts.marks` goes; the marks the mailbox copies are computed from `followed`, so `Effect::Marks` is sent when the computed value changes.

`Change`'s `follow`, `foreground`, `dialog` and `focus_path` become one `focus: Option<FocusCommit>`, and `drop_system_caret`, `absorb` and `clear_redirect` one `mark: Option<MarkChange>`, since each commit that makes one makes only that one:

```rust
enum FocusCommit {
    Adopt { followed: Box<Followed>, reported: LastReported, dialog: Option<DialogMemo> },
    /// A focus-now answer naming the focus already followed.
    Again { target: Target, owed: Option<OwedWork>, trace: TraceId, observed_at_us: u64 },
    Foreground { node: NodeId, reported: LastReported, dialog: DialogMemo },
}

enum MarkChange { Absorb(isize), DropSystemCaret, ClearRedirect }
```

Rules, each with its reason:

- Adoption replaces the record whole, so no fact of an older focus survives beside a newer one, and a focus route of an older focus cannot be read for the new one. The previous subscription moves to `kept_groups` at that moment.
- The role and batch sit outside the record because a foreground report writes them and follows nothing, and the menu rule compares with either.
- `dialog` sits outside too: a foreground report writes it for the focus that comes after. A focus-now adoption has none and leaves it.
- An element found after its report changes the record's subscription only while its node is followed; otherwise its tokens are let go.
- A caret key's watch on a node other than the one an adoption or a foreground report names is ended, in every column, because the watch's node can be a query's node rather than the followed one.

### States against rules

| Rule | Nothing followed | Followed, this node | Followed, another node |
| --- | --- | --- | --- |
| Adopt | Record set; subscription moved; a watch on another node than the adopted one ended | Record replaced whole; a subscription on the same elements is not moved; a watch on another node ended | Record replaced whole; old subscription kept by its group; a watch on another node ended |
| Again (focus-now on the focus followed) | Cannot occur: the program adopts instead | Subscription moved to its target; owed work replaced; nothing else changes | Cannot occur: programs run one at a time and an abandoned worker's commit is refused |
| Foreground report | `last_reported` and `dialog` replaced; no record; a watch on another node than the reported one ended | The same; record unchanged | The same; record unchanged |
| Element found | Tokens let go | Subscription moved to it | Tokens let go |
| Owed work taken by a worker | Nothing owed | Attempting set to this worker's generation; `absorbed` cleared for a terminal's first read, so what it observes reaches it | Nothing owed |
| Owed work done | Nothing | Cleared if this generation attempts it | Nothing |
| Worker attempting owed work abandoned | Nothing | Owed cleared | Nothing |
| Node released | Kept groups of it dropped | Subscription none, its tokens let go; the rest stays for the walks | Kept groups of it dropped |
| `MarkChange` | Cannot occur: every site tests the focus first | Applied | Cannot occur: every site tests the focus first |
| Entry that may end a held terminal's hold | Nothing | `absorbed` cleared | `absorbed` cleared |
| Mailbox's copy | Default marks | Computed from the record | Computed from the record |

### Removed

`Tracking`, `Marks` as stored state, `FollowState::{followed, followed_element, followed_slider, focus_path, owed}`, `Owed::node`, `Follow::adopt`, and seven of `Change`'s independent optional fields. `Follow` becomes the construction of a `Followed`.

### Copies kept, and why

`Followed.object` and `Held`'s newest report are separate facts: the first is what this outpost adopted and tracks, the second what Core has been sent and may still judge, and each is written by its own commit part because a foreground report or a focus-now answer can add a report without adopting.

### Costs

None added. The same facts are copied into each program; reads and calls into the application are unchanged. The worst case stays the chain and titles of a deep focus, as today.

### Limits

A focus whose origin is not the listener's read drops the previous focus's route. An event about an element of that older chain then has no route and is read by the ordinary lookup, or is treated as gone. Today the old route stays until the next listener read. `absorbed` stays a window rather than a variant of a console terminal, because it names the window the program held and is cleared by the same rule either way.

### Test

No generated test covers this today. The new one's world drives the reducer through its entries, its intake and scripted programs, in the shapes real applications produce them: focus events and foreground changes in the orders they arrive, a program whose element is found after a newer focus was adopted, focus-now answers, releases, owed work attempted, done or abandoned, and held consoles. Programs live in the outpost's `worker.rs`, outside the logic crate, so its scripted programs build `Adopt` and `Again` themselves and the generated test cannot reach the worker's choice between them; a hand-written test at the worker's level pins that choice, a control already followed under the same node being only followed and any other adopted. The reference model is written from `docs/parity/focus-events.md`, `docs/parity/terminals.md` and the recorded decisions: a list of focuses in the order the outpost read them, the elements of every report not yet released, and the work owed. It checks what the record exists to decide, none of its fields directly: a later event of the newest focus is planned and read as that focus's, an event of an older focus is not; every unreleased element is followed exactly once with its group; each token is let go exactly once; owed work is done or dropped once; a held console's updates are dropped only while the terminal is held and its first read is not under way; a caret key's watch ends when the focus moves elsewhere.

## Order of building

Each model is one commit series, its generated test committed first by an agent other than the builder, failing on the known defects:

1. The listener's record, which touches `verbatim-listener-logic`, `verbatim-listener` and one message from the supervisor, and nothing the others do.
2. The hung windows, which touch the reducer's `lib.rs` and the outpost's shell.
3. The followed focus, which touches `follow.rs`, `intake.rs` and the worker's commit sites.

The caret and selection build comes last and rebases once. Its move of the watch into `TextFacts` touches, in this code, only `end_watch_elsewhere`, `apply_watch` and the watch and text fields of `Change`, all of which stay outside `Followed`. `caret_read` and `watch` stay siblings of `followed` until that build replaces them.
