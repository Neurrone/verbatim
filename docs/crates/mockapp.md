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
<window title>] [--show]` (default title `mockapp`). It creates one real
top-level Win32 window titled per `--title`, hidden unless `--show` is
given, prints `ready` (flushed) once the window exists and the provider is
answering, then processes stdin commands until `quit`. The cross-process
tests leave the window hidden; the end-to-end suite's `spelling_errors`
scenario shows it and drives it with real keys (`docs/crates/verbatim-e2e.md`).
The focus starts on the first node in the `focused` state, if the fixture
has one, as an application opens with its focus on a control.

Fixture format: one JSON object per node — `id` (unique string), `role` (a
`Role` name in snake case, e.g. `check_box`), optional `name` and `value`
strings, `states` (an array of `State` names in snake case, e.g.
`read_only`), optional detail properties (`description` and
`keyboard_shortcut` strings, one-based `position_in_set`, `set_size`, and
`level` integers — the M3 `NodeDetails` vocabulary; each backend serves
the subset its API can express), an optional `default_action` (MSAA's
`accDefaultAction`, which `accDoDefaultAction` then succeeds for), an
optional `controller_for` (the `id`
of a node this one controls, served on the UIA backend as the
`ControllerFor` relation, as a search box names its suggestion list), an
optional `text` (milestone M4: the node's text, with bare line feeds; on
the UIA backend the node serves the text pattern over it, and on the MSAA
backend, since MSAA has no text interface, mockapp creates a real
multi-line Win32 `EDIT` control holding the first such text, its line
feeds made carriage return and line feed pairs, read through the edit
control's own messages), and `children` (nested nodes). The root
node conceptually corresponds to the window itself.
`fixture::role_from_fixture_str` and `state_from_fixture_str` hold the
complete name tables.

Stdin commands, one per line: `focus <id>` (raises the backend's
focus-changed notification — `UiaRaiseAutomationEvent` for UIA,
`NotifyWinEvent(EVENT_OBJECT_FOCUS, ...)` for MSAA), `set-focus <id>`
(moves the focused state as `focus` does but raises nothing, so no client
on the machine, a running screen reader included, calls into mockapp in
response; for the tests that count an operation's calls exactly, which
hand the focus to the outpost themselves), `set-name <id> <text>`
and `set-value <id> <text>` (update the tree and raise the matching
property-change or name/value-change notification), `select <id>` (marks
the node selected, moving the state off any previous selection, and raises
`SelectionItem_ElementSelected` for UIA or `EVENT_OBJECT_SELECTION` for
MSAA), `notify <text>` (raises a UIA `AutomationNotification` from the
root provider with `text` as the display string, kind `Other`, processing
`All`, and a fixed `mockapp-notify` activity id; reported as unsupported
on the MSAA backend, which has no notification event),
`active-text-position <id> <start> <end>` (raises UIA's active text
position changed event from a text node, with the range of its text from
`start` to `end`, UTF-16 offsets; unsupported on the MSAA backend), `caret <id> <start>
[<end>]` (selects the text node's text from `start` to `end`, UTF-16
offsets, the caret alone at `start` when `end` is left out, raising no
event, as an application's caret moves before a client asks where it is;
on the MSAA backend the offsets are the edit control's, with its carriage
returns, sent as `EM_SETSEL`), `set-text <id> <text>` (replaces a UIA
text node's text, with `\n` for a line feed and `\\` for a backslash,
raising no event, as a terminal's buffer changes before a client reads it;
the terminal tests write lines, discard the oldest, and clear the screen
with it; UIA only), `stall <ms>`
(blocks the window thread for that long, so every cross-process call into
the window waits, as with an application that is starting up or busy;
the window thread prints `stall started` on stdout as it begins and
`stall ended <us>`, the time in microseconds since the Unix epoch, as it
ends, so a test waits for the stall itself rather than for a guessed
time), and `quit`.

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
- `uia::text` — the UIA text pattern for a node with `text`
  (`ITextProvider2` and `ITextRangeProvider`, both answered for either
  pattern id). A range is two UTF-16 offsets into the node's text, read
  afresh on every call, so the `caret` command shows through at once. The
  units are simple and fixed: a character is one code unit; a word is a run
  of letters and digits with the spaces after it, or one other character;
  a line ends after its line feed, and a text ending in one has an empty
  last line, as an editor shows; a paragraph is a line; a stretch of the
  format unit ends wherever one of the node's `spelling_errors` or `bold`
  stretches (optional fixture fields, each a list of UTF-16 start and end
  offsets) starts or ends; the page and the document are the whole text. Moving lands on a unit's
  start and never past the last unit, so a client sees the text's ends; a
  move back from inside a unit to its start counts as one, as UIA
  specifies. The caret (`GetCaretRange`) is the selection's start, as the
  edit controls report it, and on the UIA backend the keys pressed in the
  window move the focused node's caret by these units (`caret_key`): Right
  Arrow by a character, Control+Right Arrow to the next word's start, and
  Down and Up Arrow to the same column of the next or previous line, or its
  end when that line is shorter, raising no event; the language (`Culture`) is `en-US`; the
  annotation types are the spelling error type (60001) for a range
  touching one of the spelling errors and unsupported otherwise, as
  Windows 11 Notepad reports them; the font is 11 point Consolas in black,
  neither italic nor underlined, weighing 700 within a bold stretch, 400
  outside, and mixed across both; every other attribute is unsupported,
  and a node with `italic_fails` set fails its `IsItalic` read with
  `E_FAIL`, as a provider that fails an attribute read. A range handed back by a client
  (`CompareEndpoints`, `MoveEndpointByRange`) is one mockapp made, so its
  offsets are read from its implementation.
- `edit` — the MSAA backend's real edit control: created inside the host
  window, found by class, and selected with `EM_SETSEL` on the window
  thread.
- `stdin` — command parsing and the reader thread.
- `hits` — the provider-side hit counters: one atomic per provider method
  (every `IRawElementProviderSimple`, `IRawElementProviderFragment`,
  `IRawElementProviderFragmentRoot`, and pattern-provider method, the text
  pattern's and text range's methods a client reads with, every
  `IAccessible` and `IDispatch` method), one for `WM_GETOBJECT`, and one
  for stdin commands applied. A test reads them with two synchronous
  window messages to the host window, answered by the same thread that
  runs every provider call, so a read made after a client's call returns
  counts every hit that call caused: `WM_APP + 2` returns the counter
  whose index in `Method::ALL` is `wParam`, and `WM_APP + 3` zeroes them
  all. The tests compile `src/hits.rs` into their shared module, so the
  method list and message numbers cannot drift apart.

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
  A list or tab control also serves an `ISelectionProvider` whose
  selection is its children in the `selected` state, so a fixture's
  initial selection and the `select` command both show through the
  Selection pattern and its `Selection` property, as a real list reports
  its selected item. A list's provider also serves `ISelectionProvider2`
  (`FirstSelectedItem`, `LastSelectedItem`, `CurrentSelectedItem`, the
  last selected, and `ItemCount`), and a tab control's does not, so the
  tests see both a provider with `SelectionPattern2` and one without.
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
`wait_until` with a generous timeout). The test files that use UIA as a
client (`arbitration.rs`, `call_counts.rs`, `controller_for.rs`,
`events.rs`, `remote_ops.rs`, `text.rs`, `uia_tree.rs`)
run through `tests/common/harness.rs` instead of libtest (`harness =
false`): it runs and reports the tests as libtest does, then ends the
process without running DLL detach code, because `UIAutomationCore.dll`'s
own detach code sometimes hangs or crashes in a process that has connected
to providers; the file's comment gives the evidence. `uia_tree.rs` and `msaa_tree.rs` walk
a rich scripted tree through each real client stack and assert normalized
roles, names, values, states, and detail properties match the fixture —
every node's details must read back exactly what was scripted, and
all-`None` for every node the fixture left plain, so both presence and
absence are pinned (rectangles excluded: mockapp scripts no geometry, but
UIA merges the real window's rectangle into the hwnd-hosted root).
`msaa_tree.rs` also reads a list's selected child through
`verbatim_ia2::acquire::selected_child`, from the list object kept when the
tree was walked, after a scripted `select`: the MSAA provider's
`accSelection` returns the selected child as its own object, or an empty
variant when none is selected.
`arbitration.rs` asserts `verbatim_uia::has_server_side_provider` and
`verbatim_outpost::Arbitrator` resolve a `uia`-backend window to UIA and a
`msaa`-backend one to MSAA. `events.rs` asserts that
`set-name`/`set-value` commands are observed by a property
`verbatim_uia::Registration` and `verbatim_ia2::WinEventHook`,
that `select` is observed by a selection `verbatim_uia::Registration` (with
the delivered element's mapped snapshot carrying its name and `Selected`
state) and by the WinEvent hook as `WinEventKind::Selection`, and that
`notify` is observed by a notification `verbatim_uia::Registration` with its
full payload, that one registration of all three, one event handler
group, hears each, that a registration on a focus hears its changes
and neither its group's nor a sibling's, and that an active text position
change arrives with its element and a range whose text is the one
raised — property, value, selection, and notification changes are
used rather than focus, so the tests never depend on real keyboard focus
or `SetForegroundWindow` succeeding, and pass headless on GitHub
`windows-latest` runners. `controller_for.rs` selects items with `select`
and asserts that `verbatim_uia::Uia::controlled_descendant` finds a
result inside the list the search box's `ControllerFor` names, and
nothing for an item elsewhere, for the list itself, or from a box that
controls nothing. `remote_ops.rs` runs `verbatim-uia-rops`'s remote and
classic focus ancestry over `ancestry.json` (a five-level chain, a list
and a tab control with selected children) and asserts they return the
same ancestors with the same cached properties, that an element that
lost the focus returns early, and how both behave against a stalled
mockapp and one that has exited; the crate's guide records the findings.
`text.rs` drives `verbatim-outpost`'s text module over `text.json`'s text
on both stacks, as the outpost's worker does once it has a node's text
(the worker finds a UIA focus by reading the keyboard focus, which a test
must not take): lines, words, and characters read; movement stopping at
the empty last line; a position inside a chunk resolved through its text;
UIA's missing sentence unit and the edit control's paragraph in its place;
a page unsupported; UIA's language on a chunk; and a caret key answered
with the word it reached and with a selection reported as selected. Its
caret wait never waits: every test moves mockapp's caret first, and the
wait panics if called. `call_counts.rs` pins a caret move and a caret
report on both stacks this way too (`docs/performance.md`).
`slow_application.rs` runs a real
`verbatim_outpost::Outpost` in the test process against an `msaa`-backend
mockapp: it captures the address of mockapp's own scripted focus event,
stalls mockapp's window thread with `stall`, delivers the focus as a
listener fact once mockapp has acknowledged that the stall began, and
asserts the outpost still reports it, having started the read before
the stall ended, so the read waited longer than the outpost's old 1.5
second deadline. The
scripted focus event is raised with `NotifyWinEvent`, so this test too
needs no real keyboard focus.

`call_counts.rs` is the operation ledger's ratchet (`docs/performance.md`):
over `tests/fixtures/counts.json`, and `tests/fixtures/dialog.json` (a
message box: a dialog holding a question and two buttons) for a dialog's
own text, it measures each ledger operation on each
backend and asserts exactly how many cross-process calls the client side
made, by kind, and how many calls mockapp's providers answered, by method.
The MSAA operations (a focus change cold and in the steady state, a focus
into a list, an arrow to the next list item, and a next-sibling and parent
navigation step) run through a real outpost, as `slow_application.rs`
does, reading the calls from the event's or reply's timing. The UIA ones
run on the test's own thread, making the same `verbatim-uia` calls in the
same order as the outpost's worker once it has the element: the outpost
finds a UIA focus's element by reading the system's keyboard focus, which
a test must not take. Event registrations, which run on a registration's
own thread, are pinned by the provider calls they cost mockapp: the focus
listener's desktop-wide group and an outpost's focus-following property
subscription. A container's selected child is pinned over
`tests/fixtures/ancestry.json`, through a list's `SelectionPattern2` and
through a tab control's `Selection` pattern, classically and in the
focus's remote program; and so are a text focus's registration, its caret
and text changes and active text position changes as one group, and the
handling of an active text position change, which makes no call. On a
mismatch the test prints every measured count,
so a deliberate change updates all the numbers that moved in one pass.
