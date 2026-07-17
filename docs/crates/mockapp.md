# mockapp

The scripted UIA and MSAA provider test host (architecture section 13,
layer 2): a real, separate-process Win32 application that answers
`WM_GETOBJECT` as a genuine out-of-process accessibility *provider* over a
JSON-scripted tree, so `verbatim-uia` and `verbatim-ia2`'s real client
stacks — and the outpost's arbitration — are exercised cross-process with
zero real applications, on plain CI Windows runners. `mockapp`'s own binary
depends only on `verbatim-model` (for `Role`, `State`, and `StateSet`); the
client crates it is built to be tested against (`verbatim-uia`,
`verbatim-ia2`, `verbatim-outpost`) are dev-dependencies, used only by its
own integration tests.

CLI: `mockapp --fixture <path.json> --backend <uia|msaa> [--title
<window title>]` (default title `mockapp`). It creates one real top-level
Win32 window titled per `--title`, prints `ready` (flushed) once the window
exists and the provider is answering, then processes stdin commands until
`quit`.

Fixture format: one JSON object per node — `id` (unique string), `role` (a
`Role` name in snake case, e.g. `check_box`), optional `name` and `value`
strings, `states` (an array of `State` names in snake case, e.g.
`read_only`), optional detail properties (`description` and
`keyboard_shortcut` strings, one-based `position_in_set`, `set_size`, and
`level` integers — the M3 `NodeDetails` vocabulary; each backend serves
the subset its API can express), and `children` (nested nodes). The root
node conceptually corresponds to the window itself.
`fixture::role_from_fixture_str` and `state_from_fixture_str` hold the
complete name tables.

Stdin commands, one per line: `focus <id>` (raises the backend's
focus-changed notification — `UiaRaiseAutomationEvent` for UIA,
`NotifyWinEvent(EVENT_OBJECT_FOCUS, ...)` for MSAA), `set-name <id> <text>`
and `set-value <id> <text>` (update the tree and raise the matching
property-change or name/value-change notification), `select <id>` (marks
the node selected, moving the state off any previous selection, and raises
`SelectionItem_ElementSelected` for UIA or `EVENT_OBJECT_SELECTION` for
MSAA), `notify <text>` (raises a UIA `AutomationNotification` from the
root provider with `text` as the display string, kind `Other`, processing
`All`, and a fixed `mockapp-notify` activity id; reported as unsupported
on the MSAA backend, which has no notification event), and `quit`.

Public API is otherwise internal (`mockapp` is a binary, not a library);
its crate-internal modules are the reviewable surface:

- `fixture` — JSON parsing and role/state name validation.
- `tree` — the owned, mutable scripted tree: a flat arena (`Tree`, shared as
  `SharedTree` behind `Arc<Mutex<_>>`) so provider COM objects and stdin
  command handling can both reach it without borrowing from the window's
  state; index 0 is always the root.
- `window` — Win32 class registration, window creation, and the message
  loop; dispatches `WM_GETOBJECT` to whichever backend is active and drains
  stdin commands posted from the reader thread.
- `uia` — the UIA provider. Every node gets its own COM object on demand:
  the root is a `RootProvider` (also the fragment root), every other node a
  `ChildProvider`; only the root implements `IRawElementProviderFragmentRoot`,
  so non-root elements never misreport themselves as fragment roots. Role
  and property mapping is the inverse of `verbatim_uia::map`.
- `msaa` — the MSAA provider. Every node is its own full `IAccessible`
  object (never a numbered "simple child"), addressed two ways: the
  window's default client object (`OBJID_CLIENT`) is always the root, and
  every node additionally answers `WM_GETOBJECT` under a custom positive
  object id (`index + 1`) so `NotifyWinEvent` can address any node directly.
  Deliberately never answers the UIA root object id, so the arbitration
  probe finds nothing and the window arbitrates to MSAA. Role and state
  mapping is the inverse of `verbatim_ia2::map`.
- `stdin` — command parsing and the reader thread.

Implementation notes:

- **Threading.** The window thread joins a single-threaded apartment
  (`COINIT_APARTMENTTHREADED`); every provider COM object is built with
  `Agile = false`, so cross-process calls into them are marshaled back onto
  this thread's message queue — which is why the message loop must keep
  pumping for the process's whole lifetime. Stdin is read on a separate
  thread (reading blocks, which the message loop cannot afford) and handed
  to the window thread over a channel, woken by a payload-free posted
  message rather than by smuggling a pointer through `LPARAM`.
- **`idObject` recovery.** `WM_GETOBJECT`'s `lParam` carries `idObject`
  zero-extended into the full pointer width on at least some code paths
  (observed from `oleacc`'s `AccessibleObjectFromWindow`, whose probes
  arrive as a huge positive `lParam` rather than sign-extended), so
  recovering the standard negative object ids (`OBJID_CLIENT` and friends)
  requires truncating back to 32 bits and reinterpreting (`lparam.0 as
  i32`), not a checked conversion — `i32::try_from` silently rejects
  exactly the values this needs to recognize, which was the root cause of
  an early "the window answers, but with the wrong data" failure mode.
- **Null interface results.** UIA's `IRawElementProviderFragment`/
  `IRawElementProviderFragmentRoot`/`IRawElementProviderSimple` methods
  that can legitimately return "no such element" (`Navigate` at a tree
  boundary, `GetPatternProvider` for an unsupported pattern,
  `HostRawElementProvider` for a non-root node) cannot represent a null
  interface pointer safely in `windows-core`'s `NonNull`-backed interface
  types. `Err(windows_core::Error::empty())` is the documented escape
  hatch: the generated COM glue turns it into `S_OK` with an untouched
  (effectively null) out-parameter, exactly the UIA contract for "nothing
  here."
- **Toggle, expand-collapse, value, and selection need real pattern
  objects.** `GetPropertyValue` overrides for pattern-availability and
  pattern-value properties are documented as an optional shortcut, but
  empirically `IUIAutomationCacheRequest`'s cache-building still calls
  `GetPatternProvider` for `TogglePattern`, `ExpandCollapsePattern`,
  `ValuePattern`, and `SelectionItemPattern` before trusting a cached
  value — which is exactly the path `verbatim-uia`'s base cache request
  always uses. mockapp therefore implements small `IToggleProvider`,
  `IExpandCollapseProvider`, `IValueProvider`, and
  `ISelectionItemProvider` objects (returned from `GetPatternProvider`,
  gated on role for toggle, on the fixture's `expanded`/`collapsed` states
  or presence of a `value` for the middle two, and on its
  `selectable`/`selected` states for selection) alongside the
  `GetPropertyValue` overrides, rather than relying on the shortcut alone.
- **Raw-view host furniture.** A real `hwnd`'s UIA raw tree (`TreeScope_Children`
  with a true condition) can include host-provided native elements — for
  example window-chrome furniture merged in via `HostRawElementProvider` —
  alongside the fixture's own children. This is expected UIA behavior for
  any real top-level window, not a mockapp gap; the UIA tree-construction
  test tolerates unrecognized nodes only among the window's direct children
  for exactly this reason, while still requiring every fixture node to be
  found somewhere.

The cross-process integration tests in `tests/` spawn the compiled binary
via `env!("CARGO_BIN_EXE_mockapp")`, using fixtures under
`tests/fixtures/`, and a shared `tests/common/mod.rs` harness
(`MockApp`, killed on drop; `find_window` by exact, per-test-unique title;
`wait_until` with a generous timeout). `uia_tree.rs` and `msaa_tree.rs` walk
a rich scripted tree through each real client stack and assert normalized
roles, names, values, states, and detail properties match the fixture —
every node's details must read back exactly what was scripted, and
all-`None` for every node the fixture left plain, so both presence and
absence are pinned (rectangles excluded: mockapp scripts no geometry, but
UIA merges the real window's rectangle into the hwnd-hosted root).
`arbitration.rs` asserts `verbatim_uia::has_server_side_provider` and
`verbatim_outpost::Arbitrator` resolve a `uia`-backend window to UIA and a
`msaa`-backend one to MSAA. `events.rs` asserts that
`set-name`/`set-value` commands are observed by
`verbatim_uia::PropertyRegistration` and `verbatim_ia2::WinEventHook`,
that `select` is observed by `verbatim_uia::SelectionRegistration` (with
the delivered element's mapped snapshot carrying its name and `Selected`
state) and by the WinEvent hook as `WinEventKind::Selection`, and that
`notify` is observed by `verbatim_uia::NotificationRegistration` with its
full payload — property, value, selection, and notification changes are
used rather than focus, so the tests never depend on real keyboard focus
or `SetForegroundWindow` succeeding, and pass headless on GitHub
`windows-latest` runners.
