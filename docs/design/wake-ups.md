# Wake-ups: what wakes Verbatim's threads, and why

Status: adopted; each source below is fixed or recorded as remaining.

Dickson asked for efficiency as well as latency, ideally as few wake-ups
as possible, and for root causes rather than mitigations (2026-10-13).
Raising thread priorities had cut some latency but made typed echoes
slower and hid a writer and an audio thread waking each other thousands
of times a second, so priorities were reverted and every thread's
wake-ups were counted and each source fixed where it starts. The rule
that follows is a standing one (`CLAUDE.md`, "Standing rules"): every
change is judged on wake-ups and processor time as well as latency. The
current figures per workload are in `docs/performance.md`; the
communication design's baseline is `docs/design/communication.md`,
section 18.

A wake-up is a thread switched in after waiting. Traces are taken and
read as `docs/tooling/measuring.md`, "Measuring performance", describes.

## The sources and what was done about each

The items keep their numbers, since code and the crate guides cite them.

1. The mixer's writer and audio thread woke each other every few frames
   while a synthesizer ran ahead of playback (about 75,000 synthesizer
   thread wake-ups in an 11-second typing run). Fixed at its source: a
   blocked writer waits for half its source's room, the audio thread
   wakes a writer only once that room is free, and a write wakes the
   audio thread only when the device needs frames (`room_wanted` in
   `crates/verbatim-audio/src/mixer.rs`).
2. Idle, Verbatim wakes about 50 times in 8 seconds across its
   processes: the three-second liveness pings to each child, and the
   outposts handling applications' own events (File Explorer raises a
   burst on the desktop's icon list every 3 to 4 seconds, which costs an
   outpost what it would cost NVDA). The audio thread's one-second poll
   for a reopen request is gone: the idle mixer waits with no limit, and
   a failed open's retry time ends the device's wait
   (`docs/crates/verbatim-audio.md`). No WASAPI stream runs while
   Verbatim is silent, and no Verbatim process raises the global timer
   resolution, idle or speaking (15.625 ms throughout, read with
   `NtQueryTimerResolution`).
3. Each outpost's watchdog woke to arm and disarm the deadline for every
   entry its worker took, about 1,000 times a second in a console flood,
   more than the worker itself. Fixed: the worker sets and cancels a
   waitable timer, which wakes no thread, so the watchdog wakes only when
   a deadline or grace passes (`docs/crates/verbatim-outpost.md`, the
   worker and its watchdog). The listener's reading threads' watchdog
   sleeps on the same kind of alarm, set as each read starts and as it
   reaches its bound, and cancelled once no read is under way, so it wakes
   only for a read still under way at its bound, grace or deadline.
4. Core answered each outpost message with its list of held nodes
   (`NodesHeld`), a round trip per message in a terminal flood. Fixed: a
   changed list waits in the outpost's writer and goes in the same write
   as Core's next command to that outpost, or with the next ping, so it
   never costs a write of its own and is at most one ping interval late
   (`docs/crates/verbatim-app.md`; `docs/design/communication.md`,
   step 11).
5. Every outpost message woke three threads before the reducer handled
   it. The transport of the communication design removed the outpost's
   outbound thread and Core's per-outpost readers; what remains is
   item 5 of "What remains" below.
6. The hosted synthesizer's driver, and OneCore in the host, waited in
   10 ms slices to notice a cancel, 100 wake-ups a second while
   speaking, through the whole of a OneCore utterance's synthesis.
   Fixed: a sink's `on_cancel` registers a waker the cancel calls, and
   the drivers wait on their replies and their cancel together
   (`docs/crates/verbatim-speech.md`, `docs/crates/verbatim-synth-host.md`).
   Waking at once exposed an ordering the poll had hidden, so the speech
   manager cancels the mixer before the job.
7. `WinEvent` hooks are out of context, so each wakes its thread for
   every event of its kinds before Verbatim's filter runs (in File
   Explorer, an outpost's event thread about 1,100 times a second). NVDA
   hooks about thirty kinds out of context from every process, so it
   pays the same per event. Only filtering inside the application before
   the event leaves it removes the wake, which is the injection helper
   of decision D2; until then each kind is hooked only where it is needed
   (item 13, and "Narrowing the outpost's hook" below).
8. UI Automation's own threads in the listener wake for every event its
   desktop-wide subscriptions deliver, in its client-side dispatch, with
   no Verbatim code on the stack. The threads that run Verbatim's
   handlers name themselves `verbatim-uia-callback`, so a trace tells
   them from UI Automation's other threads
   (`docs/crates/verbatim-uia.md`). The application's event rate decides
   this cost; it remains (item 3 of "What remains").
9. verbatim.exe's GUI thread wakes for messages Windows broadcasts to
   every top-level window, which its hidden frame and tray icon are,
   about 67 a second while typing elsewhere. A message-only window
   receives no broadcasts, but the tray icon and the menus need the
   frame; this remains a design question.
10. The keyboard hook thread hooked every console's events to learn
    which thread to translate typed keys with, so a console flooding in
    the background woke the thread Windows waits on for every key
    (47,061 times in 6 seconds). Fixed: the hook thread hooks no event;
    the focus listener learns the console's thread from its foreground
    event and Core publishes it (`docs/crates/verbatim-input-windows.md`;
    architecture section 5).
11. Threads that exist only for the test harness (the WAV recorder, the
    control plane's speech writers) wake often and cost a user nothing;
    figures leave them out.
12. Event handlers read properties their registration had not cached,
    and UI Automation answers such a read by raising and formatting an
    error inside itself: in a console flood the console's outpost used
    more than a processor, 3.5 seconds in 6 on UI Automation's callback
    threads, and reading only what was cached cut that to 0.83. The same
    pattern was found across the workspace, and Dickson chose to fix it
    systematically: `docs/design/uia-shapes.md`.
13. A console window read through UIA raised tens of thousands of MSAA
    events that woke the event thread and the worker only to be dropped
    at the window's UIA test. Fixed: the event thread drops an event
    whose window is already known to be read through UIA before it is
    queued, as NVDA drops it, keeping a console's updates and window
    destruction, which the worker uses for UIA windows too
    (`docs/crates/verbatim-outpost.md`, the event thread). The event
    thread still wakes for each event (item 7).
14. While typing, the outpost's worker wakes for each reply to its
    cross-process calls, from the application and from `ctfmon.exe`, the
    Text Services Framework's server, about seven wakes a key. Why
    `ctfmon.exe` answers a windowless worker is not known.
15. Once event reads were cheap, UI Automation delivered every event the
    console host raised (about 17,500 text changes and 17,000 caret
    changes in one footer flood, against about 160 each while callbacks
    were slow), and the console host spent so long raising them that its
    output slowed. Its console updates (`EVENT_CONSOLE_UPDATE_*`) tell
    the outpost of the same output more cheaply, and only they report a
    screen being cleared; its own caret event (`EVENT_CONSOLE_CARET`)
    comes on the cursor's blink timer, about every 530 ms, and for a
    selection's move at once, where the UIA caret event came with nearly
    every line. Decided with Dickson on condition of no regressions: the
    console host's text area takes the focus-following group without its
    text and caret events (`Group::ExtendedWithoutTextEvents`,
    `docs/crates/verbatim-uia.md`), its console updates drive its reads,
    and its console caret events are its caret events. NVDA, for a UIA
    console, listens to UIA text changes alone. Windows Terminal
    throttles its own UIA events and is unchanged.

A caret key in the console host cannot wait for the blink timer, so a
caret watch the first check leaves open re-reads the caret every 5 ms
until it finds the evidence, or 50 ms after the key: at most 10 worker
wake-ups a key, none while no watch polls (`docs/design/key-holding.md`,
"The console host's caret poll").

## Narrowing the outpost's hook

Approved by Dickson; amended on 2026-10-20, when the system caret's
location change left the narrowed kinds.

The problem: an outpost hooked a fixed set of `WinEvent` kinds for its
application's life, and the event thread dropped the value, state, name,
description, selection and text selection events of a window read
through UIA only after they had woken it. In a console flood those were
57 percent of the events the outpost took, nearly all UI Automation's own
`WinEvent` copies of its events, which UI Automation raises only while
some client hooks them.

The rule: while the application's focus window and foreground window are
both known to be read through UIA, those kinds are unhooked; as soon as
the focus, a menu opening, or the foreground is in a window read through
MSAA, or not yet decided, they are hooked again before that event is
queued. Focus, foreground, menu opening, alert, tooltip, window shown,
the system caret's location change, window destroyed, and the console's
updates and caret stay hooked, because the worker uses them for UIA
windows too, the system caret's events as a caret event on a UIA focus,
as NVDA handles them. Once a focus has raised a UIA caret event of its
own, and in a focused console host terminal, the intake drops the system
caret's events as they are pushed, until the next focus, since each
would otherwise cost a caret read that the UIA caret event repeats. The
mechanism is in `docs/crates/verbatim-outpost.md`, the event thread, and
`docs/crates/verbatim-ia2.md`, `suspend_kinds`.

The effect, in a console flood: the event thread's wake-ups fell by about
40 percent and the outpost received about 23,700 `WinEvent`s in a run
instead of 53,800; in a Windows Terminal flood it received 7.

What can be lost:

- An event of a narrowed kind raised by a window read through MSAA after
  the focus or foreground entered it but before the event thread took
  that focus or foreground event. Checked in the console host's Find
  dialog: one value change on its edit, raised before its foreground
  event, was not received; the hook was in again 0.1 ms after the
  foreground event, before the dialog's focus, and what was said was
  unchanged, since the worker reads the edit with the focus anyway.
- Such events from a window read through MSAA that is neither the focus
  window nor the foreground window, which the worker accepts only from a
  topmost window such as a menu or a drop-down list, whose own focus or
  menu opening hooks them again. Application support that asks for
  events from any window (`RequestedEvents`, empty today) will need those
  kinds kept hooked.

Narrowing exposed a race it had not caused: with value changes unhooked,
a caret watch's read could run while the console host wrote a character
and moved its cursor between two of its provider calls, pairing the new
text with the old caret, and the reducer then echoed the character
wrongly. A remote operation is not atomic against the provider, so the
watch's program reads the caret again at its end and answers nothing when
it moved; the update after the write reads again
(`docs/crates/verbatim-uia-rops.md`, `recheck` and `moved_while_read`).
Hooking value changes again only hid the race.

## What remains, largest first

Each is measured unless marked inferred.

1. Each new File Explorer process gets a full outpost, and nearly all its
   cost is while its window is open (starting UI Automation, its
   subscription, a few hundred `WinEvent`s): in a windows-opening
   workload the new windows' outposts together cost more than the
   listener. Starting an outpost only when a process's window is focused
   or read would remove most of it (inferred). Idle retirement ends the
   idle ones after two minutes.
2. Out-of-context `WinEvent` intake in a console flood still wakes the
   event thread for every console update; only filtering inside the
   application (decision D2's injection helper) removes it (item 7).
3. The listener's desktop-wide UI Automation subscriptions: for every
   window shown, UI Automation registers the listener's event group,
   with its cache request, inside the window's process, and looks for a
   control type in each shown window (inferred to serve the desktop-wide
   menu-opened subscription for providers that raise none). Many of its
   threads' wakes are hand-offs of the window manager's lock, some taken
   by the listener's own `GetProp`, `GetClassName` and `GetAncestor`
   calls, so fewer window-manager calls in the listener's process would
   remove that share.
4. A few reducer stalls of 7 to 25 ms in a console flood, coinciding with
   speech starting or being cancelled, are unexplained; they need a trace
   of the reducer's samples over those moments.
5. The relay and the reducer each wake once per outpost message in a
   flood (item 5).
6. Console updates are planned in batches of one or two, and each costs
   the worker a wake; letting a burst's updates gather before planning
   would trade a little latency for those wakes (inferred).
7. The listener's focus read is the largest single stage on a focus's
   way (median 14 to 22 ms); reading less there is the lever.
8. The worker's cross-process calls while typing (item 14).
9. The synthesizer thread's resampling (`rubato`'s sinc interpolation)
   is its largest processor cost, 250 to 300 ms per 8 seconds of speech
   in debug; release was not profiled. A cheaper resampler, or eSpeak NG
   at the device's rate, would remove most of it if release has the same
   shape.
10. The hook thread readies the speech queue on about every key;
    communication step 10, which would have removed it, is not adopted
    (Dickson, 2026-10-15).
11. verbatim.exe's GUI thread and broadcasts (item 9).

## Limits of the measurements

- A waker is the thread that was running when another was readied. When
  a thread is readied from an interrupt (a timer, an I/O completing), the
  waker named is whatever thread the interrupt fell on, so a waker in an
  unrelated process means a timer or a device.
- Tracing overstates processor time: about three quarters of UI
  Automation's threads' samples were the trace's own stack walking.
- The suite logs the outposts at debug level, about 20 percent of an
  event thread's processor time in a flood; users run at info level, so
  traces are taken at info level and on the default floods
  (`docs/tooling/measuring.md`).
- Tracing at a scenario's start or end has several times left File
  Explorer putting the focus on the Start button whenever the desktop is
  brought forward, until it is restarted. The cause is not understood,
  so traces are kept inside a scenario, away from its start and end.
