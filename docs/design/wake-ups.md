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
  the elevated tasks (`docs/tooling.md`, "Measuring performance").
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
