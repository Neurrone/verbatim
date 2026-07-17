# verbatim-app

The composition root: `verbatim.exe`. Its `clipboard` module is the one
shared copy-to-clipboard path (NVDA's `api.copyToClip` analog): it owns the
Win32 clipboard interaction and the localized spoken confirmation, and every
copying gesture routes through it — the report-object triple-press is the
first caller. The reducer thread selects on both the outpost stream and a
command channel the gesture router feeds, so a review or object-navigation
gesture is reduced and its effects executed by the same path as an
accessibility event; `Effect::Activate` and `Effect::CopyToClipboard` are
executed there alongside `Speak` and `Fetch`. The router builds its gesture
map and its gesture-to-script table from `verbatim_input::bindings_for` for
the configured keyboard layout, so the active review and navigation bindings
follow `settings.toml`'s `keyboard.layout`.

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
- `datetime` (milestone M3) — `local_time()` and `local_date()` format the
  current time (no seconds) and long date through `GetTimeFormatEx` and
  `GetDateFormatEx` with the user default locale, NVDA's approach: the OS
  localizes per the user's regional preferences, so no Fluent message is
  involved for the values. It lives here because the composition root owns
  command routing and nothing else needs the pair.
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
  config store, the supervisor with its focus listener (decision D13;
  targeting the current foreground once at startup by poll, since the
  listener thereafter reports foreground changes as facts — Core no longer
  runs its own foreground hook, and learns of a foreground change through
  `OutpostMessage::ForegroundChanged`, which it stores into the
  current-foreground atomic and notes as a targeted pid), the reducer thread
  (drains outpost messages via `incoming_input`, feeds `reduce`, records each
  input into the shared
  flight recorder, executes effects — `Speak` to the pipeline, `Fetch`
  back to the outpost; a `DumpTreeReply` is routed around the reducer
  entirely, straight into whatever one-shot sender is parked in the
  `PendingDumpTree` slot, since a tree dump is a one-shot diagnostic query
  rather than reducer-shaped input), the gesture router (bound gestures to
  imperative commands — `GuiCommand`s or direct speech — never into the
  reducer), the control server with its
  injected handlers (`dump_tree` registers that one-shot sender, sends
  `DumpTree` to the outpost through the supervisor, and waits with a five
  second timeout, a second concurrent request finding the slot already
  occupied and failing immediately rather than queuing; `dump_recorder`
  calls `flight_dump::dump_now` directly, no outpost round trip needed),
  the keyboard hook last among input paths, the startup announcement, and
  finally the GUI loop on the main thread. The gesture router binds three
  gestures in M3: Verbatim+V pops the menu, Verbatim+F12 speaks the time
  (an Interrupt-priority text-span utterance with no source node), and
  Verbatim+F11 opens the system tray list via
  `GuiCommand::OpenShellItemList`. The double-press variants — the date,
  the taskbar list — wait on `verbatim-input`'s multi-press gesture
  counting (M3 Track D); the seam is explicit: `speak_time_or_date` takes
  a repeat count and `shell_list_kind` maps one to the listed surface,
  both called with 0 today, so wiring the real press count into those two
  calls is the whole integration. When the loop exits — Exit item,
  control-plane quit, or a replacing instance's `WM_QUIT` — teardown drops
  the hooks and lets job objects reclaim the outposts.
