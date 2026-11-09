# State models

This page explains three records, each holding one lifecycle as one state value with one writer: the focus listener's record of an application, the outpost's record of hung windows, and the outpost's followed focus. Why they work this way, with the options weighed and the limits accepted, is [ADR 0002: State models](../../reference/adr/0002-state-models.md). The mechanisms are in the crate guides of [verbatim-listener-logic](../../reference/crates/verbatim-listener-logic.md), [verbatim-outpost-logic](../../reference/crates/verbatim-outpost-logic.md) and [verbatim-supervisor](../../reference/crates/verbatim-supervisor.md).

In the tables below, a cell marked "cannot occur" is a state the rules exclude. Where a step can still see such a state, it returns a logic error, never a silent default.

## Terms

- An application generation is one run of one process: a process id reused by a new process is a new generation.
- A reader is the listener's thread reading one application's UI Automation events. An abandoned reader is one whose read passed its deadline or whose user moved on, and which has not returned.
- A program is one piece of outpost work run on its worker; a commit is the one change it asks the reducer to apply.
- Telling a hang is sending Core `NotResponding`. Core plays the "not responding" sound only if the application holds attention then, so a hang met in the background is never sounded, even after the user returns.
- The followed focus is the last focus this outpost reported and adopted: the one whose caret, text and subscription it tracks.

## The listener's record of an application

### State

The listener's schedule holds two maps of records. `running` has one record per process id, and `exited` has the records of processes that have exited, by application generation. An event looks in `running` alone, so it cannot reach an exited record, and no list of current processes has to agree with the records. Every input, effect and job that names an application names its generation, so a returning thread of an exited generation cannot wake, forget or match a new generation with the same id.

A running record has a generation, a life and what is kept whatever its state. Its life is live, or ending once the supervisor has asked that the reader be let go when it has nothing to do. Within that, its reader is one of three states:

- Idle: no thread in charge, nothing queued, and one or more abandoned readers still out.
- In charge: one thread in charge, fewer than the limit abandoned, with the read under way and the events waiting.
- Starved: the limit abandoned since a time, no thread in charge, and events may wait.

An exited record has only a draining reader, idle or in charge, because no event can reach it.

What is kept whatever its state is the place of the newest event that moved away from the application, the top-level window of its last read, and the numbers a new thread's work continues from, so a thread that has lost charge can never match a later one of its record. Which thread is in charge is the charge's alone.

Rules, each with its reason:

- A record with no thread in charge, no reader out and nothing queued does not exist: it is dropped at the step that reaches that, since what it held serves only a thread's last read and no thread is left. So neither map grows with every process.
- Starved carries no count, since it means exactly the limit. The limit is reached only through starved, so a live idle or in-charge reader is below it. The one place the count rises checks it.
- Ending is turned back to live by any event of the current generation, because the application is running.
- A process's exit moves the record to `exited`. A new event then starts a new generation with no abandoned readers. An outpost that crashed or retired with its application running keeps the record and its count, because the application's hang is still there.
- A read that returns for an exited generation is dropped, whatever it found, because once the exit has been answered a new process may have the id.
- A read reports its top-level window once, when it finds it, and a successful return keeps it as the record's last. The element's own window, which the reading thread keeps, is another fact.

### How the listener learns of an exit

The supervisor owns the fact. `forget_app`, the one place it learns a held process has exited, including a retired outpost's application found by its 30-second sweep, sends the listener `ApplicationExited`. It keeps the process's handle until the listener answers `ExitHandled`, because Windows reuses no process id while a handle is open: no process with that id can exist until the listener has moved the record. With no listener running, or one still launching, the handle closes at once and nothing is sent, since a new listener has no record of the process.

Every process a listener has a reader for is held. An application that sends only facts that start no outpost, such as background selections, is held from its first fact so that the sweep finds its exit. A process the supervisor cannot hold never gets the message; `HookRemoved`, sent for it, ends its reader.

### States against rules

| Rule | Live Idle n | Live InCharge n | Live Starved | Ending Idle n | Ending InCharge n | Ending Starved |
| --- | --- | --- | --- | --- | --- | --- |
| Event pushed, key current | InCharge n, a new thread | Queued, its thread woken | Queued; past since plus deadline, every waiting event dropped | Live, then as Live Idle | Live, then as Live InCharge | Live, then as Live Starved |
| Thread asks next, queue has an event | Not in charge, nothing | Job given | Not in charge, nothing | Not in charge, nothing | Job given | Not in charge, nothing |
| Thread asks next, queue empty | Not in charge, nothing | Thread waits | Not in charge, nothing | Not in charge, nothing | n is 0, record dropped; otherwise Ending Idle n | Not in charge, nothing |
| In-charge read returns | Cannot occur | Outcome sent, last top-level window kept, stays | Cannot occur | Cannot occur | Outcome sent, last top-level window kept, stays | Cannot occur |
| Abandoned read returns | n less one; at 0 dropped | InCharge n less one | Queue empty, Idle at limit less one; otherwise InCharge at limit less one, reusing the returned thread | n less one; at 0 dropped | Ending InCharge n less one | As Live Starved, staying Ending |
| Read abandoned | Cannot occur | n plus one: at the limit, Starved at now; otherwise queue waiting, InCharge n plus one with a new thread, queue empty, Idle n plus one | Cannot occur | Cannot occur | As Live InCharge, staying Ending | Cannot occur |
| Alarm at since plus deadline | Nothing | Nothing | Waiting events dropped, stays | Nothing | Nothing | Waiting events dropped, stays |
| Thread failed to start | Not in charge, nothing | Queue dropped; n is 0, dropped; otherwise Idle n | Not in charge, nothing | Not in charge, nothing | As Live InCharge, staying Ending | Not in charge, nothing |
| Outpost ended | Ending Idle n | Ending InCharge n, thread woken | Ending Starved | Nothing | Nothing | Nothing |
| Application exited | Exited Idle n | Exited InCharge n, thread woken | Queue dropped at once, Exited Idle at the limit | Exited Idle n | Exited InCharge n | Queue dropped at once, Exited Idle at the limit |
| Another application's focus or menu opening, or an MSAA focus | Marks moved away | Marks moved away | Marks moved away | Marks moved away | Marks moved away | Marks moved away |
| Foreground event | Marks moved away if the last read's window differs | Marks moved away if the read's window, or else the last read's, differs | Marks moved away if the last read's window differs | As Live Idle | As Live InCharge | As Live Starved |

The "cannot occur" cells follow from a read existing only under a thread in charge. An exited record follows the Ending column of the same reader, with these differences: an event cannot reach it; the outpost's end and the exit change nothing; a read abandoned to reach the limit leaves it idle at the limit with its queue dropped, rather than starved; and an in-charge read that returns is dropped. An abandoned read of an exited generation decrements that record, never the new generation's.

### Copies, and costs

The supervisor's list of applications and the listener's `running` both know which processes live. The supervisor owns the fact, and `running` is what it has told the listener, because the listener stamps every event with a generation and cannot ask across a pipe for each one.

An exit costs two pipe writes, the exit and its answer, and a wake of each side per held process. An application first seen through a fact that starts no outpost costs one process open, or one failed open per such fact for a process that cannot be opened. Each read takes the listener's schedule lock once more, to report its top-level window, which is on the event path and is recorded in [the performance ledger](../../reference/performance.md). There is no new timer and no call into another process.

## The outpost's record of hung windows

### State

The reducer holds one value per outpost: no hang told, or told of one window. It ends with the outpost generation, as all reducer state does.

Rules, each with its reason:

- One hang is told per application: a second window seen hung while the told window is still hung is the same hang, since windows on one thread hang together.
- A later hang in another window is told again, plays the sound if attended, and replaces the told window. A window still hung when the told window recovers is such a hang, sounded once when next seen if the application holds attention.
- While a hang is told, the told window is judged live, by the outpost's one judge of whether a window is hung (a local call that counts a ghosted window as hung), at most once per reducer input, on an input that takes an entry, applies a commit or starts a program. The finish of a cut read continues the program already started, so it does not count. The answer names the window judged and any window waiting on it, so an answer for a window no longer told changes nothing. A not-hung answer ends the hang, so a window that recovers and later freezes again is a new hang.
- Every commit that names a window hung is confirmed live before it counts, so a program begun before a window's destruction cannot record it, as a destroyed handle answers not hung. A commit ends no hang itself.
- Whether a program's worker is parked on a hung window is one fact of that program, its hang mark, written once and read at its abandonment, because a program whose worker names its window has no window to compare with the one told.
- The two counts of parked workers reach the pong as one value, written together when a worker is parked or returns and read once per ping, so each parked worker is counted once. The lock that guards it is held only for that assignment. The supervisor's copy is the value each pong replaces.

### States against rules

| Rule | Nothing told | Told of this window | Told of another window |
| --- | --- | --- | --- |
| Hang seen of this window | Told of it; `NotResponding` sent | Nothing | The told window judged, this window waiting on the answer |
| An entry taken, a commit applied or a program started, the first on this input | Nothing | The told window judged | The told window judged |
| This window judged still hung | Nothing | Nothing: the same hang | Nothing: the hang it judged has ended |
| This window judged not hung, no window waiting | Nothing | Nothing told | Nothing: the hang it judged has ended |
| This window judged not hung, another window waiting | Nothing | Told of the waiting window; `NotResponding` sent | Nothing: the hang it judged has ended |
| Commit names this window hung | Confirmed live | Confirmed live, its hang seen then changing nothing | Confirmed live |
| Window in front readable | Nothing | Nothing told | Nothing |
| Window's destroy event, as it first reaches the reducer | Nothing | Nothing told | Nothing |
| Program's check finds its window hung | As hang seen, and the program's hang mark set | As hang seen, hang mark set | As hang seen, hang mark set |
| Worker abandoned | Parked as its hang mark says | Parked as its hang mark says | Parked as its hang mark says |

### Copies, and costs

The shell's map of each parked worker and whether it is on a hung window is the writer's own: it says what to take away when a worker returns. The one value in the pong is the total the pipe reader needs, which cannot touch shell state, and both are written in one step.

While a hang is told, each reducer input that judges it costs one local window call; with no hang told it costs none, so nothing is added on the normal path. A commit naming a window hung adds one to confirm it, and when another window is told a third, to judge the told window with the named window waiting. There is no new timer, no new wake-up and no call into the application.

## The outpost's followed focus

### State

Core's queue changes what Core does with reports, not what the outpost needs to read and plan its own events, so every fact of the followed focus has a reader. They are one record, replaced whole when a focus is adopted: the node, its role, object, window, chain and titles, how it was read, its terminal mark, its redirect and system-caret marks, its subscription and what it owes. How it was read is the listener's read with its focus route, an MSAA focus with what a live re-read needs, a focus-now answer, or an MSAA menu opening.

Beside the record are the role and batch last reported, the memo of the dialog text the last foreground report gathered, the absorbed console window, the groups kept for earlier focuses, and the caret key's watch. The marks the mailbox copies are computed from the record and the absorbed window, and sent when the computed value changes. A commit makes at most one change of the focus and one change of a mark.

Rules, each with its reason:

- Adoption replaces the record whole, so no fact of an older focus survives beside a newer one, and a focus route of an older focus cannot be read for the new one. The previous subscription keeps its group among the kept groups at that moment.
- The role and batch sit outside the record because a foreground report writes them and follows nothing, and the menu rule compares with either.
- The dialog memo sits outside too: a foreground report writes it for the focus that comes after. A focus-now adoption has none and leaves it.
- The absorbed window sits outside too: it is a held terminal's window and not a fact of the followed focus, because a terminal stays held after the focus moves elsewhere in its console, and keeping the mark then keeps one absorb dropping the updates at the mailbox instead of a wake for each. An adoption clears it, as do an entry that may end the hold and a terminal's first read.
- An element found after its report changes the record's subscription only while its node is followed; otherwise its tokens are let go.
- A caret key's watch on a node other than the one an adoption or a foreground report names is ended, in every state, because the watch's node can be a query's node rather than the followed one.

### States against rules

| Rule | Nothing followed | Followed, this node | Followed, another node |
| --- | --- | --- | --- |
| Adopt | Record set; subscription moved; a watch on another node than the adopted one ended; absorbed window cleared | Record replaced whole; a subscription on the same elements is not moved; a watch on another node ended; absorbed window cleared | Record replaced whole; old subscription kept by its group; a watch on another node ended; absorbed window cleared |
| Focus-now answer on the focus followed | Cannot occur: the program adopts instead | Subscription moved to its target; owed work replaced; nothing else changes | Cannot occur: programs run one at a time and an abandoned worker's commit is refused |
| Foreground report | Last reported and dialog memo replaced; no record; a watch on another node than the reported one ended | The same; record unchanged | The same; record unchanged |
| Element found | Tokens let go | Subscription moved to it | Tokens let go |
| Owed work taken by a worker | Nothing owed | Attempting set to this worker's generation; absorbed window cleared for a terminal's first read, so what it observes reaches it | Nothing owed |
| Owed work done | Nothing | Cleared if this generation attempts it | Nothing |
| Worker attempting owed work abandoned | Nothing | Owed cleared | Nothing |
| Node released | Kept groups of it dropped | Subscription none, its tokens let go; the rest stays for the walks | Kept groups of it dropped |
| A console's updates absorbed | Applied: it names a held terminal's window and needs no record | Applied | Applied: the terminal held may be another node's, and it is cleared by the same rules |
| The system caret dropped, or a redirect cleared | Cannot occur: every site tests the focus first | Applied | Cannot occur: every site tests the focus first |
| Entry that may end a held terminal's hold | Absorbed window cleared | Absorbed window cleared | Absorbed window cleared |
| Mailbox's copy | Default marks with the absorbed window | Computed from the record and the absorbed window | Computed from the record and the absorbed window |

### Copies, and costs

The followed record's object and the newest report Core may hold are separate facts: the first is what this outpost adopted and tracks, the second what Core has been sent and may still judge, and each is written by its own part of a commit because a foreground report or a focus-now answer can add a report without adopting.

Nothing is added: the same facts are copied into each program, and reads and calls into the application are unchanged. The worst case stays the chain and titles of a deep focus.

## How each is tested

Each record has a generated test that compares it after every step with a much simpler reference model, described in its crate's guide: the listener's schedule in `crates/verbatim-listener-logic/src/schedule/generated.rs`, whose world has applications that start, exit and reuse an id, and readers that return, hang or are slow; the hung windows in `crates/verbatim-outpost-logic/src/tests/reducer.rs`, whose world holds each window's true condition, changing with no event; and the followed focus in `crates/verbatim-outpost-logic/src/tests/followed.rs`, which drives the reducer through its entries, intake and scripted programs. A hand-written test at the worker's level pins the choice between adopting a focus and following it again, which the generated test cannot reach.
