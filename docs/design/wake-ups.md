# Wake-ups: an audit of every thread's wake-ups (2026-10-13)

Dickson asked for efficiency as well as latency, ideally as few wake-ups as
possible, and for root causes rather than mitigations (2026-10-13). The
communication design's step 3 had raised thread priorities to cut latency;
a WPR trace then showed the mixer's writer and audio thread waking each
other thousands of times a second, and raised priorities made typed echoes
slower. The migration is paused (`docs/design/communication.md`, section
11) while every thread's wake-ups are measured and each source is fixed
where it starts.

## How it is measured

- `cargo xtask trace start` and `stop` record WPR's CPU profile through
  the elevated tasks (`docs/tooling.md`, "Tracing the CPU's scheduling").
  A trace is about 20 MB a second, so each covers 5 to 8 seconds of one
  scenario and is deleted once read.
- `cargo xtask trace waits` counts, for every thread of Verbatim's
  processes, how many times it was switched in after waiting: its wake-ups.
  `--wakers` names, for one thread, the thread running on the processor
  that readied it. That attribution is right when one thread signals
  another; when a thread is readied from an interrupt (a timer expiring, an
  I/O completing), it names whatever thread the interrupt fell on, so a
  waker in an unrelated process means a timer or a device, not that
  process.
- The workloads: Verbatim idle (started directly with its console hidden
  and the desktop in front, nothing happening; an idle period is time,
  which the suite's scenarios may not wait on), `text_box_typed_words` (typing), `measure_focus_explorer`
  (focus moving in File Explorer), `conhost_flood` (terminal output) and
  `measure_speech_real` (the real synthesizer host and audio device), all
  at default thread priorities after the scheduling policy was reverted
  (1651ab0).

## What was found

Counts are for one traced window of 5 to 8 seconds.

1. The mixer's writer and audio thread woke each other for every few
   frames while a synthesizer ran ahead of playback: 75,039 synth thread
   wake-ups in an 11-second typing run. Fixed at its source (6d5836f): a
   blocked writer waits for half its source's room, the audio thread wakes
   writers only when that room is free, and a write wakes the audio thread
   only when the device needs frames. The same run now has 1,088.
2. Idle, Verbatim is quiet: about 50 wake-ups a second across all its
   processes, of which the audio thread's one-second wait (a poll for a
   reopen request, `mixer.rs` `IDLE_WAIT`) and the three-second liveness
   pings to each child are its own. The audio thread's poll is fixed: the
   new default device's callback already signalled the wait, and the
   WASAPI device now ends a wait by its own retry time after a failed
   open, so the idle mixer waits with no limit and did not wake once in 5
   idle seconds. The rest are applications' events: File
   Explorer raises a burst on the desktop's icon list every 3 to 4 seconds
   (destroy, focus, selection), and each costs its outpost's worker about
   ten cross-process calls, as it would cost NVDA.
3. Each outpost's watchdog wakes for every item its worker starts and
   ends, to arm or disarm the deadline (`worker.rs`, the watch's
   condition variable): 7,972 wake-ups in 8 seconds of `conhost_flood`,
   about 1,000 a second, more than the worker's own 6,431. The
   communication design's finding 12 named it. Fixed: the worker sets and
   cancels a waitable timer, which wakes no thread, so the watchdog wakes
   only when a deadline or grace passes; the same window then had too few
   to list.
4. Core answers the outpost's messages with its held nodes
   (`NodesHeld`) as each message is handled, so a flood of terminal output
   becomes a round trip per message: in 8 seconds of `conhost_flood`,
   verbatim.exe's writer for the outpost woke 3,092 times, the outpost's
   pipe-reading thread 3,397, and its worker handled a "nodes held" item
   for each. Finding 12 again. Sending them only when the reducer was
   drained did not help, since in a flood it drains between every message;
   they are now sent once 32 messages have passed since the last, the
   unconditional resend every 256 kept. "Nodes held" items fell from 8,729
   to 294 in a run, and the outbound, reader and reducer threads' wake-ups
   by about two thirds.
5. Every message from an outpost wakes three threads before the reducer
   handles it: the outpost's outbound thread, verbatim.exe's reader for
   that outpost, and the reducer (3,296, 3,061 and 3,622 in the flood). The
   transport of the communication design (steps 4 to 7) removes the first
   two hand-offs; this is the part of that design the audit confirms.
6. The hosted synthesizer's driver waits for its host in 10 ms slices to
   notice a cancel (`synth-hosted` `CANCEL_CHECK`), 100 wake-ups a second
   while speaking, and `OneCore` in the host polls the same way. Fixed:
   `SynthSink::on_cancel` lets a driver register a waker the cancel calls;
   the hosted driver waits on the host's replies and its cancel channel
   together, `OneCore` on its operation's completion and its cancel in one
   channel, and the host's sink calls the waker as the cancel arrives.
   Waking the driver at once exposed an ordering the poll had hidden: the
   speech manager now cancels the mixer before the job, so an utterance the
   woken driver returns from is already ended cancelled. The saving is in
   the long waits: `OneCore` sends nothing until it has synthesized the
   whole utterance, and the driver's thread and the host's both polled
   through it. `measure_speech_real` (eSpeak, whose replies come faster
   than every 10 ms) does not show it: its synth thread is readied about
   120 times a second before and after, varying by half between runs, by
   the audio thread, the speech queue and the host reader, as it moves
   each chunk from the host to the mixer. Every latency stage of the
   measure set is unchanged or better: in release, replies taking over
   1 ms on the way back from the host fell from 3.5% to 0.45%, and their
   maximum from 10.5 ms to 2.2 ms.
7. `WinEvent` hooks are out of context, so each wakes its thread for every
   event in its subscription before Verbatim's filter runs. In 8 seconds
   of `measure_focus_explorer`, File Explorer's outpost's event thread woke
   8,734 times and the listener's 3,076. The listener subscribes to object
   shows from every process (`LISTENER_SUBSCRIPTIONS`), and every outpost
   to thirteen kinds from its application (`APP_SUBSCRIPTIONS`), among them
   name, state and show changes that File Explorer raises in thousands.
   The stacks (`--stacks`) show who raises them: File Explorer, Steam's web
   helper (about 850 in 5 seconds while Steam merely runs), and the test
   agent. NVDA's main thread hooks about thirty kinds, location changes
   among them, from every process out of context too
   (`internalWinEventHandler.py` 35 to 62), so it pays the same per event;
   its console events, unlike Verbatim's before item 10, it hooks only
   while connected to a focused legacy console (`winConsoleHandler.py`).
   Narrowing this means filtering inside the application before the event
   leaves it, which is the injection helper of decision D2; until then, the
   question for each kind is whether that hook needs it.
8. In the same run one of the listener's unnamed threads woke 10,089
   times, about 1,260 a second, and two others 4,185 and 1,674. They are
   not Verbatim's (it names its threads); they are most likely UI
   Automation's own, serving the listener's desktop-wide subscriptions,
   including the `WinEvent` processing UI Automation does to raise events
   for windows without a native provider. The threads UI Automation runs
   Verbatim's handlers on now name themselves `verbatim-uia-callback` at
   their first event, and their stacks show them readied from File
   Explorer, the source of the events; UI Automation's other threads stay
   unnamed. Each handled event costs this hop, which the application's
   event rate decides.
9. verbatim.exe's GUI thread woke 361 times in 8 seconds of typing and
   almost never while idle, though no Verbatim window was open and nothing
   in Verbatim's code wakes its event loop except the menu, a shell list
   and shutdown. Its wakers are unrelated processes after waits of 20 ms or
   more. The stacks show it readied from File Explorer, Steam's web helper,
   PowerShell and Edge's web view: messages Windows broadcasts to every
   top-level window, which Verbatim's hidden frame and tray icon are; any
   application with a hidden top-level window pays them. A message-only
   window receives no broadcasts, but the tray icon and the menus need the
   frame; a later design question, about 67 wake-ups a second while
   typing.
10. The keyboard hook thread hooked every console's events from every
    console on the desktop, to learn which console host thread to
    translate typed keys with: a console flooding in the background woke
    it 47,061 times in 6 seconds, on the thread Windows waits on for every
    key. Fixed: those events are hooked only while the console window in
    front has not yet named its thread, and unhooked at the first; the
    foreground event, which names it when a console comes to the front, is
    hooked throughout. The same window then had 43.
11. Threads that exist only for the test harness (the WAV recorder, the
    control plane's speech writers) wake often in every run and cost a
    user nothing.

Tracing has one hazard, seen twice on 2026-10-13 and not understood:
after a trace started while a scenario was starting, File Explorer kept
putting keyboard focus on the taskbar's Start button whenever the desktop
was brought forward, and every scenario then failed at its start until
Explorer was restarted. Traces are therefore started well after a
scenario's startup.

## What follows

- Fix 3, 4 and 6 at their sources, each measured with the same workloads
  before and after, its wake-ups and its latency stages both. Done:
  fac2f04 for 3 and 4, 3ebb7be for 6.
- Read the trace's stacks for 7, 8 and 9 (`verbatim-trace-waits` grows a
  report of the readying stacks), then decide each.
- Revise `docs/design/communication.md` so that every step is judged on
  wake-ups and CPU as well as latency, with these workloads and counts as
  its baseline, and bring the revision to Dickson before the migration
  resumes. Thread priorities are judged again only after that, on
  measurements free of these sources.
