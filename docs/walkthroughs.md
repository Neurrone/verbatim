# Walkthroughs: six end-to-end narratives

[The crate guides](crates/readme.md) describe each crate on its own; these
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
(architecture section 1; [verbatim-process](crates/verbatim-process.md)):

1. **Core**, `verbatim.exe` ([verbatim-app](crates/verbatim-app.md)). The
   threads that matter here:
   - `verbatim-input-hook`, the low-level keyboard hook;
   - `verbatim-router`, the gesture router;
   - `verbatim-reducer`, which owns the reducer state, the request table,
     and the set of live outposts (`ReducerThread` in
     `crates/verbatim-app/src/main.rs`);
   - `verbatim-relay`, the supervisor's lifecycle owner, which reads the
     focus listener's pipe and every outpost's and finishes every child's
     writes on one completion port, plus short-lived launch helpers
     (`crates/verbatim-outpost/src/supervisor/`);
   - `verbatim-speech-queue` and `verbatim-synth`, the speech manager;
   - `verbatim-audio`, the mixer's one audio thread;
   - one control-plane reader and writer per connected client;
   - the GUI thread, which is the main thread.
2. **The focus listener**, `verbatim-outpost.exe --listener`: one per
   Verbatim, desktop-wide (decision D13). Its callbacks make no
   cross-process call; it holds the one desktop-wide UIA focus registration
   and the global MSAA hooks, and judges and reads each UIA focus on a
   reading thread per application.
3. **One outpost per application**, `verbatim-outpost.exe --target-pid
   <pid>` (decision D9). Inside: an event thread (`verbatim-event`) for its
   own MSAA hooks, UIA callback threads, an I/O thread on the main thread,
   which reads Core's commands and finishes writes to Core that pended,
   one worker (`verbatim-worker`) that is the only thread calling into
   the application, and a watchdog (`verbatim-watchdog`). Each thread
   writes its own messages to Core.
4. **One synthesizer host per synthesizer in use**,
   `verbatim-synth-host.exe` (decision D18).

For the end-to-end suite there is also `verbatim-agent.exe`, which runs in
the signed-in user's session and is not a child of Verbatim.

## 1. The life of a focus change: Alt+Tab to Notepad

The user is on the desktop, holds Alt, presses Tab to reach Notepad, and
releases Alt. Notepad has no outpost yet.

1. **Key presses cut off current speech.** In Core, the hook thread runs
   every key-down through `DecisionMachine::on_key`
   ([verbatim-input](crates/verbatim-input.md)). Alt and Tab are not bound,
   so they pass to Windows, but each key-down's decision carries
   `KeySpeechEffect::Cancel`, and the hook calls the speech manager's
   `SpeechControl::cancel_through` (a non-blocking channel send) before
   anything else. See walkthrough 3.
2. **Windows raises events.** When Alt is released, Windows raises
   `EVENT_SYSTEM_SWITCHEND`, `EVENT_SYSTEM_FOREGROUND` for Notepad's
   window once it is the foreground window, a UIA focus-changed event for
   Notepad's text area, and an MSAA focus event for it
   (`docs/parity/focus-events.md`, "Stale focus events").
3. **The listener captures facts.** In
   `crates/verbatim-outpost/src/listener.rs`
   ([verbatim-outpost](crates/verbatim-outpost.md), "How focus reaches an
   outpost"):
   - The MSAA events arrive on its event thread. Since Notepad has no
     outpost whose own hook is in, `forward_msaa_event` reads the owning
     pid with `GetWindowThreadProcessId`, a local call, and forwards the
     raw window, object id, and child id as a `DeliveredFact::Foreground`
     or `DeliveredFact::MsaaFocus`. Once Notepad's outpost has its own
     hook in, the listener forwards none of Notepad's MSAA events, and the
     outpost takes them from its own hook in the order Windows raised them
     (the hand-over).
   - The UIA focus callback (`focus_subscription`) receives the element
     with its properties already cached and makes no call. It sends
     Notepad a marker (`DeliveredFact::UiaFocusMarker`) and queues the
     element for Notepad's reading thread (`focus_read::Readers`).
   - That thread judges and reads the focus from the event's own element
     (`focus_read::FocusReader::read`), as NVDA's focus handler does: a
     repeat of the focus Core holds, a window that fails NVDA's acceptance
     test against the foreground window (`outpost::acceptance`), or an
     element without the keyboard focus is dropped; otherwise the classic
     walk reads its value, states, details, selected child and raw-view
     ancestors, each with its own window handle, within a two-second
     budget. The result follows the marker as a
     `DeliveredFact::UiaFocusRead`, or a `DeliveredFact::UiaFocusDropped`
     saying what the drop tells the outpost. A UIA element cannot be handed
     to another process (`docs/uia-bugs.md`, "Elements cannot be passed to
     another process"), which is why the listener reads it.
   - The switcher's end is noted and a threadpool timer set; 50 ms later
     its callback sends `MenuOrSwitchEnded` with the time it ended
     (step 12), behind any fact still waiting.
   - Each fact gets a fresh trace id and an observation timestamp, and the
     thread that captured it writes it at once up the listener's pipe to
     Core as a `FocusFact` frame (`Outgoing::fact`). Only while an earlier
     write is in flight does it wait, coalesced, and the listener's I/O
     thread writes what waits once that write completes; markers, reads
     and drops are never coalesced.
4. **Core routes the facts and starts an outpost.** The relay reads the
   listener's pipe on its completion port (`Owner::read_pipe` and
   `Owner::listener_message` in
   `crates/verbatim-outpost/src/supervisor/owner.rs`) and handles each fact
   in the pass that read it; it alone decides lifecycles. `Owner::route_fact`
   finds no outpost for Notepad's pid. Focus, foreground, menu,
   notification, and alert facts may start one
   (`DeliveredFact::may_start_outpost`), so it gives Notepad a spare
   outpost, or launches one, with a fresh outpost id (the incarnation,
   never reused), and holds the fact. The app hears `OutpostMessage::Started`
   before any of the incarnation's messages. Facts arriving meanwhile are
   held in arrival order (`HeldFacts`); on the outpost's `Ready`,
   `Owner::on_ready` releases them onto its writer as
   `SupervisorToOutpost::DeliverFact`.
5. **The outpost's intake queues the facts.** The reader
   (`Outpost::handle_command` in `crates/verbatim-outpost/src/outpost/mod.rs`)
   only pushes each fact onto the intake queue (`Intake::push` in
   `outpost/intake.rs`). The queue applies NVDA's limiter rules to MSAA
   events, drops events from a window the system reports hung, and holds
   everything behind a UIA focus marker until its read or drop takes the
   marker's place. When planning a batch (`plan`), each consecutive run of
   MSAA focus events and foreground changes is tried newest first until
   one is reported, each UIA focus is handled at its own place, and a menu
   opening is moved last ([verbatim-outpost](crates/verbatim-outpost.md),
   "Event flow").
6. **The worker reports the foreground change.** The worker thread takes
   the batch (`run` in `outpost/worker.rs`) at once. `Worker::foreground`
   drops a foreground change whose window is not the foreground window, as
   NVDA does, then reads the window's own accessible object
   (`read::foreground_window`), and publishes it as a `FocusChanged` with
   `foreground: true`, stamped with the later of its observation time and
   the time it was confirmed in front. Every call into Notepad runs under
   the watchdog's deadline (walkthrough 4).
7. **The worker reports the listener's read.** `Worker::uia_focus` reports
   the read at once, its node minted from its runtime id; then, off the
   way to speech, it finds its own copy of the element by the path the
   listener read (`outpost::focus_path`), which it needs for navigation,
   property changes, and the caret. Arbitration decides which backend owns
   the window (`Arbitrator`: NVDA's class lists, then the raw probe, one
   `WM_GETOBJECT`), so the MSAA focus for the same control is dropped in
   `Worker::msaa_focus`. A WinEvent focus in a UIA window that no UIA focus
   follows within a second is read by the outpost itself
   (`outpost/gap.rs`).
8. **The worker publishes, and keeps what it reported.** `emit_focus`
   publishes an `OutpostToSupervisor::Event` carrying the trace id, the
   observation time, the backend, the window's `WindowFacts` (top-level
   window, root owner, topmost, and `in_foreground`, NVDA's test of whether
   the window is in the system's foreground window now, read locally by
   `window_facts`), and the event. The worker moves its focus-following
   UIA subscription to the new focus. The element behind every node it
   reported stays in the outpost's registry until Core says it no longer
   holds it ([verbatim-outpost](crates/verbatim-outpost.md), "Window
   destruction and held objects").
9. **The pipe.** Publishing happens under the watchdog's lock
   (`Watch::publish`), so an abandoned worker can never publish. The
   worker writes to Core's pipe itself (`outpost/outbound.rs`) when no
   write is in flight; otherwise the message waits, merged by object and
   kind. In Core the relay posts each frame whole to the reducer thread
   (`OutpostMessage::Frame`), whose `Frames` parses it, stamps every node
   id with the incarnation whose pipe it arrived on, and hands each on as
   `OutpostMessage::Event`.
10. **The reducer thread gates and records.** In
    `ReducerThread::on_outpost_message`, a message from an incarnation not
    in the live set (`LiveOutposts` in `crates/verbatim-app/src/live.rs`) is
    dropped. An accepted event goes to the latency ledger, to control-plane
    subscribers, and to `apply`, which runs the pure `reduce`, records the
    input in the flight recorder, executes the effects, and sends the
    derived views (`send_views`).
11. **The reducer decides (D14).** In `crates/verbatim-core/src/reduce.rs`
    ([verbatim-core](crates/verbatim-core.md)):
    - **Stale check** (`is_stale_focus`): across outposts, a focus observed
      before the newest focus already applied from another outpost is
      dropped, unless it is in the same top-level window.
    - **Classification** (`classify`): a foreground change is attended;
      another focus is attended if its window facts say it was in the
      system's foreground window, is topmost, or is a `Windows.UI.Core`
      window under the active window (`ForegroundRelation::related`).
    - **Attention** (`reduce_focus_changed`): the foreground change moves
      attention to Notepad's process and window; a nameless foreground
      window becomes the focus silently.
    - **Speech**: every focus change first emits `Effect::DropExpiredSpeech`;
      a focus that brings a new top-level window, or enters a menu, also
      emits `Effect::StopSpeech`. Then come the `Speak` effects, all
      `Queued`: each newly entered presentable container, then the focus's
      name, role, value, and states in NVDA's order. For Notepad that is
      the window's title, then, as a separate focus change, the text area.
      Each focus utterance carries a `FocusValidity`. A focus identical to
      the one applied is not spoken again, and the navigator and review
      cursor snap to the new focus.
12. **The switcher's end.** About 50 ms after the switcher closed, Core
    receives `MenuOrSwitchEnded`. `ReducerThread::fake_focus` does nothing
    if the reducer has applied a focus observed since
    (`SrState::latest_focus_observed_at`), the usual case; otherwise it
    asks the foreground application's outpost for its focus
    (`Asker::FakeFocus`), NVDA's fake focus.

The ordering rules exist because Windows' events race: a foreground
change is stamped with the time its window was confirmed in front, and a
focus is judged against the real foreground through `in_foreground`, as
NVDA judges it, so a desktop focus raised just before the switch cannot
outrank Notepad's.

Not yet, or known limitations:

- A window nameless when focus enters it is not announced later, and a
  window that takes the foreground well after Windows announces it is not
  announced either; NVDA behaves the same.
- The per-source cap on background events, tooltip and notification-bar
  windows are not implemented, and background progress bars are not yet
  reported (M4.3) ([parity ledger](parity/focus-events.md), "Event
  acceptance").
- The latency stages and their figures are in [performance](performance.md).

## 2. The life of a command keystroke: Verbatim+numpad6

The user presses Verbatim+numpad6 (move the navigator to the next object)
while focus is in Notepad's text area.

1. **The hook decides, in microseconds.** On Core's hook thread
   ([verbatim-input-windows](crates/verbatim-input-windows.md)), the
   Verbatim key's key-down is swallowed and cancels speech; numpad6's
   key-down completes the bound chord `kb:verbatim+numpad6`, so it is
   swallowed too (its key-up will be as well), its `Cancel` speech effect
   is carried out first, and an `EmittedGesture` with a fresh trace id
   and a repeat count is sent on the router's unbounded channel. The hook
   never blocks, since Windows silently removes a low-level hook that is
   too slow, and never drops a key.
2. **The router turns it into a reducer command.** The router thread
   (`router_loop` in `crates/verbatim-app/src/main.rs`) looks the gesture
   up in the layout's table (`verbatim_input::bindings_for`), finds
   `ScriptAction::MoveToNextSibling`, maps it to
   `ReviewCommand::NextSibling` (`ScriptAction::review_command`), and sends
   it to the reducer thread as `ShellCommand::Input`. The router speaks the
   time, the date, and a lock key's new state itself; Verbatim+V and the
   tray and taskbar lists go to the GUI as `GuiCommand`s.
3. **The reducer asks for the neighbor.** `reduce_command` calls
   `navigate`, which allocates a query id, records it as the latest
   navigation (`SrState::latest_navigation`), and returns `Effect::Fetch`.
   Nothing is spoken yet.
4. **The reducer thread sends a query.** `ReducerThread::execute` records
   it in the request table (`RequestTable::begin` in
   `crates/verbatim-app/src/requests.rs`) with a fresh request id, the
   outpost incarnation the node id names, and `Asker::Reducer`, and queues
   `SupervisorToOutpost::Query` with `Query::Navigate`
   (`Supervisor::send_to_outpost`). If the incarnation has ended or its
   queue is full, the request fails at once.
5. **The outpost answers in order.** The outpost strips Core's stamp from
   the node id (`unstamped`) and queues the query behind any events, so
   the answer never overtakes an event the user caused earlier. The worker
   runs `read::navigate` under a 400 ms deadline: one raw-view tree walker
   step for a UIA node, `accNavigate` for an MSAA node, with a neighbor in
   a window the other backend owns re-read through it (`corrected_backend`,
   NVDA's `correctAPIForRelation`). It publishes one `Reply`: the
   neighbor, no neighbor (a real edge), gone, or failed.
6. **The outcome re-enters the reducer.** `RequestTable::finish` accepts
   it only from the incarnation it was sent to and only once, as
   `Input::FetchCompleted` with a `FetchResult`.
7. **The reducer acts on it** (`reduce_navigate_completed`), only if its
   query id is still the latest navigation. A node moves the navigator and
   is announced; no neighbor speaks NVDA's edge message ("No next" and the
   like, `edge_message_of`); gone re-seeds the navigator from the focus;
   unanswered says nothing.
8. **Speech**, queued, as in walkthrough 3.

Other commands follow the same path. Verbatim+numpad5 (report the current
object) needs no outpost: the reducer speaks the navigator's copy, spells
it on the second press, and on the third emits `Effect::CopyToClipboard`.
Activation emits `Effect::Activate`, which becomes a `Query::Activate`
under the same 400 ms deadline. A gesture injected through the control
plane (`send_gesture`) cancels speech itself, since its keys never pass
the hook, and is always a first press.

Not yet, or known limitations: an object with no text interface is
reviewed by its flat name or value, whose start and end of line speak the
line rather than the character there ([review parity](parity/review.md));
layout changes take effect only after a restart.

## 3. The life of an utterance

Following the focus announcement for Notepad's text area from the
reducer's `Speak` effect to the speaker (decisions D12, D17, D18;
[verbatim-speech](crates/verbatim-speech.md),
[verbatim-audio](crates/verbatim-audio.md),
[verbatim-synth-hosted](crates/verbatim-synth-hosted.md),
[verbatim-synth-host](crates/verbatim-synth-host.md)).

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
   cancelled. Otherwise it is registered with the mixer
   (`Source::register`), which from then on owns its ending, and sent to
   the synth thread with its own cancellation flag.
4. **Synthesized in another process.** On the synth thread, `run_job`
   calls the active driver's blocking `speak`. The driver is a
   `HostedSynth`: it sends `ToHost::Speak` over a private pipe to
   `verbatim-synth-host.exe`, which runs eSpeak NG or OneCore and writes
   audio as `Pcm` frames and marks as `Mark` frames, then `Done`. The synth
   thread reads the pipe itself and passes each frame to the pipeline
   sink, which trims silence and writes to the mixer (`Source::write`).
5. **Backpressure.** `Source::write` blocks while the source is more than
   the device buffer plus 40 ms ahead of playback, so the synth thread
   stops reading, the pipe fills, and the host's synthesizer blocks. Little
   audio is ever queued, so cancelling is cheap.
6. **Played.** The mixer's audio thread (`crates/verbatim-audio/src/mixer.rs`)
   sums the sources (`mix_sources`) into the `AudioDevice`: WASAPI shared
   mode on the default device
   ([verbatim-audio-wasapi](crates/verbatim-audio-wasapi.md)), or the
   silent real-time device under `VERBATIM_TEST_AUDIO=null`. When an
   utterance's first frame has played it reports `PlaybackEvent::Started`,
   and when every frame has played, `Ended` with `Completed`; the ledger
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
  cancelled too): waiting utterances end cancelled, the in-flight job's
  flag is set, and `Source::cancel_all` discards unplayed audio at once.
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
[verbatim-control](crates/verbatim-control.md)). This shows the rules
every query follows.

1. **Asked.** On the control server's connection thread,
   `request_dump_tree` mints a `DumpTicket`, sends `ShellCommand::DumpTree`
   to the reducer thread, and waits up to five seconds.
2. **Recorded.** `ReducerThread::dump_tree` finds the attention
   application's newest live incarnation, records the request with
   `Asker::DumpTree`, and queues `Query::DumpTree`. Request ids are never
   reused, so an old reply can never satisfy a newer request.
3. **Queued in the outpost.** The query waits in the intake queue, exempt
   from the batch limits; its deadline starts only when the worker starts
   it.
4. **Run under a deadline.** The worker calls `Watch::start` with the
   entry's budget (`budget` in `outpost/worker.rs`): five seconds for a
   tree dump or ancestor walk, two for a text read, 400 ms for a
   navigation step or activation, ten seconds for an event or a focus-now
   query. The watchdog abandons the worker when the deadline passes, or,
   after half a second, when the user has moved to a window of the same
   application on another UI thread (`abandon_reason`).
5. **One reply, whichever way it goes.** Exactly one of these happens:
   - The worker finishes and publishes `Done`, `Gone`, or `Failed` under
     the watch lock.
   - The watchdog abandons it first, under the same lock: it bumps the
     worker generation, sends `Abandoned`, and starts a replacement worker
     for the rest of the queue. The abandoned thread, if its call ever
     returns, publishes nothing and exits (`Watch::finish`).
   - The handling panics, and `run_entry` sends `Failed`.
   - Core withdraws it while still queued (`Intake::cancel`), and the
     outpost sends `NotStarted`.

   "Not started" means it never ran; "abandoned" means it started and
   passed its deadline, so an effect such as an activation may already
   have happened.
6. **The caller gives up.** After five seconds the connection thread
   sends `ShellCommand::DumpTreeGivenUp`, and the reducer thread sends the
   outpost `SupervisorToOutpost::Cancel`. A dump still queued answers "not
   started"; one already running finishes within its own budget.
7. **Delivered.** `RequestTable::finish` drops an outcome for an id that
   already has one, or from another incarnation. The first outcome goes to
   its asker: a tree to the control client; `Input::FetchCompleted` for
   navigation; `Input::ActivationCompleted` for an activation, except that
   an abandoned activation is only logged; for a focus-now query, a
   foreground change and a focus.
8. **The outpost ends.** On `OutpostMessage::Ended` the reducer thread
   removes the incarnation from the live set, applies
   `Input::OutpostEnded`, and `RequestTable::outpost_ended` answers every
   outstanding request for it "gone".

Incarnations make this safe: every spawn gets a fresh `OutpostId`, Core's
reader stamps it on every node id from the pipe, never from the message
body, and the request table and the live set both check it, so nothing an
old outpost said can reach the reducer or satisfy a query sent to its
successor. The tests are in `crates/verbatim-app/src/requests.rs`,
`live.rs`, `crates/verbatim-outpost/src/outpost/worker.rs`, and
`crates/verbatim-core/tests/reduce.rs`.

Known limitation: the app's wiring of these pieces
(`ReducerThread::on_outpost_message`) has no test of its own, and a long
tree dump delays that application's events, since queries within one
application run one at a time by design.

## 5. Recovery

Each failure, what notices it, and what happens next
([verbatim-outpost](crates/verbatim-outpost.md), "The supervisor").

1. **A call into an application hangs.** The outpost's watchdog abandons
   the worker and starts a replacement (walkthrough 4); an abandoned
   thread keeps its stack until its call returns or the process ends.
   Events from a window the system reports hung are dropped before any
   read.
2. **An outpost is stuck.** The lifecycle owner pings every child every
   three seconds; the outpost's I/O thread answers pings itself, so a busy
   worker never looks dead. Nine seconds without a pong, or eight
   abandoned workers (unless the application's own windows are reported
   hung), ends the outpost as killed (`Owner::heartbeat`, the pure
   `wedge_decision`) by closing its job handle; there is no shutdown
   message, because a process that has used UIA can hang or crash in UIA's
   own exit code (`docs/uia-bugs.md`, "A process that used UIA can hang or
   crash as it exits").
3. **An outpost crashes.** Its pipe closes; Core's reader forwards
   everything it wrote, then the owner sends `Ended` as exited.
4. **After either.** The reducer thread drops later messages from that
   incarnation, the reducer marks the focus dead but keeps its copied
   data, and outstanding queries end "gone". The owner gives the
   application the next spare outpost (`Owner::outpost_failed`), at once
   for its first failure and after a backoff of 1, 2, 4, up to 30 seconds
   for each further one, never giving up; an application that has exited
   is left alone. The reducer thread asks the replacement for the current
   focus when it is ready (`focus_now_wanted`) and compares the answer with
   the dead focus's copy (`reads_the_same`): if they match, it takes the
   new node ids silently; otherwise it announces the focus.
5. **An idle outpost is retired.** Every 30 seconds, an outpost whose
   application has not held attention for two minutes, which is not
   Core's own, and in which the reducer holds no nodes, is ended as
   retired (`retirement_decision`).
6. **The listener is replaced.** If its pipe closes or it misses nine
   seconds of pongs, the owner ends it and starts another, telling every
   outpost that the old listener's reads have ended
   (`DeliveredFact::UiaFocusReadsEnded`). When the replacement is ready the
   reducer thread hears `OutpostMessage::ListenerReady`, reads the
   foreground window itself, and asks that application for its focus,
   taken silently if unchanged. Desktop-wide UIA events from the gap are
   lost.
7. **The synthesizer host dies or hangs.** `HostedSynth` ends a host whose
   pipe closed, that sent nothing for ten seconds while Core waited, or
   that answered out of turn. The utterance in flight fails, except that
   one whose host's pipe ended before any of it was relayed is sent once
   more to a fresh host. The next request starts a new host and restores
   every setting the old one had. The `synth_host_crash_recovery` scenario
   kills the host and expects the next announcement in full.
8. **A synthesizer will not start.** At startup the speech manager tries
   every other registered synthesizer in order, eSpeak NG first, without
   saving the fallback as the user's choice; a failed switch leaves the
   previous synthesizer active.
9. **The audio device fails or changes.** The mixer reopens it
   (walkthrough 3); utterances already converted for an old format end as
   failed.
10. **Verbatim is started outside an interactive session.** `main` calls
    `check_interactive_session` (`verbatim_process::session::current`)
    before replacing a running instance, so a launch from WinRM,
    PowerShell Direct, or a service exits with a diagnosis and never stops
    a working Verbatim. The agent makes the same check before binding its
    port.
11. **Verbatim itself dies.** The kernel closes Core's job handles and
    kills every outpost, the listener, and every synthesizer host. A panic
    on any thread writes a flight-recorder dump first
    (`flight_dump::install_panic_hook`).

Not verified live: the refusal outside an interactive session and device
recovery. The UIA registry has no counterpart to MSAA's per-window
forgetting, so a reused window handle could inherit a held node's
identity.

## 6. The life of an end-to-end run

`cargo test -p verbatim-e2e -- --ignored --skip demo_ --skip measure_ --test-threads=1`
on this machine, or the same suite on GitHub's runner or against a Hyper-V
guest ([verbatim-e2e](crates/verbatim-e2e.md),
[verbatim-agent](crates/verbatim-agent.md), [the end-to-end
guide](tooling/e2e.md)).

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
   injects through `SendInput`, each numbered in its `dwExtraInfo`, and
   with `SendGesture`.
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
   ([troubleshooting](tooling/troubleshooting.md)). The run's directory
   under `target/e2e-artifacts/<scenario>/` receives the logs, the audio,
   any crash dump, and `summary.txt` (`ScenarioSummary`), and is copied
   into `history/`.
7. **Recording.** The agent's desktop capture (`verbatim-agent
   capture-desktop`) records the video and Verbatim's mixer tap the audio;
   `Scenario::finish_recording` muxes them into `<scenario>.mp4`.
   `VERBATIM_E2E_RECORD=0` turns recording off.
8. **On CI.** The `e2e` job in `.github/workflows/ci.yml` starts the agent
   in the same step as the suite, runs every scenario but the
   demonstrations, the measure set and the local-only ones, and uploads the
   artifacts for passing and failing runs alike.
9. **Against a Hyper-V guest.** `cargo xtask vm test` builds, copies
   changed binaries into the guest, restores the golden checkpoint only
   with `--restore`, and runs each scenario as its own `cargo test`
   subprocess with `VERBATIM_E2E_REMOTE=1` ([the VM harness guide](tooling/vm.md)).

The scenarios are listed in [verbatim-e2e](crates/verbatim-e2e.md).

Not yet, or known limitations: a Verbatim whose launch fails before its
control plane answers leaves no video; `switch_to_onecore` assumes OneCore
voices are installed; and on a locked desktop injected input goes nowhere,
which is why parking matters.
