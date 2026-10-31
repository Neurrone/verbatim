# verbatim-win32

The local window and process queries more than one of Verbatim's roles
makes: an outpost, the focus listener, the MSAA and UIA clients, and
Core's supervisor. None of them sends a window a message or calls into
another process, so each is safe on any thread, a hung application's
windows included, and none is counted (`docs/performance.md`, "What
counts as a call"). Window handles are `isize`, zero for none. It is a
Windows-specific crate and depends on `verbatim-model`,
`verbatim-reader-rules` and the Windows bindings.

Public API:

- `window` — queries by window handle. The general ones: `window_exists`,
  `window_is_visible`, `is_enabled`, `is_child`, `desktop_window`,
  `window_class_name`, `window_thread`, `style`, `rect`, `parent`,
  `top_child`, `related` and `is_multiline_edit`; `top_level_of`,
  `window_owner` (thread and process), `foreground_window_handle`,
  `focus_window` (the window with the keyboard focus, else the active
  window) and `window_is_hung`. `window_classes(hwnd)` reads the
  `WindowClasses` the class verdict judges
  ([verbatim-reader-rules](verbatim-reader-rules.md)).
  `window_is_hidden_frame` and `window_belongs_to_hidden_frame` recognize
  Core's hidden frame by its marker property and its owner, Core being
  the calling process's parent. `event_window_unreadable(hwnd)` is why an
  event's window cannot be read, if it cannot: no window, a ghost window
  standing in for it, or a hung application, in NVDA's order;
  `window_unreadable` is the same as the system's judgement, which a
  `WindowReadabilityReader` replaces in tests, as a `ForegroundReader`
  replaces `GetForegroundWindow`. `served_by(hwnd, pid)` and
  `top_level_windows(pid)` judge a window's application by the process
  that serves its UIA. `front_is_another_thread_of_its_application(hwnd)`
  is whether the user has moved on from `hwnd` to a window of the same
  application on another UI thread, the test both readers'
  `MOVED_ON_GRACE` is judged by. `SystemWindows` answers the acceptance
  test's `WindowQueries` with local calls, with the foreground window its
  caller read.
- `provider` — the process that serves a window's UI Automation
  (`docs/design/listener-without-connections.md`, section 4.3):
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
