# Walkthroughs: six end-to-end narratives

[The crate guides](crates/readme.md) describe each crate on its own; these
walkthroughs follow one realistic case at a time across the crates, naming
the process and thread at each step and what crosses each boundary. They
are written for someone who needs to explain who owns what, in what order
things happen, how failures are recovered, and when speech is cut off.
Each narrative ends with what is not implemented yet or known to be
limited, kept separate from what the code does today.

Kept current as of 2026-10-05 (phase 5 of the 2026-09-02 handoff, commit
a842bf9). Code is cited by file path and function or type name.

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
   - the supervisor's lifecycle owner thread, plus two threads per child
     process (a writer and a reader) and short-lived launch helpers
     (`crates/verbatim-outpost/src/supervisor/`);
   - `verbatim-speech-queue` and `verbatim-synth`, the speech manager;
   - `verbatim-audio`, the mixer's one audio thread;
   - one control-plane reader and writer per connected client;
   - the GUI thread, which is the main thread.
2. **The focus listener**, `verbatim-outpost.exe --listener`: one per
   Verbatim, desktop-wide, never calling into another process (decision
   D13).
3. **One outpost per application**, `verbatim-outpost.exe --target-pid
   <pid>` (decision D9). Inside: an event thread (`verbatim-event`) for its
   own MSAA hooks, UIA callback threads, a reader on the main thread, one
   worker (`verbatim-worker`) that is the only thread calling into the
   application, a watchdog (`verbatim-watchdog`), and a writer
   (`verbatim-outbound`).
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
   `SpeechControl::cancel_through` (a non-blocking channel send) before anything
   else. Whatever was being spoken stops. See walkthrough 3.
2. **Windows raises events.** When Alt is released, Windows raises
   `EVENT_SYSTEM_SWITCHEND`, `EVENT_SYSTEM_FOREGROUND` for Notepad's
   window, a UIA focus-changed event for Notepad's text area, and an MSAA
   focus event for it. The foreground event is raised before the change
   completes: measured live, Notepad's event arrived while the desktop was
   still the foreground window, which it stayed for about 130 ms more.
3. **The listener captures facts, without calling anyone.** The listener
   process is the only receiver of focus events (D13, amended 2026-10-05;
   outposts do not subscribe to their own, so no event is handled twice).
   In `crates/verbatim-outpost/src/listener.rs`:
   - The MSAA events arrive on its event thread. `forward_msaa_event`
     reads the owning pid with `GetWindowThreadProcessId`, a local call,
     and forwards the raw window, object id, and child id untouched as a
     `DeliveredFact::Foreground` or `DeliveredFact::MsaaFocus`. A
     foreground event is not checked against the foreground here; the
     outpost's worker checks it later, because a starting application's
     window raises the event before it is actually in front.
   - The UIA focus callback (`install_focus_registration`) receives the
     element with its properties already cached. `capture` reads the
     cached pid, the cached window handle, and the cached snapshot parts
     (runtime id, role, name, value, states, details) as a
     `DeliveredFact::UiaFocus`. For an element with no window of its own,
     it also records `focus_window`: the foreground thread's keyboard focus
     window, if it belongs to the element's process (`focus_window_of` in
     `crates/verbatim-outpost/src/outpost/window.rs`, using
     `GetGUIThreadInfo`, a local read). NVDA would walk up to the
     element's nearest window, which is a cross-process call the listener
     may not make.
   - The switcher's end becomes a note in the outgoing queue, sent 50 ms
     later as `MenuOrSwitchEnded` with the time it ended (step 12).
   - Each fact gets a fresh trace id and an observation timestamp, and is
     coalesced in the listener's outgoing queue (`Outgoing::fact`): a
     newer fact for the same element and kind replaces a waiting one.
     Pongs and `Ready` go first. The writer thread writes each as a
     `FocusFact` (newline-delimited JSON) up the listener's pipe to Core.
4. **Core routes the facts and starts an outpost.** Core's listener
   reader thread (`read_listener` in
   `crates/verbatim-outpost/src/supervisor/owner.rs`) hands each fact to
   the lifecycle owner thread, which alone decides lifecycles.
   `Owner::route_fact` finds no record for Notepad's pid. Focus,
   foreground, menu, notification, and alert facts may start an outpost
   (`DeliveredFact::may_start_outpost`), so it records Notepad as
   starting with a fresh outpost id (the incarnation, never reused),
   holds the fact, and hands the launch to a helper thread. The helper
   launches the process already inside a kill-on-close job (the job is
   attached at creation, `PROC_THREAD_ATTRIBUTE_JOB_LIST`, so the child is
   contained from its first instruction), starts its writer thread, sends the app `OutpostMessage::Started`, and
   only then starts its reader, so the reducer thread hears of an
   incarnation before any of its messages. Facts arriving meanwhile are
   held in arrival order, one per object and kind (`HeldFacts`). On the
   outpost's `Ready`, `Owner::on_ready` releases them in that order onto
   the outpost's writer queue as `SupervisorToOutpost::DeliverFact`. Had
   Notepad already had a ready outpost, the fact would have gone straight
   to its writer.
5. **The outpost's intake queues the facts.** In Notepad's outpost, the
   reader (`Outpost::handle_command` in
   `crates/verbatim-outpost/src/outpost/mod.rs`) only pushes each fact onto
   the intake queue (`Intake::push` in `outpost/intake.rs`) and returns.
   The queue applies NVDA's limiter rules: one waiting entry per object
   and kind; a batch is everything that accumulated while the worker
   handled the previous batch; per batch the newest 4 focus events and 10
   other events per application UI thread, with the focused object's
   events always kept; events from a window the system reports hung
   (`IsHungAppWindow`) are dropped before any read. When planning a batch
   (`plan`), only the newest foreground change is kept, focus events are
   grouped per backend (the newest three in all, tried newest first until
   one is reported), and a menu opening is moved last.
6. **The worker waits for the window to really be in front.** The worker
   thread takes the batch (`run` in `outpost/worker.rs`). Because the
   batch holds a foreground change, `Intake::next` names its window, and
   before handling anything the worker calls `wait_for_foreground`: it
   checks `GetForegroundWindow` every 10 ms for up to 250 ms until
   Notepad's window is the foreground window, and records the time it was
   confirmed. This is NVDA holding back event handling after a foreground
   event (NVDA issue 3831). Measured live on 2026-10-02 over 245 such
   events, the window arrived 5 to 100 ms after its event (median 44 ms);
   the 130 ms in step 2 is a single case from the 2026-10-05 observer run.
7. **The worker reports the foreground change.** `Worker::foreground`
   reads the window's own accessible object through its backend
   (`read::foreground_window`: a UIA element from the handle, or the MSAA
   client object; a window that cannot be read yet is reported from local
   window data; an empty name takes the window text). If the window is no
   longer the foreground window by then, the report is dropped, as NVDA
   drops it. Otherwise it is published as a `FocusChanged` with
   `foreground: true`, stamped with the later of its observation time and
   the confirmation time, not the time Windows raised it. Every call into
   Notepad runs under the watchdog's 10 s deadline (walkthrough 4).
8. **The worker reads the focused control.** The UIA focus fact comes
   next (`Worker::uia_focus`):
   - It is accepted only if its cached states say it has the keyboard
     focus, as NVDA requires.
   - The element itself lived in the listener's process and cannot cross
     to this one, so the worker reads the focused element again
     (`live_focus_element`, waiting at most one second) to get its own
     live copy. There are three outcomes (`LiveFocus`): **found**, the
     same runtime id; **in another application**, the fact is out of date
     because focus has moved on, and it is dropped, since the newer
     focus's own event reports it; or **unresolved**, the read failed,
     timed out, or answered with another element of Notepad (a starting
     application can answer with a stand-in), and the focus is reported
     from the fact alone with its ancestors unknown, while an
     `Item::ResolveFocus` follow-up (up to three tries, while the focus is
     unchanged) finds the element later for navigation and property
     changes.
   - The window it is reported with is the event's own window, else the
     live element's nearest window, else the listener's `focus_window`.
   - Arbitration decides which backend owns that window
     (`read::window_uses_uia` and `Arbitrator`; NVDA's class lists, then
     the `UiaHasServerSideProvider` probe, a positive verdict kept for the
     window's life, a negative one for 500 ms). Exactly one backend
     reports: for a UIA window the MSAA fact for the same control is
     dropped in `Worker::msaa_focus`, and the other way round.
   - With the element in hand, `read::uia_enrichment` walks the ancestors,
     stopping where it meets the previous focus's chain and reusing the
     rest, and reads a list's or tab control's selected child, all within
     a two-second budget, after which the ancestors are reported unknown.
     The walk switches to MSAA where it crosses into a window MSAA owns.
9. **The worker publishes, and keeps what it reported.** `emit_focus`
   publishes an `OutpostToSupervisor::Event` carrying the trace id, the
   observation time, the backend, the window's `WindowFacts`, and the
   event. The window facts are local reads made as the worker publishes
   (`window_facts`): top-level window, root owner, whether it or its
   top-level window is topmost, for a `Windows.UI.Core` window whether it
   is under the input thread's active window, and `in_foreground`, NVDA's
   test of whether the window is in the system's foreground window right
   now. The worker also moves the focus-following UIA property
   subscription to the new focus, records the chain for
   the next walk, and tells the intake which object is focused. The UIA
   element behind every node it reported stays in the outpost's registry
   until Core says it no longer holds it ("Held objects" in
   [verbatim-outpost](crates/verbatim-outpost.md)).
10. **The writer and the pipe.** Publishing happens under the watchdog's
    lock (`Watch::publish`), so an abandoned worker can never publish.
    The writer thread (`outpost/outbound.rs`) puts pongs and `Ready`
    ahead of ordinary messages, which wait in a queue of 256. In Core,
    the outpost's reader thread (`read_outpost`) stamps every node id
    with the incarnation whose pipe the message arrived on, numbers the
    messages that carry node ids, and forwards each to the reducer
    thread as `OutpostMessage::Event`.
11. **The reducer thread gates and records.** In
    `ReducerThread::on_outpost_message`, a message from an incarnation
    that is not in the live set (`LiveOutposts` in
    `crates/verbatim-app/src/live.rs`: joined on `Started`, left on
    `Ended`) is dropped, so nothing from an ended outpost reaches the
    reducer. An accepted event goes to the latency ledger, to control-plane
    event subscribers, and to `apply`, which runs the pure `reduce`,
    records the input in the flight recorder, executes the effects, and
    sends the derived views: the application holding attention to the
    supervisor, and to each live outpost the nodes the reducer holds in it
    (`send_views`).
12. **The reducer decides (D14 as amended on 2026-10-05).** In
    `crates/verbatim-core/src/reduce.rs`:
    - **Stale check** (`is_stale_focus`). Within one outpost, order comes
      from its queue. Across outposts, a focus observed before the newest
      focus already applied from another outpost is dropped, unless it is
      in the same top-level window as that focus (a Settings page's
      content and its frame are two processes in one window).
    - **Classification** (`classify`). A foreground change is always
      attended: its outpost has just confirmed the window is in front. Any
      other focus is attended only if its window facts say it was in the
      system's foreground window when its outpost read it, is topmost, or
      is a `Windows.UI.Core` window under the active window
      (`event_window_is_foreground`); a focus with no window facts is
      judged by its application against the attention record. Other
      events are judged against the attention record (same top-level
      window, same root owner, topmost, UWP rule, or in the foreground),
      UIA notifications by the focus's application, and toasts and the
      shell's snap results are accepted from anywhere as background.
    - **Attention** (`reduce_focus_changed`). The foreground change moves
      attention to Notepad's process and window. It is then ignored if
      the current focus is already in that top-level window. A nameless
      foreground window becomes the focus silently and is never announced
      later. A focus in the system's foreground window unrelated to the
      attention window also moves attention, since Windows sometimes
      raises no foreground event at all.
    - **Speech.** On every focus change the reducer first emits
      `Effect::DropExpiredSpeech` with the new focus, its ancestors, and
      the foreground node. A focus that brings a new top-level window, or
      enters a menu, also emits `Effect::StopSpeech`. Then come the
      `Speak` effects, all `Queued`: each newly entered presentable
      container as its own utterance, then the focus's name, role, value,
      and states in NVDA's order, with a list's selected child. For
      Notepad that is the window's title, then, as a separate focus
      change, the text area. Each focus utterance carries a
      `FocusValidity`, so the speech manager can drop it if the user has
      already moved on.
    - The navigator and review cursor snap to the new focus. A focus
      identical to the one already applied is not spoken again.
13. **Speech.** The reducer thread executes the effects on the speech
    manager without waiting (walkthrough 3).
14. **The switcher's end.** About 50 ms after the switcher closed, Core
    receives `MenuOrSwitchEnded`. `ReducerThread::fake_focus` does nothing
    if the reducer has applied a focus observed since that time
    (`SrState::latest_focus_observed_at`), the usual case. Otherwise it
    asks the foreground application's outpost for its focus
    (`Asker::FakeFocus`), NVDA's fake focus, and only the focused control
    re-enters the reducer.

Why the ordering rules exist. On 2026-10-05 the scenario
`notepad_and_verbatim_menu` failed in every full-suite run because
Notepad's title was not spoken. An independent WinEvent observer showed
that Windows raises Notepad's foreground event about 130 ms before the
window is in front, while the desktop's icon list raises its own focus
events. With the foreground change stamped at the time Windows raised it,
the desktop's focus events were newer and the reducer dropped Notepad's
foreground change as stale. Commit `b1917ee` stamps a foreground change
with the time its window was confirmed in front (step 7) and exempts a
focus in the same top-level window from the stale rule. One failure
remained: the desktop list raised a focus 4 ms after Notepad was
confirmed, it reached Core before Notepad's report, and the attention
record still named the desktop, so it was spoken. Commit `9e250d2` judges
a focus against the real foreground through `in_foreground`, as NVDA
does (D14 amended), and `a842bf9` gives a windowless UIA focus the
listener's `focus_window`, so it too is judged that way (D13 amended).
The full suite then passed 6 of 6 runs.

Not yet, or known limitations:

- For a late event in an application with several windows, the
  listener's `focus_window` can name another of its windows. Focus speech
  for it is still cancelled when the newer focus arrives (walkthrough 3),
  as in NVDA.
- A window nameless when focus enters it is not announced later, as in
  NVDA. A window that takes the foreground well after Windows announces
  it (about 600 ms, seen under load) is not announced either; NVDA
  behaves the same.
- D15's budget of 10 ms from event to speech queued is met only in
  Verbatim's own dialog; measured medians are about 13 ms in Notepad,
  26 ms in File Explorer, and 45 ms in the Start menu, dominated by the
  outpost's focus read (handoff, "Latency investigation, shelved
  2026-10-05"). The per-stage ledger D15 asks for is partly implemented:
  one `verbatim::latency` line per announcement.
- The per-source cap on background events, tooltip and notification-bar
  windows, and background progress bars are not implemented
  ([parity ledger](parity.md), "Event acceptance").

## 2. The life of a command keystroke: Verbatim+numpad6

The user presses Verbatim+numpad6 (move the navigator to the next object)
while focus is in Notepad's text area.

1. **The hook decides, in microseconds.** On Core's hook thread
   ([verbatim-input-windows](crates/verbatim-input-windows.md)), the
   Verbatim key's key-down is swallowed and cancels speech; numpad6's
   key-down completes the bound chord `kb:verbatim+numpad6`, so it is
   swallowed too (its key-up will be as well), its `Cancel` speech effect
   is carried out first, and an `EmittedGesture` with a fresh trace id
   and a repeat count (0 for a first press) is sent with `try_send` on a
   bounded channel. The hook never blocks: Windows silently removes a
   low-level hook that is too slow.
2. **The router turns it into a reducer command.** The router thread
   (`router_loop` in `crates/verbatim-app/src/main.rs`) looks the gesture
   up in the layout's table (`verbatim_input::bindings_for`), finds
   `ScriptAction::MoveToNextSibling`, maps it to
   `ReviewCommand::NextSibling` (`review_command_of`), and sends an
   `Input::Command` to the reducer thread as `ShellCommand::Input`. The
   router speaks the time, the date, and a lock key's new state itself;
   Verbatim+V and the tray and taskbar lists go to the GUI as
   `GuiCommand`s.
3. **The reducer asks for the neighbor.** `reduce_command` calls
   `navigate`, which allocates a query id, records it as the latest
   navigation (`SrState::latest_navigation`), and returns
   `Effect::Fetch` naming the navigator's node and the direction. Nothing
   is spoken yet.
4. **The reducer thread sends a query.** `ReducerThread::execute` records
   the query in the request table (`RequestTable::begin` in
   `crates/verbatim-app/src/requests.rs`) with a fresh request id, the
   outpost incarnation named by the node id, and `Asker::Reducer`, and
   queues `SupervisorToOutpost::Query` with `Query::Navigate` on that
   outpost's writer (`Supervisor::send_to_outpost`). If the incarnation
   has ended or its queue is full, the send fails at once and the request
   gets a "failed" outcome on the spot.
5. **The outpost answers in order.** Notepad's outpost reader strips
   Core's stamp from the node id (`unstamped`) and queues the query. The
   worker reaches it after any events already queued, so the answer can
   never overtake an event the user caused earlier. It runs
   `read::navigate` under a 400 ms deadline: for a UIA node, the kept
   element is refreshed and the raw-view tree walker takes one step; for
   an MSAA node, `accNavigate` runs on the object that was announced. A
   neighbor in a window owned by the other backend is re-read through
   that backend (`corrected_backend`, NVDA's `correctAPIForRelation`).
   The worker publishes one `Reply`: done with the neighbor, done with no
   neighbor (a real tree edge), gone (the node cannot be reached), or
   failed.
6. **The outcome re-enters the reducer.** The reply crosses the pipe like
   any event. `RequestTable::finish` accepts it only from the incarnation
   it was sent to and only once, and turns it into
   `Input::FetchCompleted` with a `FetchResult`: `Node`, `NoNeighbor`,
   `Gone`, or `Unanswered` for failed or abandoned.
7. **The reducer acts on it** (`reduce_navigate_completed`), but only if
   its query id is still the latest navigation. A focus event that
   arrives in between snaps the navigator to the focus but does not
   discard the pending move; a second move or "move to focus" does. A
   node moves the navigator and is announced; no neighbor speaks NVDA's
   edge message ("No next" and the like, `edge_message_of`); gone re-seeds
   the navigator from the focus and announces it; unanswered says nothing
   and leaves the navigator where it was.
8. **Speech**, queued, as in walkthrough 3. The next key press cuts it
   off.

Other commands follow the same path. Verbatim+numpad5 (report the current
object) needs no outpost: the reducer speaks the navigator's copy on the
first press, spells it on the second, and on the third emits
`Effect::CopyToClipboard`, which the app's `clipboard` module carries out
and confirms. Activation emits `Effect::Activate`, which becomes a
`Query::Activate` under the same 400 ms deadline; its outcome is spoken as
the action's name, "Activate", or "No action", except an abandoned one
(walkthrough 4). A gesture injected through the control plane
(`send_gesture` in `control_handlers`) cancels speech itself, since its
keys never pass the hook, and is always a first press.

Not yet, or known limitations: the review cursor's line, word, and
character motions walk the navigator's flat name or value, waiting for
M4's text model; layout changes take effect only after a restart.

## 3. The life of an utterance

Following the focus announcement for Notepad's text area from the
reducer's `Speak` effect to the speaker (decisions D12, D17, D18;
[verbatim-speech](crates/verbatim-speech.md),
[verbatim-audio](crates/verbatim-audio.md),
[verbatim-synth-hosted](crates/verbatim-synth-hosted.md),
[verbatim-synth-host](crates/verbatim-synth-host.md)).

1. **Handing over.** The reducer thread calls `SpeechManager::speak`,
   which gives the utterance an `UtteranceId` and sends it to the queue
   thread. It never waits.
2. **Queued.** On the queue thread (`QueueThread::speak` in
   `crates/verbatim-speech/src/manager.rs`), the presenter (`ThemePresenter`)
   flattens the structured utterance through the active theme into a `SpeechSequence`: text items
   with index marks, pitch changes, and sounds, plain data that can cross a
   process. `SpeechEvents::utterance_queued` goes to the latency ledger
   (`crates/verbatim-app/src/latency.rs`), which broadcasts a `Speech`
   frame to control-plane speech subscribers. The utterance joins a lane
   by priority: `Interrupt` cancels everything first, `Next` jumps ahead
   of `Queued`. Almost everything the reducer says is `Queued`, as in
   NVDA; only a selection in a list the focus controls, and a
   notification whose hint asks for it, interrupt.
3. **Handed on.** When the synth thread is idle, `pump` takes the next
   waiting utterance. If it carries a `FocusValidity` that no longer holds
   for the latest focus the manager was told of, it is ended as cancelled
   and skipped. Otherwise it is registered with the mixer
   (`Source::register`), from which point the mixer owns its ending, and
   sent to the synth thread as a job with its own cancellation flag.
4. **Synthesized in another process.** On the synth thread, `run_job`
   splits the sequence at marks for a driver that cannot place them, and
   calls the active driver's blocking `speak`. The driver is a
   `HostedSynth`: it sends `ToHost::Speak` over a private pipe to
   `verbatim-synth-host.exe`, whose main thread runs eSpeak NG (or
   OneCore) and writes each block of audio back as a `Pcm` frame and each
   mark as a `Mark` frame, then `Done`. In Core, the host's reader thread
   relays frames through a channel of two to the synth thread, which
   passes them to the pipeline sink. The sink trims leading silence and
   held-back trailing silence and writes to the mixer (`Source::write`).
5. **Backpressure.** `Source::write` blocks while the source is more than
   the device buffer plus 40 ms ahead of playback, so the synth thread
   stops reading the channel, the reader stops reading the pipe, the
   pipe's 4096-byte buffer fills, and the host's synthesizer blocks
   inside its own write. Little audio is ever queued, so cancelling is
   cheap.
6. **Played.** Each source's audio is converted to the device's format as
   it is written (`Source::write`, on the synthesizer thread), and the
   mixer's audio thread (`crates/verbatim-audio/src/mixer.rs`) sums the
   sources (`mix_sources`) and writes to the `AudioDevice`, which is WASAPI shared mode on the default
   device ([verbatim-audio-wasapi](crates/verbatim-audio-wasapi.md)), or
   the silent real-time device under `VERBATIM_TEST_AUDIO=null`. It
   records which mix positions hold which utterance's frames, and on each
   pass works out how much the device has really played. When the
   utterance's first frame has played it reports `PlaybackEvent::Started`;
   when every frame has played, `Ended` with `Completed`. These become
   `SpeechEvents` calls on the audio thread, and the ledger broadcasts
   `SpeechStarted` and `SpeechEnded` frames and logs the announcement's
   `verbatim::latency` line. When `VERBATIM_RECORD_AUDIO` is set, a
   `WavRecorder` tap receives exactly the frames that played, for the
   end-to-end videos (D16).

Every utterance ends exactly once, as completed, cancelled, or failed.
Until the queue thread hands it on, the queue thread owns its ending (an
utterance cleared from a lane is reported cancelled there); after that,
the mixer does: completed when its last frame has played, cancelled when
its source is cancelled, failed when synthesis fails, its host dies, or
the device comes back in another format.

When speech is cut off (`docs/parity.md`, "When speech is cut off"):

- **A key press.** Every key-down except the volume keys cancels
  everything (`SpeechControl::cancel_through`, with the press's key
  sequence number, so speech an earlier press caused that arrives after it
  ends cancelled too): waiting utterances end cancelled,
  the in-flight job's flag is set, and `Source::cancel_all` ends every
  utterance the mixer holds and discards its unplayed audio at once. The
  synth thread's next write returns `Break`, and `HostedSynth` sends the
  host `Cancel` naming that utterance and reads until its `Done`, so a
  cancel crossing the end of one utterance can never cancel the next.
  Shift alone pauses and resumes instead.
- **A new foreground window or entering a menu.** The reducer's
  `Effect::StopSpeech` is the same cancel.
- **The focus moved on.** On every focus change, `Effect::DropExpiredSpeech`
  tells the queue thread where the focus now is. If any utterance already
  handed on no longer holds (it was about a focus that is neither the new
  focus, one of its ancestors, nor the foreground window), everything
  handed on is stopped. Waiting utterances are judged when their turn
  comes, so valid speech queued ahead of expired speech is still heard.
  Entered containers never expire.

Device recovery: on any device error, or when the default device changes
(an `IMMNotificationClient` sets `needs_reopen`), the mixer rewinds every
source to what had actually played and reopens the device, retrying once
a second, with its shared state unlocked so speech is never held behind
the device. If no device can be opened, the WASAPI device falls back to
the silent real-time device until a device returns.

What the end-to-end harness observes: three frames per utterance on its
speech subscription (`Speech` at queue time with the text, `SpeechStarted`,
and exactly one `SpeechEnded`). It matches text only against `Speech`
frames, then waits for that utterance's own ending (walkthrough 6). The
control server disconnects a subscriber whose queue of 256 frames fills
rather than drop a frame, since a missing ending would be invisible.

Not yet, or known limitations:

- When focus speech already handed on expires, speech handed on after it
  is stopped too, because audio cannot be taken out of the middle of the
  mixer's buffer; NVDA keeps it.
- The validity check lacks NVDA's clause for a menu item when focus moves
  to a popup menu, and the foreground node it uses changes only with a
  foreground report.
- NVDA's settings for not cancelling speech on typed characters or Enter
  are not offered; Verbatim always behaves as their defaults.
- Device recovery has not been verified live (this machine has one render
  device); the mixer's unit tests cover reopening and rewinding.
- eSpeak NG drops an index mark after a full stop, so its driver starts a
  new synthesis at such a mark; OneCore ignores a sequence's language.

## 4. The life of a query: a tree dump that times out

A developer runs `verbatim-inspect` to dump the tree of the application
holding attention (the control plane's `DumpTree` request;
[verbatim-control](crates/verbatim-control.md)). This shows the rules
every query follows.

1. **Asked.** On the control server's connection thread,
   `request_dump_tree` mints a `DumpTicket`, sends
   `ShellCommand::DumpTree` with a reply channel of one to the reducer
   thread, and waits up to five seconds.
2. **Recorded.** On the reducer thread, `ReducerThread::dump_tree` finds
   the attention application's newest live incarnation, records the
   request in the request table with `Asker::DumpTree`, and queues
   `Query::DumpTree` on that outpost's writer. Request ids are never
   reused, so an old reply can never satisfy a newer request.
3. **Queued in the outpost.** The query waits in the intake queue like
   any entry; queries are exempt from the batch limits and never expire
   while waiting. The deadline starts only when the worker starts the
   entry.
4. **Run under a deadline.** The worker calls `Watch::start` with the
   entry's budget (`budget` in `outpost/worker.rs`): five seconds for a
   tree dump or ancestor walk, 400 ms for a navigation step or
   activation, ten seconds for an event, a focus, or a focus-now query.
   The watchdog thread abandons the worker when the deadline passes, or,
   after half a second, when the user has moved to a window of the same
   application on another UI thread (`abandon_reason`).
5. **One reply, whichever way it goes.** Exactly one of these happens:
   - The worker finishes and publishes `Done`, `Gone`, or `Failed` under
     the watch lock.
   - The watchdog abandons it first, under the same lock: it bumps the
     worker generation, sends `Abandoned` for the running query, counts
     the abandoned worker, and starts a replacement worker that continues
     with the rest of the queue. The abandoned thread, if its call ever
     returns, finds it is no longer in charge, publishes nothing, lowers
     the count, and exits (`Watch::finish`).
   - The handling panics, and `run_entry` sends `Failed`.
   - Core withdraws it while still queued: the reader removes it
     (`Intake::cancel`) and sends `NotStarted` ahead of ordinary
     messages.

   "Not started" means it never ran and had no effect. "Abandoned" means
   it started and passed its deadline, so an effect such as an activation
   may already have happened.
6. **The caller gives up.** If five seconds pass, the connection thread
   sends `ShellCommand::DumpTreeGivenUp`, and the reducer thread sends the
   outpost `SupervisorToOutpost::Cancel` for that request. A dump still
   queued answers "not started"; a dump already running finishes within
   its own budget. Either way the request keeps its one outcome, which
   now goes nowhere (the reply channel's receiver is gone).
7. **Delivered.** `RequestTable::finish` drops an outcome for an id that
   already has one, or from any incarnation other than the one asked. The
   first outcome removes the entry and goes to its asker: a tree to the
   control client; for navigation, `Input::FetchCompleted`; for an
   activation, `Input::ActivationCompleted`, except that an abandoned
   activation is only logged and says nothing, since whether it happened
   is unknown and whatever it caused announces itself; for a focus-now
   query, a foreground change and a focus.
8. **The outpost ends.** If the outpost ends with requests outstanding,
   the reducer thread hears `OutpostMessage::Ended`, removes the
   incarnation from the live set, applies `Input::OutpostEnded`, and then
   `RequestTable::outpost_ended` answers every outstanding request for it
   "gone". A reply the dead outpost wrote that arrives afterwards is
   dropped by the live-set gate.

Incarnations are what make this safe. Every spawn gets a fresh
`OutpostId`; Core's reader stamps it on every node id in the incarnation's
messages from the pipe, never from the message body; a node id names the
incarnation that issued it; and the request table and the live set both
check it. So nothing an old outpost said can reach the reducer or satisfy
a query sent to its successor. The deterministic tests are in
`crates/verbatim-app/src/requests.rs`, `live.rs`,
`crates/verbatim-outpost/src/outpost/worker.rs`, and
`crates/verbatim-core/tests/reduce.rs`.

Known limitation: the app's wiring of these pieces
(`ReducerThread::on_outpost_message`) has no test of its own. Parallel
queries within one application are given up by design: a long tree dump
delays that application's events.

## 5. Recovery

Each failure, what notices it, and what happens next.

1. **A call into an application hangs.** The outpost's watchdog abandons
   the worker and starts a replacement (walkthrough 4); an abandoned
   thread keeps its stack (Rust's default 2 MiB, reserved rather than
   committed) until its call returns or the process ends. Events from a window the system already reports hung are
   dropped before any read, so they never reach a call.
2. **An outpost wedges.** The lifecycle owner pings every child every
   three seconds; the reader thread answers pings itself, so a busy
   worker never looks dead. Nine seconds without a pong, or eight
   abandoned workers (unless the application's own windows are reported
   hung, when a replacement would hang the same way), ends the outpost as
   killed (`Owner::heartbeat`, the pure `wedge_decision`). Ending closes
   its job handle, which kills the process; there is no shutdown message,
   because a process that has used UIA as a client can hang or crash in
   UIA's own exit code (architecture section 1).
3. **An outpost crashes.** Its pipe closes; Core's reader forwards
   everything it wrote, then reports the end; the owner sends `Ended` as
   exited.
4. **After either.** The reducer thread drops later messages from that
   incarnation, the reducer marks the focus dead but keeps its copied
   data and clears a navigator or pending navigation in that outpost, and
   outstanding queries end "gone". The owner replaces the outpost at once
   only if its application holds attention (`Owner::after_crash`);
   otherwise the next fact for the application starts one. After three
   crashes within a minute it stops replacing it until the next
   foreground change to that application; an application that has itself
   exited is left alone. The reducer thread asks the replacement for the
   current focus when it is ready (`focus_now_wanted`); for a focus whose
   application is not the attention one (a Settings page's content inside
   `ApplicationFrameHost`'s window) it also asks the supervisor to start
   one. When the answer arrives, the reducer compares it with the dead
   focus's copy (role, name, value, states, and ancestors' names and
   roles, `reads_the_same`): if they match, it takes the new node ids
   silently; otherwise it announces the focus as usual.
5. **An idle outpost is retired.** Every 30 seconds, an outpost whose
   application has not held attention for two minutes, which is not
   Core's own, and in which the reducer holds no nodes, is ended as
   retired (`retirement_decision`). No focus-now query follows.
6. **The listener is replaced.** If its pipe closes or it misses nine
   seconds of pongs, the owner ends it and starts another. When the
   replacement is ready, the owner sends `OutpostMessage::ListenerReplaced`;
   the reducer thread reads the foreground window itself (a local call)
   and asks that application for its focus, taken silently if unchanged.
   Desktop-wide UIA events from the gap are lost.
7. **The synthesizer host dies or hangs.** `HostedSynth` ends a host whose
   pipe closed, that sent nothing for ten seconds while Core waited, or
   that answered out of turn. The utterance in flight fails, with one
   exception: when the host's pipe ended before any of it was relayed and
   speech was not cancelled, it is sent once more to a fresh host. A host
   that hung or answered out of turn is not retried, since a second host
   would likely do the same (`HostedSynth::speak`). The next request starts a new host and restores every
   setting the old one had; a setting the new host refuses (a voice since
   uninstalled) is skipped. A synthesis error the host reports in turn
   fails only that utterance. The `synth_host_crash_recovery` scenario
   kills the host and expects the next announcement in full.
8. **A synthesizer will not start.** At startup, if the configured
   synthesizer cannot be built, the speech manager tries every other
   registered one in order, eSpeak NG first, and logs which it used; the
   fallback is not saved as the user's choice. A saved setting the driver
   refuses is skipped with a warning. A failed switch leaves the previous
   synthesizer active.
9. **The audio device fails or changes.** The mixer reopens it
   (walkthrough 3). Utterances whose audio had not played are replayed
   from where playback got to; if the device returns in another sample
   rate or channel count, utterances already converted for the old format
   end as failed.
10. **Verbatim is started outside an interactive session.** `main` calls
    `check_interactive_session` (`verbatim_process::session::current`)
    before replacing a running instance, so a launch from WinRM,
    PowerShell Direct, or a service exits with a diagnosis and never stops
    a working Verbatim. A locked desktop is not refused. The agent makes
    the same check before binding its port.
11. **Verbatim itself dies.** The kernel closes Core's job handles and
    kills every outpost, the listener, and every synthesizer host. A
    panic on any thread writes a flight-recorder dump first
    (`flight_dump::install_panic_hook`).

Not verified live: the refusal outside an interactive session (making
such a session here needs elevation) and device recovery. The UIA registry
has no counterpart to MSAA's per-window forgetting, so a reused window
handle could inherit a held node's identity.

## 6. The life of an end-to-end run

`cargo test -p verbatim-e2e -- --ignored --skip demo_ --test-threads=1`
on this machine, or the
same suite on GitHub's runner or against a Hyper-V guest
([verbatim-e2e](crates/verbatim-e2e.md),
[verbatim-agent](crates/verbatim-agent.md), [Tooling](tooling.md)).

1. **Preparing this machine (runner-direct).** The agent runs in the
   signed-in user's session: `target\debug\verbatim-agent.exe
   --bind-address 127.0.0.1 --port 44001`. It refuses to start in a
   non-interactive session. The desktop must be unlocked: on the
   development VM, `cargo xtask park` moves the
   Remote Desktop session onto the console, still signed in and unlocked,
   so a run works with no RDP client connected (it disconnects any client,
   so only when Dickson is away or has agreed). NVDA in the same session
   must be closed, since the run injects real keys; an NVDA in another
   Windows session cannot see them and can stay.
   `VERBATIM_E2E_ENDPOINT=127.0.0.1:44001` points the suite at the agent.
   Every live test is `#[ignore]`d, so `cargo xtask ci` lists them as
   ignored; run with `--ignored` and no endpoint, each fails.
2. **One scenario at a time.** Each `#[test]` wrapper calls
   `registry::run_named`. `--test-threads=1` is required: each scenario
   launches a real Verbatim on the real desktop, and a second
   `verbatim.exe` would replace the first.
3. **Launch.** `Scenario::launch` builds `verbatim-app`,
   `verbatim-outpost`, `verbatim-synth-host`, and `mockapp` once per test
   binary, copies them, `espeak-ng-data`, and `sounds` into
   `target/e2e-stage`, and writes a
   fixed `settings.toml` (`Settings::for_e2e`, eSpeak NG). A run is silent
   by default: `VERBATIM_TEST_AUDIO=null` makes Verbatim play through the
   silent real-time device, so every utterance still takes its real
   duration; `VERBATIM_E2E_AUDIBLE=1` uses the real device instead. It
   sweeps what an earlier run left (processes the agent launched, by their
   own handles; harness windows and Notepad tabs; harness files),
   minimizes every window and brings the desktop forward, starts the
   recording (step 7), and launches `verbatim.exe` through the agent with
   its output sent to a file and `RUST_LOG` set to
   `info,verbatim_outpost=debug`. Verbatim sets a named event the agent
   created once it is ready for input; the harness then opens the command
   tunnel and a second tunnel used only for speech (the agent relays bytes
   between its TCP connection and Verbatim's named pipe).
4. **Setup and body.** `registry::run` first asserts the startup speech
   and the desktop's announcement exactly. The scenario opens its
   applications through the agent, each window its own, and drives
   Verbatim with keys the agent injects through `SendInput`, each numbered
   in its `dwExtraInfo`, which pass through Verbatim's hook like a user's,
   and with `SendGesture`. Every injected key cancels speech, as a real
   one does.
5. **Asserting speech.** `SpeechCollector` reads the speech tunnel, which
   first replays what Verbatim said before it subscribed. Each assertion
   says the next utterances are exactly these, in order, each ending as
   expected, nothing in between; before every injected input, any
   utterance no assertion matched is a harness error. The body ends with
   `expect_nothing_more`: an `AwaitIdle` request on the speech tunnel,
   answered once Core has handled the last numbered key and is idle, after
   every utterance it queued. Failure messages print the expected and
   actual sequences, where they first differ, and the timeline: every
   injected key and gesture interleaved with every utterance's queue time,
   audio start, and ending.
6. **Teardown and artifacts.** After the body, the run saves the latency
   timelines, Core's focus (`focus.txt`), and the flight recorder, checks
   that none of Verbatim's processes exited unexpectedly, quits Verbatim
   (exit code 0), runs teardown, and closes everything the scenario
   opened, by process id or window, failing on anything that will not
   close. Pass or fail, the run's directory under
   `target/e2e-artifacts/<scenario>/` receives the timeline,
   `latency.csv`, `foreground.txt`, Verbatim's stderr, every log in the
   launch's `logs\<Verbatim's pid>` directory (listener, each outpost, each
   synthesizer host), `verbatim-audio.wav`, any crash dump, and
   `summary.txt` (`ScenarioSummary`); losing one fails the run. Each run is
   also copied into `history/` (the newest 100 per scenario, without
   video).
7. **Recording.** Before Verbatim starts, ffmpeg is launched through the
   agent to capture the desktop (`gdigrab`, fragmented MP4).
   `VERBATIM_RECORD_AUDIO` makes Verbatim's mixer tap write everything it
   plays to a WAV file with its start time, in every run. At the end,
   `Scenario::finish_recording` ends ffmpeg, lines the audio up with the
   video by their start times, muxes them on the agent's machine, and
   copies `<scenario>.mp4` back. A recording that cannot start or finish
   fails the run; `VERBATIM_E2E_RECORD=0` turns recording off.
8. **On CI.** The `e2e` job in `.github/workflows/ci.yml` builds on
   `windows-latest`, whose jobs run in an interactive session, installs
   ffmpeg, starts the agent in the same step as the suite (the runner ends
   a step's processes when the step finishes), runs every scenario but the
   demonstrations and the local-only Windows 11 Notepad ones
   (`--ignored --skip demo_ --skip notepad_`), and uploads the artifacts
   and videos for passing and failing runs alike.
9. **Against a Hyper-V guest.** `cargo xtask vm test` builds first,
   copies changed binaries into the guest (`xtask/src/vm/deploy.rs`),
   restores the golden checkpoint only when `--restore` is given, runs
   `session_info` as a precondition, then runs each scenario as its own
   `cargo test` subprocess with `VERBATIM_E2E_REMOTE=1` and an audible
   run, and builds its summary from each `summary.txt` ([vm.md](vm.md)).

The nine scenarios: `menu_and_settings_dialog`,
`rapid_tabbing_in_settings`, `switch_to_onecore`,
`synth_host_crash_recovery`, and `lock_key_announcements` (speech); `notepad_and_verbatim_menu` and
`start_menu_search` (shell); `object_navigation_in_settings` and
`system_information_tree` (navigation). On 2026-10-05 the full suite, with
`lock_key_announcements` added, passed 4 of 4 runs on this machine with the session parked and NVDA
closed, and CI's job passed on every push. (Since 2026-10-07,
`notepad_and_verbatim_menu` is `second_application_and_verbatim_menu`,
against the harness's Windows Forms text box, and `start_menu_search` is
gone; `docs/crates/verbatim-e2e.md` lists the scenarios as they are.)

Not yet, or known limitations: the Hyper-V path has not been run since
phase 4 (its golden image still needs rebuilding without VB-CABLE); a
Verbatim whose launch fails before its control plane answers leaves no
video; and `switch_to_onecore` assumes OneCore voices are installed.
(`synth_host_crash_recovery` then killed synthesizer hosts machine-wide;
it now ends Verbatim's own host by its process id.) On a
locked desktop injected input goes nowhere and scenarios fail or hang,
which is why parking matters.
