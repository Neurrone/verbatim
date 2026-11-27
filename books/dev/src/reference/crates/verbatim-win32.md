# verbatim-win32

The local window and process queries more than one of Verbatim's roles
makes: an outpost, the focus listener, the MSAA and UIA clients, and
Core's supervisor. None of them sends a window a message or calls into
another process, so each is safe on any thread, a hung application's
windows included, and none is counted (`books/dev/src/reference/performance.md`, "What
counts as a call"). Window handles are `isize`, zero for none. It is a
Windows-specific crate and depends on `verbatim-model`,
`verbatim-reader-rules` and the Windows bindings.

Public API:

- `calls` — the call gate an outpost's worker installs on its thread,
  bound to its charge (`calls::install`, the `calls::Charge` trait), and
  uninstalls as its thread ends (`calls::uninstall`), so the charge is
  dropped then and not by the thread-local destructors:
  `check()`, which every call the backends count makes first, fails with
  `CallError::Abandoned` once the worker's generation is abandoned, so
  the call is not made, its error `E_ABORT` (`ABANDONED`, `is_abandoned`)
  never read as a fact about the application. A thread with no gate makes
  every call. Beside the gate is the one per-thread count of the calls
  both backends make: `count(kind)` counts one once `check()` lets it
  through, and runs the debug-build lock check first;
  `count_connection()` marks the call just counted as a connection to a
  window by its handle; `take()` returns the thread's `CallCounts` since
  the last take and resets them, and `peek()` returns them without
  resetting. An outpost's worker takes them once around each entry.
- `held_locks` — the debug-build lock check: `hold(location)` records in
  the calling thread's eight slots that it holds a lock taken there,
  returning a `Held` whose drop empties that slot, so the record is right
  whatever order guards drop in; `check(what)` fails when any slot is
  filled, writing a message naming `what`, its place and every lock's
  place to standard error and the log, then panicking with a
  `LockHeldAcrossCall` payload; a ninth lock held at once fails the same
  way. `abort_if_lock_check(payload)` ends the process when a panic caught
  around application calls carries that payload, so no catch hides it.
  In release builds `hold` and `check` do nothing. The outpost's `Lock`
  takes the slot; the backends check at every counted call and agile reference
  ([verbatim-outpost](verbatim-outpost.md)). The check panics, and in
  debug builds aborts where a panic around application calls would be
  caught (the worker and UIA's event callbacks), after writing its message
  (Dickson, 2026-10-31). It covers the outpost's own locks and the calls,
  agile references and store releases the backends count; an object released under another
  crate's lock is not checked. Measured by `measure_lock_check` on
  2026-11-01, it costs about 60 ns per acquisition in debug builds, about
  3.9 ms over `measure_keys_notepad`'s 64,800 acquisitions, and nothing in
  release builds.

- `window` — queries by window handle. The general ones: `window_exists`,
  `window_is_visible`, `is_enabled`, `is_child`, `desktop_window`,
  `window_class_name`, `combo_box_list` (a combo box's drop-down list by
  `GetComboBoxInfo`, a message to its application), `window_thread`, `style`, `rect`, `parent`,
  `top_child`, `related` and `is_multiline_edit`; `top_level_of`,
  `root_owner_of` (the top of the owner chain, `GA_ROOTOWNER`),
  `foreground_window_handle`,
  `focus_window` (the window with the keyboard focus, else the active
  window), `thread_focus_window(thread)` (a thread's focus window alone,
  as NVDA's reads of the focus take it). `window_classes(hwnd)` reads the
  `WindowClasses` the class verdict judges
  ([verbatim-reader-rules](verbatim-reader-rules.md)).
  `window_is_hidden_frame` and `window_belongs_to_hidden_frame` recognize
  Core's hidden frame by its marker property and its owner, Core being
  the calling process's parent. `readability(hwnd)` is whether a window
  can be read, by Windows' own state, a `Readability`: `Gone` for no
  window, or one no longer, which is never judged hung; `Ghosted` when a
  ghost window stands in for it, which counts as hung (`is_hung`); `Hung`
  when the system reports its application hung; `Readable` otherwise, in
  NVDA's order, all local calls. `event_window_unreadable(hwnd)` is why an
  event's window cannot be read, if it cannot (`Readability::reason`).
  `hung_window_from_ghost(hwnd)` is the hung window a ghost window stands
  in for, by user32's undocumented `HungWindowFromGhostWindow` as NVDA
  uses it, `None` for any other window: an outpost maps the window in
  front through it, since a ghost's own process is the desktop window
  manager's, and the app maps Verbatim+Tab's focus window through it.
  `input_desktop_opens()` is NVDA's test after a desktop switch, whether
  `OpenInputDesktop` opens the input desktop now: it does on the user's
  desktop and not on the secure desktop; local calls.
  `readability` is what an outpost's and the listener's
  `WindowReadabilityReader` answers, the one judge of a hung window each
  asks at the moment of its decision, which tests replace, as a `ForegroundReader`
  replaces `GetForegroundWindow`. `top_level_windows(pid)` judges a
  window's application by the process that serves its UIA
  (`provider::served_by`). `is_another_thread_of_its_application(hwnd, other)`
  is whether `other` is a window of the same application as `hwnd` on
  another UI thread, so, with `other` the window with the keyboard focus,
  whether the user has moved on from `hwnd`;
  `front_is_another_thread_of_its_application(hwnd)` asks it of the
  window with the keyboard focus. Neither reader calls them now: Core
  judges the foreground. `is_topmost(hwnd)` and `active_window()` are the
  local reads the window facts take for Core's acceptance.
- `provider` — the process that serves a window's UI Automation
  (`books/dev/src/explanation/design/listener-without-connections.md`, section 4.3):
  `serving_process(reads, hwnd, raised_by)`, `served_by`,
  `is_top_level_window`, `parent_across_process_boundary`,
  `raw_view_parent_window`, `parent_window`, `outermost_nearest_window`
  and `plain_windows_start`. Callers in both backends use them from
  here; `verbatim-uia`'s guide describes them ([verbatim-uia](verbatim-uia.md),
  "Provider checks and the process boundary").
- The process serving a window is decided by one rule,
  `window_facts::Owner::serving_process(class, reads, raised_by)`, which
  `provider::serving_process(reads, hwnd, raised_by)` reads the window's class and
  owner for: the owner by `GetWindowThreadProcessId`, except a console
  window, whose owner is its client and whose provider is its console
  host: given the thread that raised an event on the window, a
  `WinEvent`'s, which for a console window is the host's own, the
  thread's process (`GetProcessIdOfThread`), the most direct evidence;
  with no thread, the host read from the client (`console_host_of`,
  private). The listener's `WinEvent` tells and its foreground report
  (the event-named thread of a console in front) give a thread; every
  other caller gives none. `None` for no
  window and for a console whose host cannot be read, as for a client
  that runs as another user, never the client nor 0. Every reader, the
  shell and the tools call it: the listener's tells and its route of
  Core's MSAA reads, its foreground report, the facets and `node_window`,
  the outposts' node reads, the shell's ghost and desktop fronts, the
  event recorder and the UIA walk's process-boundary step. `Owner`'s
  process is private to this crate and `window_owner` is crate-private,
  so no other crate can take a window's owning process for the one that
  serves it; a window's thread is `window::window_thread`.
- `window_facts` — the window facts a reader's tell carries for Core's
  acceptance, and a node's window, gathered with calls to Windows only:
  `WindowReads`, the reads (class, owner as an `Owner`, a console
  client's console host, parent, root, root
  owner, `IsChild`, topmost, the desktop, the foreground, the foreground
  thread's active window and the hidden frame), with `SystemReads` making
  them, and `ReadOnce`, made for one event, reading each of a window's
  own facts at most once and the foreground, the active window and
  `IsChild` each time they are asked; `WindowPart`, the window's own part read as the event arrives,
  completed by the foreground's part (`WindowPart::with_foreground`) into
  `AcceptanceFacts`, each facet's process the serving process
  (`Option<Pid>`, `None` when none can be read), the one the event's
  tell already decided passed in with the event's thread so it is not
  read twice, and a console window's thread the event's; and
  `node_window`, the `NodeWindow` a node read carries, its process the
  serving process as an `Option<Pid>` (Claude, 2026-11-27). The listener gathers its tells' facts with it, and an
  outpost's callback the facts of its followed node's window.
- `integrity` — `process_integrity(pid)` and `own_integrity()`, a
  process's mandatory integrity level, which the listener reads for each
  application and Core for itself.

Tests: the window queries against message-only windows this test
process makes, on one thread and on two, and the console tests against a
console from `verbatim-test-console`: a console window is its console
host's, never its client's, and a console window and its client's own
window are not one application.

Limitation: `provider::parent_window` and the general `window::parent`
make the same call; joining them is a follow-up.
