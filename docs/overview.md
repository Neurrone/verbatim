# Crate Overview

A reviewer's guide to the workspace as of the end of milestone M2. Crates
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
  role, optional name and value, states, plus a `NodeDetails` group of
  optional properties (description, keyboard shortcut, position in set,
  set size, level, bounding `Rect`) that backends fill as they learn to
  fetch each one; everything in it defaults to "not reported", and its
  serde default keeps snapshots recorded before it existed deserializing
  unchanged.
- `TreeNode` — a `NodeSnapshot` plus its children in tree order: the
  shared vocabulary for a walked tree, carried unchanged by the outpost
  protocol's `DumpTree` reply and the control protocol's `DumpTree` reply,
  so a tree dump travels from the outpost through Core to
  `verbatim-inspect` without translation.
- `NormalizedEvent` — `FocusChanged` (carrying a full snapshot),
  `PropertyChanged` (name, value, or the complete new `States` set), and
  `ValueChanged`.
- `Input` and `Effect` — the reducer's contract. Inputs are strictly
  accessibility-shaped: events, fetch completions, timer ticks. Effects are
  strictly `Speak`, `StopSpeech`, `Fetch`, and `PlayEarcon` (an `Earcon`
  names a sound semantically — `AppNotResponding` first — and themes decide
  what it sounds like); menu and quit concerns never appear here.
- `Utterance`, `UtteranceSegment`, `SegmentContent`, `UtteranceSource`,
  `SpeechPriority` — structured speech per decision D12. Segments are
  semantic spans: literal text, `Label`, `Value`, `Description`, role and
  state tokens (including `NegatedState` for announcements like "not
  checked"), `Position` (a "2 of 5" pair), and `Level`. The pure reducer
  never touches localization; spans become words at the speech pipeline's
  presentation stage. An utterance optionally carries an
  `UtteranceSource` — the described node's role and screen rectangle — so
  M11 presentation themes can key earcons off the role and pan audio by
  position without a pipeline change.
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
- `position_in_set(position, set_size)` and `level(n)` — the localized
  "2 of 5" and "level 3" phrases for the corresponding utterance spans.
  Both pass their numbers as pre-rendered strings so no locale applies
  digit grouping to an ordinal position.

Implementation notes: the loader disables Fluent's bidi argument isolation
globally — Fluent wraps interpolated arguments in invisible directional
isolate marks by default, which protects visually rendered mixed-direction
text but would leak invisible characters into spoken text, dictionary and
symbol processing, and braille. `LocaleDirAssets` exists because
i18n-embed's own filesystem assets type yields bare file names without the
language folder, which breaks language negotiation; this implementation
yields `language/file` paths. The pseudo-locale test (required from M1) generates
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
  `verbatim_keys`, `keyboard` (all global; profiles cannot carry them), and
  `speech` (the base profile's section).
- `VerbatimKeys` — which keys act as the Verbatim modifier (`caps_lock`,
  `insert`, `numpad_insert`) plus `share_modifier`, which passes the
  modifier's own transitions down the hook chain for a screen reader
  running behind Verbatim.
- `KeyboardConfig`, `KeyboardLayout` — the active gesture-binding layout
  (`desktop`, the default, or `laptop`), M3's `keyboard` section. Exposed
  only in `settings.toml`; a GUI choice arrives with M8's gesture-remapping
  work. `verbatim-input`'s `bindings_for` consumes the resolved layout
  through its own decoupled layout enum (the app maps one to the other),
  the same pattern `DecisionConfig` already follows for `VerbatimKeys`.
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
`TraceId`, blocking `write`, draining `end`, discarding `stop`),
`WasapiSink`, and `NullSink`.

Implementation notes, `WasapiSink`: event-driven shared mode with a small
(roughly 40 ms) buffer, initialized with the auto-convert-PCM flags so any
synth output rate is accepted without manual resampling; the device opens
lazily on the first `begin` and the stream is reused across utterances of
the same format. COM is initialized per thread as MTA, tolerating
`RPC_E_CHANGED_MODE`. On the first buffer of each utterance it emits the
`audio_started` tracing event tagged with the utterance's trace ID — the
final leg of the latency timeline. `stop` issues Stop plus Reset to discard
buffered audio immediately, which is how speech interruption sounds instant.

Implementation notes, `NullSink`: a device-free sink for test and CI runs
with no sound card, active only when `verbatim-app` sees
`VERBATIM_TEST_AUDIO=null` at startup (test-only; documented there).
Accepts any format and discards every sample, but still emits the
`audio_started` tracing event on the first `write` of each utterance,
exactly when `WasapiSink` would emit it on its first real buffer, so
`LatencyLedger` still records a complete timeline with nothing actually
playing.

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
- `Theme` and `PlainTheme` — the presentation stage (decision D12): a theme
  flattens each structured utterance to the flat request handed to the
  synthesizer, on the queue thread, just before dispatch. `PlainTheme` is
  the default (selected when `SpeechManagerConfig::theme` is `None`) and
  renders plain speech: labels, values, and descriptions as their text,
  roles and states through `verbatim-i18n`, positions as "2 of 5" (nothing
  without a set size — a bare position has no useful spoken form), levels
  as "level 3". M11's earcon and voice-styling themes implement the same
  trait, which is why utterances carry their source node's role and screen
  rectangle even though `PlainTheme` ignores both.

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

The pure reducer (architecture section 2), the flight recorder, and its
on-disk dump format.

Public API:

- `reduce(state, input)` — the frozen signature: pure, no I/O, no clocks;
  returns the next state and the effects to execute.
- `SrState` — `new()`, `focused()`, `last_seen_version(source)`,
  `pending_fetch_count()`.
- `FlightRecorder<T>` — a bounded ring buffer; `ReducerRecorder` and
  `RecordedInput` specialize it for reducer inputs; `replay(initial,
  inputs)` re-runs a recorded sequence and returns the effects per step,
  proven deterministic by test. `RecordedInput` derives `Serialize` and
  `Deserialize` so it survives the trip to disk.
- `dump` — the flight-recorder dump format (milestone M2): a versioned
  JSON-lines file, a header line (`DumpHeader`: format version, the writing
  crate's version, and a caller-supplied timestamp string — this module
  never reads a clock itself) followed by one compact-JSON `RecordedInput`
  per line. `write_dump(writer, crate_version, timestamp, inputs)` writes
  one; `read_dump(reader)` reads one back as `DumpContents` (header, the
  inputs that parsed completely, and whether the file ended mid-line).
  `DumpReadError` distinguishes a missing header, a malformed header, an
  unsupported format version, and a malformed *complete* line — a
  malformed final line with no trailing newline is not an error: the
  writer always terminates a complete line with a newline, so an
  unterminated tail can only be a crash-time dump cut off mid-write, and
  `read_dump` reports it as `DumpContents::truncated` instead of failing,
  keeping every record parsed before the cut. `crates/verbatim-core/tests/
  replay_fixture.rs` commits a dump captured from a scripted focus-change,
  value-change, states-change session under `tests/fixtures/` and replays
  it on every test run, asserting the per-step effect counts match what
  was recorded and that a second replay is identical — the template every
  future live-dump regression follows; its `regenerate_fixture` test
  (`#[ignore]`d) is how the fixture was produced and how an intentional
  change to the scripted shapes regenerates it.

Implementation notes, `reduce`:

- A focus change speaks name, role, value, then states in a fixed order
  (checked or its negation first, then mixed, pressed, selected, expanded,
  collapsed, has-popup, default, read-only, disabled, busy), at Interrupt
  priority. Per decision D12 the name travels as a `Label` span and the
  value as a `Value` span, never anonymous text, and every utterance
  carries its source node's role and rectangle (`UtteranceSource`) for
  presentation themes. The negated-checked rule: a `CheckBox` or `RadioButton`
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
- `nearest_window_handle(element)` — NVDA's `getNearestWindowHandle`:
  resolves the native window handle of `element` itself, or of its nearest
  ancestor that has one, in one cross-process round trip
  (`NormalizeElementBuildCache` against a tree walker whose condition
  excludes every element without a native window handle). If `element`
  already has a window handle the call still works, since `NormalizeElement`
  degenerates to returning the starting element unchanged. Like the probe
  above, this sends a cross-process COM call and can block on a hung
  application; unlike the probe, it is documented as callable from UIA
  event-callback threads specifically because each outpost watches a single
  application (decision D9), so a hang here stalls only that application's
  own outpost, which the recovery ladder already covers — the same trade
  NVDA makes running this same walk on its own UIA event-handler thread.
  Backed by a thread-local `IUIAutomation` instance, walker, and cache
  request, lazily built the first time a given thread calls it and reused
  after that, matching `Uia`'s one-client-per-thread rule so nothing COM
  here crosses a thread boundary.
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
left for M6, landing with the browsers that motivate it; M3's backend
parity work covers MSAA and UIA only).

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
sections 1 and 4, decision D9, implemented in full — generalized from M1's
single instance to the many-concurrent-processes design at the end of M2).

An outpost's target application is fixed at spawn and never retargeted:
the pid arrives on its command line (`--target-pid`), hooks and UIA
registrations install once during construction, and there is no rebind
path. Outposts stay alive when their application loses foreground — a
foreground change to an application Core already has an outpost for sends
that outpost an `AnnounceFocus`, never a respawn.

Public API:

- `protocol` — the wire vocabulary the supervisor and each outpost speak.
  `SupervisorToOutpost`: `SetBackendOverride` (forces one backend for every
  window of the target, or restores normal arbitration — the old
  `Configure`'s backend-override half), `AnnounceFocus` (the old
  `Configure`'s implicit synthetic-focus half, now explicit and reusable
  across the outpost's whole life, not just at spawn), `Fetch`, `Ping`,
  `DumpTree` (walk the target's tree from its top-level window),
  `Shutdown`. `OutpostToSupervisor`: `Ready`, `Event` (trace id,
  observation timestamp, backend, snapshot version, normalized event),
  `FetchReply`, `Pong`, `DumpTreeReply` (a `DumpedTree` — the root
  `verbatim_model::TreeNode` plus whether the walk was truncated — or a
  human-readable failure reason), `Fault`. Framing is newline-delimited
  compact JSON via `write_message` and `read_message`.
- `Arbitrator` — NVDA's per-window backend decision:
  `resolve_with(hwnd, class, probe)` walks the ladder (good class list, bad
  class list seeded from NVDA's, then the injected probe), caches verdicts
  per window handle for 500 ms, and supports a forced override from
  `SetBackendOverride`. The probe is a closure so tests fake it.
- `QueryPool` — the deadline-guarded workers: `run(deadline, work)` blocks
  the caller up to the deadline and abandons the call on expiry (the worker
  stays parked, a counter increments, and a replacement spawns — recovery
  ladder rung two, since a thread blocked in a hung app's COM call cannot
  be safely killed); `submit` is fire-and-forget. Workers lazily own their
  own `Uia` client.
- `Outpost`, `run_pipe`, `run_attach` — the runtime. `Outpost::new(writer,
  target_pid)` installs hooks and UIA registrations for the fixed pid and
  announces `Ready`; `run_pipe` is the production mode over inherited pipe
  handles; `run_attach` watches a pid directly, immediately announces its
  focus, and prints outbound messages as JSON lines to stdout, the
  standalone dev mode.
- `Supervisor` — `new(events_tx)`, `note_foreground(pid)` (spawn if this
  pid has no outpost yet, otherwise send the existing one an
  `AnnounceFocus`; the call to make on every foreground change),
  `ensure_spawned(pid)` (warm an outpost without touching foreground
  tracking — used once, at Core startup, for Core's own pid; see its doc
  comment), `send_to(pid, command)`. State is a map keyed by target pid,
  genuinely N-ready now: a reader thread per outpost forwards messages into
  the channel as `OutpostMessage::Event(pid, message)`, respawning on end
  of stream only if that pid's map entry still has the same generation
  *and* the watched application's process is itself still alive (checked
  via `OpenProcess`/`GetExitCodeProcess`) — otherwise the entry is dropped
  and Core is told via `OutpostMessage::Retired(pid)`. A background sweep
  (every foreground change, plus a coarse 30-second timer) retires any
  outpost whose application has not held foreground for two minutes
  (`IDLE_RETIREMENT`, risk R2's memory-use mitigation), skipping whichever
  pid currently holds foreground; retirement removes the map entry *before*
  sending `Shutdown`, which is what makes the ordinary respawn path
  correctly do nothing for a deliberate retirement instead of resurrecting
  it.
- `ForegroundTrigger::new(callback)` — the one global WinEvent hook in
  Core: a dedicated thread reporting only the new foreground window's pid
  and handle, no property fetches, wired to `Supervisor::note_foreground`.

Implementation notes:

- Spawning (`Supervisor`): the outpost is created suspended with two
  anonymous pipes whose child ends are the only inheritable handles, placed
  in a job object carrying kill-on-job-close and a 200 MB memory cap, and
  only then resumed — inside the job before executing a single
  instruction. Core holds the only job handle, so kernel teardown of Core,
  however it dies, kills every outpost. The command line also carries
  `--target-pid`, fixing the watched application for the outpost's whole
  life; immediately after resuming, the supervisor writes the spawn's own
  implicit `AnnounceFocus` down the pipe (buffered by the OS; the outpost
  need not be reading yet).
- Foreground announcements (`AnnounceFocus`, `run_announce`): the newly
  authoritative outpost announces the top-level foreground window, then the
  focused control, both sharing one retry budget — up to five attempts
  across roughly two seconds. The window step stops retrying once it
  succeeds (or is deliberately skipped, when every top-level window is
  Core's own hidden frame); the control step keeps going until it succeeds
  or the attempts run out. A single deadline-guarded attempt for the window
  step was tried first, reasoned as safe because Windows raises the
  foreground event only once the application's top-level window already
  exists — true in the common case, but live testing against the VM under
  load found `EnumWindows` and `GetForegroundWindow` can still race a
  window's own creation closely enough to miss it on the very first
  attempt, losing the window announcement outright with no later chance to
  recover it; the window step now retries for exactly that reason. It
  reads the window's own accessible object specifically: UIA via
  `element_from_handle`, MSAA via a direct `OBJID_WINDOW` query (not
  `OBJID_CLIENT`, which is what `DumpTree`'s walk starts from and which
  reads back as role "client", unmapped to anything nameable — confirmed
  live against Windows 11 Notepad, whose window announcement read "Untitled
  - Notepad, unknown" until this was fixed). The window itself is located
  by `GetForegroundWindow`, deliberately not `GetGUIThreadInfo`'s
  `hwndFocus`: the latter can legitimately name a non-top-level descendant
  that still belongs to the target process (Windows 11 Notepad hosts its
  text area in its own child `hwnd` distinct from the frame), which made an
  earlier version of this code read the edit control's own snapshot instead
  of the window's. The control step's retries answer a different race —
  the second focus-timing race `docs/roadmap.md`'s M2 section names, the
  foreground trigger and this query racing the target process's own
  control creation — using `GetGUIThreadInfo`'s `hwndFocus` specifically,
  since that question ("what control is focused") is genuinely different
  from "what is the top-level window". The whole loop runs off the command
  loop on its own thread so `Ping` and `Fetch` stay responsive during the
  retry window; a per-outpost generation counter, bumped on every
  `AnnounceFocus`, is compared before every attempt and before every
  emission, so a superseding announce (a rapid re-foreground, or several in
  Notepad's own bursty startup events) aborts a stale retry loop rather
  than letting it starve real event acquisition or emit late.
- Hidden-frame suppression (decision D9): Core's hidden 1x1 main frame is
  marked with the `verbatim_model::HIDDEN_FRAME_WINDOW_PROP` window
  property by `verbatim-gui` (see that crate's section) and must never be
  announced — it transits real focus during the prePopup show/raise/force-
  foreground dance and would otherwise read as a nameless "Verbatim"
  window with role unknown, the first of the two M2 focus-timing races.
  Every `FocusChanged` emission path checks the property (`GetPropW`, which
  tolerates any handle and never blocks) before emitting: the MSAA event
  path (scoped to `id_child == verbatim_ia2::CHILDID_SELF`, re-exported
  from that crate's `com` module for exactly this check, so a child
  element's event on some unrelated window is never accidentally
  suppressed by hwnd coincidence), the UIA focus callback (reusing the
  window handle the arbitration filter already resolved, rather than
  resolving it twice), and the synthetic focus query (`focused_snapshot`
  treats the hidden frame as "nothing focused", so a caller retrying on
  `None` naturally retries past it). The top-level-window announcement's
  own window search additionally skips hidden-frame windows when choosing
  among a process's top-level windows, so resolution lands on a real window
  (a popup menu, a dialog) instead.
- `verbatim-gui`'s `force_foreground` (see that crate's section) injects a
  bare `VK_CONTROL` tap before attempting `SetForegroundWindow`: a gesture
  that arrived via the control plane (no physical input, as every E2E test
  and any future remote session sends) fails Windows' foreground-lock
  heuristic and falls back to the slow `AttachThreadInput` path, observed
  at roughly two seconds; the tap satisfies the heuristic directly, cutting
  that to roughly 150 to 450 ms. `VK_MENU` was tried and rejected — a lone
  Alt press activates menu bars and bounces foreground straight back.
- Event flow (runtime): the event thread hosts the WinEvent hooks,
  installed once for the fixed target pid before the message loop starts
  (never rebound — a second live `WINEVENT_OUTOFCONTEXT` hook set on a
  thread that already has one has been observed to permanently kill
  WinEvent delivery on that thread for the rest of the process, which decision
  D9's one-pid-per-outpost-for-life design sidesteps entirely rather than
  risking); UIA registration lives on its own thread; both funnel through
  the cross-filter so exactly one backend survives per window. Because
  every Win32 and wx control is its own window handle, per-window
  arbitration is per-control there, while a WinUI top level resolves once
  for its whole subtree. Trace IDs are minted when the OS event first
  arrives, snapshot versions increment per emitted event, and each event
  carries its observation timestamp for the latency ledger.
  - MSAA side (`handle_msaa_event`): the event's own hwnd is exact — MSAA
    events always carry the real window, never an inferred one — so the
    filter just arbitrates it directly. A UIA verdict drops the MSAA event;
    a non-UIA verdict or no verdict yet delivers it, scheduling a probe in
    the no-verdict case. Delivering on an unresolved verdict is what makes
    dropping safe on the UIA side below: MSAA is the backend of record
    whenever arbitration has not yet decided.
  - UIA side (`uia_passes_filter`): most elements that raise UIA events are
    not windows themselves — a menu item or a list item is a descendant of
    one — so the cached native window handle is usually 0. Attribution
    resolves the window in three tiers: the cached handle when the element
    is itself a window, otherwise `verbatim_uia::nearest_window_handle`
    (NVDA's `getNearestWindowHandle`, one cross-process call that walks up
    to the nearest ancestor with a real handle), and only if that itself
    fails, the window holding keyboard focus as a last resort; finding no
    window at all keeps the event, since there is nothing to arbitrate on.
    Once a window is attributed, a UIA verdict delivers, a non-UIA verdict
    drops, and no verdict yet drops while scheduling a probe — symmetric
    with the MSAA side's delivery in that case, because the MSAA hook for
    the same logical element carries the event instead. This replaced an
    M1 heuristic that used the keyboard-focus window unconditionally, which
    is wrong for a popup menu: a popup never takes keyboard focus, so the
    heuristic found the menu's *owner* window while the MSAA event for the
    same menu item carried the popup window itself, and the two backends
    could both defer on their two different windows, losing the
    announcement entirely (found by the M2 E2E suite against the VM). The
    heuristic's compensation was to keep every event on an unresolved
    verdict rather than risk that silence, at the cost of occasional
    duplicate announcements. `nearest_window_handle` resolves a popup menu
    item straight to the popup window itself — the same window the MSAA
    event for that item carries — so both backends now arbitrate on one
    shared hwnd and the compensation is no longer needed.
- `DumpTree` (runtime): answered on a query-pool thread guarded by a five
  second deadline (`QueryPool::run`), the same pattern the foreground
  announcement's queries use, so a hung target abandons the call rather
  than wedging the outpost. Finds the target's currently active top-level
  window the same way the announcement's window step does (`GetForegroundWindow`,
  falling back to its first non-hidden-frame top-level window), arbitrates
  its backend, then walks it: UIA via `Uia::walk_tree`, a raw-view
  `IUIAutomationTreeWalker` driven with the same cache request as every
  other UIA read, so no step of the walk blocks on an uncached property;
  MSAA via `verbatim_ia2::acquire::walk_tree` (rooted at `OBJID_CLIENT`,
  unlike the window announcement's `OBJID_WINDOW` query — a tree dump wants
  the client subtree, not the window's own accessible object), recursing
  through `AccessibleChildren` since this backend has no cache requests to
  prefetch with. Both walkers share the same caps — depth 64, node count
  4096 across the whole walk — and report whether either cap cut the walk
  short.

## mockapp

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
`read_only`), and `children` (nested nodes). The root node conceptually
corresponds to the window itself. `fixture::role_from_fixture_str` and
`state_from_fixture_str` hold the complete name tables.

Stdin commands, one per line: `focus <id>` (raises the backend's
focus-changed notification — `UiaRaiseAutomationEvent` for UIA,
`NotifyWinEvent(EVENT_OBJECT_FOCUS, ...)` for MSAA), `set-name <id> <text>`
and `set-value <id> <text>` (update the tree and raise the matching
property-change or name/value-change notification), and `quit`.

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
- **Toggle, expand-collapse, and value need real pattern objects.**
  `GetPropertyValue` overrides for pattern-availability and pattern-value
  properties are documented as an optional shortcut, but empirically
  `IUIAutomationCacheRequest`'s cache-building still calls
  `GetPatternProvider` for `TogglePattern`, `ExpandCollapsePattern`, and
  `ValuePattern` before trusting a cached value — which is exactly the path
  `verbatim-uia`'s base cache request always uses. mockapp therefore
  implements small `IToggleProvider`, `IExpandCollapseProvider`, and
  `IValueProvider` objects (returned from `GetPatternProvider`, gated on
  role for toggle and on the fixture's `expanded`/`collapsed` states or
  presence of a `value` for the other two) alongside the `GetPropertyValue`
  overrides, rather than relying on the shortcut alone.
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
roles, names, values, and states match the fixture. `arbitration.rs` asserts
`verbatim_uia::has_server_side_provider` and `verbatim_outpost::Arbitrator`
resolve a `uia`-backend window to UIA and a `msaa`-backend one to MSAA.
`events.rs` asserts that `set-name`/`set-value` commands are observed by
`verbatim_uia::PropertyRegistration` and `verbatim_ia2::WinEventHook`
respectively — property and value changes are used rather than focus, so
the tests never depend on real keyboard focus or `SetForegroundWindow`
succeeding, and pass headless on GitHub `windows-latest` runners.


## verbatim-control

The control plane (architecture section 10, decision D8): protocol v0 and
the named-pipe server. Pulled forward from M2 so every M1 change is
verifiable live.

Public API:

- `protocol` — `Request` (`Hello`, `Status`, `SubscribeEvents`,
  `SubscribeSpeech`, `SendGesture`, `SendKeys`, `Latency`, `DumpTree`,
  `DumpRecorder`, `Quit`) in a `RequestEnvelope` with a correlation id;
  `Frame` (`Reply`, `Error`, `Event`, `Speech`); `StatusInfo`,
  `OutpostStatus`, `LatencyRecord`; `PIPE_NAME`, `PROTOCOL_VERSION`; the
  same newline-JSON framing helpers. A speech frame carries the trace id,
  rendered text, the observation timestamp of the triggering event when
  there is one, the queue time, and the audio-start time once known.
  `ReplyPayload::DumpTree` answers `Request::DumpTree` with the walked
  tree (`verbatim_model::TreeNode`) and whether the outpost's depth or
  node-count cap cut it short; a walk that could not complete at all comes
  back as `Frame::Error`, the same convention `SendGesture` uses.
  `ReplyPayload::DumpRecorder` answers `Request::DumpRecorder` (milestone
  M2) with the path Core wrote its flight recorder's contents to.
- `client` — the control-plane client, promoted here from
  `verbatim-inspect` so any client, not just the CLI, can share it:
  `Client::connect_pipe()` on the well-known pipe, `connect_pipe_named(name)`
  for tests, and `connect_tcp(addr)` for a Verbatim reached over TCP (a
  remote session, or from inside a VM host); `request` completes the
  `Hello` handshake and matches replies by correlation id, discarding
  stream frames that arrive while a reply is pending; `next_frame` reads
  any frame, for subscription loops. Single-threaded by design, which is
  why its shared-handle `try_clone` is safe where the server needed
  overlapped I/O.
- `ServerHandlers` — the app-injected callbacks answering status, gesture
  routing, latency queries, tree dumps, flight-recorder dumps, and quit,
  keeping this crate ignorant of the application's internals.
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

Every subcommand accepts a global `--connect <ADDRESS>` option: an address
of the form `tcp:HOST:PORT` selects TCP, anything else is treated as a
named-pipe path, and omitting it connects to the well-known local pipe —
the same three transports `verbatim-control`'s promoted `Client` exposes.

Subcommands: `status`; `watch-events`; `watch-speech`; `watch` (both
subscriptions on one connection, lines prefixed `event` or `speech`,
interleaved in arrival order so an event reads directly above the speech
it caused); `send-gesture`; `send-keys`; `latency --last N`; `dump-tree`
(prints the foreground application's accessibility tree from its
top-level window, one node per line, indented two spaces per depth level
and reusing the same role-and-name summary the event stream uses; a
trailing line notes when the outpost's depth or node-count cap truncated
the tree); `dump-recorder` (milestone M2: asks Core to write its flight
recorder's current contents to disk and prints the path it wrote to);
`quit`. Each speech line shows the queue-time delta since the triggering
event, and a follow-up line appears when audio actually starts, carrying
the true event-to-audio latency; an interrupted utterance simply never
gets the follow-up. Timestamps render as local wall-clock time
(`2026-07-14T10:42:32.158`, no zone suffix) via the Win32 conversion that
is correct across DST transitions.

## verbatim-agent

The in-guest test agent for the M2 VM harness. Runs inside the interactive
session of a Hyper-V guest, or reachable over loopback on a CI runner, and
is the only externally reachable doorway into that machine: host-side E2E
tests reach it over TCP to manage processes and to tunnel through to
Verbatim's own control-plane named pipe, which deliberately never listens
on the network itself (architecture section 10, decision D8).

Public API:

- `protocol` — the agent's own wire vocabulary, versioned separately from
  the control plane's (`AGENT_PROTOCOL_VERSION`, currently 0) and framed
  with the same newline-JSON helpers the control plane uses
  (`verbatim_control::protocol::write_message`/`read_message`), reused
  rather than reinvented. Deliberately a distinct vocabulary from
  `verbatim_control::protocol`: this crate's pids are raw OS process ids
  naming a process a test is driving (Notepad, `verbatim.exe` itself), not
  `verbatim_model::Pid`, which names an application Verbatim is
  *observing*. `Request`: `Hello` (must be first, refused outright on any
  version mismatch), `LaunchProcess`, `KillProcess`, `ProcessStatus`,
  `SessionInfo`, `ReadFile`, `OpenControlTunnel`. `KillOutcome` makes
  "the process was already gone" a first-class non-error reply
  (`AlreadyExited`) distinct from `Terminated`, rather than an error.
  `LaunchProcess` inherits the launched child's stdio (uncaptured) by
  default; its `stderr_to` field, an `Option<String>` defaulted via
  `serde(default)` so an older client that omits it on the wire still
  deserializes, names a path the agent creates (truncating any existing
  content) and redirects both the child's stdout and stderr into, so a
  Verbatim that panics at launch leaves its message somewhere a host-side
  test or `cargo xtask vm logs` can actually read, instead of vanishing
  with the process.
- `server::serve(listener, pipe_name)` — the TCP accept loop, one thread
  per connection; blocking, so callers needing to do other work run it on
  a background thread.
- `session::current()` — session id, whether the process's window station
  is interactive, and the input desktop's name when it can be opened. This
  is what `Request::SessionInfo` answers, and what the binary checks at
  its own startup: a screen reader test driven from a non-interactive
  session (the "session 0" problem WinRM and PowerShell Direct create) can
  never work, so the agent refuses to even bind a socket in that case,
  with a diagnosis printed instead of a downstream mystery.

Implementation notes: process management (private `process` module)
launches via `std::process::Command`, inheriting the agent's own
interactive session and stdio (never captured) — the reason this exists
at all rather than something reachable over WinRM or PowerShell Direct.
Lookup and termination act on raw pids via `OpenProcess`,
`TerminateProcess`, and `GetExitCodeProcess` rather than tracking handles
from launch, so a test can manage a process it did not itself spawn.
`KillProcess`'s tolerance for an already-exited process handles two
distinct races: a pid that cannot be opened at all, and one that opens
fine but has already exited — the latter discovered because
`TerminateProcess` on a zombie process object returns access denied
rather than "not found," so the exit code is checked both before
attempting termination and after a failure.

The control-plane tunnel (private `tunnel` module) is the crate's most
intricate corner. Opening the pipe is split from running the relay so a
failure to open is reported in `OpenControlTunnel`'s own reply, before any
byte relaying begins. The pipe handle is opened with
`FILE_FLAG_OVERLAPPED` and uses the identical overlapped-read/write
pattern `verbatim_control::server`'s pipe transport uses on the other end
of the same kind of pipe, for the identical reason documented there: a
synchronous handle serializes reads and writes at the driver level even
across independent handles to the same instance, which would deadlock a
full-duplex relay needing one thread reading and another writing at once.
Two threads copy bytes in each direction; whichever direction finishes
first cancels the pipe's pending I/O (`CancelIoEx`) and shuts down the TCP
socket, so the other thread also unwinds instead of hanging.

## verbatim-e2e

The end-to-end suite, restructured in milestone M3 Track B into a scenario
registry: drives a real, running Verbatim (and target applications such as
Notepad) through `verbatim-agent` and, tunneled through it, Verbatim's own
control plane. Dev-only; a library rather than only test binaries because
both `crates/verbatim-e2e/tests/` and `xtask vm test` drive it. See
`docs/tooling.md` for how to run it by hand and how to read a failure.

Public API:

- `endpoint()` — reads `ENDPOINT_ENV` (`VERBATIM_E2E_ENDPOINT`), the
  live-suite skip guard every scenario in this crate checks first; `None`
  means no agent is reachable, and callers print a one-line skip notice and
  return rather than failing. This is what keeps `cargo test` and
  `cargo xtask ci` green with no agent anywhere.
- `AgentClient` — a typed host-side client for `verbatim_agent::protocol`:
  connects over TCP, completes the agent's `Hello` handshake, and exposes
  `launch_process`, `kill_process`, `process_status`, `session_info`,
  `read_file`, and `open_control_tunnel` as plain methods.
  `open_control_tunnel` is the seam into Verbatim's own control plane: it
  asks the agent to stop speaking its own protocol on the connection and
  relay Verbatim's control-plane pipe instead, then completes the control
  protocol's own `Hello` on the same socket and hands back a ready
  `verbatim_control::client::Client`.
- `Scenario` — the lifecycle owner for one live, agent-driven Verbatim run:
  a guard struct, not a manual-cleanup checklist. `Scenario::launch` writes
  a `settings.toml` selecting the capture synthesizer next to
  `verbatim.exe` (audio-free, no installed voices needed — deliberately
  not `OneCore`, whose `new` fails outright with none installed), launches
  Verbatim through the agent with `VERBATIM_TEST_AUDIO=null`, waits for its
  control plane to answer over the agent's tunnel, opens a *second*,
  dedicated tunnel connection for speech collection, and pauses briefly
  (`GUI_SETTLE_DELAY`) for the GUI thread's gesture handle to exist before
  returning — see `docs/tooling.md`'s troubleshooting section for what
  happens to a gesture sent before that pause. `control()` and `speech()`
  expose the two connections; `send_gesture`, `send_keys`, `launch_target`,
  `kill_target`, `process_status`, `quit_verbatim`, and `report_latency`
  drive the running instance; `latency_snapshot` is the non-asserting,
  non-printing fetch the registry's run summary uses (see `registry`
  below), and `collect_failure_artifacts` is what a failed scenario calls to
  save its diagnostics (see `artifacts` below). Its `Drop` impl kills every
  process it launched, unconditionally, even after a panic — because every
  live scenario here launches a real `verbatim.exe` on the real desktop. A
  same-process `Mutex` (`live_instance_lock`) enforces one live instance
  per test *process*, not per machine; `--test-threads=1` (mandatory,
  documented on the type) is what makes that sufficient, since Windows has
  no notion of "only one verbatim.exe" and `single_instance
  ::acquire_replacing` inside `verbatim.exe` *replaces* a running instance
  rather than refusing to start. The hardcoded `SWEPT_TARGET_IMAGE_NAMES`
  constant this pre-launch sweep once read from is gone: it now reads
  `registry::swept_target_image_names()`, derived from every registered
  scenario's own declared target images instead of a name maintained by
  hand.
- `registry` — the scenario registry itself. `ScenarioDef` is one named,
  grouped scenario: `name` (also its `#[test]` function name, its
  `cargo xtask vm test --scenario` selector, its artifacts directory name,
  and its recording file name prefix — one identifier, everywhere),
  `group` (a `Group`: `Speech`, `Shell`, `Legacy`, or `Navigation`, a coarse
  `--group` selector, not a strict taxonomy — see the module's own doc
  comment for what each currently holds), `target_images` (image names its
  `setup`/`teardown` may launch or kill, unioned by
  `swept_target_image_names`), and `setup`/`body`/`teardown` function
  pointers. `SCENARIOS` is the fixed, ordered list of every registered
  scenario — today `m1_exit_regression`, `notepad_focus`, and
  `multi_outpost_switch`, each implemented in `crates/verbatim-e2e/src/
  scenarios/`. `find` looks one up by name; `select` resolves
  `--scenario`/`--group` filters (both repeatable, unioned, deduplicated,
  registry order preserved, empty means "every scenario") into a list,
  erroring on any unrecognized name; `run_named` is the thin entry point
  every `#[test]` wrapper under `crates/verbatim-e2e/tests/` calls.
  `run_named`'s internal `run` launches, runs `setup` then `body` then
  `teardown` — `body` and `teardown` each in their own
  `std::panic::catch_unwind`, so a panicking `body` still lets `teardown`
  run with whatever `setup` produced (borrowed, not moved, so the panic
  leaves it intact) rather than skipping cleanup — asserts a clean
  `quit_verbatim` only when both succeeded (an already-failed scenario's
  Verbatim is in an unknown state, and `Scenario::drop` kills it regardless,
  so nothing more is proved by also demanding a graceful quit), collects
  failure artifacts on any failure, always writes a `ScenarioSummary`, then
  re-raises whatever panic occurred so `cargo test` still reports the
  original failure. None of this weakens `Scenario`'s own guard-struct
  discipline; `setup`/`body`/`teardown` are structure on top of it for
  scenario-specific state `Scenario` itself does not track, not a
  replacement for it. `crates/verbatim-e2e/src/scenarios/` holds the actual
  setup/body/teardown logic per scenario — the scripted walks themselves are
  otherwise unchanged from before the restructuring, just moved out of
  `#[test]` functions into free functions the registry wires together.
- `artifacts` — the host-side seam both a scenario subprocess and
  `cargo xtask vm test` read through without any argument passing between
  them, since both independently compute the same paths.
  `artifacts_root()` is `VERBATIM_E2E_ARTIFACTS_DIR` when set, otherwise
  `target/e2e-artifacts` under the workspace root; `scenario_dir(root,
  name)` joins in the scenario's name. `ScenarioSummary` (`name`, `passed`,
  `latency_records`, `latency_reached_audio`) is written by
  `ScenarioSummary::write` at the end of every scenario run, pass or fail,
  as plain `key: value` lines, and read back by `ScenarioSummary::read` —
  `xtask vm test`'s own run summary is built from this file, never by
  parsing a subprocess's stdout.
- `latency::fetch` — fetches the most recent `last_n` latency timelines with
  no printing and no assertion, the raw building block `report` (below) and
  `Scenario::latency_snapshot` both use.
- `latency::report` — `fetch`, then prints one fact per line and asserts at
  least one timeline reached audio — but only outside audible mode, since a
  real synthesizer is legitimately interrupted before playback at this
  suite's pace (a capture-synth invariant, not a real-synth one).

Implementation notes: `REMOTE_ENV` (`VERBATIM_E2E_REMOTE`) marks a run where
Verbatim lives in a guest rather than sharing this process's filesystem —
set by `xtask vm test`, not normally by hand — and skips the two ordinary
host-filesystem steps (`verbatim.exe` existence check, writing
`settings.toml`) that `xtask vm deploy` has already done inside the guest
instead. `crates/verbatim-e2e/tests/` holds one thin `#[test]` wrapper per
registered scenario (`m1_exit_regression`, `notepad_focus`,
`multi_outpost_switch`, each just calling `registry::run_named` with its own
name) plus `session_info` (the agent reports an interactive session — a
precondition every scenario depends on, not itself a scenario, so it stays a
plain `#[test]` outside the registry). The thin wrappers are what keep
runner-direct CI (`.github/workflows/ci.yml`'s `e2e` job) and plain libtest
filtering (`cargo test -p verbatim-e2e <name> -- --exact`) working
unchanged: `cargo test -p verbatim-e2e -- --test-threads=1` still discovers
and runs every one of them exactly as before the restructuring.
`m1_exit_regression` is the scripted walk of the M1 exit criteria that
`docs/roadmap.md`'s M2 section describes, including exactly what it does and
does not assert about the capture synth's Speech page; `notepad_focus` and
`multi_outpost_switch` are described in `registry`'s own `Group` doc comment
above.

## xtask VM harness

`xtask/src/vm/` implements `cargo xtask vm <verb>` (`docs/architecture.md`
section 14, decision D3): building and importing the golden Hyper-V VM,
deploying builds into it, and running `verbatim-e2e`'s registered scenarios
against it, one at a time (milestone M3 Track B). See `docs/tooling.md` for
the full verb reference and the steps to rebuild the golden image from
scratch; this section is the code-level map.

Public structure (all `pub(crate)`; this is a binary target's internal
module tree, not a library):

- `host` — the `Host` trait: every Hyper-V (or, later, other hypervisor)
  operation a verb needs, abstract enough that a non-Hyper-V implementation
  is plausible (`vm_exists`, `import_vm`, `rename_vm`,
  `ensure_guest_file_transfer`, `start_vm`/`stop_vm`/`restart_vm`,
  `checkpoint_vm`, `restore_checkpoint`, `delete_vm`, `guest_ip`,
  `copy_file_to_guest`, `run_in_guest`, `read_guest_file`,
  `list_guest_dir`). `HyperVHost` is the only implementation, over Hyper-V's
  PowerShell module; every method drives `powershell.exe -File` against a
  temp script (never `-Command`, so arguments only ever cross one layer of
  parsing) and, where a value must come back, wraps it in unique text
  markers rather than trusting a cmdlet's own stdout is clean (some Hyper-V
  cmdlets write incidental text to the success stream). `wait_for_agent`
  is free-standing rather than a trait method: it is pure orchestration
  over other `Host` calls (poll `guest_ip`, then raw-TCP-probe the agent's
  port), so a future non-Hyper-V host gets it for free.
- `create`, `deploy`, `test`, `lifecycle` (`start`/`stop`/`restart`
  /`restore`/`delete`), `logs`, `connect` — one module per verb or verb
  family, each orchestrating `Host` calls; `mod.rs` dispatches
  `cargo xtask vm <verb>` to them. `deploy::stage_and_copy` always stages a
  `settings.toml` selecting the real `OneCore` synthesizer now — there is
  no more capture-synth choice or `--audible` flag on the VM path, since
  `test` is audible by default; the capture synth remains the runner-direct
  default, independently, in `verbatim_e2e::scenario`. `deploy::build`
  probes for `libclang.dll` before its `cargo build` the same way
  `xtask`'s own `ci` command does (reusing `find_libclang`), since building
  `verbatim-app` pulls in `verbatim-gui`'s wxDragon dependency.
- `test` (milestone M3 Track B: per-scenario selection and boundaries,
  replacing "the whole suite runs as one blob with one recording"). `xtask`
  now depends on `verbatim-e2e` directly, reading `registry::SCENARIOS` and
  `registry::select` rather than duplicating the scenario catalog.
  `--list` prints the registry (name and group) and exits, touching neither
  build nor VM. Otherwise `test::test` resolves `--scenario`/`--group` via
  `registry::select` (erroring out before any build or restore on an
  unrecognized name), builds, restores (unless `--no-restore`), deploys,
  then runs `session_info`'s own test as a precondition — once, unrecorded,
  regardless of selection — before entering `run_one_scenario`'s per-scenario
  loop: `--record` (when set) brackets exactly that scenario's own `cargo
  test -p verbatim-e2e <name> -- --exact` subprocess with
  `recording::start_recording_with_fallback`/`stop_recording`/
  `pull_recording`, so the recording's boundary is exactly one scenario's
  `Scenario::launch`, setup, body, and teardown — all of which run inside
  that one subprocess — never spilling into a neighboring scenario's
  recording; this is the module's own answer to "how does `xtask` control
  scenario boundaries" (a dedicated multi-scenario runner mode inside
  `verbatim-e2e` was the other option considered — see the module's doc
  comment for why a fresh, exactly-filtered subprocess per scenario was
  chosen instead: it gets a process-lifetime boundary for free, no new IPC).
  After each subprocess exits, `run_one_scenario` reads back the
  `verbatim_e2e::artifacts::ScenarioSummary` that scenario's own run wrote,
  rather than parsing the subprocess's stdout; `print_run_summary` prints
  the final one-line-per-scenario pass/fail-plus-latency report. No retry of
  any kind exists at this level either: one scenario's failure is
  accumulated into the run's error list and the loop continues to the next
  scenario, never re-running the one that failed.
- `recording` — `test`'s `--record` flag, now started and stopped around
  one scenario at a time (see `test` above) rather than once for the whole
  run: a small client speaking `verbatim_agent::protocol` directly (`Hello`,
  `LaunchProcess`, `ProcessStatus`, `KillProcess`; not `Host`, and not
  `verbatim-e2e`'s own fuller `AgentClient` — see the module's doc comment
  for why) to pin VB-CABLE as the guest's default render device, launch
  ffmpeg inside the guest's interactive session, confirm it is still running
  a moment later, terminate it once that scenario's subprocess finishes, and
  pull the fragmented-MP4 result back to `artifacts/vm-recordings` on the
  host via `Host::read_guest_file` (the same PowerShell Direct mechanism
  `logs` uses, since `Copy-VMFile` only copies host-to-guest).
  `pull_recording` now takes the scenario's name and names the file after
  it (`<scenario_name>-<unix-seconds>[-no-audio].mp4`), one recording per
  scenario instead of one per run. Recording audio and a connected RDP
  session are mutually exclusive (`docs/tooling.md` has the full constraint
  and why); `test`'s own `start_recording_with_fallback` treats a failure to
  pin the render device, or ffmpeg exiting immediately after an
  audio-capturing launch, as the expected fallout of a connected session —
  not fatal — and retries `recording::start_recording` with
  `with_audio: false` instead, so the scenario still runs and a video-only
  recording is still pulled. `recording::pull_recording`'s own
  ffprobe-based check, not which launch path was taken, is what decides the
  pulled file's `-no-audio` filename tag.
- `dotenv` — a minimal hand-rolled `.env` reader (`KEY=VALUE` lines,
  comments, quoting) for `VERBATIM_VM_USERNAME`/`VERBATIM_VM_PASSWORD` from
  the repository-root `.env`, deliberately not a crate dependency for a
  format this small.
- `packer_build` — wraps `vm/scripts/Build-VerbatimWindows11Image.ps1` and
  locates the `.vmcx` it exports (reading `output_directory` out of
  `vm/local.pkrvars.hcl` with a minimal line-oriented HCL reader, the same
  approach `dotenv` takes) so `create` can hand it to `Import-VM`.

Constants worth knowing when reading any of the above: `VM_NAME` is always
`verbatim`; `CHECKPOINT_NAME` is `golden`; `AGENT_PORT` is 44001, duplicated
from `verbatim_agent::protocol::DEFAULT_PORT` rather than depending on that
crate for one constant; `VERBATIM_DIR` (`C:\VerbatimLab\verbatim`) and
`AGENT_DIR` (`C:\VerbatimLab\agent`) are the guest install paths `deploy`
writes into and `logs` reads out of, matching
`vm/scripts/Initialize-VerbatimHarness.ps1`'s own paths.

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
single-instance rendezvous and dialog parent. It is stamped, right after
creation, with the `verbatim_model::HIDDEN_FRAME_WINDOW_PROP` window
property (`hidden_frame::mark`, `SetPropW`) and unstamped at shutdown
(`hidden_frame::unmark`, `RemovePropW`) — the marker every outpost checks
before announcing a `FocusChanged` (decision D9; see `verbatim-outpost`'s
section), so the frame's transit through real focus during the popup dance
below is never spoken as a nameless "Verbatim" window with role unknown.
One localized menu object serves both the tray icon and the Verbatim+V
popup. Showing the menu or the dialog performs NVDA's prePopup dance — show
the frame, raise it, and force it foreground through the native window
handle — because a popup from a hidden background window never receives
foreground or keyboard focus, and no outpost would ever be watching it (D9:
outposts are per-application, spawned or re-announced by Core's foreground
trigger). `force_foreground` tries `SetForegroundWindow` directly first,
but a gesture that arrived via the control plane (no physical input, as
every E2E test and any future remote session sends) fails Windows'
foreground-lock heuristic outright; a bare `VK_CONTROL` tap injected first
satisfies the heuristic directly (confirmed live: roughly 150 to 450 ms,
versus roughly two seconds for the `AttachThreadInput` fallback the code
still keeps for the case even the tap does not help). `VK_MENU` was tried
and rejected as the nudge — a lone Alt press activates menu bars and
bounces foreground straight back. The frame hides again after the menu
closes or the dialog is dismissed. The settings dialog mirrors NVDA's
shape: a labeled single-column report list of categories on the left, a
lazily built panel on the right, OK, Cancel, and Apply buttons, hand-rolled
Enter, Ctrl+S, and Ctrl+Tab handling (wxDragon binds no accelerator
tables), and a title that tracks the active category. The Speech panel is
generated from the settings host's descriptors; every control change
applies live, OK and Apply persist, Cancel reverts. Escape and window close
follow the dialog's escape id.

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
- `flight_dump` (milestone M2) — `dump_now(recorder, dumps_dir)` clones the
  shared `Arc<Mutex<ReducerRecorder>>`'s retained entries under a brief
  lock (recovering a poisoned lock rather than propagating it, since the
  reducer thread panicking while holding it is exactly the case the panic
  hook below exists for), then writes them through
  `verbatim_core::dump::write_dump` to `dumps_dir` as
  `flight-<UTC timestamp>.jsonl`, creating the folder if needed and logging
  the path. `install_panic_hook(recorder, dumps_dir)` chains the previous
  panic hook and wraps its own dump attempt in `catch_unwind`, so the hook
  itself can never turn a panic into a second, masking panic; it logs and
  swallows any failure before calling the previous hook.
- `run` wires everything: the flight recorder (`Arc<Mutex<ReducerRecorder>>`,
  shared by the reducer thread, the control plane's `DumpRecorder` handler,
  and the panic hook installed as early as possible so it covers every
  thread spawned after it), the speech pipeline (`build_speech_manager`:
  OneCore through WASAPI by default, configured from the base profile,
  observed by the ledger — `VERBATIM_TEST_AUDIO=null` at startup is a
  test-only escape hatch that registers the capture synth from
  `verbatim-synth-capture` alongside OneCore and swaps in `NullSink` for
  `WasapiSink`, logging a warning, so E2E and CI runs work with no sound
  card), the settings host with a persist callback writing through the
  config store, the supervisor plus foreground trigger (targeting the
  current foreground immediately, since the trigger only fires on
  changes), the reducer thread (drains outpost messages via
  `incoming_input`, feeds `reduce`, records each input into the shared
  flight recorder, executes effects — `Speak` to the pipeline, `Fetch`
  back to the outpost; a `DumpTreeReply` is routed around the reducer
  entirely, straight into whatever one-shot sender is parked in the
  `PendingDumpTree` slot, since a tree dump is a one-shot diagnostic query
  rather than reducer-shaped input), the gesture router (bound gestures to
  `GuiCommand`s, never into the reducer), the control server with its
  injected handlers (`dump_tree` registers that one-shot sender, sends
  `DumpTree` to the outpost through the supervisor, and waits with a five
  second timeout, a second concurrent request finding the slot already
  occupied and failing immediately rather than queuing; `dump_recorder`
  calls `flight_dump::dump_now` directly, no outpost round trip needed),
  the keyboard hook last among input paths, the startup announcement, and
  finally the GUI loop on the main thread. When the loop exits — Exit item,
  control-plane quit, or a replacing instance's `WM_QUIT` — teardown drops
  the hooks and lets job objects reclaim the outposts.

## Placeholders and tooling

- `verbatim-uia-rops` — UIA remote operations, lands in M4.
- `verbatim-ext` and `verbatim-ext-api` — the Wasm extension host and WIT
  contract, land in M5.
- `xtask` — workspace automation. `cargo xtask ci` is the standard check
  and exactly what GitHub Actions runs: rustfmt, pedantic clippy with
  warnings denied, unit tests on x64, then a release-profile ARM64
  cross-build (build-verified only; never run on this x64 machine). It
  probes known Visual Studio and LLVM locations for `libclang.dll` so
  wxDragon's bindgen works without manual environment setup. `cargo xtask
  vm` is the milestone M2 Hyper-V harness (build, deploy, and E2E-test a
  real VM) — see this document's "xtask VM harness" section above and
  `docs/tooling.md` for the full verb reference.
