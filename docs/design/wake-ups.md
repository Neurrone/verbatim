# Wake-ups: an audit of every thread's wake-ups (2026-10-13)

Status: audit log, 2026-10-13 to 2026-10-14.

Dickson asked for efficiency as well as latency, ideally as few wake-ups as
possible, and for root causes rather than mitigations (2026-10-13). The
communication design's step 3 had raised thread priorities to cut latency;
a WPR trace then showed the mixer's writer and audio thread waking each
other thousands of times a second, and raised priorities made typed echoes
slower. The migration (`docs/design/communication.md`, section 11) was paused
while every thread's wake-ups were measured and each source fixed where
it starts.

## How it is measured

- `cargo xtask trace start` and `stop` record WPR's CPU profile through
  the elevated tasks (`docs/tooling/measuring.md`, "Measuring performance").
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
   by about two thirds. Replaced since by communication
   step 11 (c65b6e95): Core leaves a changed list in a slot in the outpost's writer, and it goes in the same write as Core's next command to that outpost, after the command, or with the relay's next ping, so it never costs a write of its own and is at most one ping interval (3 seconds) late; a change in the acknowledged position counts as a change, so the resend every 256 messages is gone too.
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
12. In a console flood (`conhost_flood`, 6 seconds from 8 seconds in) the
    console's outpost used 7.5 seconds of processor time, more than a
    processor, 3.5 seconds of it on UI Automation's callback threads. Their
    CPU samples, named through `verbatim-trace-waits --samples` with the
    build's PDBs, put a third of it under Verbatim's text event handler
    reading the event's element (`capture`, through
    `snapshot_parts_from_cached_element`) and the rest in UI Automation
    raising, unwinding and formatting errors: the handler read every
    property a snapshot is made of, while a text event's cache request
    holds three, and UI Automation answers a read of a property its cache
    lacks by raising and reporting an error inside itself. Property events
    did the same for the details their cache request leaves out. An
    experiment that read an event's element through the properties its
    registration cached alone, each other one reading as unsupported
    without asking UI Automation, gave the same window 0.83 seconds on the
    callback threads and 3.2 in the outpost. The same pattern was found
    across the workspace (the listener reads 25 absent properties on every
    desktop-wide event), and Dickson chose to fix it systematically:
    `docs/design/uia-shapes.md`, whose event shapes replace the
    experiment.
13. In the same flood the console window, read through UIA, raised about
    56,000 MSAA `WinEvents` the outpost took in, text selection and value
    changes above all, each waking the event thread and the worker, which
    dropped it at the window's UIA test after its acceptance and state
    checks. Fixed: the event thread drops one whose window is already known
    to be read through UIA (the class lists or a kept verdict, no probe),
    as NVDA drops it, keeping a window shown, a console's update and a
    destroyed window, which the worker uses for UIA windows too. Over the
    whole scenario the worker handled about 9,400 items instead of 38,000.
    The event thread still wakes for each event, which only filtering
    inside the application (D2's injection helper) avoids.
14. While typing, the outpost's worker woke once for each reply to its
    cross-process calls, from the application through `win32k` and from
    `ctfmon.exe`, the Text Services Framework's server (634 of the ten
    most frequent readying stacks in 8 seconds). Why `ctfmon.exe` answers
    a windowless worker is not yet known.
15. With the event reads cheap, UI Automation delivered every event the
    console host raised in a footer flood (`conhost_footer_overflow`):
    17,528 text changes and 17,024 caret changes, against 162 and 150 while
    the callbacks were slow, when UI Automation dropped the rest. Over the
    whole flood (20 seconds from 8 seconds in) the console host then spent
    16.5 seconds of processor time delivering them, against 4.7, and wrote
    so much more slowly that the scenario's 12,000-line write had not
    finished when its group of thirty lines had been spoken. The same
    flood's console updates (`EVENT_CONSOLE_UPDATE_*`, about 16,600) tell
    the outpost of the same output more cheaply, and only they report a
    screen being cleared: driving the console host's reads from them alone,
    without UIA text changes, cut the flood's processor time in all
    processes from 25.0 to 15.5 seconds and its wake-ups from about 296,000
    to 154,000. Fixed, with Dickson's agreement on condition of no
    regressions: the console host's text area takes the focus-following
    subscription's extended group without its text changes (then
    `Group::ExtendedWithoutTextChanges`), and its console updates drive
    its reads; NVDA, for a UIA console, listens to UIA text changes alone.
    Windows Terminal throttles its own UIA events (94 text changes and 74
    caret changes in a whole flood, 0.34 seconds in the outpost) and is
    unchanged. The UIA caret events went the same way, with Dickson's
    agreement on the same condition: in `conhost_flood` the console host
    raised 15,979 of them, one with nearly every line, which cost a wake on
    UI Automation's receiving thread and one on a callback thread each
    (12,701 and 9,776 wakes in a 10-second window) besides the console
    host's own cost of raising them, while its own caret event
    (`EVENT_CONSOLE_CARET`) came 25 times. That event comes on the
    cursor's blink timer, about every 530 milliseconds, when the cursor
    has moved, and at once for a selection's move, and outside output the
    UIA caret event came with it, at the same tick, for the same cell; the
    reads the console updates cause read the caret with the text. The text
    area now takes the group without its text events
    (`Group::ExtendedWithoutTextEvents`), and the console's caret events
    are its caret events.

Tracing has one hazard, seen three times on 2026-10-13 and not
understood: after a trace started while a scenario was starting, twice,
and once after a trace that covered the end of `measure_focus_explorer`,
File Explorer kept putting keyboard focus on the taskbar's Start button
whenever the desktop was brought forward, and every scenario then failed
at its start until Explorer was restarted. The same scenario run without
a trace left Explorer as it was. Traces are therefore kept inside a
scenario, away from its start and end, and Explorer is restarted when it
happens.

The baseline these fixes leave, and what the communication design's
remaining steps are worth against it, is `docs/design/communication.md`
section 18.

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

## Measurements after communication step 5 (2026-10-13)

A measurement batch on main at 0f59a8b9 (step 5, the relay on its
completion port; a55b3660 after it changed only a document), on this
development machine, with NVDA not running (its log shows it exited at
09:05, before the earlier figures were taken too, so both sets ran
without it). Nothing in the product changed.

### How it was measured

- Latency: `cargo xtask measure`, debug and release, for the measure set
  (all eight `measure_` scenarios), `measure_speech_real` (audible, the
  real eSpeak host and WASAPI device), and the other workloads named with
  `--scenario`. Every figure below is given as minimum, P50, P95, P99 and
  maximum in milliseconds, over every announcement's latency line, the
  percentiles taken by nearest rank. `stages.txt` gives the median and the
  99th and 99.9th percentiles instead, so these were computed from the
  same `stderr.log` files by a scratch script, not a tooling change. The
  ledger rounds to 0.1 ms, so 0.0 means under 0.05 ms. Step 5's criteria
  are stated in P99.9, which is quoted only where a criterion needs it.
- Earlier figures: latency from the measure set run at 12:45 that day
  (step 3's configuration B as adopted, before step 4), and for
  `measure_speech_real`, `conhost_flood` and `text_box_typed_words` from
  the same build's runs kept in `target\e2e-artifacts\history`; wake-ups
  from the traces behind section 18.2 of `communication.md` (idle,
  `measure_focus_explorer`, `text_box_typed_words`, `conhost_flood`,
  `measure_keys_text_box` twice, `measure_speech_real`), traced again with
  the same windows. No old commit was rebuilt for these.
- The added workloads (`settings_system_page`, `notepad_say_all`,
  `notepad_review_cursor`, `measure_focus_windows_terminal`,
  `measure_keys_windows_terminal`, `object_navigation_over_uia`,
  `menu_and_settings_dialog`, `system_information_tree`) were measured
  before and after: 698b9807, the commit before the day's console, shapes
  and communication work, was built in a separate worktree with its own
  target folder (debug only, which kept more than 8 GB free on C:), and
  each workload was measured and traced on it and on main with the same
  windows. The worktree was removed afterwards.
- Wake-ups and processor time: `cargo xtask trace` windows of 3 to 10
  seconds read by `verbatim-trace-waits`, for Verbatim's three processes
  and the target applications. In each window the six most-woken of
  Verbatim's threads had their readying stacks and wakers read, and the
  three busiest their CPU samples. Counts leave out the harness's own
  threads (the WAV recorder, the control plane's readers and writers).
  Every trace was deleted once read.

Everything below is measured unless marked inferred.

### Step 5's pass criteria

- verbatim.exe's own stage, at most 1 ms maximum, P99.9 at most 0.2 ms in
  release and 0.5 ms in debug: met. Over the measure set, debug 0.0, 0.0,
  0.0, 0.2, 0.2 (P99.9 0.2, 442 samples); release 0.0, 0.0, 0.0, 0.1, 0.1
  (P99.9 0.1, 434 samples). In `measure_speech_real` the stage never
  exceeded 0.1 ms in either build, where it had reached 5.8 ms in debug
  (P95 1.8) and 3.0 ms in release before.
- Listener to relay plus relay, P99.9 at most 1 ms and maximum at most 2
  ms: not met as the ledger measures it, and not attributable. The
  listener stage has only 14 samples in each build of the measure set:
  debug 0.1, 12.7, 47.9, 72.4, 72.4 and release 0.1, 11.2, 86.0, 365.3,
  365.3 (365.3 ms on one File Explorer focus), against 0.2, 13.7, 98.4,
  98.4, 98.4 and 0.1, 12.3, 63.8, 63.8, 63.8 before. For a UIA focus the
  listener stamps the event when it arrives, and its reading thread
  (`verbatim-listener-read`) then reads the focus across processes before
  the fact is written, so the stage holds that read; no stamp marks the
  listener's write. Facts on the listener's path with no read (the text
  box's) show 0.1 to 0.2 ms. The ETW side: in File Explorer the relay
  thread was readied 497 times in 8 seconds from the listener's pipe
  writes (`npfs.sys` on the readying stack) and 36 times from its own
  process.
- Requests' relay stage (`reducer to verbatim.exe's relay`) at most 1 ms:
  met in debug (0.0, 0.0, 0.1, 0.2, 0.6) and in `measure_speech_real`. In
  release one sample of 410, in `measure_keys_windows_terminal`, took 1.3
  ms; a second release run of both key workloads gave 0.0, 0.0, 0.0, 0.1,
  0.1 over 333 samples. Before: debug 0.0, 0.0, 0.1, 0.1, 0.2; release
  0.0, 0.0, 0.0, 0.1, 0.1.

### The hops to speech and their cost

The cross-process hops the ledger stamps, over the measure set, before
then after:

- verbatim.exe to an outpost, facts and requests: debug 0.1, 0.1, 0.2,
  0.4, 1.0 then 0.1, 0.2, 0.3, 0.7, 2.5; release 0.0, 0.0, 0.1, 0.1, 1.4
  then 0.0, 0.0, 0.1, 0.1, 0.7.
- An outpost to verbatim.exe: debug 0.1, 0.2, 0.3, 0.4, 2.1 then 0.1,
  0.2, 0.3, 0.4, 10.0; release 0.0, 0.1, 0.1, 0.2, 0.3 then 0.0, 0.1,
  0.1, 0.2, 0.7. The stage covers the worker's publish, the outpost's
  outbound thread, the pipe, and verbatim.exe's reader with its parse; no
  stamp separates the outbound thread's hand-off from the pipe.
- The wait for the reducer after it: debug 0.0, 0.1, 1.9, 3.3, 6.8 then
  0.0, 0.1, 0.5, 2.4, 18.1 (over 1 ms in 14.3 then 2.1 percent); release
  0.0, 0.0, 1.4, 2.7, 11.3 then 0.0, 0.0, 0.3, 2.0, 6.3 (16.4 then 1.5
  percent). The earlier run had the message-moving threads at raised
  priority, which section 11 step 3 found moved time into this wait.
- Summed per announcement, verbatim.exe's stage, both hops and that wait:
  debug 0.1, 0.3, 2.0, 3.7, 6.9 then 0.1, 0.3, 0.8, 2.8, 18.2 (over 1 ms
  14.8 then 3.3 percent); release 0.0, 0.1, 1.4, 2.7, 11.3 then 0.0, 0.1,
  0.4, 2.0, 6.3 (16.6 then 1.6 percent). P95 improved by about 1 ms in
  both builds; the debug maximum is one 18 ms wait for the reducer in
  `measure_keys_windows_terminal`.
- To the synthesizer host: P99 at most 0.2 ms, maximum 1.1 debug and 0.2
  release. Back from the host with the first audio: debug 0.0, 0.1, 0.3,
  0.6, 4.8 and release 0.0, 0.0, 0.1, 1.7, 2.5, against 0.0, 0.1, 0.3,
  0.7, 2.4 and 0.0, 0.0, 0.2, 0.8, 2.6 before.
- With the real device (`measure_speech_real`, release, after): to the
  host 0.0, 0.0, 0.1, 0.1, 0.1; the host's first chunk 0.0, 0.1, 0.7,
  1.7, 5.2; back 0.0, 0.1, 0.3, 1.9, 2.5; mixer and device 0.4, 5.0, 9.6,
  10.1, 49.6; the device clock 9.2, 9.9, 10.6, 10.7, 13.2.

The in-process stages on the same path, after: the hook to Core debug
0.1, 0.2, 2.9, 5.1, 9.3 and release 0.0, 0.1, 3.6, 4.1, 5.5, with 10 to 14
percent of keys over 1 ms in both builds before and after; the reducer
under 0.3 ms at P99; to speech P99 2 to 3 ms, with a tail to 90 to 155 ms
that is speech waiting behind earlier speech; the outpost's read P50 1.1
to 1.9 ms, focus reads 17 to 31 ms. The outpost's read in
`measure_keys_conhost` moved from P99 3.6 to 5.8 ms in debug and 2.8 to
5.0 ms in release, the console's reads now being driven by console
updates; the other workloads' reads are unchanged.

Hops with no stamp, so not measured: the application raising an event to
the outpost or listener taking it (the `Windows` stage has tick resolution
and reads 0.0 almost always); the listener's write, as above; the
outpost's worker to its outbound thread; the hook thread to the router and
the reducer, inside "hook to Core"; the speech queue to the synthesizer
thread, inside "to speech"; and the host's audio after its first chunk.
The thread hand-offs show as wakers instead: on keys in the text box the
speech queue was readied by the hook thread 117 times in 8 seconds, by the
synthesizer thread 69, the audio thread 43 and the reducer 40; the
synthesizer thread by the audio thread 136 and the speech queue 105; in
the console flood the outbound thread by the worker 2,931 times and
verbatim.exe's reader by the outbound thread 2,924, one each per message.

### Per workload, before and after

Wake-ups and processor time are for one trace window across Verbatim's
processes, harness threads left out; latency is per workload where the
pooled set hides it.

- Idle (Verbatim started directly, 8 seconds from 12 seconds in): before
  56 wake-ups and 3 ms. After, debug: 95 and 6.3 ms in a first run that
  caught one of File Explorer's desktop bursts (the listener's UI
  Automation threads woke 31 times for it, and Explorer itself 135 times
  against 19 with Verbatim not running), then 45 and 3.8 ms on a repeat;
  release 48 and 2.1 ms. What is left is the 3-second pings, now through
  the relay (4 to 8 wakes) instead of the listener's reader and writer and
  the per-outpost writers, which are gone.
- `measure_focus_explorer`: the windows caught different phases (after,
  the ten new windows opening and closing; before, the arrow rounds), so
  they are not comparable: 32,756 wake-ups and 2.02 s before, 24,724 and
  1.25 s after. Over the whole scenario, from the outposts' logs, items
  taken in fell from about 3,700 to 2,900 and items handled from about
  2,350 to 1,800 (eight runs before, three after). In the window after,
  the listener's unnamed UI Automation threads woke 15,848 times (510 ms),
  its event thread 3,250, its reading thread 1,278 (139 ms), its outbound
  thread 536, the relay 541; File Explorer itself used 11.3 s of processor
  time in 8 seconds. Latency, debug after: verbatim.exe and its hops at
  most 1.1 ms; first audio 15.6, 65.9, 140.0, 140.0, 140.0 (before 35.9,
  74.7, 284.8, 284.8, 284.8, four samples).
- `measure_focus_text_box` (5 seconds long, no room for a trace): the
  outpost's read 13.3, 23.9, 26.8, 26.8, 26.8 before and 17.3, 23.8, 28.9,
  28.9, 28.9 after in debug; everything between the outpost and the speech
  queue under 0.6 ms in both builds before and after.
- `text_box_typed_words` (9 seconds in, 4 long, a window that caught the
  scenario's end in both builds, so its counts are not compared): key to
  speech queued, debug, 0.0, 0.2, 3.3, 4.6, 4.6 before and 0.1, 0.3, 2.5,
  3.4, 3.4 after; first audio 0.8, 8.2, 66.5, 104.8, 104.8 then 0.7, 2.5,
  69.1, 106.6, 106.6. Items handled over the scenario 98 to 100 in both.
- Keys in the Windows Forms text box (`measure_keys_text_box`, the
  earlier `b-keys` and `b-keys2`): 9,686 and 9,805 wake-ups, 914 and 900
  ms before; 9,291 and 10,393, 890 and 922 ms after, within the variation
  of two runs, for the same work (about 1,600 items handled and 519
  announcements over the scenario in every run). Latency, debug: summed
  hops and reducer wait 0.2, 0.4, 0.7, 0.9, 1.3 before and 0.2, 0.4, 0.9,
  2.1, 3.2 after; release 0.0, 0.1, 0.3, 0.5, 2.1 before, then 0.0, 0.1,
  0.3, 0.7, 2.0 and 0.0, 0.1, 0.3, 1.5, 3.5 in two runs after. Key to
  speech queued, release, 0.0, 0.1, 2.4, 3.3, 7.6 before, then 0.0, 0.1,
  3.5, 4.2, 10.1 and 0.0, 0.1, 2.9, 3.8, 9.7. These differences are within
  the run-to-run variation the tooling guide describes.
- `conhost_flood`: the console now takes its reads from console updates
  and drops MSAA events from UIA windows early (19533f4), and the flood is
  processed so much faster that the earlier window (8 seconds in, 6 long)
  caught none of it after. Over the whole scenario, items taken in fell
  from about 80,300 to 40,000 and items handled from about 38,600 to
  between 6,300 and 10,600 (three runs). A 10-second window over the
  second flood burst after held 89,917 wake-ups and 10.1 s of processor
  time in Verbatim, and 96,261 wakes and 7.8 s in the console host; the
  earlier 6-second window held 21,519 and 7.2 s, a different part of the
  flood. Latency, debug: summed hops and reducer wait 0.1, 0.3, 2.3, 2.5,
  2.7 before and 0.2, 0.3, 1.0, 1.5, 1.7 after; release after 0.0, 0.1,
  0.4, 0.8, 2.6.
- Real speech (`measure_speech_real`, audible, 8 seconds from 20 seconds
  in): 9,371 wake-ups and 835 ms before, 9,918 and 878 ms after, within
  the synthesizer thread's known variation; `audiodg.exe` woke 3,021 times
  and used 275 ms in the window. Latency, debug: verbatim.exe's stage 0.0,
  0.1, 1.8, 3.7, 5.8 before and 0.0 throughout after; summed hops and
  reducer wait 0.2, 0.5, 2.4, 5.5, 6.4 then 0.2, 0.4, 0.9, 2.1, 5.7; key
  to speech queued 0.0, 0.3, 3.8, 9.1, 11.7 then 0.1, 0.2, 2.8, 6.0, 13.1;
  first audio 2.1, 19.2, 26.4, 37.9, 69.1 then 2.3, 18.6, 32.1, 44.0,
  72.5. Release after: first audio 1.3, 17.3, 27.3, 47.7, 78.8, against
  1.4, 17.8, 24.3, 35.3, 77.8 in step 2's release run, before the audit.

The added workloads, 698b9807 then main, debug, one run each unless
said (few latency samples each, so one focus decides a maximum):

- `measure_focus_windows_terminal` (5 seconds in, 7 long): 7,587 wake-ups
  and 746 ms before; after, 7,620 and 686 ms in one run and 2,215 and 367
  ms in another, so the window's phase decides it and no change is shown.
  Listener stage 13.9 and 41.1 ms before, 15.3 and 34.0 after (release
  4.9 to 20.8); first audio 1.9, 3.6, 61.3, 61.3, 61.3 then 2.3, 16.0,
  64.5, 64.5, 64.5.
- `measure_keys_windows_terminal` (15 in, 8 long): 7,236 wake-ups and
  1,358 ms before, the outpost 2,732 and 617 ms; 6,776 and 1,178 ms after,
  the outpost 2,351 and 442 ms. The worker's wakes fell from 1,175 to 809
  and its processor time from 318 to 267 ms. Windows Terminal itself used
  4.4 s in both. Outpost read 0.0, 2.2, 2.8, 6.1, 22.1 then 0.0, 2.4, 3.1,
  3.9, 15.8; summed hops and reducer wait 0.2, 0.3, 0.5, 1.1, 4.1 then
  0.2, 0.4, 0.5, 1.5, 18.2 (the one 18.1 ms wait for the reducer; release
  after 0.0, 0.1, 0.2, 0.8, 5.3).
- `settings_system_page` (5 in, 3 long): 1,120 wake-ups and 177 ms
  before, 1,008 and 174 ms after. The listener stage's P50 was 6.3 ms
  before and 56.7, 6.0, 16.4 and 9.4 ms in four runs after: it varies
  this much from run to run on five samples, so no change is shown.
- `notepad_say_all` (5 in, 3 long): 851 wake-ups and 177 ms before, 1,097
  and 175 ms after (Notepad itself 327 then 1,538 wakes, in a run that
  failed its assertion, below). First audio P50 16.9 then 6.5 ms.
- `notepad_review_cursor` (5 in, 5 long): 1,263 and 258 ms before, 1,490
  and 276 ms after; first audio 0.7, 2.0, 75.9, 159.4, 159.4 then 0.7,
  2.0, 62.6, 131.4, 131.4.
- `object_navigation_over_uia` (5 in, 6 long): 1,437 and 289 ms before,
  1,277 and 285 ms after; summed hops P99 1.5 then 1.0 ms.
- `menu_and_settings_dialog` (6 in, 8 long): 4,978 and 510 ms before,
  4,594 and 500 ms after, most of it verbatim.exe's own GUI thread (1,202
  then 1,091 wakes) and its unnamed threads, since the dialog is
  Verbatim's.
- `system_information_tree` (6 in, 5 long): 1,077 and 237 ms before,
  1,066 and 241 ms after; latency unchanged.

Once the application is quiet, the synthesizer thread (about 150 to 300
ms per window in debug) and the audio thread are the largest of
Verbatim's processor users in every one of these.

### Idle battery checks

- No WASAPI stream runs while Verbatim is silent. The mixer stops the
  stream (`IAudioClient::Stop` and `Reset`) once what was written has
  played, then waits with no limit. Idle, Verbatim's audio thread did not
  wake in 8 seconds, and `audiodg.exe` woke 6 to 7 times with Verbatim
  idle and 8 times with Verbatim not running (measured). `powercfg
  /requests`, which would list an audio stream in use, needs elevation
  and was not run.
- Timer resolution: `powercfg /energy` needs elevation and was not run.
  The global timer resolution was read instead, without elevation,
  through `NtQueryTimerResolution`, which reports the finest resolution
  any process holds: 15.625 ms before Verbatim started, with it idle, and
  after it stopped, and 15.625 ms in all 339 samples taken every 200 ms
  through an audible `measure_speech_real` run. So no Verbatim process
  raised the timer resolution, idle or speaking (measured; attribution to
  a process would need `/energy`). The outposts' caret watch uses a
  high-resolution waitable timer (`alarm.rs`), which does not change the
  global resolution and is armed only during a watch.

### Optimization opportunities, largest first

1. The focus listener's desktop-wide UI Automation subscriptions
   (measured in File Explorer). Its unnamed UI Automation threads woke
   15,848 times and used 510 ms in 8 seconds, 64 percent of Verbatim's
   wake-ups and 41 percent of its processor time there, readied from
   `explorer.exe` through `win32k` (the top three stacks 2,456, 1,398 and
   1,368 times). Their samples are UI Automation's client-side event
   dispatch (`uiautomationcore.dll`, with `vswprintf` among it) with no
   Verbatim code on the stack, so every desktop-wide registration costs
   this per event whatever its handler does. Narrowing the listener to the
   events it needs and leaving the rest to each application's outpost
   would save up to that share in focus-heavy use (inferred: most of the
   15,848 wakes and 510 ms per 8 seconds).
2. Out-of-context `WinEvent` intake in a console flood (measured). The
   console outpost's event thread woke 43,853 times and used 3.05 s in 10
   seconds, readied by the console host's message and driver threads, 49
   percent of Verbatim's wake-ups. Of its samples with Verbatim's code on
   the stack, 578 of 1,081 are `tracing` dispatch, the test runs' debug
   logging, which a user does not pay; the rest of its time is the kernel
   delivering each event. Only filtering inside the application (D2's
   injection helper) removes the wake: up to about 4,400 a second while a
   console floods.
3. The per-message chain from an outpost to the reducer (measured in the
   flood). The outbound thread woke 3,158 times (384 ms), verbatim.exe's
   reader 3,091 (471 ms) and the reducer 2,993 (586 ms) in 10 seconds,
   each readied by the one before it once per message. Steps 6 and 7
   remove the outbound thread's hop (about 3,150 wakes and 380 ms there);
   decision 3's alternative, the reducer reading the pipes, would remove
   the reader's too (about 3,100 wakes and 470 ms more). In the key
   workloads the chain is about 200 wakes per thread per 8 seconds.
4. The console outpost's worker and UI Automation callbacks in a flood
   (measured). The worker woke 11,998 times and used 2.88 s, readied 1,508
   times by UI Automation's own thread (waits of half a millisecond or
   more), its samples led by `CoreAutomationRemoteOperation::Execute`
   (135) and string handling (`char::decode` and reverse searches, about
   250 together; since 2026-10-13 a screen's rows lose their padding on
   the UTF-16 text before the rest is decoded, and a terminal's caret
   that has not changed is not sent to Core again); the callbacks woke 9,776 times and used 1.57 s, in the
   text event handler (`register_focus_events` 414, `Context::take_in`
   267) and in `GetRuntimeId` and `GetCachedPropertyValue`. Reading once
   per burst of console updates rather than per event is the lever
   (inferred); the console host spent 7.8 s delivering the events in the
   same window.
5. The worker's cross-process calls while typing (measured in the text
   box). The worker woke 3,354 to 3,589 times per 8 seconds and used about
   170 ms, readied from the application (the Windows Forms host
   `powershell.exe`, 707 on the top stack) and from `ctfmon.exe` (217, 154
   and 108 on three stacks), about seven wakes per key. Remote operations
   for each caret and echo read, and the open question of why `ctfmon.exe`
   answers the worker, are the levers (inferred: about half the worker's
   wakes).
6. The synthesizer thread's resampling (measured in debug, inferred for
   release): 250 to 300 ms of processor time per 8 seconds of typing or
   speech, its samples led by `rubato`'s sinc interpolation
   (`get_sinc_dot_product_unsafe` 30, `asynchro_sinc` 13) and debug-only
   bounds checks (`from_raw_parts::precondition_check` 25). No release
   profile of the thread was taken; a cheaper resampler, or eSpeak at the
   device's rate, would remove most of it if release has the same shape.
7. A key's way to the reducer (latency, measured): "hook to Core" has P95
   2.9 to 3.6 ms and P99 3.8 to 5.1 ms, 10 to 14 percent of keys over 1 ms
   in both builds before and after, the largest share of a key's way to
   its speech queued. No stamp separates the router from the reducer's
   wait. The hook thread also readies the speech queue on about every key
   (94 to 117 times per 8 seconds), which step 10 removes.
8. The listener's UIA focus read before the fact is sent (latency,
   measured): P50 11 to 13 ms, up to 72 ms in debug and 365 ms in release
   for a File Explorer focus, inside the listener stage. A stamp at the
   listener's write would show how much is the read; reading less there,
   or in the outpost as the text box's path does, is the lever.
9. verbatim.exe's GUI thread (measured): 109 to 234 wakes per window with
   no Verbatim window open in the settings, Notepad and terminal
   workloads, broadcasts to its hidden top-level frame (decision 17).

### Failures and tooling notes

- `notepad_say_all` fails intermittently on both builds, so the failure
  is not from this work: after Control interrupts say-all and the right
  arrow is pressed, the first utterance is "M" where "o" is expected
  (`registry.rs` line 1671). On main it failed in both release runs and in
  one of three debug runs (logs under
  `target\e2e-artifacts\history\notepad_say_all`, the runs
  `2026-10-13T12-03-56.805Z-fail`, `2026-10-13T12-05-18.934Z-fail` and
  `2026-10-13T12-25-14.736Z-fail`); on 698b9807 it failed once and passed
  once.
- `system_information_tree` failed once at its start with "Start toggle
  button not pressed", after windows that began 3 to 4 seconds into the
  short scenarios before it: the hazard described above. It passed after
  File Explorer was restarted and those windows were moved to start 5 or
  6 seconds in, and the added workloads were then traced again on both
  builds.
- `verbatim-trace-waits --wakers` matches a bare thread name across every
  process and ignores a `<process.exe>:` prefix, which the tooling guide
  says it accepts, so such a request prints nothing, and unnamed threads
  cannot be asked for at all; `--stacks` and `--samples` honour the
  prefix.
- The listener stage has no stamp at the listener's write, which step 5's
  second criterion needs before it can be judged.

## Measurements after the terminal and typing changes (2026-10-14)

A measurement batch on main at 475fdfce, on this development machine,
with NVDA not running (it was not running when the batch began and was
left so). It compares with "Measurements after communication step 5
(2026-10-13)" above, the baseline, and covers what changed since 12ac774a:
a terminal's caret read once per burst of output (69378e34), a window's
class asked once per window on the outpost's event thread (3f0b8002), the
console host's caret taken from `EVENT_CONSOLE_CARET` instead of UI
Automation's caret events (2a9de42a), the read that covers a caret event
answering a caret key's watch (6b0bf0d4), an unchanged caret not sent
again and rows trimmed before decoding (713a4dcc), kept remote programs
(475fdfce), and the new ledger stamps (a48099ea, cd0c2308, 19cf75e0).
Nothing in the product changed for these measurements.

### How it was measured

- Builds: debug and release from 475fdfce, built before anything ran;
  nothing was built while a scenario or trace ran.
- Latency: `cargo xtask measure`, debug and release, for the measure set,
  and with `--scenario` for `conhost_flood` and `windows_terminal_flood`;
  `conhost_flood` was measured twice in each build, since its first run
  showed a longer wait for the reducer. Every figure is minimum, P50, P95,
  P99 and maximum in milliseconds over every announcement's latency line,
  nearest rank, computed from the runs' `stderr.log` by a scratch script as
  the baseline's were (the baseline's own `stderr.log` files, kept under
  `target\perf`, were read again with the same script). The ledger rounds
  to 0.1 ms.
- Wake-ups and processor time: one trace per workload and build, with the
  baseline's windows: idle 12 seconds in for 8; `measure_keys_text_box`,
  `measure_keys_conhost` and `measure_keys_windows_terminal` 15 in for 8;
  `measure_focus_windows_terminal` 5 in for 7; the floods a 10-second
  window over the second burst, 54 seconds in for `conhost_flood` and 56
  for `windows_terminal_flood`, checked against each run's log (the
  second burst began 2 to 3 seconds after the trace command, and the
  recording starts about a second after the command). Each trace was read
  for Verbatim's processes and the target applications, with wakers and
  readying stacks for the six most-woken of Verbatim's threads and every
  unnamed outpost thread (UI Automation's), and samples for the three
  busiest; the harness's own threads are left out of the totals. Every
  trace was deleted once read.
- Corrected later on 2026-10-14: in `conhost_flood` that window falls on
  the scenario's floods 2 and 3, which run with "Report new output" off,
  so Core never holds the outpost and it reads on every change (about
  2,150 reads per run). Most of that window's wake-ups and processor time
  are that mode's cost, not the default's; a user with the default pays
  what floods 1 and 4 cost.
- Corrected later on 2026-10-14: both builds logged at debug level, since
  the suite's default filter (`DEFAULT_RUST_LOG` in
  `crates/verbatim-e2e/src/scenario.rs`) sets `verbatim_outpost` and
  `verbatim_ia2::hook` to debug: about 111,000 lines per `conhost_flood`
  run, about 20 percent of the event thread's processor time. Users run at
  info level, at which the flood logs 2 lines. `docs/tooling/measuring.md`,
  "Measuring performance", now says to trace at info level and on the
  default floods.
- Whole-scenario work for the floods: `intake pushed` and `worker: handled`
  lines in the outposts' logs, over every run of the day kept in
  `target\e2e-artifacts\history`.
- `measure_keys_conhost` and `windows_terminal_flood` had no trace or flood
  latency in the baseline, so for them only the after figures are given.

Everything below is measured unless marked inferred.

### Correctness

- Run once on 475fdfce: the first 22 `conhost_` scenarios in alphabetical
  order (`conhost_commands` to `conhost_scrollback_overflow`); the other
  20 `windows_terminal_` scenarios (close tab, control flood, footer
  flood, history flood, history flood during group, leave flood, long
  lines, pager, progress, raised flood, redraw limit, redrawn list, review
  grid, same flood, scrollback overflow, scrollback overflow during group,
  shift flood, short output, tabs, two windows); and `text_box_editing`,
  `text_box_word_selection` and `text_box_typed_words`. All 45 passed the
  first time, so nothing was rerun.
- Not run again, on Dickson's instruction, because they had passed on the
  final code in the previous session's runs: the other eight `conhost_`
  scenarios; the `windows_terminal_` typing, editing, up typing, line key
  flood, review output, flood, commands, spoken password, full screen and
  marker list scenarios; and `notepad_editing`, `notepad_word_selection`
  and `notepad_typed_words`.
- In the measurement runs, `conhost_flood` passed six more times,
  `windows_terminal_flood` four times (the doubled "terminal" at focus did
  not recur), and the measure set twice. `measure_focus_explorer` failed
  once in ten runs, at its start, with "the utterance "Desktop list" was
  expected to end Completed, but ended Cancelled": the File Explorer focus
  hazard below, with "Start toggle button not pressed" spoken first (log
  under `target\e2e-artifacts\history\measure_focus_explorer`, run
  `2026-10-13T17-00-17.486Z-fail`). It passed after File Explorer was
  restarted. `notepad_say_all` was not run.

### Caret keys in the console host

`EVENT_CONSOLE_CARET` comes on the cursor's blink timer, so a caret key
whose speech waited for it would wait up to about half a second. None did.

- In `measure_keys_conhost` the outpost answered 81 caret-key queries per
  run. In the debug run three of them opened a watch, each answered after
  1 to 3 ms; in the release run two, each after 1 ms. The rest knew the
  caret before the key and were answered by the screen read that followed
  it.
- The latency lines of the caret keys (those with "hook to Core"), as
  printed to speech, before then after: debug 1.6, 2.3, 5.1, 8.9, 16.0
  (77 keys) then 1.5, 2.9, 5.8, 9.8, 17.4 (79); release 1.0, 1.4, 3.4,
  4.0, 4.2 (79) then 1.0, 1.3, 4.8, 7.7, 15.6 (80). To sound: debug 2.3,
  4.0, 22.9, 27.9, 30.1 then 2.2, 3.6, 23.3, 29.0, 30.9; release 1.3,
  2.0, 27.2, 31.4, 31.6 then 1.3, 6.6, 25.8, 26.9, 28.6. The release tail
  is the reducer and the speech queue, not the caret: the 15.6 ms key
  spent 11.7 ms in "to speech", and the others over 5 ms waited 1.4 to
  4.4 ms for the reducer while the script's output was handled. The caret
  wait stage had two or three samples per run, 1.4 to 3.7 ms.
- Across the day's terminal runs, the only watches answered after more
  than 5 ms were in `conhost_pager` (438 ms), `windows_terminal_pager`
  (504 ms) and `conhost_redrawn_list` (206 ms). They are as long in
  Windows Terminal, which does not use the console's caret event, and the
  console host's watch was answered by the pager's layout update, not by
  a caret event, so they are the scenario's own waits for the pager to
  redraw, not the blink timer (inferred). The three earlier runs of each
  in the history (431 to 442 ms and 206 to 208 ms) may already have had
  the change, so no clean before exists for them.

### The hops to speech and their cost

Over the measure set, before then after. The new stamps split two of the
baseline's stages, so where a stage is new only the after figure exists.

The cross-process hops:

- The listener's pipe to verbatim.exe (new): debug 0.1, 0.2, 0.2, 0.3, 0.3
  (22 facts); release 0.0, 0.0, 0.1, 0.1, 0.1 (12).
- verbatim.exe to an outpost: debug 0.1, 0.2, 0.3, 0.7, 2.5 then 0.1, 0.2,
  0.3, 0.6, 0.7; release 0.0, 0.0, 0.1, 0.1, 0.7 then 0.0, 0.0, 0.1, 0.2,
  1.9.
- An outpost's pipe to verbatim.exe (new; its writer's write to
  verbatim.exe's reader having read and parsed the message): debug 0.1,
  0.1, 0.2, 0.2, 6.2; release 0.0, 0.0, 0.1, 0.1, 0.3. The baseline's
  whole "outpost to verbatim.exe", which also held the outbound queue:
  debug 0.1, 0.2, 0.3, 0.4, 10.0; release 0.0, 0.1, 0.1, 0.2, 0.7.
- To the synthesizer host: debug 0.0, 0.0, 0.1, 0.2, 1.1 then 0.0, 0.1,
  0.1, 0.2, 2.6; release 0.0, 0.0, 0.1, 0.1, 0.2 then 0.0, 0.0, 0.1, 0.2,
  1.7. The driver now stamps as its write begins, so the write moved into
  this stage from "synthesizer request", whose P99 fell from 0.5 and 0.2
  ms to 0.0.
- Back from the synthesizer host: debug 0.0, 0.1, 0.3, 0.6, 4.8 then 0.0,
  0.0, 0.1, 0.2, 28.0 (one sample); release 0.0, 0.0, 0.1, 1.7, 2.5 then
  0.0, 0.0, 0.0, 0.1, 1.6. The reader's hand-off to the synthesizer thread
  is now a stage of its own (below), which is why P99 fell.

The in-process stages between them:

- Listener handling (new): debug 0.0, 21.5, 84.1, 130.9, 130.9; release
  0.0, 13.7, 79.3, 80.5, 80.5. It is the reading thread's focus read, as
  the baseline inferred: the queue and the pipe after it add at most 0.5
  ms. The baseline's whole listener stage was debug 0.1, 12.7, 47.9, 72.4,
  72.4 and release 0.1, 11.2, 86.0, 365.3, 365.3, over 14 facts of fewer
  kinds, so the two are not comparable.
- Listener queue (new): 0.0 in every sample of both builds.
- verbatim.exe's own stage: debug 0.0, 0.0, 0.0, 0.2, 0.2 before and
  after; release 0.0, 0.0, 0.0, 0.1, 0.1 then 0.0, 0.0, 0.0, 0.0, 0.1.
- Outpost outbound (new): debug 0.0, 0.1, 0.1, 0.2, 0.7; release 0.0, 0.0,
  0.1, 0.1, 0.7.
- Waiting for the reducer: debug 0.0, 0.1, 0.5, 2.4, 18.1 then 0.0, 0.1,
  0.4, 2.2, 8.8; release 0.0, 0.0, 0.3, 2.0, 6.3 then 0.0, 0.0, 0.2, 1.2,
  16.2 (one focus in `measure_focus_windows_terminal`).
- Host reader to synthesizer thread (new): debug 0.0, 0.0, 0.2, 0.5, 4.3;
  release 0.0, 0.0, 0.1, 0.6, 5.9.
- Summed per announcement, verbatim.exe's stage, its hop to the outpost,
  the outpost's outbound queue and pipe, and the reducer wait: debug 0.1,
  0.3, 0.8, 2.8, 18.2 then 0.1, 0.3, 0.7, 2.7, 8.9; release 0.0, 0.1, 0.4,
  2.0, 6.3 then 0.0, 0.1, 0.3, 1.2, 16.3.

The rest of the way, before then after: hook to Core debug 0.1, 0.2, 2.9,
5.1, 9.3 then 0.1, 0.2, 2.6, 5.0, 12.6, release 0.0, 0.1, 3.6, 4.1, 5.5
then 0.0, 0.1, 1.9, 5.1, 12.8; the outpost queue debug P99 2.8 then 2.5
(maximum 67.2 then 34.8), release P99 1.6 then 1.2; the outpost read
debug 0.0, 1.9, 3.3, 12.2, 28.9 then 0.0, 1.5, 3.1, 20.8, 47.6, release
0.0, 1.1, 2.0, 20.8, 79.4 then 0.0, 1.1, 1.8, 19.3, 86.8 (the tails are
focus reads); to speech P99 3.3 then 4.0 in debug, 2.7 then 2.8 in
release.

End to end, every announcement of the measure set, as the ledger prints
it: to speech, debug 0.1, 2.8, 6.1, 27.4, 101.3 then 0.1, 2.3, 6.1, 27.9,
151.0, release 0.0, 1.5, 5.2, 25.7, 445.3 then 0.0, 1.5, 5.2, 23.7,
123.7; to sound, debug 0.9, 3.9, 25.1, 74.0, 180.3 then 0.8, 3.3, 25.6,
62.7, 245.6, release 0.3, 2.1, 27.2, 74.6, 445.8 then 0.3, 2.0, 29.1,
79.6, 204.1. The maxima are speech waiting behind earlier speech and File
Explorer focus reads, which vary from run to run.

### Per workload, before and after

Wake-ups and processor time are for one trace window across Verbatim's
processes, harness threads left out, debug unless said.

- Idle: before 45 wake-ups and 3.8 ms (debug) and 48 and 2.1 ms
  (release); after 48 and 4 ms, and 44 and 2 ms: unchanged, the pings. The
  global timer resolution was 15.625 ms before Verbatim started, while it
  ran and after it stopped, in both builds.
- `measure_focus_windows_terminal`: before 7,620 wake-ups and 686 ms and,
  in another run, 2,215 and 367 ms; after 5,490 and 590 ms; release after
  7,895 and 492 ms. The window's phase decides it, as in the baseline, so
  no change is shown. Summed hops and reducer wait, debug, 0.3, 0.4, 1.0,
  1.0, 1.0 then 0.2, 0.5, 0.9, 0.9, 0.9.
- `measure_keys_text_box` (the MSAA path, which the changes did not
  touch): before 9,291 and 890 ms and 10,393 and 922 ms; after 9,976 and
  1,000 ms; release after 10,197 and 532 ms. The worker woke 3,365 times
  (185 ms) against 3,354 and 3,589: within the variation of runs. Summed
  hops and reducer wait debug 0.2, 0.4, 0.9, 2.1, 3.2 then 0.2, 0.4, 0.7,
  2.8, 4.8; release 0.0, 0.1, 0.3, 0.7, 2.0 then 0.0, 0.1, 0.3, 0.4, 0.7.
  Key to speech queued, release, 0.0, 0.1, 3.5, 4.2, 10.1 then 0.0, 0.1,
  0.6, 5.7, 17.7.
- `measure_keys_conhost` (no trace before): after, 5,625 wake-ups and 989
  ms in debug and 5,230 and 477 ms in release, most of it the speech path
  (the audio and synthesizer threads, 1,875 wakes and 424 ms in debug);
  the console outpost's worker 475 wakes and 187 ms, its event thread 406
  and 40 ms, UI Automation's threads in it 239 and 18 ms; the console host
  itself 3,521 and 333 ms. Latency, before then after: the outpost read
  debug 0.0, 1.7, 2.3, 5.8, 15.7 then 0.0, 1.3, 1.6, 3.7, 16.7, release
  0.0, 1.0, 1.3, 5.0, 19.6 then 0.0, 0.9, 1.2, 6.4, 16.8; summed hops and
  reducer wait debug 0.1, 0.3, 0.9, 3.2, 10.7 then 0.1, 0.2, 0.8, 3.6,
  8.9, release 0.0, 0.1, 0.5, 2.4, 6.3 then 0.0, 0.0, 0.4, 2.1, 8.0; to
  speech debug 0.5, 2.4, 6.3, 13.3, 101.3 then 0.5, 1.9, 5.1, 14.1, 107.7,
  release 0.1, 1.3, 4.5, 8.8, 90.2 then 0.1, 1.2, 3.0, 10.2, 123.7.
- `measure_keys_windows_terminal`: before 6,690 and 1,139 ms and 6,776 and
  1,178 ms; after 6,224 and 1,064 ms; release after 6,073 and 544 ms. The
  worker woke 655 times and used 216 ms, against 788 to 809 and 267 to
  269 ms. The outpost read debug 0.0, 2.4, 3.1, 3.9, 15.8 then 0.0, 1.9,
  2.4, 3.3, 18.5, release 0.0, 1.5, 1.9, 2.2, 20.3 then 0.0, 1.4, 1.7,
  3.1, 18.0; summed hops and reducer wait debug 0.2, 0.4, 0.5, 1.5, 18.2
  then 0.1, 0.3, 0.5, 1.0, 5.2, release 0.0, 0.1, 0.2, 0.8, 5.3 then 0.0,
  0.1, 0.2, 0.5, 5.4.
- `conhost_flood`, the 10-second window over the second burst: before
  89,917 wake-ups and 10.08 s in Verbatim, after 56,848 and 6.51 s (37
  and 35 percent fewer); the console host 96,261 and 7.79 s, then 93,470
  and 6.66 s. By thread, before then after: the event thread 43,853 and
  3.05 s, then 38,235 and 2.57 s; the worker 11,998 and 2.88 s, then
  9,562 and 2.33 s; UI Automation's own threads 12,701 and 737 ms, then
  1,542 and 161 ms; the UI Automation callback thread 9,776 and 1.57 s,
  then it did not run in the window; the outbound thread 3,158 and 384 ms,
  then 1,886 and 273 ms; verbatim.exe's reader 3,091 and 471 ms, then
  1,751 and 328 ms; the reducer 2,993 and 586 ms, then 1,596 and 436 ms.
  Release after: 85,449 wake-ups and 4.81 s, the event thread 55,037 and
  2.27 s and the worker 16,599 and 1.55 s (a faster worker takes smaller
  batches, so it wakes more), the console host 118,509 and 7.80 s.
- `conhost_flood` over the whole scenario, from the outposts' logs: items
  taken in fell from about 40,000 to between 22,900 and 23,800 (five runs
  after). Items handled rose from between 6,300 and 10,600 to between
  12,300 and 13,700: the 3,200 to 5,100 UI Automation caret items per run
  are gone (28 console caret items instead), while the console updates
  are planned in smaller batches (3,400 to 5,400 batches before, 12,200 to
  12,700 after) and each is handled. The screen reads are of the same
  order (815 to 1,369 before, 1,076 to 1,734 after), and the separate
  caret reads fell from 2,945 to 4,615 per run to 20 or 21. In the
  worker's samples with Verbatim's code innermost, UTF-16 decoding and
  reverse searches came to about 66 against about 250 before, in windows
  of the same phase.
- `conhost_flood` latency, before then after (two runs after in each
  build): summed hops and reducer wait debug 0.2, 0.3, 1.0, 1.5, 1.7 then
  0.1, 0.3, 1.5, 2.8, 4.2 and 0.2, 0.3, 1.0, 1.8, 3.1; release 0.0, 0.1,
  0.4, 0.8, 2.6 then 0.0, 0.1, 1.4, 4.8, 6.3 and 0.0, 0.1, 2.1, 8.9, 21.3.
  Nearly all of it is the wait for the reducer: release 0.0, 0.0, 0.4,
  0.7, 1.6 before, then 0.0, 0.0, 1.4, 4.8, 6.3 and 0.0, 0.0, 1.6, 8.9,
  21.3, the longest on flood lines 2 and 3 at a burst's start and on typed
  characters. This was taken for a regression in the tail, over one
  baseline run and two runs after of about 70 announcements each. Corrected
  later on 2026-10-14, it is mostly an artefact: in the baseline the
  timelines of flood lines 2 to 4, spoken 300 to 700 ms after their events,
  had been evicted from the ledger by the burst's unspoken events and were
  never measured (the ledger now keeps a timeline whose speech is owed);
  the caret keys' maximum is one key varying (key 49 took 16.0 ms in the
  debug baseline); and over about 870 announcements the release reducer
  wait is P99 2.1 ms after against 2.5 before. What remains is a few
  unexplained reducer stalls of 7 to 25 ms coinciding with speech starting
  or being cancelled, to be traced in the next measurement batch. The
  outpost read in debug is as before: 0.5, 1.7, 2.2, 3.6, 15.8 then 0.4,
  1.4, 1.9, 2.9, 18.9 and 0.8, 1.4, 2.2, 13.2, 20.6.
- `windows_terminal_flood` (no baseline): 4,364 wake-ups and 785 ms in
  debug and 4,331 and 398 ms in release over the second burst, against
  29,435 wakes and 1.08 s in OpenConsole and 30,852 and 1.30 s in the
  flooding PowerShell. Over the whole scenario the outpost handled 218 to
  282 items, Windows Terminal throttling its own events as before. Summed
  hops and reducer wait debug 0.2, 0.4, 0.7, 0.7, 0.8, release 0.0, 0.1,
  0.3, 0.7, 2.0. In one debug run of four the window's first focus waited
  2,993 ms in the outpost queue: the worker had asked the arbitrator
  whether UI Automation owns a `Windows.UI.Input.InputSite.WindowClass`
  window, and that probe took 3,008 ms (`arbitration probed`, with
  `elapsed_ms=3008`, in `outpost-windowsterminal-8740.log` of history run
  `2026-10-13T16-34-57.069Z-pass`).

The wakers, across all threads: in the console flood the event thread was
readied by the console host's WinEvents (11,525 readyings through
`NtUserNotifyWinEvent` setting its wake bit, and 10,949 as the console
host left the USER lock inside `NtUserNotifyWinEvent`), the worker by UI
Automation's threads in its own process (1,457 waits of half a
millisecond or more) and by the event thread, UI Automation's threads by
the console host's pipe writes (1,517 of 1,539), the outbound thread by
the worker, verbatim.exe's reader by the outbound thread, and the reducer
by the reader (about 1,500 each, one per message). In the key workloads
the worker was readied by UI Automation's threads (122 and 230 waits) and
by the event thread or the UIA callback thread, UI Automation's threads
by the terminal (the console host or Windows Terminal, 216 to 439 waits),
the input hook by the agent's injected keys, and the speech queue,
synthesizer and audio threads by each other, as in the baseline.

### File Explorer attribution

`measure_focus_explorer` now runs 17 to 26 seconds, and its phases start
earlier and move from run to run, so the windows the task named
(`--after 10 --seconds 5` and `--after 16 --seconds 2`) caught the end of
the windows phase with the arrows, and the end of the scenario. Checked
against `timeline.txt`, `listener.log` and Verbatim's log, they were
replaced by traces started from Verbatim's live log: 2 seconds after
"Desktop list" for 4 seconds (the windows phase, about five new windows
opened and closed in it), and half a second after the first arrow's
"Inner 1 of 4" for 2 seconds (the arrows). Both were read with
`--symbol-server`.

In this scenario every new File Explorer window is its own `explorer.exe`
process (12 such processes per run, each with an outpost).

The windows phase, the listener's process (measured):

- UI Automation's three unnamed threads in the listener woke 3,846 times
  and used 143 ms; the listener's event thread 1,128 and 41 ms, its
  reading thread 105 and 12 ms. File Explorer's processes woke 46,442
  times and used 7.5 s.
- One thread (1,992 wakes, 89 ms) is UI Automation's WinEvent thread
  (`WinEventManager::ThreadProc`, which pumps with `PeekMessage`). For each
  WinEvent it runs `WinEventManager::StaticWinEventProc`,
  `PreWinEventProc`, `WinEventSourceMap::EventSourceIsUia`,
  `ClientProxyManager::GetCompleteProxyNode` and
  `ProviderEntryPoint::GetNativeUiaNode`. In 8 of its 22 samples with a
  user-mode stack, `GetNativeUiaNode` had thrown a C++ exception that was
  caught and reported (`CxxThrowException`, `RaiseException`,
  `wil::details::ReportFailure_CaughtException`,
  `wil::GetFailureLogString` with `vswprintf`): the `vswprintf` the
  baseline saw. That the exceptions come from windows that are closing or
  have no native provider is inferred.
- A thread-pool thread (1,776 wakes, 48 ms) runs
  `ClientEventManager::OpenShowWinEventCallbackThreadProc` and
  `HideCloseReparentWinEventCallbackThreadProc`, which call
  `EventManager::AddEventRecordGroup`, `DoCalldownsForGroup` and
  `DoCalldownsForGroup_ClientSideOptimized`,
  `LocalUiaNodeProxy::RealCrossProcess_EntireProcessAddEventGroup` with
  `ProtobufHelper::WriteCacheRequest`, and, as windows go,
  `EventManager::OnProviderRemoved` and `RemoveFromScopeCheckCache`, with
  `HwndUtils::HwndToRuntimeId` throughout. So for each window shown, UI
  Automation registers the listener's desktop-wide event group, with its
  cache request, inside the new window's process, across processes, and
  takes it out again when the window is hidden or closed. 202 of its
  top-stack readyings are the thread pool releasing a worker
  (`NtReleaseWorkerFactoryWorker`).
- The third (78 wakes here; 440 wakes and 31 ms in the arrows) is UI
  Automation's I/O thread (`OverlappedIOManager::IoThreadProc`,
  `ChannelBasedClientConnection::OnData`, protobuf parsing of responses),
  readied by File Explorer's pipe writes (422 of 423 in the arrows).
- Most readyings of the first two threads in their top stacks are not
  event deliveries but hand-offs of the window manager's lock: the thread
  was waiting for the USER critical section, and was readied by
  `ReleaseUserCriticalSection` as another thread left it inside
  `NtUserGetMessage`, `NtUserPeekMessage`, `NtUserPostThreadMessage`,
  `xxxClientCallWinEventProc`, `NtUserGetClassName` or `NtUserGetProp2`.
  The lock holders were File Explorer (157 of the WinEvent thread's 507
  top-stack readyings), the test agent (252, inferred to be the harness's
  own and absent for a user), and the listener's process itself (98:
  `NtUserGetProp2` 62 and `NtUserGetClassName` 36).

The arrows phase (measured): a thread-pool thread woke 3,140 times and used
88 ms in 2 seconds, in `OpenShowWinEventCallbackThreadProc` calling
`ClientEventManager::FindControlTypeInWindowNode`, which walks the shown
window's elements across processes (`InProcClientAPIStub::UiaNode_Find`,
`ElementFinder::Find`, `UiaNodeTraverser::Traverse`,
`UiaNodeFactory::CompleteNodeInfo`, `ProviderEntryPoint::GetNativeUiaNode`);
1,958 of its 2,077 top-stack readyings were File Explorer leaving the
USER lock. The WinEvent thread woke 1,670 times (50 ms), 153 of them
readied by a WinEvent being posted to it (`SetWakeBit` from
`xxxProcessNotifyWinEvent`), most of the rest lock hand-offs, 54 of them
from the listener's own `NtUserGetAncestor`. Looking for a control type in
each shown window is, inferred, how UI Automation raises events such as a
menu opening for windows whose providers do not raise them, which the
listener's desktop-wide menu-opened subscription asks for. Over the 2
seconds UI Automation's threads in the listener woke 5,250 times and used
169 ms, Verbatim 13,305 and 682 ms, File Explorer 40,604 and 4.1 s.

On Verbatim's side, other than the desktop-wide group:

- The per-window outposts cost more than the listener in the windows
  phase (measured): the outposts of the new windows' processes together
  woke 5,807 times and used 586 ms in the 4 seconds, against the
  listener's 5,434 and 222 ms. Each outpost lived until the scenario's
  end, after its window had closed (its log ends at Verbatim's shutdown).
  Corrected later on 2026-10-14, this is not a leak: their `explorer.exe`
  processes are still alive when Verbatim exits, idle outposts woke 4 to 6
  times per trace, and nearly all of the 5,807 wake-ups come from each
  outpost while its window is open (starting UI Automation, its
  subscription, and about 400 `WinEvent`s). Idle retirement already bounds
  the tail at 2 minutes. That the agent launches each folder in its own
  process is likely why there are 12 (inferred).
  verbatim.exe's `verbatim-retire` thread woke 236 times and used 54 ms in
  the same window.
- The listener's process's own window-manager calls (`GetProp`,
  `GetClassName`, `GetAncestor`) take the USER lock, and UI Automation's
  threads in the same process then wait for it (measured). Which of those
  calls are Verbatim's own and which UI Automation's (its
  `HwndToRuntimeId` calls `GetProp`) needs user-mode stacks for the
  readying thread, which these traces did not give (inferred).
- About three quarters of the UI Automation threads' samples had only
  kernel frames, in the trace's own stack walking (`RtlpWalkFrameChain`,
  `RtlpUnwindPrologue`, `RtlpLookupFunctionEntryForStackWalks`), so their
  processor time is overstated while traced (inferred).

### Remaining opportunities, largest first

1. The per-window outposts in File Explorer (measured: 5,807 wakes and 586
   ms in 4 seconds of windows opening and closing, more than the
   listener). Each new `explorer.exe` process gets a full outpost, and
   nearly all of its cost is while its window is open (starting UI
   Automation, its subscription, about 400 `WinEvent`s); the outposts left
   after their windows close are idle, their processes still alive, and
   idle retirement ends them after 2 minutes (corrected later on
   2026-10-14). Starting an outpost only when a process's window is
   focused or read would remove most of that (inferred).
2. Out-of-context WinEvent intake in a console flood (measured): the event
   thread still woke 38,235 times and used 2.57 s in 10 seconds in debug
   (55,037 times in release), readied by the console host's
   `NtUserNotifyWinEvent`; 132 of its 352 samples with Verbatim's code
   innermost are debug logging, which a user does not pay. Only filtering
   in the application's process (D2's injection helper) removes the wakes.
3. The wait for the reducer in `conhost_flood`: the regression first
   reported here is mostly an artefact of the ledger evicting the
   baseline's flood lines (corrected later on 2026-10-14; see the
   per-workload figures). A few reducer stalls of 7 to 25 ms coinciding
   with speech starting or being cancelled remain unexplained; what the
   reducer is doing then needs a trace with the reducer's samples over
   those moments (inferred).
4. The per-message chain from the outpost to the reducer (measured): the
   outbound thread, verbatim.exe's reader and the reducer still wake about
   1,600 to 1,900 times each per 10 seconds of flood, one per message;
   steps 6 and 7 of the communication design remove the outbound thread's
   hop.
5. The worker's batches in a flood (measured): console updates are now
   planned in about 12,500 batches of one or two items per run, three
   times as many as before, and each costs the worker a wake; the screen
   reads did not grow with them. Letting a burst's updates gather before
   planning would trade a little latency for those wakes (inferred).
6. The arbitration probe on the worker (measured once in four
   `windows_terminal_flood` runs): a 3,008 ms probe of a Windows Terminal
   input-site window held the window's first focus for 2,993 ms. Probing
   off the worker, or with a short time limit, removes the stall
   (inferred).
7. The listener's focus read (measured): listener handling P50 14 to 22
   ms, up to 131 ms, the largest single stage on a focus's way; the queue
   and pipe after it add at most 0.5 ms.
8. UI Automation's lock hand-offs in the listener (measured share,
   inferred cause): most wakes of its WinEvent and thread-pool threads in
   File Explorer are waits for the USER lock, some caused by the
   listener's own `GetProp`, `GetClassName` and `GetAncestor` calls;
   fewer window-manager calls in the listener's process would remove that
   share.
9. Unchanged from the baseline: the synthesizer thread's resampling, the
   hook thread readying the speech queue on every key, and verbatim.exe's
   GUI thread woken by broadcasts.

### Failures and tooling notes

- The File Explorer focus hazard (the focus on the Start button, "Start
  toggle button not pressed" spoken at the scenario's start) appeared in
  four of ten `measure_focus_explorer` runs, about 2.2 seconds in, twice
  in runs whose trace began 10 seconds in, so it does not come only from a
  trace over a scenario's start. The measure scenario passes anyway
  unless the hazard cancels its first utterance, which happened once.
  File Explorer was restarted three times.
- One release trace of `measure_focus_windows_terminal` named no
  verbatim.exe threads and gave an outpost's threads the names `sig` and
  `iconv`, names of other processes' threads; a second run read normally.
- The tooling guide's window for `measure_focus_explorer` (`--after 10
  --seconds 8`) no longer falls in one phase of it.
- `verbatim-trace-waits --stacks` prints the ten most frequent stacks, so
  the readying causes above are shares of those, not of every readying.

## Narrowing the outpost's hook (2026-10-14)

Approved by Dickson. An outpost hooked its application for a fixed set of
`WinEvent` kinds for the process's life. The event thread already dropped
every value, state, name, description, selection, text selection and
caret location event of a window known to be read through UIA, but only
after the event had woken it: in `conhost_flood` those were 57 percent of
the events the console's outpost took (30,888 of 53,783 over a whole run,
nearly all UI Automation's own `WinEvent` copies of its events), and UI
Automation raises those copies only while some client hooks them.

The change (`crates/verbatim-outpost/src/outpost/narrowing.rs`): while the
application's focus window and foreground window are both known to be read
through UIA, those seven kinds are unhooked
(`verbatim_ia2::suspend_kinds`, one raw event at a time within the one
hook set, as the keyboard hook thread hooks the console's events); as soon
as the focus, a menu opening, or the foreground is in a window read
through MSAA, or not yet decided, they are hooked again, before that event
is queued. Focus, foreground, menu opening, alert, tooltip, window shown,
window destroyed and the console's updates and caret stay hooked: the
worker uses them for UIA windows too. The event thread takes the focus
and foreground windows from its own hook's events, and from the
listener's routed facts until its hook delivers one; the worker nudges it
to decide again when it keeps a UIA verdict, and the reader when Core
changes the backend override. Selection and mark mode are untouched: the
console host's comes through `EVENT_CONSOLE_CARET`, Windows Terminal's
through UI Automation's text selection events, and neither kind is
narrowed; every review and marker scenario below passed.

What can be lost, and the evidence:

- An event of a narrowed kind raised by a window read through MSAA after
  the focus or foreground entered it but before the event thread took
  that focus or foreground event. Checked by hand in the console host's
  Find dialog (Control+F), with a separate logging hook on every event of
  the console host: the dialog raised one `EVENT_OBJECT_VALUECHANGE` on
  its edit before its foreground event, which the outpost did not
  receive; the hook was in again 0.1 ms after the foreground event
  reached the event thread, before the dialog's focus event, and every
  later event of the dialog arrived. Verbatim said "Find dialog", "Find
  what: edit Alt+n", "blank", the same in both trials, the second with
  the logging hook making UI Automation raise its copies. The lost value
  change was on an edit that was not yet the focus, which the worker
  reads with the focus anyway.
- Such events from a window read through MSAA that is neither the focus
  window nor the foreground window, which the worker's acceptance takes
  only from a topmost window, such as a menu or a drop-down list, whose own
  focus or menu-opening event hooks them again. Application support may
  later ask for events from any window (`RequestedEvents`, empty today);
  it will need those kinds kept hooked.

Measured, debug build, one trace each with the 2026-10-14 windows (10
seconds, 54 seconds in for `conhost_flood` and 56 for
`windows_terminal_flood`), harness threads left out, before (from
"Measurements after the terminal and typing changes") then after:

- `conhost_flood`: the event thread 38,235 wakes and 2.57 s, then 22,673
  and 1.52 s; Verbatim's processes 56,848 wakes and 6.51 s, then 36,663
  and 4.75 s; the worker 9,562 and 2.33 s, then 5,828 and 1.83 s; UI
  Automation's threads in the outpost 1,542 and 161 ms, then 1,371 and
  133 ms. Over the whole run the outpost received 23,705 `WinEvent`s
  (23,672 console updates, 25 console carets, 4 value changes before the
  hook was narrowed, 4 focus events) against 53,783; it was narrowed 18
  ms after its first focus event, once the worker had probed the window.
- `windows_terminal_flood`: Verbatim's processes 4,364 wakes and 785 ms,
  then 3,887 and 737 ms; the event thread did not wake in the window.
  Over the whole run the outpost received 7 `WinEvent`s (5 focus, a
  window shown and a name change before it was narrowed) against 249 to
  391 in the day's earlier runs. It was narrowed, hooked again for a
  focus in a window not yet decided, and narrowed again once the worker
  had probed the input site window, all within 60 ms of starting.

Correctness: `conhost_flood` and `windows_terminal_flood` passed in the
traced runs; the other 37 scenarios named for this change (every
`conhost_` and `windows_terminal_` scenario with review, marker, typing,
editing, commands, pager or flood in its name, `menu_and_settings_dialog`
and `system_information_tree`) were run once, and 36 passed.
`conhost_up_typing` failed, and failed again in three more runs: Up Arrow
and "x" pressed at once are now spoken "x", "echo onex" instead of
"echo onex" (`docs/parity/terminals.md`, "New terminal output" keeps Verbatim's single reading, Dickson,
2026-10-09). It passed twice on the same build without the change, and
three times with value changes left hooked. The caret key's answering
read now finds the cursor still before the "x" the shell had just
written, one column short, so the update after it reports the caret
moved and the reducer echoes the character; with value changes hooked,
UI Automation raises its copy of each text change inside the console
host, and the cursor had moved by the time of the read. Not fixed here:
the read racing the shell's redraw is the cause, and hooking value
changes only hid it.

Resolved on 2026-10-14. The race, from the failing runs' outpost logs
and flight recorder: the watch's one read (the caret, then a copy of it
expanded to its line and read) began as the request arrived, while the
console host was writing; the shell's redraw of the recalled line had put
the cursor at column 15, the read took the caret there, and the "x" was
written (its `EVENT_CONSOLE_UPDATE_SIMPLE` reached the outpost during the
read) before the line's text was read. A remote operations program is not
atomic: the console host takes its lock for each provider call, and
writes a character and moves its cursor under one lock between two of
them. The answer paired "ready> echo onex" with the caret at 15, a state
the console never had. Core did not speak it (a later key, the "x", had
been pressed), but kept it as its caret; the update's read then found the
caret at 16 on the same text, reported it, and the reducer took a caret
moving over the typed "x" on an unchanged line as typing the terminal
shows without changing its text (the rule for a space at a line's end and
for ghost text), echoing "x" before the line's change was spoken. With
the value changes hooked the read had come later, after the write.

Fixed by reading the caret again at the end of the watch's program
(`CaretQuery::recheck`, `CaretAnswer::moved_while_read`): when it moved
while it was read, the check answers nothing, as an edit control's read
by parts has done since 676d1275, and the update that follows the write
reads again and answers with the caret past the "x". Two provider calls
more inside the same round trip for each check, three more classically
(`docs/performance.md`). A mockapp test has the terminal write between
the program's calls (`write-on-read`) and failed without the change.
`conhost_up_typing` then passed three runs in a row, the third having met
the race and waited; every `conhost_` and `windows_terminal_` typing,
editing, line key and commands scenario, and `notepad_editing`, passed.
