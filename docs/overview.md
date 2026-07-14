# Crate Overview

A reviewer's guide to the workspace as of the end of milestone M1. Crates
appear in dependency order — each one uses only concepts explained before
it — so the document reads front to back. `docs/architecture.md` holds the
decisions of record this code implements; `docs/roadmap.md` holds the
milestone scoping. Where a method's behavior is not obvious from its
signature, an implementation note explains how it works and why.

## verbatim-model

The normalized accessibility vocabulary (architecture section 3), the one
language every other crate speaks. No I/O; serde derives exist so the same
types travel over the Core-outpost pipe, the control plane, and the flight
recorder unchanged.

Public API:

- `TraceId` — correlates one observed OS event or keypress with everything
  it causes, through outpost, reducer, speech queue, synth, and audio.
  `mint()` allocates process-unique increasing IDs; `namespace(pid)` seeds
  the counter with the process id in the high 32 bits, called once at every
  process's startup so IDs minted in Core and in each outpost never collide
  when they meet in the latency ledger.
- `NodeId`, `Pid`, `SnapshotVersion`, `QueryId` — small identity types.
- `Backend` — `Uia` or `Msaa`; which client stack sourced a node or event.
  Diagnostics only above the outpost.
- `Role`, `State`, `StateSet` — the role vocabulary (window, dialog, menu
  item, button, check box, slider, and so on) and a bitmask state set with
  `contains`, `insert`, `remove`, `with`, and ordered `iter`. Both enums are
  non-exhaustive so later milestones can grow them without breaking
  matches.
- `NodeSnapshot` — everything needed to announce one node: id, backend,
  role, optional name and value, states.
- `NormalizedEvent` — `FocusChanged` (carrying a full snapshot),
  `PropertyChanged` (name, value, or the complete new `States` set), and
  `ValueChanged`.
- `Input` and `Effect` — the reducer's contract. Inputs are strictly
  accessibility-shaped: events, fetch completions, timer ticks. Effects are
  strictly `Speak`, `StopSpeech`, and `Fetch`; menu and quit concerns never
  appear here.
- `Utterance`, `UtteranceSegment`, `SegmentContent`, `SpeechPriority` —
  structured speech. Segments carry text or role and state tokens
  (including `NegatedState` for announcements like "not checked"), so the
  pure reducer never touches localization; tokens become words at the
  speech-pipeline boundary.
- `GestureId` — normalized gesture identifiers, NVDA's scheme.

Implementation note, `GestureId::parse`: splits `source:parts`, lowercases
everything, and sorts the plus-separated parts, exactly like NVDA's
`normalizeGestureIdentifier` — so `kb:Verbatim+V` and `kb:v+verbatim` are
one gesture and binding lookup is order- and case-insensitive.
Deserialization re-parses, so an identifier read from config or the wire is
always normalized.

## verbatim-i18n

Fluent localization (decision D10). English resources compile into the
binary as the permanent fallback; other locales load at startup from a
`locale` folder next to the executable.

Public API:

- `loader()` — the process-wide `FluentLanguageLoader`; `new_loader()`
  builds an isolated one for tests.
- `load_locale_dir(dir, requested)` — negotiates and loads locale-folder
  languages over the embedded fallback.
- `messages` — one typed accessor per statically known UI string
  (`menu_settings()`, `settings_title_with_category(category)`, and so on),
  each compile-time checked by the `fl!` macro, so a message-id typo fails
  the build.
- `message(id)` — runtime lookup for ids that arrive as data, such as
  setting-descriptor label keys.
- `role_name(role)`, `state_name(state)`, `negated_state_name(state)` — the
  localized spoken words for utterance tokens; states that are never spoken
  (focused, focusable, selectable, offscreen) return `None`.

Implementation notes: `LocaleDirAssets` exists because i18n-embed's own
filesystem assets type yields bare file names without the language folder,
which breaks language negotiation; this implementation yields
`language/file` paths. The pseudo-locale test (required from M1) generates
a bracket-wrapped translation of every English message into a temporary
locale, loads it, and asserts every message id resolves through it — proof
no string bypasses the loader. It parses the `.ftl` line by line, which is
why the resource file keeps every message on a single line.

## verbatim-config

Portable configuration next to `verbatim.exe`. `settings.toml` is the base
configuration — global settings plus the base profile's sections — and the
`profiles` folder holds named profiles as sparse overlays, mirroring NVDA's
base-plus-diffs model. Because the base lives in `settings.toml`, any file
name in `profiles` is a legitimate user profile.

Public API:

- `Settings` — the `settings.toml` schema: `locale`, `log_filter`,
  `verbatim_keys` (all global; profiles cannot carry them), and `speech`
  (the base profile's section).
- `VerbatimKeys` — which keys act as the Verbatim modifier (`caps_lock`,
  `insert`, `numpad_insert`) plus `share_modifier`, which passes the
  modifier's own transitions down the hook chain for a screen reader
  running behind Verbatim.
- `SpeechConfig`, `ConfigValue` — active synthesizer plus per-synthesizer
  setting values, keyed by synth id then setting id so switching synths
  never loses the other synth's values.
- `Profile` — one named profile. Deliberately has no global fields, so a
  profile file cannot smuggle in modifier keys or a locale; unknown
  sections are ignored by the schema (tested).
- `ConfigStore` — `load(root)`, `settings()`, `settings_mut()`,
  `save_settings()`, `ensure_files_exist()`, and `active()`.
- `ActiveConfig` — the resolved read-only view: `synthesizer()`,
  `synth_setting(synth, id)`, and `synth_settings(synth)` resolve each
  setting through the active overlays (most specific first) down to the
  base. M1 activates no overlays; the layering is implemented and tested.

Implementation notes: writes are atomic (write a temporary file, rename
over the target; `std::fs::rename` replaces on Windows).
`ensure_files_exist` materializes missing files with defaults at startup so
they are discoverable and hand-editable, but never touches existing files —
including corrupt ones, which `load` surfaces as errors rather than
silently replacing. The corollary: fields added to the schema later do not
appear in an existing file; they read as defaults until the file is
regenerated or saved.

## verbatim-input

The keyboard hook (architecture section 5), split into a pure decision
state machine and a thin never-blocking hook shell.

Public API:

- `KeyEvent`, `KeyDecision` — one raw key transition, and the
  swallow-or-pass verdict returned to Windows.
- `keys` — the NVDA-style key-name vocabulary: `vk_from_name` and
  `name_from_vk` (letters and digits resolve procedurally, everything else
  through a table where the extended flag distinguishes twins like insert
  and numpad insert), plus `VERBATIM_MODIFIER_NAME`. Shared with the
  control plane's key injection so both sides speak identical names.
- `DecisionConfig`, `DecisionMachine`, `Decision`, `EmittedGesture` — the
  pure state machine. `on_key(event, now)` takes a caller-supplied clock,
  so tests script entire key streams with fake time.
- `GestureMap`, `SharedGestureMap` — the bound-gesture set behind an
  arc-swap snapshot the hook reads lock-free; rebinding is one atomic store.
- `InputHook::start(config, map, events)` — installs `WH_KEYBOARD_LL` on a
  dedicated thread; drop uninstalls.

Implementation notes, `DecisionMachine::on_key` (the intricate one — the
semantics follow NVDA's `keyboardHandler`):

- A Verbatim-modifier key-down is always swallowed, so caps lock never
  toggles while acting as the modifier. In share mode the modifier's
  transitions pass down the hook chain instead, so a screen reader hooked
  behind Verbatim sees the modifier held and swallows it itself.
- A key-down forming a bound gesture is swallowed, recorded as trapped, and
  emitted with a freshly minted `TraceId`. An unbound companion key falls
  through to the application as a bare keypress — NVDA behavior, so an
  unrecognized chord is not eaten.
- Trapped keys are swallowed on their key-up too, so an application never
  sees the release of a key whose press it never saw.
- Double-tap passthrough: releasing the modifier with no other key pressed
  during the hold arms a window (`multi_press_timeout`, 500 ms). Pressing
  the same physical key again inside the window bypasses modifier handling
  for that entire press, including auto-repeats, so caps lock actually
  toggles. The bypass ends at the next key-up and does not re-arm itself, so
  a triple tap makes the third press a modifier again.
- Injected keys are processed identically to physical ones, which is what
  lets the control plane drive gestures with synthetic input.
- Chords normalize modifiers to generic names (left and right control both
  become `control`), and gesture assembly relies on `GestureId::parse` for
  ordering, so press order never matters.

The hook shell (`hook.rs`) keeps the machine in a thread-local on the hook
thread, forwards emitted gestures with a non-blocking `try_send` that drops
on a full channel, and returns 1 to swallow or calls `CallNextHookEx` to
pass. The never-block constraint is load-bearing: Windows silently removes
low-level hooks that exceed the system timeout.

## verbatim-audio

The `AudioSink` seam (decision D5) and its WASAPI implementation.

Public API: `PcmFormat` (rate and channel count; samples are 16-bit
throughout M1), `AudioError`, the `AudioSink` trait (`begin` with a
`TraceId`, blocking `write`, draining `end`, discarding `stop`), and
`WasapiSink`.

Implementation notes, `WasapiSink`: event-driven shared mode with a small
(roughly 40 ms) buffer, initialized with the auto-convert-PCM flags so any
synth output rate is accepted without manual resampling; the device opens
lazily on the first `begin` and the stream is reused across utterances of
the same format. COM is initialized per thread as MTA, tolerating
`RPC_E_CHANGED_MODE`. On the first buffer of each utterance it emits the
`audio_started` tracing event tagged with the utterance's trace ID — the
final leg of the latency timeline. `stop` issues Stop plus Reset to discard
buffered audio immediately, which is how speech interruption sounds instant.

## verbatim-speech

The speech pipeline (architecture section 6): priority lanes, synth
threads, token rendering, and the settings host the GUI talks to.

Public API:

- `SynthDriver` — the synchronous driver contract: identity, `pcm_format`,
  data-driven `supported_settings`, `setting` and `set_setting`, and a
  blocking `speak(request, sink)`. Synchronous by design so a Wasm
  component can implement it unchanged later; Verbatim owns all threading.
- `SynthSink` — receives PCM (`push_pcm` returns `ControlFlow::Break` to
  request cooperative cancellation) and index-mark echoes.
- `SpeechRequest`, `RequestMark`, `IndexMark`, `SynthError`.
- `SettingDescriptor` (`Numeric` with range and steps, `Choice` with option
  pairs, `Toggle`), `SettingId`, `SettingValue`, `SynthId`, `SynthChoice` —
  NVDA's driver-setting model, the source the settings GUI generates its
  controls from. Label fields are Fluent message ids, never display text.
- `SynthRegistry`, `SynthFactory` — drivers registered by id, built on
  demand, switchable at runtime.
- `SpeechManager` — `new(SpeechManagerConfig)`, `speak(utterance)`,
  `settings_host(persist)`. The config carries the registry, the initial
  synth and its persisted setting values, the audio sink, and an optional
  observer.
- `SpeechSettingsHost` (trait) and `SettingsHost` (implementation) — the
  GUI's live handle: list synthesizers, switch the active one, read
  descriptors and values, `set_setting` applying immediately (slider drags
  are audible as they happen), `commit` persisting through an injected
  `PersistFn` so this crate never depends on the config layer, and `revert`
  restoring the last committed values.
- `SpeechEvents` — the observability seam: `utterance_queued` and
  `audio_started`, called on pipeline threads and required to be cheap.
- `render_utterance` — token rendering at the boundary: turns structured
  segments into spoken text via `verbatim-i18n`'s role and state words.

Implementation notes, `SpeechManager`: two dedicated threads. The queue
thread owns the priority lanes — `Interrupt` cancels current and queued
speech (a shared flag the sink checks makes the driver stop, and the audio
sink's `stop` discards buffered sound), `Next` jumps ahead of `Queued` —
and dispatches one job at a time, synchronized by a finished handshake from
the synth thread. The synth thread owns the active driver and the audio
sink and is where `SynthDriver::speak` blocks. Setting changes and synth
switches travel to the synth thread as commands, and `SettingsHost` keeps a
mirror of descriptors and values so GUI reads never hop threads.

## verbatim-synth-onecore

The OneCore driver over WinRT `Windows.Media.SpeechSynthesis` — the voice
Verbatim first speaks with.

Public API: `OneCoreSynth::new()`, `ONECORE_ID`, and `factory()` plus
`register(registry)` conveniences. Everything else is the trait.

Implementation notes: voices enumerate into a `voice` Choice descriptor;
rate, pitch, and volume are NVDA-convention 0 to 100 numerics mapped onto
the WinRT options (rate 50 is exactly 1.0x, following NVDA's curve with
minimum 0.5). The `rate-boost` toggle mirrors NVDA's `_set_rateBoost`
semantics precisely: flipping it preserves the percent and re-applies it
against the boosted maximum of 6.0 (1.5 unboosted), so enabling boost at 50
audibly jumps from 1.0x to 3.25x — the jump is the feature. `speak` blocks
on the synthesis async operation (its thread is dedicated), then parses the
returned WAV by walking RIFF chunks — the format chunk for the real rate
and channel count rather than assuming, the data chunk for samples — and
pushes PCM in roughly 50 ms slices, checking the sink's `ControlFlow`
between slices so cancellation is prompt. Index marks are echoed at
completion, since OneCore cannot track positions mid-text. The display name
resolves through `verbatim-i18n`. A `#[ignore]`d integration test audibly
speaks the word "test" through the real WASAPI sink.

## verbatim-synth-capture

The test synth: records every `SpeechRequest` with a timestamp into a
shared `CaptureLog` (`Arc<Mutex<Vec<CaptureRecord>>>`), emits a short burst
of silent PCM, echoes all marks, and honors cancellation. Pipeline unit
tests assert on its log; it exposes a voice choice and a rate numeric so
the settings host is exercised too.

## verbatim-core

The pure reducer (architecture section 2) and the flight recorder.

Public API:

- `reduce(state, input)` — the frozen signature: pure, no I/O, no clocks;
  returns the next state and the effects to execute.
- `SrState` — `new()`, `focused()`, `last_seen_version(source)`,
  `pending_fetch_count()`.
- `FlightRecorder<T>` — a bounded ring buffer; `ReducerRecorder` and
  `RecordedInput` specialize it for reducer inputs; `replay(initial,
  inputs)` re-runs a recorded sequence and returns the effects per step,
  proven deterministic by test.

Implementation notes, `reduce`:

- A focus change speaks name, role, value, then states in a fixed order
  (checked or its negation first, then mixed, pressed, selected, expanded,
  collapsed, has-popup, default, read-only, disabled, busy), at Interrupt
  priority. The negated-checked rule: a `CheckBox` or `RadioButton`
  carrying neither Checked nor Mixed announces "not checked". Focus-related
  states are never announced.
- A value change on the currently focused node speaks just the bare value,
  Interrupt — the slider-drag announcement. Value changes elsewhere are
  ignored in M1.
- A states change on the focused node is diffed against the stored
  snapshot: newly gained announceable states are spoken, and losing Checked
  on a check box or radio button announces the negation — the
  spacebar-toggle-off case, which has no newly gained state to catch it.
- Staleness: an event whose snapshot version is lower than the last seen
  for its source is never trusted; the reducer instead emits a `Fetch` for
  the focused node. The completion is compared against what was last
  actually announced (tracked separately from the live snapshot, since
  silent updates move one but not the other) and speaks only on a real
  difference. A completion arriving after focus moved on is dropped.
- Every `Speak` carries the triggering input's trace ID, which is what
  makes end-to-end latency timelines possible.

## verbatim-uia

The UIA client stack (architecture section 4): cache requests, dedicated
threads, and the arbitration probe.

Public API:

- `Uia` — a per-thread client (one COM apartment, one `IUIAutomation`
  instance; nothing COM crosses threads): `focused_element`,
  `element_from_handle`, `element_by_runtime_id`, `base_cache_request`.
- `base_cache_request(client)` — the property set prefetched with every
  event and fetch: name, control type, value, process id, native window
  handle, enabled, focus states, toggle and expand-collapse state, and the
  two pattern-availability flags described below.
- `FocusRegistration::new(target_pid, callback)` — the self-contained
  global focus listener, filtered to a target pid; drop unregisters and
  tears down its own thread. Deliberately a sealed module: UIA's focus
  registration is desktop-global and unscopeable, so exactly one exists per
  process, and in M3 this module moves wholesale into the focus-sentinel
  outpost.
- `PropertyRegistration::new(hwnds, callback)` — name, value, toggle-state,
  enabled, and expand-collapse property changes, scoped to the target's
  top-level windows with subtree scope.
- `has_server_side_provider(hwnd)` — the arbitration probe. Sends
  `WM_GETOBJECT` and can block on a hung application, so it is documented
  as callable only from deadline-guarded query threads.
- `NodeIdRegistry` — maps UIA runtime IDs to stable `NodeId`s; takes an
  injected shared counter so the UIA and MSAA registries in one outpost
  never hand out the same id. `init_mta()`, role and state mapping in
  `map`.

Implementation note on states: UIA returns default values for pattern
properties on elements that lack the pattern — `ToggleState` reads as
Indeterminate on a plain pane, which briefly made everything announce
"half checked". The state rebuild therefore gates on the cached
`IsTogglePatternAvailable` and `IsExpandCollapsePatternAvailable` flags and
only interprets those properties when the pattern is genuinely there. The
event handlers are COM objects generated by `windows-core`'s `#[implement]`
macro, with the macro's generated glue wrapped in a small module that
scopes lint allowances to generated code only.

## verbatim-ia2

The minimal MSAA client (IA2 interface acquisition is an explicit seam
left for M3).

Public API:

- `WinEventHook::install(target_pid, callback)` — out-of-context WinEvent
  hooks scoped to one process id, for focus, value, state, and name
  changes; callbacks are delivered on the installing thread's message loop
  and must never make blocking calls into the target. `WinEventKind` names
  the event; drop unhooks.
- `acquire` — the query-pool side: `snapshot_from_event` (from
  `AccessibleObjectFromEvent` through name, role, value, and state reads to
  a `NodeSnapshot`), `resnapshot` for fetches, and `focused_snapshot`,
  which answers "what is focused right now" via `GetGUIThreadInfo` for the
  synthetic focus event an outpost emits after retargeting.
- `map` — `role_from_msaa` and `states_from_msaa`, the tables from
  MSAA constants to the normalized vocabulary, pinned by unit tests against
  raw state words captured from live controls.
- `NodeIdRegistry` keyed by window handle, object id, and child id, sharing
  the outpost-wide counter with the UIA registry.

## verbatim-outpost

The per-application outpost process, the Core-side supervisor, the
foreground trigger, and the private protocol between them (architecture
sections 1 and 4, decision D9).

Public API:

- `protocol` — the wire vocabulary, frozen in phase 1 of M1.
  `SupervisorToOutpost`: `Configure` (set or retarget the watched
  application), `Fetch`, `Ping`, `Shutdown`. `OutpostToSupervisor`:
  `Ready`, `Event` (trace id, observation timestamp, backend, snapshot
  version, normalized event), `FetchReply`, `Pong`, `Fault`. Framing is
  newline-delimited compact JSON via `write_message` and `read_message`.
- `Arbitrator` — NVDA's per-window backend decision:
  `resolve_with(hwnd, class, probe)` walks the ladder (good class list, bad
  class list seeded from NVDA's, then the injected probe), caches verdicts
  per window handle for 500 ms, and supports a forced override from
  `Configure`. The probe is a closure so tests fake it.
- `QueryPool` — the deadline-guarded workers: `run(deadline, work)` blocks
  the caller up to the deadline and abandons the call on expiry (the worker
  stays parked, a counter increments, and a replacement spawns — recovery
  ladder rung two, since a thread blocked in a hung app's COM call cannot
  be safely killed); `submit` is fire-and-forget. Workers lazily own their
  own `Uia` client.
- `Outpost`, `run_pipe`, `run_attach` — the runtime. `run_pipe` is the
  production mode over inherited pipe handles; `run_attach` watches a pid
  directly and prints outbound messages as JSON lines to stdout, the
  standalone dev mode.
- `Supervisor` — `new(events_tx)`, `target(pid)` (spawn or retarget; the
  call to make on every foreground change), `send(command)`. State is a map
  keyed by target pid — N-ready by construction, with an M1 policy of one
  entry — and a reader thread per outpost forwards messages into the
  channel, respawning on end of stream if that outpost is still current.
- `ForegroundTrigger::new(callback)` — the one global WinEvent hook in
  Core: a dedicated thread reporting only the new foreground window's pid
  and handle, no property fetches, wired to `Supervisor::target`.

Implementation notes:

- Spawning (`Supervisor`): the outpost is created suspended with two
  anonymous pipes whose child ends are the only inheritable handles, placed
  in a job object carrying kill-on-job-close and a 200 MB memory cap, and
  only then resumed — inside the job before executing a single
  instruction. Core holds the only job handle, so kernel teardown of Core,
  however it dies, kills every outpost; the reverse direction is a pipe
  close the supervisor answers by respawning.
- Event flow (runtime): the event thread hosts the WinEvent hooks and a
  rebind message; UIA registration lives on its own thread; both funnel
  through the cross-filter (an MSAA event whose window arbitrates to UIA is
  dropped, and vice versa for UIA focus) so exactly one backend survives
  per window. Because every Win32 and wx control is its own window handle,
  per-window arbitration is per-control there, while a WinUI top level
  resolves once for its whole subtree. Trace IDs are minted when the OS
  event first arrives, snapshot versions increment per emitted event, and
  each event carries its observation timestamp for the latency ledger.
- On `Configure`, the outpost rebinds its hooks, then queries the currently
  focused element on a query-pool thread and emits a synthetic
  `FocusChanged` — announcing the focus change that caused its own spawn
  without having witnessed it.

## verbatim-control

The control plane (architecture section 10, decision D8): protocol v0 and
the named-pipe server. Pulled forward from M2 so every M1 change is
verifiable live.

Public API:

- `protocol` — `Request` (`Hello`, `Status`, `SubscribeEvents`,
  `SubscribeSpeech`, `SendGesture`, `SendKeys`, `Latency`, `Quit`) in a
  `RequestEnvelope` with a correlation id; `Frame` (`Reply`, `Error`,
  `Event`, `Speech`); `StatusInfo`, `OutpostStatus`, `LatencyRecord`;
  `PIPE_NAME`, `PROTOCOL_VERSION`; the same newline-JSON framing helpers.
  A speech frame carries the trace id, rendered text, the observation
  timestamp of the triggering event when there is one, the queue time, and
  the audio-start time once known.
- `ServerHandlers` — the app-injected callbacks answering status, gesture
  routing, latency queries, and quit, keeping this crate ignorant of the
  application's internals.
- `ControlServer` — `start(handlers)` on the well-known pipe name,
  `start_on(name, handlers)` for tests; `broadcast_event(..)` and
  `broadcast_speech(..)` fan frames out to subscribed connections; drop
  stops accepting and disconnects every client.
- `send_keys` — `parse_combo` and `parse_all` (validating every entry
  against the shared key-name vocabulary before anything is injected) and
  `inject`, which synthesizes the modifier-down, key, modifier-up sequence
  via `SendInput` with correct extended-key flags.

Implementation notes, the server: the pipe is created with a security
descriptor restricting access to the owning user and with remote clients
rejected — a Verbatim inside a VM is driven by a client inside that VM,
and the future remote feature is a separate authenticated transport. Every
pipe instance uses overlapped I/O with the calls wrapped to look
synchronous. That is a correctness requirement, not a style choice: a
synchronous pipe handle serializes its I/O directions at the driver level,
so a pending blocking read on one thread blocks a concurrent write from
another thread — even across duplicated handles — which deadlocked the
original implementation. With overlapped I/O one handle serves a dedicated
reader thread and a dedicated writer thread per connection. Each
connection's writer drains a bounded queue (256 frames) and drops with a
warning when a client stalls, so a slow inspector can never block Core.
The per-connection dispatch loop is generic over reader and writer, which
is what lets a loopback test exercise the identical code path with no pipe.

## verbatim-inspect

The developer CLI over the control plane. Deliberately not a child of Core
and in no job object; it attaches through the pipe like any client. Output
is plain text, one fact per line — no tables, no spinners — so it reads
well piped, redirected, or through a screen reader.

Subcommands: `status`; `watch-events`; `watch-speech`; `watch` (both
subscriptions on one connection, lines prefixed `event` or `speech`,
interleaved in arrival order so an event reads directly above the speech
it caused); `send-gesture`; `send-keys`; `latency --last N`; `quit`. Each
speech line shows the queue-time delta since the triggering event, and a
follow-up line appears when audio actually starts, carrying the true
event-to-audio latency; an interrupted utterance simply never gets the
follow-up. Timestamps render as local wall-clock time
(`2026-07-14T10:42:32.158`, no zone suffix) via the Win32 conversion that
is correct across DST transitions.

The `Client` type completes the `Hello` handshake and matches replies by
correlation id, discarding stream frames that arrive while a reply is
pending. It is single-threaded by design, which is why its shared-handle
`try_clone` is safe where the server needed overlapped I/O.

## verbatim-gui

The wxDragon GUI (decision D4, architecture section 11): the hidden main
frame, the tray icon and Verbatim menu, and the NVDA-style settings dialog.

Public API:

- `GuiCommand` (`ShowMenu`, `OpenSettings`, `Shutdown`) and `GuiEvent`
  (`QuitRequested`) — the two seams to the application. The GUI never exits
  the process; Exit raises `QuitRequested` and the app decides.
- `GuiHandle::send(command)` — cloneable, callable from any thread;
  internally enqueues through wxDragon's call-after queue and wakes the
  idle loop.
- `run_gui(settings_host, events, on_ready)` — runs the event loop on the
  calling thread (the app calls it from the process main thread); once the
  frame and tray exist, `on_ready` hands out the `GuiHandle`.
- `plan` — the pure, unit-tested layer: `plan_for(descriptor, value)` maps
  a `SettingDescriptor` to a `ControlPlan` (slider, choice, or check box
  with clamped initial value), `accessible_name` strips ampersand
  mnemonics for accessible names (a check box otherwise announces as a
  bare "check"), `cycle_index` implements category wraparound, and
  `DialogGuard` is the settings-dialog singleton state machine.

Implementation notes: the hidden one-by-one frame titled "Verbatim" is the
single-instance rendezvous and dialog parent. One localized menu object
serves both the tray icon and the Verbatim+V popup. Showing the menu or the
dialog performs NVDA's prePopup dance — show the frame, raise it, and force
it foreground through the native window handle (falling back to the
attach-thread-input maneuver when Windows' foreground lock refuses) —
because a popup from a hidden background window never receives foreground
or keyboard focus, and Verbatim's own outpost would never retarget to it.
The frame hides again after the menu closes or the dialog is dismissed. The
settings dialog mirrors NVDA's shape: a labeled single-column report list
of categories on the left, a lazily built panel on the right, OK, Cancel,
and Apply buttons, hand-rolled Enter, Ctrl+S, and Ctrl+Tab handling
(wxDragon binds no accelerator tables), and a title that tracks the active
category. The Speech panel is generated from the settings host's
descriptors; every control change applies live, OK and Apply persist,
Cancel reverts. Escape and window close follow the dialog's escape id.

## verbatim-app

The composition root: `verbatim.exe`.

Public surface: none — this is the binary. Internal structure worth
knowing for review:

- `main` orders startup: namespace trace IDs, load config (materializing
  missing files), start tracing, replace any running instance, load
  locales, then `run`.
- `single_instance::acquire_replacing` — NVDA's algorithm: find the old
  instance's hidden window by title, post `WM_QUIT` so its loop exits and
  its normal teardown runs, wait four seconds, `TerminateProcess` as the
  fallback with a further wait; then serialize startup on a named mutex
  (an abandoned mutex — a crashed predecessor — still grants ownership,
  with a warning). `ChangeWindowMessageFilter` lets a future
  lower-integrity replacer's quit message through.
- `latency::LatencyLedger` — the bounded ring of timelines keyed by trace
  ID, fed from three threads across two processes: the reducer thread
  records event observation (using the outpost's own timestamp), and the
  pipeline observer callbacks record queue and audio start. It broadcasts a
  speech frame at queue time (carrying the observed-to-queued delta) and a
  follow-up frame at audio start, and it answers the `latency` command
  newest first. Core-originated speech with no event reports its queue time
  as the timeline start.
- `run` wires everything: the speech pipeline (OneCore through WASAPI,
  configured from the base profile, observed by the ledger), the settings
  host with a persist callback writing through the config store, the
  supervisor plus foreground trigger (targeting the current foreground
  immediately, since the trigger only fires on changes), the reducer thread
  (drains outpost messages, feeds `reduce`, executes effects — `Speak` to
  the pipeline, `Fetch` back to the outpost), the gesture router (bound
  gestures to `GuiCommand`s, never into the reducer), the control server
  with its injected handlers, the keyboard hook last among input paths, the
  startup announcement, and finally the GUI loop on the main thread. When
  the loop exits — Exit item, control-plane quit, or a replacing instance's
  `WM_QUIT` — teardown drops the hooks and lets job objects reclaim the
  outposts.

## Placeholders and tooling

- `verbatim-uia-rops` — UIA remote operations, lands in M4.
- `verbatim-ext` and `verbatim-ext-api` — the Wasm extension host and WIT
  contract, land in M5.
- `mockapp` — the scripted UIA and MSAA provider host for provider-level
  tests, lands in M2.
- `xtask` — workspace automation. `cargo xtask ci` is the standard check
  and exactly what GitHub Actions runs: rustfmt, pedantic clippy with
  warnings denied, unit tests on x64, then a release-profile ARM64
  cross-build (build-verified only; never run on this x64 machine). It
  probes known Visual Studio and LLVM locations for `libclang.dll` so
  wxDragon's bindgen works without manual environment setup. `cargo xtask
  vm` is a stub until the M2 Hyper-V harness.
