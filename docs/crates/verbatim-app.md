# verbatim-app

The composition root: `verbatim.exe`. Only the reducer speaks
(`docs/architecture.md`, section 2): every utterance and earcon is an
`Effect` of `reduce`, and the shell's threads turn what they learn into
`Input`s, posted to the reducer thread's port, never calling the speech
manager to speak; the keyboard hook's cancel, pause and resume are its one
speech send. The compiler holds this: every speaking function of the
speech manager takes the `SpeakToken` `SpeechManager::new` makes, which
`run` moves into `reducer_loop`, which hands it back when the loop ends;
the settings window's sound preview plays with `play_sound`, which takes
none. Its `clipboard` module is the one
shared copy-to-clipboard path (NVDA's `api.copyToClip` analog): it owns the
Win32 clipboard interaction and reads the clipboard back, and tells the
reducer how the copy went (`Input::ClipboardCopied`, with the trace of the
input that asked for it), which confirms it (NVDA's "Copied to clipboard:"
with the text, or "Unable to copy") under that key's fence, so a later key
press drops it as it drops any earlier key's speech. Every copying gesture
routes through it — the report-object triple-press is the first caller. It writes on a thread of
its own (`Clipboard`), not the reducer thread, because emptying the clipboard sends the previous owner, a window
of whichever application copied last, `WM_DESTROYCLIPBOARD` and waits for
the answer with no bound, which an owner that has stopped answering would
have turned into a reducer that handles nothing. That thread runs no
message loop, so it opens the clipboard with a
message-only window made for that one write and destroyed after it
(`SetClipboardData` fails on a clipboard opened with no owner window, and
a window kept on a thread that pumps no messages would leave another
application's `EmptyClipboard` waiting), and frees its global memory
whenever the clipboard did not take it. A lock key's press reaches the
reducer as `Input::ToggleKeyPressed`; Core asks for its state 30 ms later
(`Effect::ReadToggleKey`), which the reducer thread keeps among its
deadlines (`toggle_reads`), reads with `GetKeyState` when it passes and
enters as `Input::LockKeyToggled`, which Core says ("caps lock on"). The
time gesture's `Effect::ReadClock` is read on the reducer thread, as
`GetTimeFormatEx` and `GetDateFormatEx` word it, and applied as
`Input::ClockRead` with the trace the effect ran under, after the current
input's effects (`follow_ups`), so one input is reduced at a time;
`Effect::Show` posts the menu or a shell list to the GUI, dropped with a
warning before the GUI is up. The reducer thread
waits only on its own I/O completion port
(`verbatim_process::port_queue`; `docs/design/communication.md`, section
6.2) and takes everything posted to it, one packet per input, in the
order it arrived: the supervisor's messages, each outpost's frames among
them, posted by the relay as it reads them (through an `EventSink` that
wraps each in `ShellCommand::Outpost`), the keyboard hook's keys,
gestures and typing, speech marks, GUI events and
control-plane commands, so a key is reduced before an application's
reaction that reached Core after it, and after one that reached Core
before it, however busy the reducer was, as NVDA queues keys with events
(`docs/design/communication.md`, section 4.5), and a review or
object-navigation gesture is reduced and its effects executed by the same
path as an accessibility event; `Effect::Activate` is executed there alongside
`Speak` and `Fetch`, and so are
`Effect::StopSpeech` and `Effect::DropExpiredSpeech`, through the speech
manager's `SpeechControl` (`cancel` and `drop_expired`), while
`Effect::CopyToClipboard` is handed to the clipboard thread. The hook's
gesture map and `key_reports`' gesture-to-script table come from
`verbatim_input::bindings_for` for the configured keyboard layout, so the
active review and navigation bindings follow `settings.toml`'s
`keyboard.layout`.

The keyboard hook's two callbacks are the `key_reports` module, which
runs on the hook thread and is checked by `cargo xtask ci`'s hook rule
(`xtask/src/ownership.rs`): the speech effect calls `cancel_through` with
the key's number and posts `Input::SpeechCancelled`, or toggles the
pause; the report callback notes the key in the latency ledger and turns
each `KeyReport` into its `ShellCommand`s, by lookups in tables made when
the hook starts (`key_reports::Tables`), and posts them to the reducer's
port in the order the hook decided them. An observed caret key becomes
`Input::CaretKey`; a clearing key `Input::ClearingKey`, then
`Input::CaretKey` with `CaretMotion::Other`; Verbatim+V `Input::Script`
with `Script::ShowMenu`; the time and tray-list bindings `Input::Script`
with `Script::ReportTime` and `Script::ShellList`; a review or
object-navigation binding `Input::Command`; a lock key's gesture
`Input::ToggleKeyPressed`; an unbound gesture nothing; `Handled` the
harness mark `ShellCommand::InputHandled` and `Barrier` the idle waiter
`ShellCommand::AwaitIdle`. Every user input carries its key number. The
reducer thread logs each `Command` and `Script` it applies, since the
hook thread writes no log. The speech queue's inbox is a completion
port too (`PortInbox` and `PortTaker` over `port_queue::<QueueItem>`), so
the hook's cancel takes no lock.

The end-to-end harness's hooks (private `harness` module):

- Ignored processes: when launched with `VERBATIM_IGNORE_PIDS` naming
  pids, separated by commas, Verbatim holds those processes open and
  ignores them entirely (`verbatim_outpost::supervisor::IgnoredProcesses`,
  handed to `Supervisor::new`): no outpost is started for one, the focus
  listener drops their facts, the shell never asks one for its focus (a
  foreground read naming one is passed over), and a focus reported in one
  of their top-level windows is dropped before the reducer. The harness
  names the owner's own Windows Terminal (`docs/tooling/e2e.md`).
- Readiness: when launched with `VERBATIM_READY_EVENT` naming an event,
  Verbatim sets that event once it is ready for input (the GUI up, the
  focus listener running, its own window's outpost ready, and the focus
  known), the same condition `StatusInfo::ready` reports, so a test waits
  on the event rather than asking again and again.
- The idle barrier: the keyboard hook reports each numbered key stroke
  handled (`KeyReport::Handled`), posted to the reducer thread behind
  whatever the stroke caused (`ShellCommand::InputHandled`), which records
  it and broadcasts `Frame::InputHandled`; a mark taken while a lock key's
  announcement is still to come is held (`idle::HeldMarks`) and handed on
  after the input that makes it. A control-plane `AwaitIdle` is injected
  through the hook thread (`Injected::Barrier`), so it is answered only
  after every key and injected gesture before it; on the
  reducer thread it waits until the input it names has been handled, then
  runs a round through every child (`idle.rs`, `IdleBarrier`;
  `docs/design/communication.md`, section 6.7). The supervisor's relay
  asks the listener, then every outpost, whether anything that came before
  the question is still in progress, and tells the reducer which outposts
  it asked (`OutpostMessage::IdleQueried`). Each child answers behind
  every message it sent before, and an outpost that ends counts as having
  answered, except an outpost that failed before its `Ready`, whose
  place goes to the generation `OutpostMessage::Regenerated` names,
  which holds its facts and its query. When every answer is in, the reducer takes in what is already
  on its port and checks its own conditions: no outpost request
  outstanding, no outpost still starting, no focus wanted from an outpost
  being started (a crashed outpost's replacement), no key the hook held back still waiting to be
  passed on, no key or command waiting in Core's held-key line
  (`SrState::holds_inputs_back`), no lock key's announcement still to come
  (`SrState::announces_toggle_key`), and no terminal's quiet-period timer armed, since what waits
  for it is spoken when it ends. If they hold, the waiters are answered once
  everything queued so far has reached the speech manager; if not, the
  round runs again once they clear. A wait that runs out answers with
  what was still outstanding.
- `DumpFocus` answers with `SrState::focus_report`, within five seconds.

Milestone M4's text protocol is wired here (`docs/crates/verbatim-model.md`,
"The text protocol"):

- The reader settings (`settings.toml`'s `[reader]` section) are the
  reducer's first input, `Input::Settings`. They change by the reducer's
  own toggle keys, whose `Effect::SettingsChanged` is saved to
  `settings.toml` through the config store, and by the settings dialog's
  Terminal page: `terminal_settings::AppTerminalHost` is the GUI's
  `TerminalHost`, reading the settings from the config store and sending
  an applied `TerminalChange` to the reducer thread
  (`ShellCommand::TerminalSettings`). There it is merged into the
  reducer's own settings (`SrState::settings`), so a toggle key pressed
  meanwhile is kept, given to the reducer as `Input::Settings`, and
  saved.
- The hook's gesture map observes the caret keys
  (`GestureMap::with_observed(caret_bindings())`): they reach the
  application, and each one the hook reports becomes `Input::CaretKey`
  through the same table, with the time the hook stamped it. It observes
  the clearing keys too (`clearing_keys()`), each of which becomes
  `Input::ClearingKey` and then, like any other key the hook reports as
  passed without typing text, `Input::CaretKey` with `CaretMotion::Other`.
  Each text a key types becomes
  `Input::CharacterTyped` (the hook's translation,
  [verbatim-input-windows](verbatim-input-windows.md)), and each key that
  cancels speech also sends `Input::SpeechCancelled`, as does a gesture
  injected through the control plane, so say-all stops. All of these go
  straight from the hook thread to the reducer thread's port, each one
  post, so the hook never waits, and in the order the keys were
  pressed. The hook's `DecisionConfig` takes the two speech interrupt
  settings from `keyboard.speech_interrupt_for_characters` and
  `keyboard.speech_interrupt_for_enter`. `key_reports` maps a script
  action to its reducer command with `ScriptAction::review_command`.
- Held caret keys (`docs/design/key-holding.md`): the hook holds the
  caret table's keys while a `KeyHolding` switch is on, which the reducer
  thread sets after every input from `SrState::holds_caret_keys`, and
  only where the hook's `ForegroundView`, which the reducer thread sets
  after every input from `SrState::hook_foreground`, says the window in
  front lets keys be held (not an elevated window's). A held
  key's report becomes `Input::HeldKey`, and its stroke joins one line
  (`held_keys`) with the transitions the hook deferred behind held keys
  (`KeyReport::Deferred`), in the order reported. A held key waits at the
  front until Core emits `Effect::PassHeldKey`; a deferred transition
  goes as soon as it reaches the front. A terminal's keys reach its
  reading rules through Core alone, which takes each key input in and a
  deferred key as reaching the program when the held key ahead of it is
  passed on ([verbatim-core](verbatim-core.md), "Core's terminal model").
  Whatever reaches the front is sent with one `SendInput`, each key carrying its place in line
  (`passed_on_info`): a held key with the gesture's Shift, Control or Alt
  that are not down pressed around it and the key released first if
  Windows still has it down, as NVDA's gesture send does, and a
  character typed as Unicode (`VK_PACKET`) as Unicode again. Each
  pass-on logs how long the key was held (`verbatim::held_key`,
  `held_us`). `Effect::ForKey` is executed on the named key's trace.
  `Effect::BoundHeldKeyWait` sets the one time at which a held key's wait
  for the key ahead of it ends, kept beside the reducer thread's other
  deadlines (`next_deadline`); when it passes, the thread enters
  `Input::HeldKeyWaitEnded` with the key's trace ID.
  `Effect::TerminalTimer` sets or clears the focused terminal's
  quiet-period timer, another of those deadlines, its due time on the
  shared clock turned into an instant; when it passes, the thread enters
  `Input::TerminalTimer` with that due time and a trace ID minted then.
  The read of the caret
  before the key (`TextOp::ReadCaret`) is a request with a deadline,
  `HELD_KEY_BOUND`, 150 milliseconds: past it the request ends as
  unanswered and Core passes the key on without it.
- `Effect::Text` goes to the outpost of the node it names as
  `Query::Text`, recorded in the request table as `Asker::Text`; its one
  outcome comes back as `Input::TextCompleted` under the reducer's query
  id, the outpost's reply when it answered, `Gone` when the node or its
  outpost is gone, and `Unanswered` for any failure.
- The speech pipeline's observer (`speech_events`) is the latency ledger
  plus the marks: every index mark playback reaches becomes
  `Input::MarkReached` posted to the same port, so say-all advances, and
  every utterance with marks that ended before playback reached them all
  (`SpeechEvents::marks_cancelled`) becomes `Input::UtteranceCancelled`,
  and every one that failed (`marks_failed`) `Input::UtteranceFailed`,
  so say-all and terminal output never wait on marks a cut has taken away.
  Every synthesizer reports marks: `OneCore` from its own bookmarks, and
  eSpeak NG from its mark events
  ([verbatim-synth-espeak](verbatim-synth-espeak.md), "Index marks"), so
  neither degrades say-all.
- `Effect::KeepDisplayOn` calls `SetThreadExecutionState` on the reducer
  thread, which lives as long as Verbatim: the display and the system
  required while say-all reads, released when it ends.
- `Effect::CopyToClipboard`, which select-then-copy emits, goes to the
  clipboard thread like every copy.
- The held anchors (`SrState::held_anchors`) go to each outpost with its
  held nodes. The list is written on its own (`Supervisor::send_nodes_held`)
  when the node set changed, or after a focus report of that outpost was
  handled, taken or refused, so the outpost releases what Core let go of
  within one pipe hop and reads changes for no more than it must
  (`docs/design/ownership-one-judge.md`, "Manageability"). Any other change
  (`Live::held_changed` answers `HeldChange::Other`: the anchors, or the
  position of the outpost's last message handled alone) is left in that
  outpost's writer (`Supervisor::leave_nodes_held`), and the next command
  written there, a request or the relay's ping within three seconds,
  carries it in the same write, after its own frame
  (`docs/design/communication.md`, step 11): the caret's anchor changes
  with every key, and a terminal's output changes the position with every
  message (`docs/design/wake-ups.md`, item 4).

Milestone M4's themes and earcons are wired here too
([verbatim-model](verbatim-model.md), "Themes"):

- `themes` loads the theme the configuration names
  (`ActiveConfig::theme_id` and `theme_options`, through
  `verbatim_config::themes::find_theme`; one that cannot be loaded is
  logged and the built-in default theme used instead) and makes a theme
  ready (`prepare`): its sounds are decoded from its own directory or the
  shared `sounds` folder beside `verbatim.exe` (`LoadedTheme::sound_path`),
  and every problem found is logged. At startup this happens before the
  speech manager is built, which is given the theme at construction, so
  speech is presented with the configured theme and its sounds from the
  first word; the reducer is sent `Input::Fetches` with what the theme
  wants fetched (`send_fetches`). `activate` does both for a theme
  chosen later, setting it on the speech manager's theme handle. It runs
  whenever the settings dialog's Theme panel
  changes the theme or its options: `AppThemeHost` is the GUI's
  `ThemeHost`, over the configuration store (the themes and sounds
  folders, the configured theme, and saving the choice in the base
  settings' `[theme]`) and the speech manager (activating a theme, its
  options alone through the theme handle, playing a sound as the sound
  choice shows it, and previews, which go to the reducer as
  `Input::PreviewSpeech` and `Input::PreviewEarcon`). A sound that is missing (a run
  without the `sounds` folder) is spoken instead, so nothing is lost.
- `Effect::PlayEarcon` plays through `SpeechManager::play_earcon`. The
  shell tells the reducer of three events whose sounds it plays: the
  start (`Input::Started`, posted once the keyboard hook is installed,
  before the foreground's focus is asked for); the exit, once the GUI
  loop has ended (`ShellCommand::Exiting`, which the reducer thread
  applies as `Input::Exiting` with whether a new instance is replacing
  this one, which plays no sound); the reducer thread plays the exit sound
  with `play_earcon_heard` and hands its `EarconHeard` back, on which the
  main thread waits until it has been heard, for at most two seconds; and
  an application not responding (`Input::ApplicationNotResponding`) when
  the outpost's watchdog abandons a query that passed its deadline
  (`QueryOutcome::Abandoned`), once per stall: `LiveOutposts` marks the
  outpost stalled and clears the mark when it answers a query or reports
  an event.
- The build script copies the top-level `sounds` folder next to the
  executables in the cargo target directory, so `target\debug` holds
  everything a run needs, and the end-to-end staging and the VM deploy
  copy it from there.
- `error_sound` is a tracing layer: every event logged at error level, on
  any thread, posts `Input::ErrorLogged` to the reducer thread, whose
  reducer plays the error sound, as NVDA plays one for a logged error. It only sends, so logging from inside the speech pipeline
  or the mixer can never reenter them.

Public surface: none — this is the binary. Internal structure worth
knowing for review:

- `main` orders startup: namespace trace IDs, load config (materializing
  missing files), start tracing, refuse a session that is not interactive
  (`verbatim_process::session`; a launch from WinRM, PowerShell Direct, or
  a service exits with a diagnosis, before it could replace a working
  instance), load locales, replace any running instance, set Windows'
  screen reader flag, then `run`.
- `screen_reader_flag::ScreenReaderFlag` — sets `SPI_SETSCREENREADER` as
  NVDA does (saved in the user's profile, every window told) and clears
  it when dropped, as `main` returns, before the startup mutex is
  released, so a replacing instance's flag is never cleared by the
  instance it replaced. A console control handler clears it on
  Control+C, Control+Break and the console closed, and a hidden window
  of its own, on a thread of its own, clears it when the session ends
  (`WM_ENDSESSION`). A crash or a process ended from outside leaves it
  set, as NVDA's does.
- `single_instance::acquire_replacing` — NVDA's algorithm
  (`terminateRunningNVDA` in `nvda.pyw`): find the old instance's hidden
  window by title, post `WM_QUIT` so its loop exits and its normal
  teardown runs, and wait up to five seconds for its process to exit
  (NVDA waits four). If it is still running, end it
  with `TerminateProcess` and wait up to two seconds more; its outposts,
  focus listener and synthesizer hosts end with it through their
  kill-on-close jobs, and the screen reader flag it leaves set is set
  again by the new instance. Then serialize startup on a named mutex (an
  abandoned mutex, from a crashed or ended predecessor, still grants
  ownership, with a warning). An old instance that cannot be opened to
  be ended, or does not exit once ended, is left running, and NVDA's
  message box says so ("Couldn't end the running Verbatim, abandoning
  start.", titled "Error") before startup fails. Before posting
  `WM_QUIT`, the new instance sets the old one's replacement event,
  `Local\Verbatim.Replacing.<pid>`, which every instance creates for
  itself as it acquires the mutex and keeps in its `InstanceGuard`;
  `InstanceGuard::being_replaced` reads it once the GUI loop has ended,
  and an instance being replaced skips its exit sound. `WM_QUIT` cannot
  carry this, since wxWidgets' loop ends on it without keeping its
  parameters. `ChangeWindowMessageFilter` lets a future
  lower-integrity replacer's quit message through.
- `ReducerThread` — owns the reducer state, the request table, and the
  live-outpost set, and is the only thread that touches them. It takes
  every input from its port (`Commands`, a `port_queue::Poster` of
  `ShellCommand`s, is what every other thread posts through), first in,
  first out across all its posters: the supervisor's messages
  (`ShellCommand::Outpost`), the keyboard hook's keys and gestures,
  speech marks, and control-plane requests. It passes every supervisor
  message through the supervisor's `Frames`, which turns each outpost's
  frame into the message it carries, on the reducer's thread whatever its
  size, in the order the frames arrive. The supervisor's relay watches the port's counts and
  logs a warning when the reducer has taken nothing for three ping
  intervals while inputs waited (`EventSink::watched`). Every outpost message arrives tagged with the
  outpost generation whose pipe carried it; an outpost joins the live set
  on `OutpostMessage::Started`, which precedes all its messages, and leaves
  it on `OutpostMessage::Ended`, and a message from an outpost not in the
  set is dropped, so nothing from an ended outpost reaches the reducer, even
  when the supervisor killed it with messages still in flight. The status
  mirror follows the same notices. Once each outpost message or command
  has been handled whole, never between the inputs one message produces,
  the thread sends the
  supervisor the derived views (`send_views`) when they change: the
  application holding attention and the outposts in which the reducer holds
  nodes. It also tells each live outpost which of its nodes the reducer
  holds, with the position of the last of that outpost's messages it has
  handled, whenever either changes, written at once or left for the next
  command as the held anchors are (below), so the outpost can release
  everything else
  ([verbatim-outpost](verbatim-outpost.md), "Held objects"). A focus-now
  answer is a foreground change and then a focus: held nodes sent after
  the first alone, with the answer's position as acknowledged, had the
  outpost release the focus it reported in that same answer before Core
  took it, so the focus came back as a new node and was announced twice at
  startup. On an end the reducer
  gets `Input::OutpostEnded`, then a "gone" outcome for each of that
  outpost's outstanding queries, and the control plane's event subscribers
  get `Frame::OutpostEnded`. There is no foreground pid gate: which
  events are spoken is the reducer's attention model. Events go to the
  reducer, the latency ledger, and the control plane's event subscribers
  (copied for the control plane only while one is subscribed);
  replies go through the request table. The thread never waits on a
  handoff: speech and control-plane sends never block, and a command for an
  outpost is queued on that outpost's writer, failing at once if the queue
  is full.
- `requests::RequestTable` — the single owner of "exactly one outcome per
  query" (`docs/architecture.md`, section 1). Every query sent to an
  outpost (a navigation step, an activation, a tree dump, a focus-now
  query) is recorded with a fresh `RequestId` and the outpost generation it
  went to, and sent as a `Query` carrying that id; the outpost's one `Reply`
  echoes it with one of five outcomes. The first outcome removes the
  entry and goes to the asker: a navigation outcome re-enters the reducer
  as `Input::FetchCompleted` under the reducer's own query id, an
  activation's becomes `Input::ActivationCompleted`, which the reducer
  speaks, except that an abandoned activation (started, then past its
  deadline, so it may or may not have happened) is only logged and says
  nothing; a tree dump's is sent on its reply channel; and a
  focus-now query's becomes reducer input: a focus on the focused control,
  with its ancestors and the text its outpost read with it, its window
  announced only as an entered ancestor when it is presentable, as NVDA
  announces the focus it starts with, or, with no focused control, a
  foreground change to the window. The reducer thread sends a focus-now
  query at startup (for the
  foreground application), when the outpost of the attention application,
  or of the focus's application if that is another, ended in a crash or
  kill after its `Ready` (the supervisor replaces every failed outpost
  itself, at once or after its backoff, so the query waits for the
  replacement's `Ready`), and when the supervisor reports
  `OutpostMessage::ListenerReady` for a replacement listener; each waits for the outpost's `Ready`
  if it is still starting. These, the tree dump, and the attention view
  sent to the supervisor go by application, which for a console's client
  is its console host's outpost: an event carrying an `application`
  (an event in a console window, reported by the console host's outpost)
  enters the reducer with that application as its source, and
  `LiveOutposts` records that the outpost serves it (`reports_for`,
  `target_for`), so its queries go there and the supervisor keeps it
  (`docs/design/focus-pipeline.md`, section 5.4). A focus-now query for a
  console's client whose console window is in front goes to the console
  host's outpost even before it has reported for the console: Core finds
  the host locally (`focus_now_server`, through
  `verbatim_outpost::provider_process`), so the client's own outpost, which
  serves none of the console window, is never asked for it. When the host
  cannot be read from the client, such as for a client run as another
  user, the window has no provider process and no outpost is asked. When the supervisor reports
  `OutpostMessage::NotWatched` (no outpost was started for the
  application, or its record was removed as it exited or its outpost
  retired), the wish for its focus is answered "gone", along with a status
  entry that was only waiting for its outpost, so nothing, the idle
  barrier included, waits for an outpost that will never come. The wishes
  have one owner, `wanted::Wanted`, keyed by the process whose outpost
  they wait for: a focus-now request and the list dialogs' shell items
  requests each end exactly once, at that outpost's `Ready`, where it is
  sent, at `NotWatched`, or, for a shell items request, at its deadline.
  An `Ended` asks the replacement for the focus only when the supervisor
  says a generation follows (`follows`), so the app never infers a
  replacement the supervisor will not start. `LiveOutposts` keeps each
  live generation as `Starting` or `Ready`; `mark_ready` consumes the
  starting one. When the supervisor reports
  `OutpostMessage::MenuOrSwitchEnded` (a menu or the Alt+Tab switcher
  closed, told 50 ms later with the time it ended), the reducer is given
  `Input::MenuOrSwitchEnded`, and unless it has applied a focus observed
  since that time (`SrState::latest_focus_observed_at`) it emits
  `Effect::ReadForegroundFocus`, for which the reducer thread asks the
  foreground application's ready outpost for
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
  always keeping the newest; a timeline whose speech is still owed (asked
  for or queued, and neither measured nor ended) goes only when every other
  but the newest is owed too, since every outpost event starts a timeline
  and a flood's unspoken events otherwise pushed out the timelines of lines
  spoken hundreds of milliseconds later before they were measured. It is fed from three threads across two processes: the reducer thread
  records event observation (using the outpost's own timestamp), and the
  pipeline observer callbacks record queue and audio start (when several
  utterances share a trace, the first to be heard counts). It also keeps
  each announcement's stages in microseconds (the outpost's
  `EventTiming`, Core's receipt and reduction, and the speech milestones
  of the trace's first utterance) and logs them as one `verbatim::latency`
  line once the device clock passes its first frame or it ends (see
  `docs/tooling/inspect.md`), the outpost read
  with its count of cross-process calls. A query reply the worker answered
  feeds it the same way as an event, so a navigation keypress's trace has
  its outpost stages and calls too. A caret key's trace starts when the
  keyboard hook saw the key (`key_pressed`, from the hook's
  `pressed_at_us`, sent with the key as `ShellCommand::CaretKey`), and the
  ledger keeps the reducer's handling of the key (`requested`, any
  reduction before an outpost answered) apart from its handling of the
  answer, so the line names the hook to Core, the request's way to the
  outpost, and caret wait stages as well. Every key the hook reports
  (`key_seen`, called on the hook thread, one atomic store) is kept as the
  last key, and each line ends with the time from it to the speech
  queued, whatever trace the speech came on. verbatim.exe's relay stamps
  (`EventTiming::core_received_at_us` and those after it, and
  `core_read_at_us` on the way back; `docs/design/communication.md`, section 10.1),
  and the senders' own stamps (the listener's handling's end and write, the
  outpost's write; `docs/design/communication.md`, section 10.1)
  split the spans across verbatim.exe: the listener's handling, its queue and
  its pipe to verbatim.exe (or the reducer's span to verbatim.exe), the
  time a fact was held for its starting outpost, verbatim.exe's own relay
  time, and verbatim.exe to the outpost; and, back from the outpost, its
  outbound queue, its pipe to verbatim.exe's reader, and the wait for the
  reducer. The event's stages are one message's own: the
  message whose reduction first asked for speech on the trace, which Core
  tells the ledger as it hands the utterance to the pipeline
  (`speech_requested`, on the reducer thread, so a message received before
  the pipeline queues the utterance on its own thread is not taken for
  it). Each message received before then replaces the one before it whole,
  receipt and reduction included, and those after it are left out: a text
  focus's line, whose caret report on the same trace speaks the line,
  keeps the focus message's stages.
  `recent` returns each timeline's
  stages and calls in its `LatencyRecord`. It mirrors
  each utterance's milestones to speech subscribers as a `Speech` frame at
  queue time, a `SpeechStarted` frame when its first frame plays, and a
  `SpeechEnded` frame with its ending, each sound played at once for an
  event as a `Sound` frame naming its indication, each pause and resume
  of speech as a `SpeechPaused` frame, and it answers the
  `latency`
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
  listener thereafter reports foreground changes as facts — no thread of
  Core's hooks the foreground, the keyboard hook's included; a foreground
  change reaches the reducer as a focus on the window, which moves its
  attention, and the listener's report of the window in front
  (`OutpostMessage::ForegroundWindow`) as `Input::ForegroundWindow`, with
  Verbatim's own integrity level read once, from which Core decides the
  keyboard hook's view of the foreground), the reducer thread
  (`ReducerThread`, described below), the keyboard hook (given
  `key_reports`' callbacks; started before the control plane, whose
  gestures and idle waiters go through its `Injector`), the control
  server with its
  injected handlers (`dump_tree` hands a `ShellCommand::DumpTree` with a
  `DumpTicket`, whether locations are read, and a one-answer reply
  channel to the reducer thread and
  waits five seconds for the answer, then sends
  `ShellCommand::DumpTreeGivenUp` with the ticket, and the reducer thread
  sends the outpost a `Cancel`, which withdraws the dump if it has not
  started; `dump_recorder` calls `flight_dump::dump_now` directly, no
  outpost round trip needed; `send_gesture` checks the gesture is
  parsable, bound and no lock key's, then injects it, and the hook thread
  numbers it and cancels speech through its number, as for a key press,
  since its keys never pass the hook, or answers "the keyboard hook is
  gone" once the hook has stopped), the startup announcement, and
  finally the GUI loop on the main thread. The reducer thread speaks each
  `Effect::Speak` with `speak_for_key` and the key number Core stamped on
  it. Verbatim+V's script opens the menu (`Effect::Show`), Verbatim+F12's
  reads the time and Verbatim+F11's opens the system tray list
  (`GuiCommand::OpenShellItemList`), each waiting behind a held caret key
  as a review command does; a quick second press speaks the date or lists
  the taskbar instead: `verbatim-input` counts quick presses on each
  emitted gesture, carried on `Input::Script` to Core, whose
  `shell_list_kind` maps it to the listed surface. The list dialog's items, and the position of the item
  a click button acts on, come through `AppShellItemsHost`, which posts
  `ShellCommand::ShellItems` with the dialog's `ShellAsk` to the reducer
  thread: it finds the taskbar's process (`Shell_TrayWnd`'s owner, a
  local call), sends that process's ready outpost `Query::ShellItems`
  under `Asker::ShellItems`, or `Query::ShellItemPosition` under
  `Asker::ShellItemPosition`, or starts an outpost for it and sends the
  query once it is ready, and hands the outcome to the dialog's delivery
  callback, exactly once, through its responder: the items or the
  candidates, or `None` for any failure, when the process gets no outpost
  (`NotWatched`), when no outpost is ready six seconds after the request,
  when no answer has come six seconds after the query was sent, or when
  the request is dropped anywhere on its way (`wanted::SHELL_ITEMS_DEADLINE`: the
  outpost's own deadline for the walk, `verbatim_outpost::protocol::WALK_DEADLINE`,
  and a second for the way through the pipes, so that Core gives up only
  after the outpost's deadline has run out and what it read by then
  reaches the dialog). The requests still waiting for an
  outpost are `wanted::Wanted`'s; a request sent is the
  request table's, recorded with its deadline (`RequestTable::begin_until`),
  so a late answer is dropped. The reducer thread waits for its next
  command only until the earliest such deadline (`recv_timeout`), and
  waits without one when none is outstanding; only the queue's end
  (`Received::Closed`) ends it, since a system wait can run out a clock
  tick before the deadline. The GUI event thread tells the reducer of `GuiEvent::ShellItemGone`
  (`Input::ShellItemGone`) when the list dialog finds its chosen item
  gone, and of `GuiEvent::ShellItemUnclickable`
  (`Input::ShellItemUnclickable`) when it finds the item with no point to
  click; the reducer says either queued. When the loop exits — Exit item,
  control-plane quit, or a replacing instance's `WM_QUIT` — teardown drops
  the hooks and shuts the outposts and the listener down cleanly through
  `Supervisor::shutdown`, killing through its job only a child that has
  not exited within the time limit (see "Shutting down" below).
