# ADR 0002: State models

Status: decided. Decided by Dickson on 2026-11-09, on decisions of 2026-11-06, 2026-11-07, 2026-11-08 and 2026-11-09 noted where they apply.

How these records work today is explained in [State models](../../explanation/design/state-models.md). This record keeps why they work that way: the problem, the options weighed, what was chosen and by whom, and the limits accepted.

## The problem

Three facts of Verbatim's focus path were kept in several places or as combinations of flags, and the copies could disagree, with consequences a user can hear. A new process given a reused process id inherited an ended process's abandoned readers, so its focus events could wait behind another process's readers. A background window that recovered unseen silenced the "not responding" sound for a later hang in another window. And what the outpost followed was rewritten piecemeal, so a fact of an older focus could survive beside a newer one. Each of the three is decided here: one state value, one writer. The fourth such record, Core's held-key line, belongs to the caret and selection rework.

## Decisions

### The listener's record of an application

Chosen: one record per application generation, not per process id, with one reader state. Reason: a process id can be reused, and a record keyed by it alone gives a new process an ended one's abandoned readers and, at the limit, reads nothing for the new process until those readers return.

The supervisor tells the listener of each exit, because the supervisor owns the fact. Weighed and rejected:

- Taking the end of an application's outpost for the end of the application: an outpost that crashed or retired while its application runs must keep the record and its count of abandoned readers, because the application's hang is still there.
- Closing the process's handle as soon as the exit message is written: the listener might not have read it, so a new process with that id could join the old record. The handle is kept until the listener answers.
- Having the listener ask the supervisor which generation an event belongs to: a pipe round trip for every event on the event path. The listener keeps what it has been told, and the supervisor owns the fact.

A read that returns for an exited generation is dropped, whatever it found, because once the exit has been answered a new process may have the id and what the old process said must not reach it. Events waiting for an exited process are dropped as soon as the exit is known, rather than at their bound, with the same outcome.

Being asked to let go of a reader is undone by any later event of the application, because an event means the application is running; the cost is in the limits below.

### The outpost's record of hung windows

Chosen: one hang told per application, the told window judged live. One hang is told per application because windows on one thread hang together (Dickson, 2026-11-07). A later hang in another window is told again, with the sound if the application holds attention, and a window still hung when the told window recovers is such a hang (Dickson, 2026-11-08). A window that recovers and later freezes again is a new hang (Dickson, 2026-11-09). Core plays the sound only if the application holds attention when told, so a hang met in the background is never sounded, even after the user returns (Dickson, 2026-11-06).

Weighed and rejected:

- A set of every window seen hung, told only as the set became non-empty: a background window that recovers unseen silences a later hang in another window.
- A periodic check of the told window: it needs a timer of its own, a wake-up with nothing else to do, because the existing periodic checks run only while a program or a marker is in flight. Instead the told window is judged on inputs that do work.
- Taking a commit that names a window hung at its word: a program begun before the window's destruction could then record a window that no longer exists, and the sound would play for it. Each such commit is confirmed live first.
- Comparing an abandoned worker's window with the told window to tell whether it is parked on a hang: a program whose worker names its window has no window to compare. Each program keeps its own fact.
- Separate counts of parked workers, read one after the other: a worker could be counted twice in one pong. The two reach the pong as one value.

### The outpost's followed focus

Core's focus queue changes what Core does with reports, not what the outpost needs to read and plan its own events, so every fact of the followed focus still has a reader. Weighed and rejected: removing the outpost's own facts because Core's focus is now a queue. The outpost's event planning, its live re-read and its ancestor walks read them.

Chosen: one record, replaced whole when a focus is adopted. Weighed and rejected: updating the facts by independent parts of a commit, because a fact of an older focus can then survive beside a newer one, and an event of the newer focus can be read by the older one's route.

The role and batch last reported, the dialog memo, and the absorbed console window sit outside the record, each because something other than an adoption writes it. Weighed and rejected: absorbing a held console's updates only while its terminal is the followed focus, because a terminal stays held after the focus moves elsewhere in its console, and each of its updates would then wake the outpost.

The record's object and the newest report Core may hold stay two separate facts: the first is what this outpost adopted and tracks, the second what Core has been sent and may still judge, and a foreground report or a focus-now answer can add a report without adopting.

### Measuring these builds (Dickson, 2026-11-09)

Wake-ups and processor time are measured before and after the caret and selection rework, which follows these three builds, rather than for each of them.

## Limits accepted

- A process the supervisor cannot hold has no generation boundary, so a reused id could inherit its record only while a reader is still out; the record ends once those readers return.
- An application whose outpost retired is learned to have exited at the next sweep, up to 30 seconds later, and its reader is idle meanwhile.
- A stale event in the pipe undoes a request to let go of a reader, so the reader is kept until the process exits.
- With no event or program between a window's recovery and its next freeze, the two count as one hang, since telling them apart needs a periodic check.
- A new outpost generation tells a continuing hang again, so the sound can play a second time if an outpost restarts during a hang, since nothing outlives the outpost to remember it (Dickson, 2026-11-09).
- A focus whose origin is not the listener's read drops the previous focus's route. An event about an element of that older chain then has no route and is read by the ordinary lookup, or is treated as gone.
- A held console's absorbed window stays a window rather than a state of a console terminal, because it names the window the program held and is cleared by the same rule either way.
