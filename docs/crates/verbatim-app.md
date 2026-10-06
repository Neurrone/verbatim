# verbatim-app

The composition root: `verbatim.exe`. Its `clipboard` module is the one
shared copy-to-clipboard path (NVDA's `api.copyToClip` analog): it owns the
Win32 clipboard interaction and the localized spoken confirmation (NVDA's
"Copied to clipboard:" with the text, after reading the clipboard back, or
"Unable to copy"), and every copying gesture routes through it — the
report-object triple-press is the first caller. The gesture router also
announces a lock key's new state ("caps lock on") 30 ms after the keyboard
hook reports it reached the operating system, waiting for its channel
with a deadline rather than starting a thread per key. The reducer thread selects on both the outpost stream and a
command channel the gesture router feeds, so a review or object-navigation
gesture is reduced and its effects executed by the same path as an
accessibility event; `Effect::Activate` and `Effect::CopyToClipboard` are
executed there alongside `Speak` and `Fetch`, and so are
`Effect::StopSpeech` and `Effect::DropExpiredSpeech`, through the speech
manager's `SpeechControl` (`cancel` and `drop_expired`). The router builds its gesture
map and its gesture-to-script table from `verbatim_input::bindings_for` for
the configured keyboard layout, so the active review and navigation bindings
follow `settings.toml`'s `keyboard.layout`.

Public surface: none — this is the binary. Internal structure worth
knowing for review:

- `main` orders startup: namespace trace IDs, load config (materializing
  missing files), start tracing, refuse a session that is not interactive
  (`verbatim_process::session`; a launch from WinRM, PowerShell Direct, or
  a service exits with a diagnosis, before it could replace a working
  instance), replace any running instance, load locales, then `run`.
- `single_instance::acquire_replacing` — NVDA's algorithm: find the old
  instance's hidden window by title, post `WM_QUIT` so its loop exits and
  its normal teardown runs, wait four seconds, `TerminateProcess` as the
  fallback with a further wait; then serialize startup on a named mutex
  (an abandoned mutex — a crashed predecessor — still grants ownership,
  with a warning). `ChangeWindowMessageFilter` lets a future
  lower-integrity replacer's quit message through.
- `ReducerThread` — owns the reducer state, the request table, and the
  live-outpost set, and is the only thread that touches them. It selects on
  the supervisor's stream and on a `ShellCommand` channel (router inputs and
  control-plane tree dumps). Every outpost message arrives tagged with the
  outpost incarnation whose pipe carried it; an outpost joins the live set
  on `OutpostMessage::Started`, which precedes all its messages, and leaves
  it on `OutpostMessage::Ended`, and a message from an outpost not in the
  set is dropped, so nothing from an ended outpost reaches the reducer, even
  when the supervisor killed it with messages still in flight. The status
  mirror follows the same notices. After each input the thread sends the
  supervisor the derived views (`note_views`) when they change: the
  application holding attention and the outposts in which the reducer holds
  nodes. It also tells each live outpost which of its nodes the reducer
  holds (`send_nodes_held`), with the position of the last of that
  outpost's messages it has handled, whenever that set changes and every
  256 messages besides, so the outpost can release everything else
  ([verbatim-outpost](verbatim-outpost.md), "Held objects"). On an end the
  reducer
  gets `Input::OutpostEnded`, then a "gone" outcome for each of that
  outpost's outstanding queries. There is no foreground pid gate: which
  events are spoken is the reducer's attention model. Events go to the
  reducer, the latency ledger, and the control plane's event subscribers
  (copied for the control plane only while one is subscribed);
  replies go through the request table. The thread never waits on a
  handoff: speech and control-plane sends never block, and a command for an
  outpost is queued on that outpost's writer, failing at once if the queue
  is full.
- `requests::RequestTable` — the single owner of "exactly one outcome per
  query" (outpost redesign, "The app shell"). Every query sent to an
  outpost (a navigation step, an activation, a tree dump, a focus-now
  query) is recorded with a fresh `RequestId` and the outpost incarnation it
  went to, and sent as a `Query` carrying that id; the outpost's one `Reply`
  echoes it with one of five outcomes. The first outcome removes the
  entry and goes to the asker: a navigation outcome re-enters the reducer
  as `Input::FetchCompleted` under the reducer's own query id, an
  activation's becomes `Input::ActivationCompleted`, which the reducer
  speaks, except that an abandoned activation (started, then past its
  deadline, so it may or may not have happened) is only logged and says
  nothing; a tree dump's is sent on its reply channel; and a
  focus-now query's becomes reducer input: a foreground change to the window,
  when the application holds the foreground, then a focus on its focused
  control. The reducer thread sends a focus-now query at startup (for the
  foreground application), when the attention application's outpost was
  replaced after a crash or kill, when the outpost of the focus's
  application, if that is not the attention one, ended in a crash or kill
  (asking the supervisor to start a new one first, since nothing else
  would), and when the supervisor reports
  `OutpostMessage::ListenerReplaced`; each waits for the outpost's `Ready`
  if it is still starting. When the supervisor reports
  `OutpostMessage::MenuOrSwitchEnded` (a menu or the Alt+Tab switcher
  closed, told 50 ms later with the time it ended), the reducer thread,
  unless the reducer has applied a focus observed since that time
  (`SrState::latest_focus_observed_at`), asks the foreground application's ready outpost for
  its focus as `Asker::FakeFocus`, whose answer re-enters the reducer as
  the focus on the control alone, NVDA's fake focus; an application with
  no ready outpost gets an ordinary focus-now query.
  Later outcomes for the same id, and outcomes from any other outpost, are
  dropped. Core makes the outcome itself when a query cannot be sent
  ("failed", which a navigation sees as `Unanswered`; only "gone" becomes
  `FetchResult::Gone`) and when the outpost ends ("gone"), so an old reply or a timed-out request can never satisfy or
  clear a newer one.
- `latency::LatencyLedger` — the bounded ring of timelines keyed by trace
  ID, bounded both by count (`DEFAULT_CAPACITY`, 256) and by an estimate of
  the bytes they hold (`DEFAULT_MAX_BYTES`, 1 MiB; each timeline counts its
  inline size plus its utterance text), dropping the oldest first and
  always keeping the newest, fed from three threads across two processes: the reducer thread
  records event observation (using the outpost's own timestamp), and the
  pipeline observer callbacks record queue and audio start (when several
  utterances share a trace, the first to be heard counts). It also keeps
  each announcement's stages in microseconds (the outpost's
  `EventTiming`, Core's receipt and reduction, and the speech milestones
  of the trace's first utterance) and logs them as one `verbatim::latency`
  line when its audio starts (see `docs/tooling.md`), the outpost read
  with its count of cross-process calls. A query reply the worker answered
  feeds it the same way as an event, so a navigation keypress's trace has
  its outpost stages and calls too. `recent` returns each timeline's
  stages and calls in its `LatencyRecord`. It mirrors
  each utterance's milestones to speech subscribers as a `Speech` frame at
  queue time, a `SpeechStarted` frame when its first frame plays, and a
  `SpeechEnded` frame with its ending, and it answers the `latency`
  command newest first. Core-originated speech with no event reports its
  queue time as the timeline start.
- `flight_dump` (milestone M2) — `dump_now(recorder, dumps_dir)` clones the
  shared `Arc<Mutex<ReducerRecorder>>`'s retained entries and the
  checkpoint state they start from under a brief lock (recovering a poisoned lock rather than propagating it, since the
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
- `run` wires everything: the flight recorder (`Arc<Mutex<ReducerRecorder>>`
  with the default bounds of 1,024 inputs and 8 MiB, to which the reducer
  thread hands the state after each input for the recorder's checkpoints,
  shared by the reducer thread, the control plane's `DumpRecorder` handler,
  and the panic hook installed as early as possible so it covers every
  thread spawned after it), the speech pipeline (`build_speech_manager`:
  eSpeak NG through a `Mixer` over `WasapiDevice` by default; eSpeak NG
  and OneCore are both registered as hosted synthesizers, eSpeak NG first
  as the default, by the ids in `verbatim_speech::hosting::synth_ids`,
  each with `verbatim_synth_hosted::factory` starting
  `verbatim-synth-host.exe` from the folder `verbatim.exe` runs from
  (decision D18), so the app links neither driver. The configured
  synthesizer is used when it is registered; when none is configured, or
  the configured one is not registered, eSpeak NG is used, with a
  warning in the second case. The pipeline is configured
  from the base profile, observed by the ledger — `VERBATIM_TEST_AUDIO=null`
  at startup is a test-only escape hatch that registers the capture synth
  from `verbatim-synth-capture` alongside the real synthesizers and
  builds the mixer over `SilentDevice` instead, logging a warning, so E2E and CI runs work
  with no sound card while every utterance still takes its real
  duration; `VERBATIM_RECORD_AUDIO=<path>` at startup starts the mixer
  with a `verbatim_audio::WavRecorder` tap writing everything Verbatim
  plays to that WAV file, for the end-to-end harness's videos, decision
  D16, and a file that cannot be created is logged as a warning and
  ignored), the settings host with a persist callback writing through the
  config store, the supervisor with its focus listener (decision D13;
  asking the current foreground application for its focus once at
  startup with a focus-now query (`ShellCommand::FocusNow`), since the
  listener thereafter reports foreground changes as facts — Core no longer
  runs its own foreground hook; a foreground change reaches the reducer as a
  focus on the window, which moves its attention), the reducer thread
  (`ReducerThread`, described below), the gesture router (bound gestures to
  imperative commands — `GuiCommand`s or direct speech — or, for review and
  object navigation, reducer commands), the control server with its
  injected handlers (`dump_tree` hands a `ShellCommand::DumpTree` with a
  `DumpTicket` and a one-answer reply channel to the reducer thread and
  waits five seconds for the answer, then sends
  `ShellCommand::DumpTreeGivenUp` with the ticket, and the reducer thread
  sends the outpost a `Cancel`, which withdraws the dump if it has not
  started; `dump_recorder` calls `flight_dump::dump_now` directly, no
  outpost round trip needed; an injected gesture cancels speech before it
  is sent, as a key press does, since its keys never pass the hook),
  the keyboard hook last among input paths (given a callback that maps
  each `KeySpeechEffect` to the speech manager's `SpeechControl`: `Cancel`
  to `cancel`, `TogglePause` to `toggle_pause`), the startup announcement, and
  finally the GUI loop on the main thread. The gesture router handles
  Verbatim+V, which pops the menu, the lock keys, whose new state it
  announces, and every gesture in the active layout's bindings table:
  review and object-navigation scripts become `Input::Command`s for the
  reducer thread, while Verbatim+F12 speaks the time
  (an Interrupt-priority text-span utterance with no source node), and
  Verbatim+F11 opens the system tray list via
  `GuiCommand::OpenShellItemList`. A quick second press speaks the date or
  lists the taskbar instead: `verbatim-input` counts quick presses on each
  emitted gesture, and the router passes that count to
  `speak_time_or_date` and to `shell_list_kind`, which maps it to the
  listed surface. When the loop exits — Exit item,
  control-plane quit, or a replacing instance's `WM_QUIT` — teardown drops
  the hooks and lets job objects reclaim the outposts.
