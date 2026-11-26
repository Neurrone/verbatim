# verbatim-app

The composition root: `verbatim.exe`. Only the reducer speaks
(`books/dev/src/explanation/architecture.md`, section 2): every utterance and earcon is an
`Effect` of `reduce`, and the shell's threads turn what they learn into
`Input`s, posted to the reducer thread's port, never calling the speech
manager to speak; the keyboard hook's cancel, pause and resume are its one
speech send. The reducer thread runs at `Highest` (`prioritize_role`, set as
`run` starts it; Dickson, 2026-11-14), so a message into Core is not
starved by the readers' and the speech side's normal threads. The compiler holds this: every speaking function of the
speech manager takes the `SpeakToken` `SpeechManager::new` makes, which
`run` moves into `reducer_loop`, which hands it back when the loop ends;
the settings window's sound preview plays through the GUI thread's own
lane to the mixer, which `SpeechManager::new` returns beside the token.
Its `clipboard` module is the one
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
reducer as `Input::ToggleKeyPressed`; Core keeps when its state is to be
read, 30 ms later, and the reducer thread reads that due time from the
state after each step (`SrState::toggle_key_read_due`) among its
deadlines, reads the key's state with `GetKeyState` once it passes and
enters it as `QueueInput::LockKeyRead`, which Core says ("caps lock on")
and which takes the key from the front. The
time gesture's `Effect::ReadClock` is read on the reducer thread, as
`GetTimeFormatEx` and `GetDateFormatEx` word it, and applied as
`Input::ClockRead` with the trace the effect ran under and the gesture's
key number the effect carries, after the current
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
path as an accessibility event; `Effect::Speak` is executed there, and so are
`Effect::StopSpeech` and `Effect::DropExpiredSpeech`, through the speech
manager's `SpeechControl` (`cancel` and `drop_expired`), while
`Effect::CopyToClipboard` is handed to the clipboard thread. The hook's
gesture map and `key_reports`' lookups come from one table,
`verbatim_input::Tables`, made for the configured keyboard layout with the
menu's gesture (`Tables::gesture_map` builds the map), so the active
review and navigation bindings follow `settings.toml`'s
`keyboard.layout`.

The keyboard hook's two callbacks are the `key_reports` module, which
runs on the hook thread and is checked by `cargo xtask ci`'s hook rule
(`xtask/src/ownership.rs`): the speech effect calls `cancel_through` with
the key's number and posts `QueueInput::SpeechCut` with the same number, or posts the
pause's toggle; the report callback notes the key in the latency ledger and turns
each `KeyReport` into its `ShellCommand`s, by lookups in the gesture
table made when the hook starts (`verbatim_input::Tables`), and posts them to the reducer's
port in the order the hook decided them. Both callbacks post through one
function, `key_reports::Ports`, over the reducer's port and the speech
queue's: a post a port refuses while its taker is still there, which
only a system out of memory does, is kept in a thread-local queue on the
hook thread, and the next post joins it and posts the queue from its
front until a port refuses again, so no command, cancel or pause toggle
overtakes a refused one and a key's cancel and its `SpeechCut` are both kept or
both made; one whose port has closed is dropped. While posts succeed this
adds one check of an empty queue, and no timer, lock or message; a kept
post waits for the next key. Unit tests refuse a post through the
`KeyPostRefused` fault point ([verbatim-faults](verbatim-faults.md)) and
check each port takes every post once, in the hook's order. Each key
becomes a press of Core's event queue (`QueueInput::Press`), with its
number: an observed caret key a `PressAction::CaretKey`, a held one with
its stroke and the caret key its report carries; a clearing key
`PressAction::Clearing`, then, unless it is Escape, a caret key with
`CaretMotion::Other`; an editing chord `PressAction::EditingChord`, after
`PressAction::Clearing` for Escape; a key passed that types no text a
caret key with `CaretMotion::Other`; typed text `PressAction::Typed`, and a
control character `PressAction::Control`; Verbatim+V `PressAction::Script`
with `Script::ShowMenu`; the time and tray-list bindings
`PressAction::Script` with `Script::ReportTime` and `Script::ShellList`;
a review or object-navigation binding `PressAction::Command`. A lock
key's gesture is `Input::ToggleKeyPressed`; an unbound gesture nothing;
`Handled` the harness mark `ShellCommand::InputHandled` and `Barrier` the
idle waiter `ShellCommand::AwaitIdle`. The speech queue's inbox is a completion
port too (`PortInbox` and `PortTaker` over `port_queue::<QueueItem>`), so
the hook's cancel takes no lock; a unit test refuses an utterance's post
through the `SpeechPostRefused` fault point and checks that it ends
failed with its marks. The supervisor's sink is
`EventSink::posting` over the reducer's port, which logs and drops a
message the port refuses.

The end-to-end harness's hooks (private `harness` module):

- Ignored processes: when launched with `VERBATIM_IGNORE_PIDS` naming
  pids, separated by commas, Verbatim holds those processes open and
  ignores them entirely (`verbatim_process::held::IgnoredProcesses`,
  handed to `Supervisor::new`): no outpost is started for one, the focus
  listener drops their facts, the shell never asks one for its focus (a
  foreground read naming one is passed over), and a focus reported in one
  of their top-level windows is dropped before the reducer. The harness
  names the owner's own Windows Terminal (`books/dev/src/how-to/e2e.md`).
- Readiness: when launched with `VERBATIM_READY_EVENT` naming an event,
  Verbatim sets that event once it is ready for input (the GUI up, the
  focus listener running, its own window's outpost ready, the spares
  ready, and the focus known), the same condition `StatusInfo::ready`
  reports, so a test waits on the event rather than asking again and
  again; once ready, it stays ready. The reducer thread sets the
  listener, own-outpost, spares and focus parts from Core's state after
  each step (`SrState::readiness`, `SrState::focus_committed`) until
  `Readiness::core` answers that Verbatim has been ready, when it drops
  its handle, so later steps take no lock; those
  parts and whether Verbatim has been ready are under one lock, and each
  setter records its parts and decides in that hold whether to set the
  event, so two setters cannot each miss the other's part.
- The idle barrier: the keyboard hook reports each numbered key stroke
  handled (`KeyReport::Handled`), posted to the reducer thread behind
  whatever the stroke caused (`ShellCommand::InputHandled`), which records
  it and broadcasts `Frame::InputHandled` once the key's entry has ended:
  a mark is held (`idle::HeldMarks`) until the entries of every key press
  the hook reported before it have left Core's event queue
  (`SrState::oldest_key_entry`) and no lock key's announcement is still to
  come, and marks are handed on in order. A control-plane `AwaitIdle` is posted
  straight to the reducer's port (`ShellCommand::AwaitIdle`, carrying the
  waiter, by `harness::IdleLine::wait`); every harness input is a
  numbered key, so on the reducer thread it waits until the input it
  names has been handled, then
  runs a round through every child (`idle.rs`, `IdleBarrier`;
  `docs/design/communication.md`, section 6.7). The supervisor's relay
  asks the listener, then every outpost, whether anything that came before
  the question is still in progress, and tells the reducer which outposts
  it asked (`OutpostMessage::IdleQueried`). Each child answers behind
  every message it sent before, and an outpost that ends counts as having
  answered. When every answer is in, the reducer takes in what is already
  on its port and checks its own conditions: Core's event queue idle
  (`SrState::queue_idle`: no entry at the front or waiting, none to be
  appended later, no request waiting or out, no window watched for
  recovery, no stroke in Core's line, no lock key's announcement still
  to come, no delay of Core's own due), no outpost still starting, and
  no terminal's quiet-period timer armed, since what waits
  for it is spoken when it ends. If they hold, the waiters are answered once
  everything queued so far has reached the speech manager; if not, the
  round runs again once they clear. A wait that runs out answers with
  what was still outstanding.
- `DumpFocus` answers with `SrState::focus_report`, within five seconds.

Milestone M4's text protocol is wired here (`books/dev/src/reference/crates/verbatim-model.md`,
"The text protocol"):

- The reader settings (`settings.toml`'s `[reader]` section) are the
  reducer's first input, `Input::Settings`. The reducer thread owns the
  section: it changes by the reducer's own toggle keys, whose
  `Effect::SettingsChanged` is handed to the saver, and by the settings
  dialog's Terminal page: `terminal_settings::AppTerminalHost` is the
  GUI's `TerminalHost`, reading the settings from the view the reducer
  thread publishes (`ReaderView`, a copy under a lock never held across a
  call, which the reducer thread alone writes) and sending an applied
  `TerminalChange` to the reducer thread
  (`ShellCommand::TerminalSettings`). There its four values are written
  into the reducer's own settings (`SrState::settings`), the other reader
  settings left as they are, given to the reducer as `Input::Settings`,
  published, and saved.
- The saver (`saver.rs`, `books/dev/src/explanation/design/ownership.md`) is the one thread, `verbatim-settings-saver`,
  that owns `settings.toml` and writes it whole. Each section has one
  owner, which hands the saver its new value through the section's
  writer, a non-`Clone` handle whose save is one post to the saver's
  completion port, taking no lock and never waiting: the speech section
  the synth thread (`SpeechWriter`, inside `speech_settings::SpeechSettings`,
  the speech manager's `SpeechSettingsStore`), the reader section the
  reducer thread (`ReaderWriter`, in `ReducerContext`), the theme section
  the GUI thread (`ThemeWriter`, inside `AppThemeHost`). The saver takes
  everything waiting, keeps the newest value of each section, and writes
  the file once, a temporary file renamed over it; a failed write is
  logged. The global fields (locale, log filter, Verbatim keys, keyboard,
  UIA) are read from the loaded settings before the saver starts and
  have no writer while Verbatim runs; the `ConfigStore` is the startup
  load and the file's format, and nothing shares it. At exit, once the
  exit sound has been heard or its bound passed, the main thread settles
  the speech pipeline (`SpeechManager::settle`: speech cancelled, then
  everything posted to the synth thread before carried out, a commit
  included), then asks the saver to finish (`Saver::finish`), both within
  one 2-second bound, so a change made just before Exit is saved; only a
  synthesizer host that has stopped answering can hold a commit past the
  bound, and that loss is logged.
- The hook's gesture map (`Tables::gesture_map`) holds the caret keys
  but Enter's, and Enter's only when held (`CaretMotion::NewLine`), each
  with its caret key, and observes the selection keys: they reach the
  application, and each one the hook reports becomes a caret key's press
  through the same table, with the time the hook stamped it. It observes
  the clearing keys too, each of which becomes a press of
  `PressAction::Clearing` and then, like any other key the hook reports
  as passed without typing text, a caret key with `CaretMotion::Other`,
  and the editing chords, each of which becomes a press of
  `PressAction::EditingChord`, Escape after its clearing key's.
  Each text a key types becomes a press of `PressAction::Typed` (the
  hook's translation, [verbatim-input-windows](verbatim-input-windows.md)),
  each control character a key types, after the key's other reports, a
  press of `PressAction::Control`, and each key that
  cancels speech also sends `QueueInput::SpeechCut`, as does a gesture
  injected through the control plane, so say-all stops. All of these go
  straight from the hook thread to the reducer thread's port, each one
  post, so the hook never waits, and in the order the keys were
  pressed. The hook's `DecisionConfig` takes the two speech interrupt
  settings from `keyboard.speech_interrupt_for_characters` and
  `keyboard.speech_interrupt_for_enter`. `key_reports` maps a script
  action to its reducer command with `ScriptAction::review_command`.
- Held caret keys (`books/dev/src/explanation/design/key-holding.md`): the hook's gesture map
  holds the caret table's keys (`GestureMap::with_held`), while the
  hook's `ForegroundView`, which the reducer thread sets after every
  input from `SrState::hook_foreground`, says the focus has text and the
  window in front lets keys be held (not an elevated window's). A held
  key's report is a press whose caret key carries its stroke, and the
  hook's deferred transitions (`KeyReport::Deferred`) join Core's stroke
  line in the order reported ([verbatim-core](verbatim-core.md)). Core
  names what goes (`QueueEffect::SendStrokes`), which `strokes` sends with
  one `SendInput`, each key carrying its place in line
  (`passed_on_info`): a held key with the gesture's Shift, Control or Alt
  that are not down pressed around it and the key released first if
  Windows still has it down, as NVDA's gesture send does, and a
  character typed as Unicode (`VK_PACKET`) as Unicode again. At exit
  (`ShellCommand::Exiting`, after the hook is dropped) Core names the keys
  whose last transition in its line is a release
  (`QueueEffect::ReleaseAtExit`), and each Windows has down
  (`GetAsyncKeyState`) is released with one `SendInput`. `Effect::ForKey`
  is executed on the named key's trace.
  `QueueEffect::WakeAt` sets or clears the time Core's event queue asked
  to be woken at, kept beside the reducer thread's other deadlines
  (`next_deadline`); when it passes, the thread enters `QueueInput::Due`,
  which carries the window Windows names in front
  (`hang_check::live_front`: the foreground window, a ghost mapped to the
  hung window it stands for, with its root owner, class, owning thread
  and the process whose outpost reads it, a console window's console
  host) while a check after a menu, popup or switcher end waits
  (`SrState::menu_check_waits`), the one read of Windows the shell makes
  for a decision of Core's (ADR 0003).
  `QueueEffect::Cancel` sends the reader of a request Core gave up on
  `CoreToListener::Cancel` or `CoreToOutpost::Cancel`.
  The focused terminal's quiet-period timer is another of those
  deadlines, read from the state after each step
  (`SrState::terminal_timer_due`), never kept here, so a terminal model
  Core dropped leaves no timer; its due time on the shared clock is turned
  into an instant, and once it passes the thread enters
  `QueueInput::TerminalDue` with that due time and a trace ID minted then.
- The speech pipeline's observer (`speech_events`) is the latency ledger
  plus the marks: every index mark playback reaches becomes
  `QueueInput::MarkReached` posted to the same port, joining Core's event
  queue in arrival order, so say-all advances, and every utterance with
  marks that ended before playback reached them all
  (`SpeechEvents::marks_cancelled`) becomes
  `QueueInput::UtteranceCancelled`, and every one that failed
  (`marks_failed`) `QueueInput::UtteranceFailed`, both handled at once,
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
  (`books/dev/src/explanation/design/ownership.md`). Any other change
  (`Live::held_changed` answers `HeldChange::Other`: the anchors, or the
  position of the outpost's last message handled alone) is left in that
  outpost's writer (`Supervisor::leave_nodes_held`), and the next command
  written there, a request or the relay's ping within three seconds,
  carries it in the same write, after its own frame
  (`docs/design/communication.md`, step 11): the caret's anchor changes
  with every key, and a terminal's output changes the position with every
  message (`books/dev/src/explanation/design/wake-ups.md`, item 4).
- The listener's told elements Core holds (`SrState::held_tokens`) go to
  the listener by the same rule, through `verbatim_core::ListenerView`:
  the thread counts each listener message carrying a token as it hands it
  to Core (`ListenerToCore::carried_token`), and after each pass writes
  the view at once (`Supervisor::send_elements_held`) when a token Core
  handled is no longer held and no view written said so, so the listener
  lets go of a tell Core refused within one pipe hop; additions alone are
  left for the next command (`Supervisor::leave_elements_held`). A
  listener started (`ListenerStarted`) counts from 0.

Milestone M4's themes and earcons are wired here too
([verbatim-model](verbatim-model.md), "Themes"):

- `themes` loads the theme the configuration names
  (the `[theme]` section's `id` and options, through
  `verbatim_config::themes::find_theme`; one that cannot be loaded is
  logged and the built-in default theme used instead) and makes a theme
  ready (`prepare`): its sounds are decoded from its own directory or the
  shared `sounds` folder beside `verbatim.exe` (`LoadedTheme::sound_path`),
  and every problem found is logged. At startup this happens before the
  speech manager is built, which is given the theme at construction, so
  speech is presented with the configured theme and its sounds from the
  first word; the reducer is sent `Input::Reported` with the details the
  theme reports (`send_reported`), from which Core chooses what each of
  its requests fetches. `activate` does both for a theme
  chosen later, setting it on the speech manager's theme handle. It runs
  whenever the settings dialog's Theme panel
  changes the theme or its options: `AppThemeHost` is the GUI's
  `ThemeHost`, over the themes and sounds folders, the theme section of
  the settings, which the GUI thread owns (the configured theme is the
  section it last saved, and saving the choice hands the section to the
  saver), the GUI thread's lane to the mixer (playing a sound as the
  sound choice shows it), and the speech manager (activating a theme,
  its options alone through the theme handle, and previews, which go to
  the reducer as
  `Input::PreviewSpeech` and `Input::PreviewEarcon`). A sound that is missing (a run
  without the `sounds` folder) is spoken instead, so nothing is lost.
- `Effect::PlayEarcon` plays through `SpeechManager::play_earcon`. The
  shell tells the reducer of three events whose sounds it plays: the
  start (`Input::Started`, posted once the keyboard hook is installed,
  before the foreground's focus is asked for); the exit, once the GUI
  loop has ended (`ShellCommand::Exiting`, which the reducer thread
  applies as `Input::Exiting` with whether a new instance is replacing
  this one, which plays no sound; Core cancels speech first); the reducer
  thread plays the exit sound
  with `play_earcon_heard`, one record to the mixer that never waits, and
  hands its `EarconHeard` back, on which the main thread waits until it
  has been heard, for at most two seconds; and
  a window not responding: a request Core's event queue finds refused or
  stopped because its window is hung plays the sound only if that
  window's application holds attention.
- The build script copies the top-level `sounds` folder next to the
  executables in the cargo target directory, so `target\debug` holds
  everything a run needs, and the end-to-end staging copies it from
  there. It also sets `VERBATIM_COMMIT` to `git rev-parse
  HEAD` and `VERBATIM_UNCOMMITTED_CHANGES` to whether `git status
  --porcelain` printed anything, `unknown` and `false` when git cannot
  answer, which `flight_dump` writes into the dump's header; it reruns when
  HEAD, the ref it names, the packed refs or the index change, so editing
  a file without staging it leaves the recorded state as it was.
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
- `ReducerThread` — owns the reducer state, the askers of Core's requests
  from outside the event queue, and the
  live-outpost set, and is the only thread that touches them. It takes
  every input from its port (`Commands`, a `port_queue::Poster` of
  `ShellCommand`s, is what every other thread posts through), first in,
  first out across all its posters: the supervisor's messages
  (`ShellCommand::Outpost`), the keyboard hook's keys and gestures,
  speech marks, and control-plane requests. It passes every supervisor
  message through the supervisor's `Frames`, which turns each outpost's
  frame into the message it carries, on the reducer's thread whatever its
  size, in the order the frames arrive. Core's event queue
  (`Input::Queue`) takes, in arrival order, the readers' tells and answers
  (an outpost's, and the listener's `OutpostMessage::Listener`, whose
  window destroyed becomes `QueueInput::WindowDestroyed`), which Core
  judges by generation, dropping a frame of an instance not current; the
  owner thread's process facts (`Started` as
  `ProcessFact::OutpostStarted`, an outpost's `Ready` and
  `ListenerReady` as `ReaderReady`, `Ended` and `ListenerEnded` as
  `ReaderEnded`, but an `Ended` for its application's exit, which Core
  takes as the `ApplicationExited` that follows, `ListenerStarted`,
  `Pong`, `SpareReady`, `LaunchFailed`, and `ApplicationExited`); the
  listener's foreground report, with
  the window's class; the hook's presses, deferred transitions and Alt
  changes; and, posted before Verbatim starts, the harness's ignored
  processes and Verbatim's own process, whose outpost Core
  starts at once; and the session's lock state (`QueueInput::Lock`), from
  the `session_lock` module's thread, which registers a message-only
  window for the session's notifications, then queries the state
  (`WTSQuerySessionInformationW`, `WTSSessionInfoEx`; unknown when the
  query fails or its flags are mixed) and posts it before any lock or
  unlock notification, so none between the two is lost. A press's caret key is bound to the committed focus
  (`SrState::caret_binding`) as the thread takes it, in the order of the
  inputs before it. Core's effects are carried out: a request
  (`QueueEffect::Send`) goes to the listener (`send_to_listener`) or to
  the outpost generation Core names, unless Windows reports its window
  hung, when it is answered unsent: a read of an application's current
  focus with the window in front, read live by `hang_check::front`
  (`QueueInput::InFront`: the foreground window, or the desktop window
  with none, then its thread's focus window, as the desktop window with
  its owner, a ghost with its text, the window it stands for with that
  window's application and thread, and its disabled style, hung,
  responsive or gone, each a read of Windows' own over a trait its unit
  tests stand in for), any other "not responding"
  (`QueueInput::RefusedHung`). The check after a menu, popup or switcher
  end (a batch that tests the focus's focused state before reading it,
`Batch::reads_focus_if_focused`) finding no window in front
  (`hang_check::nothing_in_front`) is answered unsent the same way; one that cannot be
  written is answered "reader ended"; a cancel goes with its reason; a
  hang check is answered with `IsHungAppWindow`. The `hang_check` module
  makes both checks: a ghost window is first mapped to the hung window it
  stands in for (`HungWindowFromGhostWindow`), and so is a foreground
  report naming a ghost before Core takes it, so Core judges the hung
  window and its application, not the ghost's process. `StartOutpost` is
  `start_outpost`, `EndOutpost` is `end_outpost`, `WindowDestroyed` goes
  to the outpost Core names, and `WakeAt` is one of the thread's
  deadlines, its passing fed as `QueueInput::Due`. Each answer the shell
  gives enters the queue after the input whose effects are being carried
  out. Every other outpost message arrives tagged with the
  outpost generation whose pipe carried it; an outpost joins the live set
  on `OutpostMessage::Started`, which precedes all its messages, and leaves
  it on `OutpostMessage::Ended`, and such a message from an outpost not in
  the set is dropped. The status
  mirror follows the same notices. Once each outpost message or command
  has been handled whole, never between the inputs one message produces,
  the thread tells each live outpost (`send_views`) which of its nodes the reducer
  holds, with the position of the last of that outpost's messages it has
  handled, whenever either changes, written at once or left for the next
  command as the held anchors are (below), so the outpost can release
  everything else
  ([verbatim-outpost](verbatim-outpost.md), "Held objects"). On an end, other
  than one for its application's exit, which the supervisor reports
  next, Core's event queue gets `QueueInput::ReaderEnded`, and the control
  plane's event subscribers get `Frame::OutpostEnded`. There is no foreground pid gate: which
  events are spoken is the reducer's attention model. Events go to the
  reducer and the latency ledger; the control plane's event subscribers
  get the events Core took, its `Effect::Observed` (copied for the
  control plane only while one is subscribed);
  a reader's answer enters Core's event queue as `QueueInput::Answer`,
  tagged with the instance that sent it. Core's event queue counts its
  requests from outside the queue among its own for the idle condition. The thread never waits on a
  handoff: speech and control-plane sends never block, and a command for an
  outpost is queued on that outpost's writer, failing at once if the queue
  is full.
- `requests` — who asks Core's requests from outside the event queue,
  and where each outcome goes ([verbatim-core](verbatim-core.md), "Core's
  requests"): the control plane's tree dump (a batch
  `verbatim_core::outside::dump` builds, naming the attention
  application's main window, `verbatim_win32::window::main_window_of`)
  and the notification area list's two reads (batches of walks of the
  taskbar's windows, naming `Shell_TrayWnd`), each answer built by the
  same module. Each is entered
  into Core's event queue as `QueueInput::AskOutside` under a token
  `requests::Outside` keeps its `Asker` by; Core sends it to the outpost
  of its window's process once that outpost is ready, starting one when
  there is none, with the queue's due time and live hang check, never
  stopped by a foreground change, so the click's read survives the
  dialog's closing (Dickson, 2026-11-15), and answers it once
  (`QueueEffect::Answered`), which `requests::deliver` carries out: a
  tree dump's on its reply channel, a shell read's to its dialog, `None`
  for any failure. A dump given up is withdrawn
  (`QueueInput::WithdrawOutside`), its one answer going to a receiver
  dropped. `LiveOutposts` keeps the app's own facts of each generation
  started and not ended: the position of its last message handled and
  the held nodes last left for it. Which generation is current, starting
  or ready is Core's record's; the idle conditions count the generations
  starting there (`SrState::outposts_starting`).
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
  line once the device clock passes its first frame, on a device with no
  clock once the device takes that frame, or else once it ends (see
  `books/dev/src/how-to/inspect.md`), the outpost read
  with its count of cross-process calls. A query reply the worker answered
  feeds it the same way as an event, so a navigation keypress's trace has
  its outpost stages and calls too. A caret key's trace starts when the
  keyboard hook saw the key (`key_pressed`, from the hook's
  `pressed_at_us`, sent with the key's press as `QueueInput::Press`), and the
  ledger keeps the reducer's handling of the key (`requested`, any
  reduction before an outpost answered) apart from its handling of the
  answer, so the line names the hook to Core, the request's way to the
  outpost, and caret wait stages as well. Every key the hook reports
  (`key_seen`, called on the hook thread, one atomic store of a time and
  a flag packed in one `AtomicU64`, since no lock may be taken on the hook
  thread) is kept as the last key, and each line ends with the time from it to the speech
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
- `flight_dump` (milestone M2) — `dump_now(recorder, dumps_dir)` takes a
  `FlightSnapshot` of the shared `Arc<Mutex<ReducerRecorder>>` under its
  lock, one `Arc` per segment and no entry copied, so the reducer waits
  at most that step whatever the window holds (recovering a poisoned lock
  rather than propagating it, since the reducer thread panicking while
  holding it is exactly the case the panic hook below exists for); the
  recorder keeps its window for later dumps. After releasing the lock it
  writes the snapshot's checkpoint and entries through
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
  eSpeak NG through a mixer over `WasapiDevice` by default, whose
  delivery thread `run` raises to `THREAD_PRIORITY_HIGHEST`, since it
  only moves messages; eSpeak NG
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
  ignored), the saver and its section writers, the settings host, whose
  commit saves on the synth thread through the speech section's writer,
  the supervisor with its focus listener (decision D13;
  Core's event queue asks the application in front for its focus at the
  listener's first foreground report — no thread of
  Core's hooks the foreground, the keyboard hook's included; the
  listener's report of a window that came to the foreground
  (`ListenerToCore::Foreground`, inside `OutpostMessage::Listener`)
  reaches Core once, as `QueueInput::Foreground`, with the window's
  class, a ghost window mapped to the hung window it stands in for and
  kept as `ghost`, and Verbatim's own integrity level, from which Core
  decides attention, its foreground record and the keyboard hook's
  view of the foreground), the
  reducer thread
  (`ReducerThread`, described below), the keyboard hook (given
  `key_reports`' callbacks), the control
  server with its
  injected handlers (`dump_tree` hands a `ShellCommand::DumpTree` with a
  `DumpTicket`, whether locations are read, and a one-answer reply
  channel to the reducer thread and
  waits for the answer, then sends
  `ShellCommand::DumpTreeGivenUp` with the ticket, and the reducer thread
  withdraws the request from Core, whose cancel drops the dump if it has
  not started; `dump_recorder` calls `flight_dump::dump_now` directly, no
  outpost round trip needed; `open_menu` sends the GUI
  `GuiCommand::ShowMenu`, as a click on the tray icon does, with no key
  number, or answers "the GUI is not up"), the startup announcement, and
  finally the GUI loop on the main thread. The reducer thread speaks each
  `Effect::Speak` with `speak_for_key` and the key number Core stamped on
  it. Verbatim+V's script opens the menu (`Effect::Show`), Verbatim+F12's
  reads the time and Verbatim+F11's opens the system tray list
  (`GuiCommand::OpenShellItemList`), each waiting behind a held caret key
  as a review command does; a quick second press speaks the date or lists
  the taskbar instead: `verbatim-input` counts quick presses on each
  emitted gesture, carried on `PressAction::Script` to Core, whose
  `shell_list_kind` maps it to the listed surface. The list dialog's items, and the position of the item
  a click button acts on, come through `AppShellItemsHost`, which posts
  `ShellCommand::ShellItems` with the dialog's `ShellAsk` to the reducer
  thread: it finds the taskbar window (`Shell_TrayWnd`, a local call) and
  asks Core for the item list under `Asker::ShellItems`, or the chosen
  item's position under `Asker::ShellItemPosition`, from
  outside the event queue (`requests`, above), and hands the outcome to
  the dialog's delivery callback, exactly once, through its responder:
  the items or the candidates, or `None` for any failure, no taskbar
  among them.
  The reducer thread
  turns its own timers into a moment on the monotonic clock for its
  wait, erring a microsecond late (`instant_at`), and waits for its next
  command only until the earliest (`recv_timeout`), and without one when
  none is due; only the queue's end (`Received::Closed`) ends it, since a
  system wait can run out a clock tick before the deadline. Each pass of
  the loop (`pass`) handles the command that ended the wait; then, once
  the earliest deadline has passed, it handles every command already
  waiting before it judges the deadlines (`expire`), since each may carry
  a fact a judgement depends on, such as a pong the relay read while the
  thread was held. The limit: a port that never empties, a command
  arriving as fast as each is handled, delays the deadlines' judgement
  for as long as it lasts. The GUI event thread tells the reducer of `GuiEvent::ShellItemGone`
  (`Input::ShellItemGone`) when the list dialog finds its chosen item
  gone, and of `GuiEvent::ShellItemUnclickable`
  (`Input::ShellItemUnclickable`) when it finds the item with no point to
  click; the reducer says either queued. It tells it of
  `GuiEvent::BlockedByMessageBox` (`Input::BlockedByMessageBox`) when the
  menu's Settings or Exit is refused while a modal message box is up, and
  the reducer says NVDA's "Action unavailable while a dialog requires a
  response". When the loop exits — Exit item,
  control-plane quit, or a replacing instance's `WM_QUIT` — teardown drops
  the hooks and shuts the outposts and the listener down cleanly through
  `Supervisor::shutdown`, killing through its job only a child that has
  not exited within the time limit (see "Shutting down" below).
