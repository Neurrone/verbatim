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
  never read as a fact about the application; and
  `message_readability(hwnd)` tests a window message's window with the
  outpost's reader, telling the charge of a window found hung. A thread
  with no gate makes every call and asks the system.
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
  `window_class_name`, `window_thread`, `style`, `rect`, `parent`,
  `top_child`, `related` and `is_multiline_edit`; `top_level_of`,
  `root_owner_of` (the top of the owner chain, `GA_ROOTOWNER`),
  `window_owner` (thread and process), `foreground_window_handle`,
  `focus_window` (the window with the keyboard focus, else the active
  window). `window_classes(hwnd)` reads the
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
  `readability` is what an outpost's and the listener's
  `WindowReadabilityReader` answers, the one judge of a hung window each
  asks at the moment of its decision, which tests replace, as a `ForegroundReader`
  replaces `GetForegroundWindow`. `served_by(hwnd, pid)` and
  `top_level_windows(pid)` judge a window's application by the process
  that serves its UIA. `is_another_thread_of_its_application(hwnd, other)`
  is whether `other` is a window of the same application as `hwnd` on
  another UI thread, so, with `other` the window with the keyboard focus,
  whether the user has moved on from `hwnd`: the test both readers'
  `MOVED_ON_GRACE` is judged by, the outpost passing the window its focus
  window reader gives and the listener calling
  `front_is_another_thread_of_its_application(hwnd)`, which reads the
  system's. `SystemWindows` answers the acceptance
  test's `WindowQueries` with local calls, with the foreground window its
  caller read.
- `provider` — the process that serves a window's UI Automation
  (`books/dev/src/explanation/design/listener-without-connections.md`, section 4.3):
  `provider_process(hwnd)`, `served_by`, `console_host_of(client)`,
  `is_top_level_window`, `parent_across_process_boundary`,
  `raw_view_parent_window`, `parent_window`, `outermost_nearest_window`
  and `plain_windows_start`. `verbatim-uia` re-exports them under the same
  names, where its guide describes them ([verbatim-uia](verbatim-uia.md),
  "Provider checks and the process boundary").
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
