# Walkthroughs: six end-to-end narratives

[The crate guides](../reference/crates/readme.md) describe each crate on its own; these
walkthroughs follow one realistic case at a time across the crates, naming
the process and thread at each step and what crosses each boundary. They
are written for someone who needs to explain who owns what, in what order
things happen, how failures are recovered, and when speech is cut off.
The crate guides own the detail; each step here names the guide to read
for it. Each narrative ends with what is not implemented yet or known to
be limited. Code is cited by file path and function or type name.

## The processes and threads involved

Verbatim runs as several processes, all started by `verbatim.exe` and
placed in kill-on-close job objects, so they die with it however it ends
(architecture section 1; [verbatim-process](../reference/crates/verbatim-process.md)):

1. **Core**, `verbatim.exe` ([verbatim-app](../reference/crates/verbatim-app.md)). The
   threads that matter here:
   - `verbatim-input-hook`, the low-level keyboard hook;
   - `verbatim-reducer`, which runs Core's inbox and event queue and
     carries out their effects (`ReducerThread` in
     `crates/verbatim-app/src/main.rs`);
   - `verbatim-relay`, the supervisor's lifecycle owner, which reads the
     focus listener's pipe and every outpost's and finishes every child's
     writes on one completion port, plus short-lived launch helpers
     (`crates/verbatim-supervisor/src/`);
   - `verbatim-speech-queue` and `verbatim-synth`, the speech manager;
   - `verbatim-audio`, the mixer's one audio thread;
   - one control-plane reader and writer per connected client;
   - the GUI thread, which is the main thread.
2. **The focus listener**, `verbatim-listener.exe`: one per
   Verbatim, desktop-wide (decision D13). It holds every desktop-wide
   registration, the UIA focus handler and every `WinEvent` hook but a
   console host's, and tells Core of each event. A check thread per
   application makes, as each UIA event is delivered, the calls NVDA's
   handler makes for it; a reading thread per application reads a UIA
   focus when Core asks.
3. **One outpost per application**, `verbatim-outpost.exe --target-pid
   <pid>` (decision D9). Inside: an event thread that hooks a console
   host's five events, in a console host's outpost only; UIA callback
   threads for the objects Core names it to follow, registered on a
   registering thread; an I/O thread on the main thread, which reads
   Core's requests and finishes writes to Core that pended; and one
   worker (`verbatim-worker`) that is the only thread calling into the
   application, taking Core's requests one at a time.
4. **One synthesizer host per synthesizer in use**,
   `verbatim-synth-host.exe` (decision D18).

For the end-to-end suite there is also `verbatim-agent.exe`, which runs in
the signed-in user's session and is not a child of Verbatim.

## 1. The life of a focus change: Alt+Tab to Notepad

The user is on the desktop, holds Alt, presses Tab to reach Notepad, and
releases Alt. Notepad has no outpost yet.

1. **Key presses cut off current speech.** In Core, the hook thread runs
   every key-down through `DecisionMachine::on_key`
   ([verbatim-input](../reference/crates/verbatim-input.md)). Alt and Tab are not bound,
   so they pass to Windows, but each key-down's decision carries
   `KeySpeechEffect::Cancel`, and the hook calls the speech manager's
   `SpeechControl::cancel_through` (a non-blocking channel send) before
   anything else. See walkthrough 3.
2. **Windows raises events.** When Alt is released, Windows raises
   `EVENT_SYSTEM_SWITCHEND`, `EVENT_SYSTEM_FOREGROUND` for Notepad's
   window once it is the foreground window, a UIA focus-changed event for
   Notepad's text area, and an MSAA focus event for it
   (`docs/parity/focus-events.md`, "Stale focus events").
3. **The listener tells Core.** In
   `crates/verbatim-listener/src/listener.rs`
   ([verbatim-listener](../reference/crates/verbatim-listener.md)):
   - The foreground event is told only once `GetForegroundWindow` names
     Notepad's window; until then the listener holds every later tell
     behind it, up to 20 ms, as NVDA defers its event pump. Each tell
     carries what came with the event, the application and window, when
     it arrived, and the facts the acceptance test reads about its window
     and the foreground, all reads of Windows' own state that cannot hang.
   - The MSAA focus event is told with its window, object and child ids.
   - The UIA focus callback keeps the element and hands the event to
     Notepad's check thread, which walks to the element's nearest window,
     probes it for a native UIA provider, and reads the element's keyboard
     focus, the calls NVDA's handler makes at delivery; the tell carries
     their results. A UIA element cannot be handed to another process
     (`books/dev/src/reference/uia-bugs.md`, "Elements cannot be passed to
     another process"), which is why the listener keeps it.
   - The switcher's end is told as it arrives.
4. **Core's inbox takes them in arrival order.** The relay reads the
   listener's pipe and posts each frame to Core's thread. The foreground
   tell is handled at once: Core's foreground record names Notepad's
   window, every request whose window is outside it is no longer awaited,
   and a foreground marker joins the back of the event queue. Every other
   tell joins the event queue. Notepad has no outpost, so Core's
   generation record starts one, usually by giving it a spare; a request
   to it waits for its `Ready` under its own due time.
5. **The front of the queue.** Entries are taken one at a time
   ([verbatim-core](../reference/crates/verbatim-core.md), "Core's
   requests"). At the foreground marker's turn the readers' last focus,
   on the desktop, is emptied, since Notepad's window is unrelated to it.
   The MSAA focus is judged by the window's backend verdict: Notepad's
   text area is read through UI Automation, so the MSAA focus is dropped,
   as NVDA's `winEventToNVDAEvent` drops a native UIA window's event. The
   UIA focus is judged on its tell's facts, with no call: NVDA's
   native-element test, its focus overrides, the acceptance test against
   the foreground record, the keyboard-focus answer, and the duplicate
   test against the readers' last focus, which is empty.
6. **Core asks the listener to read it.** The request names the element's
   token and the properties to read, with no ancestors Core already
   knows, since the readers' last focus is empty. Notepad's reading
   thread reads the element and walks its ancestors to the top, and
   answers a snapshot and the element's route.
7. **Core asks Notepad's outpost to complete it,** under the same due
   time. The worker finds its own copy of each element on the route,
   registers the property changes of the focus and its ancestors, reads
   the focus's states, the caret and its line, and answers the focus
   with the node ids it minted
   ([verbatim-outpost](../reference/crates/verbatim-outpost.md)). Every
   call into Notepad waits for Notepad; Core's shell checks the window for
   a hang before each request and every half second while one is out
   (walkthrough 4).
8. **Core executes and speaks it** ([verbatim-core](../reference/crates/verbatim-core.md),
   "Focus, events and objects"). The new focus's top-level window differs
   from the old one just below the desktop, so speech is cut. With the
   readers' last focus empty, the window's title is said, then the focus's
   name, role, value and states in NVDA's order, and its line. The
   committed focus and the readers' last focus are now the text area, the
   navigator and review cursor snap to it, and Core tells the outpost
   which objects to follow.
9. **The switcher's end.** At its turn Core notes the committed focus and,
   50 ms later, appends a check entry; at the check's turn, if a focus has
   been committed since, the usual case, it ends with no read; otherwise
   Core asks the outpost of the application whose window Windows named
   in front when the check came due for its focus, NVDA's fake focus.

The order is Core's arrival order, as NVDA's is its queue's: the listener
tells every event of every application through one pipe, so a desktop
focus raised just before the switch is judged before Notepad's, against
the foreground record as it was then.

Not yet, or known limitations:

- A window nameless when focus enters it is not announced later, and a
  window that takes the foreground well after Windows announces it is not
  announced either; NVDA behaves the same.
- The per-source cap on background events and tooltip and
  notification-bar windows are not implemented ([parity ledger](https://github.com/Neurrone/verbatim/blob/main/docs/parity/focus-events.md), "Event
  acceptance").
- The latency stages and their figures are in [performance](../reference/performance.md).

## 2. The life of a command keystroke: Verbatim+numpad6

The user presses Verbatim+numpad6 (move the navigator to the next object)
while focus is in Notepad's text area.

1. **The hook decides, in microseconds.** On Core's hook thread
   ([verbatim-input-windows](../reference/crates/verbatim-input-windows.md)), the
   Verbatim key's key-down is swallowed and cancels speech; numpad6's
   key-down completes the bound chord `kb:verbatim+numpad6`, so it is
   swallowed too (its key-up will be as well), its `Cancel` speech effect
   is carried out first with the key's number, and the gesture, with a
   fresh trace id, a repeat count and that number, is reported. The hook
   never blocks, since Windows silently removes a low-level hook that is
   too slow, and never drops a key.
2. **The report becomes a press, still on the hook thread.**
   The report callback (`crates/verbatim-app/src/key_reports.rs`) looks
   the gesture up in the layout's gesture table (`verbatim_input::Tables`),
   finds `ScriptAction::MoveToNextSibling`, maps it to
   `ReviewCommand::NextSibling` (`ScriptAction::review_command`), and posts
   it to Core's thread as a press for the event queue
   (`QueueInput::Press` with `PressAction::Command`), in line with the
   keys. Verbatim+V, the time and the tray and taskbar lists become
   `PressAction::Script`, and a lock key `Input::ToggleKeyPressed`, which
   Core turns into effects its thread carries out.
3. **The press waits for its turn.** The press joins Core's event queue
   behind every event the readers told before it
   ([event queue](design/focus.md)), so the command acts on the object
   those events left. At its turn the press entry
   (`crates/verbatim-core/src/keys/entry.rs`) runs the navigation
   (`navigator::navigate` in `crates/verbatim-core/src/navigator.rs`): it
   finds the navigator object and the window it is in, and asks the
   outpost of that window's process for the neighbour, a request the queue
   records and awaits, holding the queue until it ends. Nothing is spoken
   yet.
4. **Core's thread sends the request.** The queue's `Send` effect names
   the outpost generation and the request, a batch of one `Related` step
   from the navigator's node with the properties to read of the node
   reached. The shell checks the window
   for a hang first, and answers a request to a hung window "not
   responding" unsent; otherwise it sends it to the outpost
   (`CoreToOutpost::Request`). If the generation has ended, the request is
   answered at once as ended.
5. **The outpost answers.** The outpost's worker parses the batch and
   runs its step by `read::navigate`: one raw-view step for a UIA node,
   one remote program or a tree walker call, `accNavigate` for an MSAA
   node, with a neighbour in a window the other backend owns re-read
   through it (`corrected_backend`, NVDA's `correctAPIForRelation`). It
   answers once, the step's outcome: the neighbour, no neighbour (a real
   edge), gone, or failed.
6. **The answer re-enters the queue.** The answer names its request's
   number; the queue takes it only from the generation asked and only
   while the request is out, and drops a late one. The paused navigation
   resumes with the outcome, parsed once against the batch sent and read
   by the step's handle.
7. **Core acts on it.** A node moves the navigator and is announced; no
   neighbour speaks NVDA's edge message ("No next" and the like); gone
   speaks the same message and leaves the navigator where it is; no
   answer says nothing.
8. **Speech**, queued, as in walkthrough 3.

Other commands follow the same path. Verbatim+numpad5 (report the current
object) reads the navigator object live, then speaks it, spells it on the
second press, and on the third emits `Effect::CopyToClipboard`.
Activation asks the outpost for the object's default action as another
request of the queue. A gesture a test sends is pressed as its
keys, so it takes this same path, and a second press within 500 ms is
counted as a person's is.

Not yet, or known limitations: an object with no text interface is
reviewed by its flat name or value, whose start and end of line speak the
line rather than the character there ([review parity](https://github.com/Neurrone/verbatim/blob/main/docs/parity/review.md));
layout changes take effect only after a restart.

## 3. The life of an utterance

Following the focus announcement for Notepad's text area from the
reducer's `Speak` effect to the speaker (decisions D12, D17, D18;
[verbatim-speech](../reference/crates/verbatim-speech.md),
[verbatim-audio](../reference/crates/verbatim-audio.md),
[verbatim-synth-hosted](../reference/crates/verbatim-synth-hosted.md),
[verbatim-synth-host](../reference/crates/verbatim-synth-host.md)).

1. **Handing over.** The reducer thread calls `SpeechManager::speak`,
   which gives the utterance an `UtteranceId` and sends it to the queue
   thread without waiting.
2. **Queued.** On the queue thread (`QueueThread::speak` in
   `crates/verbatim-speech/src/manager.rs`), the presenter
   (`ThemePresenter`) flattens the structured utterance through the active
   theme into a `SpeechSequence`: text with index marks, pitch changes, and
   sounds. `SpeechEvents::utterance_queued` goes to the latency ledger
   (`crates/verbatim-app/src/latency.rs`), which broadcasts a `Speech` frame
   to control-plane subscribers. The utterance joins a lane by priority:
   `Interrupt` cancels everything first, `Next` jumps ahead of `Queued`.
   Almost everything is `Queued`, as in NVDA.
3. **Handed on.** When the synth thread is idle, `pump` takes the next
   utterance. One whose `FocusValidity` no longer holds is ended as
   cancelled. Otherwise it is sent to the synth thread with its own
   cancellation flag and the speech source's cancel generation, under
   which the synth thread registers it with the mixer
   (`SpeechLane::register`), which from then on owns its ending.
4. **Synthesized in another process.** On the synth thread, `run_job`
   calls the active driver's blocking `speak`. The driver is a
   `HostedSynth`: it sends `ToHost::Speak` over a private pipe to
   `verbatim-synth-host.exe`, which runs eSpeak NG or OneCore and writes
   audio as `Pcm` frames and marks as `Mark` frames, then `Done`. The synth
   thread reads the pipe itself and passes each frame to the pipeline
   sink, which trims silence and pushes it into its lane to the mixer
   (`SpeechLane::write`).
5. **Backpressure.** `SpeechLane::write` waits while the source is more
   than the device buffer plus 40 ms ahead of playback, so the synth
   thread stops reading, the pipe fills, and the host's synthesizer
   blocks. Little audio is ever queued, so cancelling is cheap.
6. **Played.** The mixer's audio thread (`crates/verbatim-audio/src/mixer/run.rs`),
   which takes no lock, drains the lanes and sums the sources
   (`mix_sources`) into the `AudioDevice`: WASAPI shared
   mode on the default device
   ([verbatim-audio-wasapi](../reference/crates/verbatim-audio-wasapi.md)), or the
   silent real-time device under `VERBATIM_TEST_AUDIO=null`. When an
   utterance's first frame has played it reports `PlaybackEvent::Started`,
   and when every frame has played, `Ended` with `Completed`, through the
   mixer's delivery thread, which runs the listeners; the ledger
   broadcasts `SpeechStarted` and `SpeechEnded` and logs the
   announcement's `verbatim::latency` line. With `VERBATIM_RECORD_AUDIO`
   set, a `WavRecorder` tap receives exactly the frames that played (D16).

Every utterance ends exactly once, as completed, cancelled, or failed:
owned by the queue thread until it is handed on, by the mixer after.

When speech is cut off (`docs/parity/focus-events.md`, "When speech is
cut off, and cancellation of expired focus speech"):

- **A key press.** Every key-down except the volume keys cancels
  everything (`SpeechControl::cancel_through`, with the press's sequence
  number, so speech an earlier press caused that arrives later is
  cancelled too): waiting utterances end cancelled, the speech source's
  cancel generation is raised (`SourceControl::cancel`), so the mixer
  discards unplayed audio at its next pass, and then the in-flight job's
  flag is set.
  `HostedSynth` sends the host `Cancel` naming that utterance and reads
  until its `Done`, so a cancel can never cancel the next utterance.
  Shift alone pauses and resumes instead.
- **A new foreground window or entering a menu.** `Effect::StopSpeech` is
  the same cancel.
- **The focus moved on.** `Effect::DropExpiredSpeech` tells the queue
  thread where the focus now is; if an utterance already handed on no
  longer holds, everything handed on is stopped, and waiting utterances
  are judged when their turn comes. Entered containers never expire.

Device recovery: on a device error or a change of default device (an
`IMMNotificationClient` sets `needs_reopen`), the mixer rewinds every
source to what had played and reopens the device, retrying once a second;
with no device, the WASAPI device falls back to the silent real-time
device until one returns.

The end-to-end harness sees three frames per utterance (`Speech` with the
text, `SpeechStarted`, and one `SpeechEnded`), matches text only against
`Speech` frames, and waits for that utterance's ending. The control
server disconnects a subscriber whose queue of 256 frames fills rather
than drop a frame.

Not yet, or known limitations:

- When focus speech already handed on expires, speech handed on after it
  is stopped too, because audio cannot be taken out of the middle of the
  mixer's buffer; NVDA keeps it.
- The validity check lacks NVDA's clause for a menu item when focus moves
  to a popup menu.
- NVDA's settings for not cancelling speech on typed characters or Enter
  are not offered.
- Device recovery has not been verified live; the mixer's unit tests cover
  reopening and rewinding.
- eSpeak NG drops an index mark after a full stop, so its driver starts a
  new synthesis at such a mark; OneCore ignores a sequence's language.

## 4. The life of a query: a tree dump that times out

A developer runs `verbatim-inspect` to dump the tree of the application
holding attention (the control plane's `DumpTree` request;
[verbatim-control](../reference/crates/verbatim-control.md)). This shows the rules
every query follows.

1. **Asked.** On the control server's connection thread,
   `request_dump_tree` mints a `DumpTicket`, sends `ShellCommand::DumpTree`
   to the reducer thread, and waits up to 25 seconds
   (`DUMP_TREE_TIMEOUT`: the focus read's due time, then the dump's own,
   and a second for the way back).
2. **Recorded.** `ReducerThread::dump_tree` finds the main window of the
   application holding attention, records the asker, and gives Core's
   event queue `QueueInput::AskOutside` with the dump's batch, a
   `Descendants` of that window (`verbatim_core::outside::dump`). It is a
   request from outside the event queue: it holds no key, it has the
   queue's due time, 11 seconds, and its live hang check, and Core does
   not stop waiting for it when the foreground changes. Request numbers
   are never reused, so a late answer can never satisfy a newer request.
3. **Sent.** Once the application's outpost is ready, Core's shell checks
   the main window for a hang and sends the request
   (`CoreToOutpost::Request`), or answers it "not responding" unsent. The
   outpost's worker takes the event queue's requests ahead of every
   queued request from outside it, so keys never wait behind the dump.
4. **Run.** The worker walks the tree within its depth and node limits,
   sixty children a remote program.
   The outpost keeps no deadline of its own: only Core stops waiting.
   While the request is out, the shell asks Windows every half second
   whether the window is hung.
5. **One ending, whichever way it goes.** Core's request table gives the
   request exactly one ending ([verbatim-core](../reference/crates/verbatim-core.md),
   "Core's requests"):
   - the outpost's answer, taken only from the generation asked: the tree,
     not there, refused, or failed;
   - "not responding", when the shell found the window hung before
     sending;
   - "ended", when the outpost generation ends first;
   - no longer awaited, when the shell finds the window hung while the
     request is out, when its due time passes, or when its asker withdraws
     it. Core tells the outpost to cancel, with the reason: a request not
     yet started is dropped, and one running is left to finish, its late
     answer dropped by number.
6. **The caller gives up.** After 25 seconds the connection thread sends
   `ShellCommand::DumpTreeGivenUp`, and the reducer thread gives the queue
   `QueueInput::WithdrawOutside`: Core stops waiting for the dump with the
   reason "withdrawn", so a dump still queued in the outpost is dropped
   and one already running finishes.
7. **Delivered.** The ending reaches its asker through the queue's
   `Answered` effect: a tree to the control client, the shell's items to
   the list dialog.

Generations make this safe: every spawn gets a fresh generation, Core's
reader stamps it on every node id from the pipe, never from the message
body, and Core's request table takes an answer only from the generation
it asked, so nothing an old outpost said can satisfy a request sent to its
successor. The tests are in `crates/verbatim-core/src/event_queue/tests.rs`.

Known limitation: a long tree dump holds that application's worker, so
Core's requests to the application wait behind a dump already running.

## 5. Recovery

Each failure, what notices it, and what happens next
([verbatim-supervisor](../reference/crates/verbatim-supervisor.md)).

1. **A call into an application hangs.** Core's shell asks Windows
   whether the request's window is hung before sending a request and every
   half second while it is out. Found hung, Core stops waiting, plays the
   "not responding" sound if the application holds attention, and tells
   the reader to cancel, leaving its thread inside the call, since a fresh
   thread would stick in the same window; the window joins Core's recovery
   set, and when it answers again Core asks the application for its focus.
   A window slow but not reported hung waits out the request's due time.
   When the foreground leaves the request's window, Core stops waiting at
   once, and a reader still inside the call abandons that thread and
   starts a fresh one. The listener drops events from a window the system
   reports hung as they arrive.
2. **An outpost is stuck.** The lifecycle owner pings every child every
   three seconds; the outpost's I/O thread answers pings itself, so a busy
   worker never looks dead, and each pong reports the workers abandoned to
   calls on windows it did not find hung. Core's generation record ends
   the outpost when nine seconds pass without a pong, or when a pong
   reports eight such workers; the owner thread carries out the end by
   closing its job handle. There is no shutdown message, because a process
   that has used UIA can hang or crash in UIA's own exit code
   (`books/dev/src/reference/uia-bugs.md`, "A process that used UIA can
   hang or crash as it exits").
3. **An outpost crashes.** Its pipe closes; Core's reader forwards
   everything it wrote, then the owner thread reports the generation's end
   to Core.
4. **After either.** Core drops every later frame of that generation, and
   every request out to it or waiting for it ends "ended". Core's record
   starts the application's next outpost, usually a spare, at once for its
   first failure and after a backoff of 1, 2, 4, up to 30 seconds for each
   further one, never giving up; an application that has exited is left
   alone. When the committed focus's outpost ended, Core asks the
   replacement for the current focus and compares the answer with the
   committed focus's copy (`reads_the_same`): if they match, it takes the
   new node ids silently; otherwise it announces the focus.
5. **The listener is replaced.** If its pipe closes or it misses nine
   seconds of pongs, the owner thread ends it and starts another, and Core
   hears that the old one ended; every request out to it ends "ended",
   and Core asks the outpost of the application holding attention for its
   focus once, taken silently if unchanged. Every
   `WinEvent` and desktop-wide UIA event from the gap is lost.
6. **The synthesizer host dies or hangs.** `HostedSynth` ends a host whose
   pipe closed, that sent nothing for ten seconds while Core waited, or
   that answered out of turn. The utterance in flight fails, except that
   one whose host's pipe ended before any of it was relayed is sent once
   more to a fresh host. The next request starts a new host and restores
   every setting the old one had. The `crash_recovery` scenario
   kills the host and expects the next announcement in full.
7. **A synthesizer will not start.** At startup the speech manager tries
   every other registered synthesizer in order, eSpeak NG first, without
   saving the fallback as the user's choice; a failed switch leaves the
   previous synthesizer active.
8. **The audio device fails or changes.** The mixer reopens it
   (walkthrough 3); utterances already converted for an old format end as
   failed.
9. **Verbatim is started outside an interactive session.** `main` calls
    `check_interactive_session` (`verbatim_process::session::current`)
    before replacing a running instance, so a launch from WinRM,
    PowerShell Direct, or a service exits with a diagnosis and never stops
    a working Verbatim. The agent makes the same check before binding its
    port.
10. **Verbatim itself dies.** The kernel closes Core's job handles and
    kills every outpost, the listener, and every synthesizer host. A panic
    on any thread writes a flight-recorder dump first
    (`flight_dump::install_panic_hook`).

Not verified live: the refusal outside an interactive session and device
recovery. The UIA registry has no counterpart to MSAA's per-window
forgetting, so a reused window handle could inherit a held node's
identity.

## 6. The life of an end-to-end run

`cargo test -p verbatim-e2e -- --ignored --skip demo_ --skip measure_ --test-threads=1`
on this machine, or the same suite on GitHub's runner or against an agent
on another machine ([verbatim-e2e](../reference/crates/verbatim-e2e.md),
[verbatim-agent](../reference/crates/verbatim-agent.md), [the end-to-end
guide](../how-to/e2e.md)).

1. **Preparing this machine.** The agent runs in the signed-in user's
   session (`target\debug\verbatim-agent.exe --bind-address 127.0.0.1
   --port 44001`) and refuses a non-interactive one. The desktop must be
   unlocked (`cargo xtask park` on the development VM), and NVDA in the
   same session closed, since the run injects real keys.
   `VERBATIM_E2E_ENDPOINT=127.0.0.1:44001` points the suite at the agent.
   Every live test is `#[ignore]`d, so `cargo xtask ci` lists them as
   ignored.
2. **One scenario at a time.** Each `#[test]` wrapper calls
   `registry::run_named`; `--test-threads=1` is required, since a second
   `verbatim.exe` would replace the first.
3. **Launch.** `Scenario::launch` builds and stages Verbatim's binaries
   and `mockapp` once per test binary in `target/e2e-stage`, writes a fixed
   `settings.toml` (`Settings::for_e2e`, eSpeak NG), and runs silently by
   default (`VERBATIM_TEST_AUDIO=null`, so every utterance still takes its
   real duration; `VERBATIM_E2E_AUDIBLE=1` for the real device). It sweeps
   what an earlier run left, minimizes every window, starts the recording,
   and launches `verbatim.exe` through the agent with debug logging for the
   outposts. Verbatim sets a named event once it is ready for input, and
   the harness opens the command tunnel and a separate speech tunnel.
4. **Setup and body.** `registry::run` first asserts the startup speech
   and the desktop's announcement exactly. The scenario opens its
   applications through the agent and drives Verbatim with keys the agent
   injects through `SendInput`, each numbered in its `dwExtraInfo`; a
   gesture is pressed as its keys, the Verbatim modifier as Insert.
5. **Asserting speech.** `SpeechCollector` reads the speech tunnel. Each
   assertion says the next utterances are exactly these, in order, each
   ending as expected, nothing in between. The body ends with
   `expect_nothing_more`: an `AwaitIdle` request answered once Core has
   handled the last numbered key and is idle. Failures print the expected
   and actual sequences and the timeline of keys and utterances.
6. **Teardown and artifacts.** The run saves the latency timelines, Core's
   focus, and the flight recorder, checks that none of Verbatim's
   processes exited unexpectedly, quits Verbatim, and closes everything the
   scenario opened, failing on anything that will not close
   ([troubleshooting](../how-to/troubleshooting.md)). The run's directory
   under `target/e2e-artifacts/<scenario>/` receives the logs, the audio,
   any crash dump, and `summary.txt` (`ScenarioSummary`), and is copied
   into `history/`.
7. **Recording.** The agent's desktop capture (`verbatim-agent
   capture-desktop`) records the video and Verbatim's mixer tap the audio;
   `Scenario::finish_recording` muxes them into `<scenario>.mp4`.
   `VERBATIM_E2E_RECORD=0` turns recording off.
8. **On CI.** The `e2e` job in `.github/workflows/ci.yml` starts the agent
   in the same step as the suite, runs every scenario but the
   demonstrations, the measure set and the local-only ones, split into groups
   that run at once, and uploads one artifact per scenario, its
   directory with the video, for passing and failing runs alike.
9. **Against another machine.** `cargo xtask e2e --endpoint
   --verbatim-dir` starts no agent and stages nothing: it runs each
   scenario as its own `cargo test` subprocess with
   `VERBATIM_E2E_REMOTE=1`, against an agent and a copy of the stage the
   developer put on that machine (architecture section 14).

The scenarios are listed in [verbatim-e2e](../reference/crates/verbatim-e2e.md).

Not yet, or known limitations: a Verbatim whose launch fails before its
control plane answers leaves no video; `switch_to_onecore` assumes OneCore
voices are installed; and on a locked desktop injected input goes nowhere,
which is why parking matters.
