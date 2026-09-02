# Architecture review, 2026-09-02

Transient and untracked; `handoff-2026-09-02.md` names it. This is the
first-principles review that the code audit (`audit-2026-09-02.md`) was not.
It covers the four questions Dickson asked on 2026-09-02: how to define
foreground, whether the latency goal is reachable, whether Proxmox's
emulated audio device suffices, and how continuous integration gets real
Windows sessions. It ends with the decisions the review proposes, which are
not yet ratified.

The method was to reread `docs/architecture.md` and `docs/roadmap.md` in
full, walk the latency path through the code already read for the audit,
and check NVDA's event-acceptance rules (`nvda/source/eventHandler.py`,
`shouldAcceptEvent`) and NVDA's own CI (`nvda/.github/workflows/
testAndPublish.yml`) against the reference submodule.

## 1. What "foreground" should mean

### The problem

Core keeps one integer, the pid of the process that owns the foreground
window, and drops every event whose source pid differs (`verbatim-app`,
`is_current_foreground`). The listener's foreground fact sets it. That
single rule is behind every deferred scenario and it blocks a class of
behaviour NVDA has:

- Broker-hosted applications. A UWP or WinUI app's top-level frame belongs
  to `ApplicationFrameHost.exe`; the content, and the process that fires
  the focus event, is the app itself. The foreground pid is the broker's,
  the focus fact's pid is the app's, and the reducer drops the app's
  events. This is the Settings scenario.
- Windows of the already-running shell. A folder window opened by
  `explorer.exe` does not reliably take the foreground on a loaded machine,
  so no foreground fact arrives and its focus events are dropped. This is
  the Explorer scenario.
- Owned and topmost windows. Menus, combo box dropdowns, the task switcher,
  the Office ribbon's owned windows, and Edge's downloads pane are not
  descendants of the foreground window. NVDA accepts them by root owner
  and by the topmost style.
- Background events that should be spoken. Toast notifications, UIA
  notification events from an app that is not in front, alerts, and
  progress bar updates when the user has asked for them. NVDA speaks all
  of these without moving focus.

### NVDA's rule, for reference

`shouldAcceptEvent` is window-based, not process-based. It accepts an
event if its window is a descendant of the foreground window, shares the
foreground window's root owner, is topmost or has a topmost root, is a
`Windows.UI.Core` window under the input thread's active window, or is the
desktop window itself. Independently of foreground it accepts value
changes when background progress bars are enabled, alerts whose parent is
a toast window, shows for tooltip and notification bar classes, and
menu-end and desktop-switch events from anywhere. Everything else is
dropped before an object is even built.

### Proposal: an attention model, with acceptance in the outposts

Three changes, none of which alter the process model.

1. Attention follows focus, not window ownership. Replace the foreground
   pid with an attention record in the reducer state: the pid and root
   window of the process that most recently received a focus fact. The
   focus fact already carries the app's own pid (the listener reads UIA's
   cached process id, or the window's owner for MSAA), so a Settings page
   moves attention to the Settings process even though the foreground
   handle belongs to the broker, and an Explorer folder window moves
   attention when its list gets focus whether or not a foreground event
   ever fired. The foreground fact keeps its job of announcing the window;
   it stops being a gate.

2. Event acceptance moves out of Core and into the outposts, where the
   event's window is known and hang-safe local reads are available. Every
   event carries a window handle (the MSAA address has one; a UIA element
   has a cached one or resolves its nearest window in the outpost, which
   it does already for arbitration). The outpost classifies each event as
   attended or background using NVDA's rules with local calls only:
   descendant of the attention window, same root owner, topmost, the
   `Windows.UI.Core` case, plus the per-kind allowances (notifications,
   toast alerts, configured progress bars, menu popups, tooltip and
   notification bar classes). The classification travels on the event.
   Core needs to tell the outposts what the attention window is; that is a
   small broadcast on the existing command pipe whenever attention moves.

3. The reducer gets two policies instead of one gate. Attended events
   behave exactly as today. Background events never move focus or the
   navigator, are spoken at Queued priority so they never cut off what the
   user is doing, and are subject to a per-source flood cap: at most a
   small number pending per pid, oldest dropped, so a chatty background
   process cannot bury the foreground. That is the D9 blast-radius idea
   applied to speech rather than to hangs.

One consequence needs a decision. Outposts exist only for processes that
have had focus. A toast host or a background installer with a progress
bar may never have one. Two options: extend the listener's global hooks to
`EVENT_SYSTEM_ALERT` and a desktop-wide UIA notification registration,
forwarding them as facts that spawn an outpost on demand, which keeps the
listener's never-block rule since both are cached reads; or accept that
background speech comes only from processes that have held focus at least
once. The first is recommended for notifications and alerts, which are the
cases users notice, and the second is acceptable for progress bars, which
are configuration-gated anyway.

What this does to the three deferred scenarios: Settings and Explorer are
fixed by change 1 alone. Start menu search results report selection rather
than focus as the highlight moves; that is already handled when focus sits
on the container, and with attention on the Start process the selection
events are attended.

## 2. Latency

### Where the time goes today

The measured numbers on record are: event-observed to speech-queued of 1 to
8 ms on a lightly loaded VM in M3, before D13, when outposts stamped their
own hook callbacks; and a menu-open announcement of roughly 100 ms after
D13, measured from the keypress and therefore including the time Windows
takes to create the menu. The ledger records only three points (observed,
queued, audio started), so the stage costs below are estimates from the
code, to be replaced by measurement as the first step.

For a focus change delivered through D13, in order:

- Listener callback, three pipe hops (listener to Core, Core to outpost,
  outpost to Core), and JSON framing: well under a millisecond in total. The
  process model is not where the time is.
- Announce lane wait: zero in the good case. But a foreground fact's window
  job holds the lane for up to three attempts spaced 200 ms while the
  window is still nameless (`WINDOW_LANE_ATTEMPTS`), and the control's
  announcement queues behind it. On a menu open, the popup window exists
  before it is named, so this path is a plausible contributor to the
  100 ms figure.
- Arbitration verdict: microseconds when cached, but the cache lives 500 ms
  (`CACHE_TTL`), so most focus changes that are not rapid pay a cold probe:
  `UiaHasServerSideProvider` sends `WM_GETOBJECT` into the app, 1 to 10 ms
  depending on how busy its message loop is.
- Acquisition: a UIA fact already carries its snapshot, so nothing, unless
  its cached window handle is zero, in which case the outpost re-resolves
  the element and asks for its nearest window, two cross-process calls, 2
  to 5 ms. An MSAA fact costs `AccessibleObjectFromEvent` plus seven
  property reads, 2 to 8 ms.
- Enrichment: the ancestor chain, one cross-process call per hop. UIA is 1
  to 2 ms per hop and a XAML control sits 5 to 15 levels deep, so 10 to
  30 ms. MSAA is `accParent` plus seven reads per ancestor, 5 to 10 ms per
  hop. This is the dominant cost before emit, and it is serial with the
  announcement because the reducer wants entered containers spoken before
  the control.
- Reducer and speech queue: the 1 to 8 ms measured in M3, most of it the
  pipe write and thread handoffs; the reducer itself is microseconds.
- Synthesis: OneCore synthesizes the whole utterance to a stream before
  returning, 40 to 150 ms depending on length, plus two one-millisecond
  polling sleeps that Windows rounds up to its timer resolution, up to
  30 ms (audit item 17). eSpeak produces first audio in 1 to 5 ms.
- Audio: a 40 ms shared-mode buffer, event driven, with the first write
  waiting one device period before it checks for space (audit finding).

So a warm UIA focus change today is roughly 20 to 60 ms from observation to
queued, dominated by enrichment and the verdict probe, and 60 to 200 ms
from queued to audio with OneCore. The 100 ms menu figure is consistent
with that accounting.

### What the goal should be

The stated goal, 10 ms from outpost event to speech starting, has to be
split, because the two halves have different ceilings.

Observation to queued at 10 ms or under is reachable without changing the
process model, by four changes:

1. Batch the ancestor walk. UIA remote operations execute the whole walk
   inside the provider process in one round trip, 2 to 5 ms regardless of
   depth. The architecture already plans this for M4 (`verbatim-uia-rops`)
   for exactly this reason; it should be the first M4 item, not a later
   one. For MSAA there is no batching short of the M6 injection helper, so
   cache: the window-level ancestry (the dialog, the frame) comes from the
   HWND tree with local calls and costs nothing, and the object-level
   chain of a sibling is the chain of the previous sibling, so an
   outpost-side ancestor cache keyed by node, invalidated on window
   destruction, makes the first focus in a window pay and the rest not.
2. Give the arbitration verdict the window's lifetime, not 500 ms.
   Invalidate on the window's destruction (the listener can hook
   `EVENT_OBJECT_DESTROY` globally as a cached read) or, more simply, on a
   class-name change, and keep a long expiry as a backstop.
3. Never hold the lane. The window job must not delay the control behind
   it: emit the control announcement as soon as it is ready and let the
   window announcement be sequenced by the reducer using the observation
   timestamps it already compares. The late-window carve-out exists for
   this, and it should speak the late window at Queued priority rather
   than Interrupt (audit item 16).
4. Fix the outpost's lock and command-loop discipline (audit items 1, 2,
   5, 6), which are variance rather than average cost but produce the
   multi-second outliers.

Queued to first audio at 10 ms is a synthesizer and audio question, and
OneCore cannot meet it: it does not stream, so its floor is the length of
the utterance's synthesis. With eSpeak it is reachable but needs three
things the code does not have: `IAudioClient3` shared-mode streams at the
device's minimum period (about 3 ms at 48 kHz on most drivers) instead of
the 40 ms buffer; MMCSS registration of the synth thread as "Pro Audio";
and the first-write prefill fix from the audit. NVDA with eSpeak on real
hardware is typically 30 to 60 ms keypress to audio, so 10 ms queued to
audio would put Verbatim well ahead of the reference. OneCore stays as the
convenient default voice; the latency budget is written against eSpeak, as
the architecture already says, and eSpeak should move earlier than M8 if
latency is the headline goal.

What is not needed: consolidating outposts into a shared host, or moving
announcement back into the listener. The per-hop cost is sub-millisecond
and the isolation is worth far more than that.

### First step

Extend the ledger from three stamps to a stage timeline: observed, routed,
lane start, verdict, acquired, enriched, emitted, reduced, queued, synth
first sample, audio first write. Every number above is an estimate; the
timeline makes them measurements, per stage, on every run, and turns the
10 ms target into a per-stage budget that can be asserted once eSpeak is
in.

## 3. Proxmox audio

Yes for playback. A QEMU `ich9-intel-hda` device with the duplex codec
appears to the guest as a standard HD Audio device with Windows's in-box
driver, so Verbatim gets a real WASAPI render endpoint without any driver
install, which Hyper-V never had. With the SPICE audio backend, the guest's
playback is forwarded to the SPICE viewer, so Dickson can hear the
secondary VM's speech from the primary VM's viewer without an RDP session
taking over the guest's desktop. That removes the RDP-versus-audio
conflict for listening.

Not for recording, but recording should stop depending on a device. The
emulated codec's capture endpoint is a microphone input, not a loopback of
what is being played, and ffmpeg's Windows inputs have no WASAPI loopback
source. Rather than keeping VB-CABLE for that, capture the audio where
Verbatim already owns it: a tee at the `AudioSink` seam writes every
utterance's PCM to a WAV file with its wall-clock start time, and the
recording step muxes that track with the screen-grab video. This works on
a hosted CI runner with no sound device, on Proxmox, and on Hyper-V with
the same code, retires VB-CABLE from provisioning, and removes the
RDP-versus-audio conflict for recordings because nothing is captured from
the session's endpoint any more. The OneCore voices are present on the
Windows Server images hosted runners use, so the audio is the real voice.
What such a video omits is other applications' sounds, which a Verbatim
demonstration does not need. The tee is a wrapper around the real sink,
not a replacement for it, so audio keeps playing while it records: a
scenario can be heard live over RDP or on a local machine and recorded in
the same run, which the device-capture design could never do. One
standing constraint follows: everything Verbatim makes audible, including
the M11 earcons and any tones, must be rendered as PCM through the
`AudioSink` seam and mixed there, never played through a separate path
such as `PlaySound`, or the recording will not contain it.

## 4. Continuous integration with real Windows sessions

The typical answer is simpler than nested virtualization. GitHub-hosted
Windows runners execute jobs in an interactive logon session with a real
desktop, which is why this repository's existing `e2e` job works: it runs
the agent and Verbatim on the runner itself and injects real keystrokes.
NVDA does the same for its entire system-test suite: its
`testAndPublish.yml` installs the NVDA it just built into the hosted
runner, then runs the Robot suites (Chrome, Notepad, symbols, browseable
message) on `windows-2022` and `windows-2025`. There is no VM and no
snapshot; each job is a fresh runner, which is the clean state.

What hosted runners cannot give: a real audio endpoint (there is no sound
device, and installing the VB-CABLE kernel driver in a job is possible but
fragile) or a persistent golden image. Recordings with audio are still
possible there once the audio comes from the sink tee described in
section 3 rather than from a device: the screen grab works on the runner's
interactive desktop and the WAV is muxed in afterwards, so audible
demonstration videos can be produced by the hosted tier and uploaded as
job artifacts. NVDA sidesteps audio entirely with its silent spy synth,
which is exactly what Verbatim's capture synth is.

NVDA itself is not part of continuous integration. The NVDA transcript
add-on is an interactive research tool used on the secondary VM to learn
what the correct behaviour is; Verbatim's tests are written by hand from
that understanding and never compare against NVDA output (handoff decision
4). So CI runs Verbatim's own suites only.

Recommendation, three tiers:

1. Hosted runner, every push and pull request: the existing `e2e` job,
   running the end-to-end suite with the capture synth and the null audio
   sink, the way NVDA's own CI runs its system tests on hosted runners. No
   audio, no VM, and it should remain the default gate.
2. Everything else is the interactive loop, not CI. Dickson's Proxmox
   setup is personal: the xtask backend points at whatever he has,
   localhost, a Hyper-V VM, or the Proxmox secondary VM, and audible runs,
   recordings against a real device, and the golden image live there. No
   self-hosted runner is registered for it.
3. Not larger hosted runners with nested Hyper-V. They are billable per
   minute and the experiment in `vm-smoke.yml` was only ever a precondition
   check. Retire that workflow.

With recordings taking audio from the sink tee, the hosted tier can
produce audible demonstration videos too, so the only things that never
reach CI are behaviours of a real audio device, such as the device
invalidation in audit item 3, which stay interactive.

## Decisions, ratified by Dickson on 2026-09-02

To be written into `docs/architecture.md` as D14 to D16 and an amended
section 14:

- D14, attention model: the reducer tracks attention as the process and
  root window that last received focus; event acceptance is classified in
  the outposts by NVDA's window rules; background events are spoken
  without moving focus, at Queued priority, under a per-source flood cap;
  the listener additionally hooks `EVENT_SYSTEM_ALERT` and a desktop-wide
  UIA notification registration and forwards them as facts that spawn an
  outpost on demand, all as cached reads under its never-block rule.
- D15, latency budget split: observation to queued at 10 ms or under on
  every backend, measured per stage; queued to first audio at 10 ms or
  under with eSpeak through `IAudioClient3` and MMCSS; OneCore exempt from
  the second half. The stage ledger is the first M4 change and remote
  operations the second.
- D16, continuous integration: GitHub-hosted Windows runners are the only
  CI, running Verbatim's end-to-end suite silently, with recordings from
  the sink tee, on every change; NVDA is never run in CI; no nested
  virtualization and no self-hosted runners. The VM harness, whether
  pointed at localhost, Hyper-V, or Proxmox, is the interactive loop and
  not CI.
- Amend section 14: recordings take their audio from a tee at the
  `AudioSink` seam, muxed with the screen grab; every audible output,
  earcons included, is rendered through that seam; VB-CABLE leaves the
  provisioning script; on Proxmox the HDA device is the render endpoint
  and SPICE audio replaces RDP for listening.

Also decided the same day: eSpeak NG moves from M8 to the opening work of
M4, alongside the stage ledger, remote operations, and the attention
model, so both halves of D15 are measurable from the first text work
onward. All of the above is now written into `docs/architecture.md` and
`docs/roadmap.md`.
