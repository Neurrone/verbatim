# M4.2: parity small items

Status: approved by Dickson on 2026-11-09. Two builds, from `docs/roadmap.md`, M4.2. Each is built after the state models and the caret and selection rework. The exact speech is from NVDA's captures of 2026-11-09, at the pinned build and under Eloquence, so it is NVDA's words whatever its synthesizer. Only table cells wait on a capture, which their fixture makes possible.

Every design here makes invalid states unrepresentable and parses at each boundary: a message carries a type that can hold only what is valid, and no later code checks it again.

## Terms

- Core is Verbatim's main process and decides what is spoken. An outpost is the helper process for one application. The focus listener is the process holding the desktop-wide subscriptions.
- A round trip is one cross-process call into the application. A remote program is a small UIA program run inside the application, so that many reads cost one round trip.
- The navigator is the object the review cursor is on. A dead object is one whose element no longer exists in the application.

## Decisions

Dickson's, each with its date:

- Dead objects follow NVDA throughout (2026-11-09), replacing the re-seeding rule of 2026-10-08. Only a focus, the to-focus command and a successful object navigation move the navigator. "No navigator object" is said only when there is no navigator. Only a focus moves NVDA's navigator (`nvda/source/NVDAObjects/__init__.py` 1329), and its absence is the only source of that message (`nvda/source/globalCommands.py` 1517 to 1520).
- A foreground UIA progress bar that is not the focus is subscribed by the foreground application's outpost on its top-level window, with one re-subscribe per foreground change (2026-11-09), so only the attended application's events wake anything.
- The toast's window-opened subscription is desktop-wide in the listener, and its scenario turns Do Not Disturb off for the run and restores what it found (roadmap, 2026-11-03).
- Review commands read live from the application (2026-10-08), at the cost of a round trip per command.
- Review presses have no queue or count limit of their own (2026-11-09). They join the caret build's key line, and what bounds them is what bounds keys: a time bound on the entry at the front.

Claude's, each with its reason:

- Toasts are built unless the capture shows Verbatim already speaks a Windows 11 toast as NVDA does through the MSAA alert, in which case only the rule dropping MSAA alerts from native UIA windows is left out with it, since the two go together in the roadmap.
- A toast's repeat within one second is dropped in Core by a hash of its runtime id, computed by the outpost, because a node id is released with its node, so a repeat would have a new one, and Core needs only equality.
- The foreground outpost is told by Core and judges nothing itself, because Core owns attention and a second judge would be a copied fact.
- Value changes in the window's subtree use a new event group holding only the range value, because the existing value group also carries the value pattern's changes and would wake the outpost for every character typed in a field of that window.
- A range value change from anything but a progress bar is dropped on the UIA callback thread from the event's cached role, with nothing woken, because no rule speaks it.
- The table's identity is a hash of its runtime id, computed once by the outpost, because Core needs only equality and a retained node would have to be held.
- A dead object reviews as an object with empty text, because NVDA's name and value then read as empty (`nvda/source/NVDAObjects/__init__.py` 1450 to 1463), with one rule at the boundary rather than one per command.
- The roadmap's "the outpost reports held MSAA nodes it forgets" is dropped, because it existed only to re-seed the navigator.
- A navigator node keeps its element in the outpost, because a live element with neither a kept element nor a focus route answers gone today (`resolve_uia_element`, `crates/verbatim-outpost/src/outpost/read.rs`), which would now be spoken as a dead object.
- A review press in progress with something behind it is bounded by 150 ms (`HELD_READ_BOUND_MS`), the key line's bound on a read before a key, and with nothing behind it by its read's own deadline, so that a press alone is always answered and no key waits long behind one. When the bound passes the press says nothing and the line moves on.
- A review read answered `Unanswered` says nothing for that press and the next entry runs, as NVDA's next queued script runs after a failed one (`nvda/source/scriptHandler.py` 234 to 252).
- A press whose read is running when a focus moves the navigator says nothing, because its words would follow the focus's, and the entries behind it run on the navigator as it then is, as NVDA's queued scripts do: a focus does not empty its queue (`scriptHandler.py` 242 to 252).
- A review press that finds a `Released` key replaces it, as a held key does, because the press cuts speech and the key's bound answer would be cut with it.
- A press with its own read does not seed first: a move's read starts from the cursor and is the live read, so each press costs one round trip.
- A generated test is added only where interleavings matter: review presses among keys, in the caret build's model of the key line.

## Build one: toasts, progress bars, controlled lists, table cells

### Order

Toasts follow the listener's record, since they add to the listener's reading queue. The progress bar follows the followed focus, since it moves the outpost's subscriptions. Table cells depend on nothing. The controlled-list items follow the caret build, which changes `settings_search_suggestions`' expectations for the arrows.

### Toasts

Where it runs: the subscription and its test on the listener's UIA callback thread; the reading on the application's reading thread, as a selection or notification is; the report in the toast host's outpost; the speech decision in Core.

- The listener adds one more part, `UIA_Window_WindowOpenedEventId`, to its desktop-wide registration, shaped as `ListenerEvent`, on the same client as its other parts.
- Its callback keeps only a window whose control type is window and whose automation id contains `ToastView`, from the event's cache, and whose window class is `Windows.UI.Core.CoreWindow`, by a local call (`nvda/source/NVDAObjects/UIA/__init__.py` 1282 to 1287).
- It goes through `queue_for_reading` as `FocusEvent::Toast`, so it is ordered with the application's other events and dropped from a hung window.
- The outpost reports it as `NormalizedEvent::Alert`, spoken queued from any application, as `reduce_alert` does today.
- MSAA alert and show events from a native UIA window are dropped, as NVDA drops every `WinEvent` from one (`nvda/source/IAccessibleHandler/__init__.py` 577 to 582). This runs on the outpost's worker when it handles the event, from the arbitrator's verdict for the window (`known_to_use_uia`, `crates/verbatim-outpost/src/arbitration.rs`), which is already kept per window. A window with no verdict yet is probed as a focus's is, once, on a deadline-guarded worker.
- Core drops a toast whose id equals the last toast's within one second (`nvda/source/NVDAObjects/UIA/__init__.py` 2826 to 2834).

Types:

```rust
// DeliveredFact
UiaToast { hwnd: isize, snapshot: NodeSnapshot, id: ToastId },

// NormalizedEvent::Alert { node, origin }
enum AlertOrigin { MsaaAlert, ToastWindowOpened(ToastId) }  // a toast cannot be told without its id
struct ToastId(u64);

// Core: only a toast is compared, so only it is remembered
struct LastToast { id: ToastId, at_us: u64 }
```

`SrState` gains `last_toast: Option<LastToast>`. Its rules:

| State | Input | What it does |
|---|---|---|
| None | toast | Speaks; records it. |
| Some | toast, same id, within one second | Drops it; leaves the record, so a third is measured from the spoken one, as NVDA records only one it speaks. |
| Some | toast, otherwise | Speaks; replaces the record. |
| Any | MSAA alert | Speaks; the record is untouched. |

Costs: the listener's callback runs once per window opened anywhere and wakes nothing else unless the window is a toast. One round trip at most to read the toast, on the reading thread. The MSAA drop is a lookup of a kept verdict on the outpost's worker, with at most one probe per window. No new timer.

Limits: a toast whose host is restarted within one second has a new runtime id and is spoken. Windows 8.1 and tooltip-opened toasts are out of scope, since Verbatim supports Windows 11 only.

Speech: NVDA's `speakObject` with the focus reason (`nvda/source/NVDAObjects/behaviors.py` 1008 to 1011). One Windows 11 toast raised from PowerShell was heard as "New notification from Windows Power Shell, Capture toast title, Capture toast body dot dot 1 of 1 window". The capture run reported those words and did not keep their log, so the `toast` scenario captures them again in compare mode. Whether Verbatim's MSAA path speaks a toast today is checked first in this build.

### The foreground progress bar

Where it runs: the decision in Core; the state in the outpost's reducer; the lookup of the window's element in a worker program; the subscription in the outpost's shell; the role test on the UIA callback thread. Core's existing progress indication speaks it.

Core sends the outposts one message when attention changes, and to an outpost that starts while its application is attended:

```rust
// CoreToOutpost::Front(FrontWindow)
enum FrontWindow { In(isize), Left }
```

An application that gains attention gets `In` with its foreground window. One that loses it gets `Left`. A change between two windows of one application is one `In`.

The outpost's state, one value in the reducer, ended with its generation:

```rust
enum BarWatch {
    None,
    Finding { window: isize },
    Subscribed { window: isize, element: u64 },
}
```

| State | Input | What it does |
|---|---|---|
| None | `In(w)` | `Finding(w)`; a program finds the element. |
| None | `Left` | Nothing. |
| None or Subscribed | `Found`, any window | Lets its token go; nothing else. |
| None or Subscribed | `FindFailed`, any window | Nothing. |
| Finding(w) | `Found { window: w, token }` | `Subscribed(w, token)`; the shell subscribes its subtree. |
| Finding(w) | `Found` of another window | Lets its token go; nothing else. |
| Finding(w) | `FindFailed { window: w }` | `None`. |
| Finding(w) | `FindFailed` of another window | Nothing: it is an earlier lookup's answer. |
| Finding(w) | `In(w)` | Nothing. |
| Finding(w) | `In(x)` | `Finding(x)`; a program finds the element. |
| Finding(w) | `Left` | `None`. |
| Subscribed(w) | `In(w)` | Nothing. |
| Subscribed(w) | `In(x)` | Withdraws the subscription and lets its token go unless the focus subscription follows it; `Finding(x)`. |
| Subscribed(w) | `Left` | Withdraws the subscription and lets its token go unless the focus subscription follows it; `None`. |

The shell gives one element one token, so the foreground window can share `BarWatch`'s token with the focus. `follows` (`crates/verbatim-outpost-logic/src/follow.rs`) includes `BarWatch`'s element, and letting either go is subject to the same rule, so neither drops the other's subscription.

- A new event group, `Group::RangeValue`, holds only `UIA_RangeValueValuePropertyId`, as NVDA's global group does (`nvda/source/UIAHandler/__init__.py` 241 to 245, 598 to 603). The registration's scope is `Scope::Subtrees` of the window's element.
- The callback drops every role but progress bar from the event's cache. A progress bar's change takes the existing value-change path, which reads states, value and location for `ProgressReading`.

Speech: the tone, by Core's existing rule (`reduce_progress_changed`, by the percentage at each place). Off-screen and non-numeric bars are ordinary value changes, as today.

Costs: one element lookup and one registration per foreground change, in a worker program. One read per progress change in the attended application only. No new timer.

Limits: a focused progress bar is reported by both its focus subscription and this one, so its value is read twice per change, and Core's one-percent rule keeps the second silent. A foreground window whose subtree holds no progress bar still pays the registration.

### Lists the focus controls

No state is added. NVDA's results below were captured on mockapp's list fixtures, and the outpost's remote program that searches the elements the relation names is changed to match.

- A selection raised on the controlled list itself: "Suggestions list selected". The search includes the named element itself, since NVDA's `NormalizeElement` answers the element itself (`nvda/source/NVDAObjects/UIA/__init__.py` 2444 to 2461).
- A selection in a second controlled element: "Elsewhere", the event's element being reported. mockapp's `controller_for` holds one target, so this was approximated by switching it to the second list with `set-controller-for`; a selection in a list the focus does not control then says nothing.
- A selection left stale by a replacement of the list, the first or a second: nothing is said. The fallback to the list's current selection goes, and `settings_search_suggestions` is run against NVDA's capture in this build, since that fallback was added for it.
- An ordinary selection after a replacement: "Sound settings  1 of 2".

### Table cells

Where it runs: the read in the outpost, in the same read as the cell; the rule in Core, written from the parity docs.

```rust
// NodeDetails, replacing row and column, which only ever came together
cell: Option<CellPosition>,
struct CellPosition { row: u32, column: u32, table: Option<TableId> }
struct TableId(u64);

// SrState: NVDA's three separate memories, each valid alone
struct CellMemory { table: Option<TableId>, row: Option<u32>, column: Option<u32> }
```

| Memory's table | Input | What it does |
|---|---|---|
| Equal to this cell's | a cell spoken | Says the row only if it differs from the memory's, and the column likewise; records what it said. |
| Different or none, or this cell has no table | a cell spoken | Says both; records them; records the table if this cell has one (`nvda/source/speech/speech.py` 2087 to 2136). |
| Any | Report Current Object on an object with row or column counts | Says them after its other properties, as "with 3 rows and 2 columns"; the table becomes none, so the next cell says both (2145 to 2152). |

NVDA reads the counts only for a query, not at a focus (`speech.py` 991 to 993), so entering a table by focus clears nothing and is told apart by its table's identity. The counts come from the object's own Grid pattern (`nvda/source/NVDAObjects/UIA/__init__.py` 2310 to 2321), read only in Report Current Object's read.

The outpost reads `GridItem.ContainingGrid`, which is an element, and that element's runtime id, in a step of the cell's read that only a cell with the `GridItem` pattern takes, while the theme reports cell coordinates (`nvda/source/NVDAObjects/UIA/__init__.py` 2322 to 2334). The shapes hold only plain properties (`crates/verbatim-uia/src/shape/shapes.rs`), so this is a step of its own in the remote program and two calls in a classic read. A cell reported by the listener's selection (`ListenerSelection`) takes the same step, as NVDA reads the table for every cell it speaks.

Costs: the build starts with a prototype measuring the cell read's remote instructions and calls, because no program reads this property today. Expected: one more step in a remote read, and two more calls in a classic read, only for a cell while the theme reports coordinates, which applies to a selection anywhere that is a cell.

Limits: row and column spans are not read, as today, so "through" is not said. A hash collision between two tables leaves out a row or column, and needs equal 64-bit hashes of two runtime ids in a row.

Speech: entering a table with cell 2, 3, then the next cell in that row: "row 2", "column 3", then "column 4". The captures had no table fixture, so this build first captures NVDA on the Alt+Tab switcher, whose items carry a row and column indication (Dickson, 2026-11-09), and gives mockapp a grid and grid items with a containing grid, and captures two cells in one row, two in one column, and another table and back. Those expectations wait on that capture.

### Tests

Hand-written, each table cell above a case: `crates/verbatim-core/tests/` for the toast record, `AlertOrigin` and `CellMemory`; the listener's callback test against each field of NVDA's toast rule; the drop of MSAA alert and show events from a native UIA window, and not from another; the outpost's reducer for every `BarWatch` cell, with `follows` covering a token shared with the focus; mockapp for the range-value group, the role test on the callback and the cell's table read; call-count tests for a cell, classic and remote. No generated test: `BarWatch`'s cells are independent, and `LastToast` is one comparison of ids. Scenarios: `toast`, a new `foreground_progress_bar` with `settings_search_suggestions`, a new table cell scenario, and steps for the three list cases in mockapp.

## Build two: the review cursor on objects that no longer exist

### Order

After the caret build, which changes `Then` and `TextOp`.

### How it works

Where it runs: the state and every word in Core; reads in the outpost's worker; the navigator's element kept in the outpost's store.

- Every review command reads live. The `grid` guard in `review_text::run` goes: a command that answers from the line reads it first, as a terminal's does today, and a move goes straight to the read that moves, which is already live, so a press costs one round trip. A column remembered across a short line is kept for every text, not only a grid, so that Down Arrow returns to it.
- Flat review and Report Current Object read the navigator's name and value live, by a new query.
- Flat review's current line leaves the offset where it was (`nvda/source/globalCommands.py` 1975 to 1990 sets no position). `ReviewStartOfLine` keeps landing at the line's start.
- A terminal's review position stays the provider's, with no bookkeeping of Verbatim's own, so once the history is full staying on the same row is acceptable (Dickson, 2026-10-08).

Messages:

```rust
// A new query: Query::ReadNode { node_id }, answered QueryResult::Node(NodeSnapshot) or QueryOutcome::Gone
```

Core's state changes in one place. A review press is an entry of the caret build's key line, so presses, held keys and commands are one ordered line, as NVDA's scripts are one queue (`nvda/source/scriptHandler.py` 242 to 252, `nvda/source/inputCore.py` 648), and no second queue exists. A press waiting is the `WaitingKey::Command` the line already has.

```rust
// CurrentKey gains
Review(Press),

struct Press { node: NodeId, query: QueryId, press: ReviewPress }
struct ReviewPress { trace_id: TraceId, command: ReviewCommand, repeat: u8 }

// the query's continuation: Then::Review(TextFollowUp), replacing the review text uses of Then::Text
```

A press stays current until the answer to its last read, a follow-up read replacing `query`, so a press cannot end before its move. Object-navigation commands are presses too. The key line's advance stops at a review press as at a held key, instead of running it and moving on, which is what keeps a later Down Arrow behind it. `held_nodes` and the outpost's ending find it as the current key.

The bound is the key line's. The entry at the front has a time bound, and there is no limit on the line's length. A press with nothing behind it has its read's own deadline. A press with anything behind it, armed when the first entry joins it or when it becomes current with entries waiting, has `HELD_READ_BOUND_MS`, 150 ms, the held key's one deadline. A held key therefore waits at most that 150 ms before its own before read starts, and Alt+Tab out of a stuck application is not delayed further.

| State | Input | What it does |
|---|---|---|
| Idle, or advance reaches a press | review command | Becomes `Review`, asks the read. |
| `Review` | any key or command arrives | Joins `waiting`; arms 150 ms if it was empty. |
| `Review` | answered with a read | Runs the press against what was read. A follow-up read replaces `query`; otherwise the press ends and the line advances. |
| `Review` | `Gone` | Runs the press on empty flat text, or the edge rule below for object navigation; ends. |
| `Review` | `AnchorLost` | Starts again from the caret, as today, and stays. |
| `Review` | `Unanswered`, or the node's outpost ends | Says nothing for the press; ends. |
| `Review` | deadline fires | Withdraws the query, says nothing for the press, and leaves the position as it was; ends. |
| `Review` | a focus moves the navigator | Withdraws the query, says nothing for the press; ends. The entries behind run on the navigator as it then is. |
| `Watched`, `Holding` | review command | Joins `waiting`, as a command does in the caret build. |
| `Watched`, `Released` | review command | Replaces the key as a held key does, sending `End` in a text field; becomes `Review`. |

Every other row of the key line's table applies to `Review` as to any busy state: a report's selection changes are said at once, and exit passes every held key on. A key held down in a slow application queues its repeats, each giving way after 150 ms, and each answered read speaks in press order, so what is heard is nothing until the application answers, then the presses it answered in time.

Object navigation, one rule at the boundary: `FetchResult::Gone` and `FetchResult::NoNeighbor` speak the direction's edge message, "No next", "No previous", "No objects inside" or "No containing object" (`nvda/source/NVDAObjects/UIA/__init__.py` 2172 to 2196; `nvda/source/globalCommands.py` 1778 to 1781), and the navigator stays. Parent from a dead element answers the parent of the element's window (`GetAncestor` with `GA_PARENT`), the navigator moving there, as NVDA's walker fails and it falls to the window's parent (`nvda/source/NVDAObjects/UIA/__init__.py` 2136 to 2145, `nvda/source/NVDAObjects/window/__init__.py` 287 to 293). Parent from a dead window says "No containing object". `FetchResult::Unanswered` still leaves the navigator put and says nothing. `TextReply::Gone`, dropped silently today, takes the `Gone` row above.

The outpost keeps the element of every node Core holds as the navigator, released by the existing release rule, so that `resolve_uia_element` says gone only of an element that has gone.

Removed: `request`'s superseding of an earlier read; `navigator_to_focus`'s use on a gone navigation, and its comment; flat review's read of `navigator.object` for its text; "Navigator death recovery" as a difference in `docs/parity/review.md`.

### Exact speech

Captured on mockapp's disconnected UIA nodes, whose calls fail as a destroyed element's do. The navigator is on a dead button or a dead static text, which the application removed, and the focus is elsewhere:

- Next Object: "No next". Previous Object: "No previous". First Child: "No objects inside". The navigator stays each time.
- Parent: the window's parent, here "Desktop window", and the navigator moves there.
- Report Current Object, once and twice: nothing.
- Review current line, word and character: "blank".
- Review next line: "Bottom", "blank", as on one empty line.
- The to-focus command: "Move to focus", then the focus, here "Holder 2  button".

With the navigator on a window that has closed: "No next", "No previous", "No objects inside", "No containing object", and Report Current Object says nothing.

A destroyed classic edit control was approximated, since mockapp cannot destroy a window: an MSAA edit node was disconnected. Next, Previous and First Child as above; Report Current Object "unknown", and the second press nothing more; Parent the MSAA parent, here "Controls list"; review line, word and character "blank".

Flat review's current line leaves the position where it was: "h", the line, "h", and Next Character then "e".

### Costs

One round trip per review command that answered from Core's cache, one remote program where the text is read remotely. A move costs the one read it costs today. In a hung application a press alone says nothing until its read's deadline, and one with something behind it for 150 ms. One kept element per navigator node. One new variant of the key line, no new timer, and no wake-up on the event or speech path.

### Limits

- A press that gives way at its 150 ms says nothing and leaves the position, so a key held in a slow application loses the moves its presses would have made. NVDA would answer them late, in order.
- A review position in text the application has rewritten follows the provider's position, not the words that were there.

### Tests

Hand-written: Core's cases per table cell above and per direction of `FetchResult::Gone`; a remembered column across a short line in plain text; mockapp through a real outpost with the node destroyed for navigation, flat review and text review; a call-count test that a review command is one round trip; the keeping of the navigator's element.

Generated: the Core model of the caret build gains review presses, written from `docs/parity/review.md` and Dickson's dated decisions, not from `Press`: its world presses review commands among keys and focus reports, and answers reads early, late, gone or never. It checks after each step that each press is answered at most once and in press order among all input, and that a press waiting when the navigator moves acts on the navigator as it is when its turn comes, as NVDA's queued scripts do.

Scenarios: a new `review_dead_object`, and the roadmap's end-to-end test in both terminals that a reviewed line stays on its text as new output is written below it without filling the history, as `windows_terminal_review_stays_on_line` and `conhost_review_stays_on_line`.

## Documents

`docs/parity/alerts.md`, `changes-and-selection.md`, `focus-announcement.md` and `review.md`, the crate guides for `verbatim-core`, `verbatim-outpost`, `verbatim-listener` and `verbatim-uia`, `books/dev/src/reference/test-coverage.md`, and `docs/roadmap.md`'s M4.2 entry, which loses the re-seeding rule.

## Open for Dickson

None. A capture that shows a difference from what is written here is matched, and any it shows that NVDA has no answer for comes back to him.
