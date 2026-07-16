# Verbatim Roadmap

Milestones are scoped by _risk retired_, not calendar time. Each has explicit
exit criteria so progress is testable. Ongoing tracks (localization,
observability, latency budgets) start early and run through every milestone
rather than being milestones themselves.

## Status

M0, M1, and M2 are complete. M1's exit behavior was verified live during
development — the menu and every settings-dialog control announced with
name, role, value, and state through a real outpost, with end-to-end
latency timelines — and the repeatable, scripted form of that verification
now runs as `crates/verbatim-e2e`'s `m1_exit_regression` test: locally
runner-direct, in CI runner-direct on plain GitHub-hosted Windows runners,
and locally against the Hyper-V VM harness via `cargo xtask vm test`. See
`docs/tooling.md` for how to run any of these by hand.

## M0 — Foundations

No technology spikes: UIA, MSAA/IA2, and OneCore are known to work from
Rust, and wx via wxDragon is known to be accessible. Anything genuinely
uncertain (the IA2 proxy/stub story, remote ops on ARM64, wasmtime on
Windows ARM64) gets settled inside the milestone that implements it.

- Cargo workspace + crate skeletons per the architecture crate map; `xtask`;
  CI on GitHub-hosted Windows runners: build (x64 + ARM64 cross), clippy,
  unit tests.
- Tracing + flight-recorder skeleton; localization framework choice wired in
  from the first user-visible string.

Exit: `cargo xtask ci` runs clean on both architectures.

## M1 — Self-voicing prototype (the stated initial prototype)

- `verbatim.exe` with the Verbatim menu (options: settings and quit,
  localized) and a settings GUI: speech tab (synthesizer selection, voice,
  rate, pitch, volume), config persisted. Internal order: the speech
  pipeline speaking through OneCore lands first, then settings persistence,
  then the wxDragon GUI on top of both — wx accessibility is proven (NVDA's
  own GUI is wxPython), so no earlier spike is needed.
- The real outpost architecture at minimum viable scope: the Core-side
  supervisor spawns one outpost process (job-object lifetime,
  inherited-handle IPC) for the focused application — Verbatim's own GUI in
  the exit scenario. Reading our own windows through an out-of-process
  outpost also sidesteps same-process UIA client/provider hazards.
- UIA focus, property-change, and value-change events flow from the outpost
  through the reducer and speech pipeline, out through OneCore and WASAPI.
  Real UIA path, no self-voicing shortcut.
- A minimal MSAA client and NVDA-style per-window arbitration: wx dialogs
  are native Win32 controls with no UIA server-side provider, so reading
  our own GUI takes the MSAA path, exactly as NVDA reads its own; the UIA
  path is still required and exercised by UIA-native windows.
- Keyboard hook with the Verbatim modifier key and a single gesture:
  Verbatim+V opens the Verbatim menu. Nothing else is bound in M1.
- Control-plane v0 and `verbatim-inspect`, so M1 work is verifiable live:
  event and speech streams, gesture and arbitrary key injection, latency
  queries, and quit.
- Latency traces visible end-to-end (event observed, speech queued, audio
  started); the capture synthesizer and the in-memory replay machinery land
  here too.
- Startup replaces any running instance (NVDA's algorithm), and
  configuration is portable — `settings.toml` next to the executable is the
  base configuration (globals plus the base profile, where speech lives),
  and the `profiles` folder holds named profile overlays, none of which are
  active in M1.

Exit: with Verbatim running, Verbatim+V opens the menu, and every item in
the menu and every control in the settings dialog is fully announced —
name, role, value, and state on focus, plus value changes while adjusting
sliders and combo boxes; keypress-to-audio latency traced and reported
end-to-end (budget enforcement starts in M3, capture-synth based).

## M2 — Test harness and VM

Make everything after this point verifiable automatically.

- `mockapp` provider host — the same scripted trees exposed as UIA providers
  and as MSAA/IA2 providers — plus first tree-construction tests (real
  client stacks against it, cross-process), including backend-arbitration
  cases.
- Flight-recorder dumps to disk (crash or user-triggered snapshot) and
  reducer replay tests built from live dumps.
- The remaining control-plane surface the harness needs beyond M1's v0,
  starting with tree dumps, and the in-guest agent that speaks the protocol
  programmatically.
- Hyper-V harness: `xtask vm create/start/stop/restart/restore/deploy/test
/logs/connect/delete`, golden checkpoint, in-guest agent
  (`verbatim-agent`); E2E scenarios (own GUI plus Speech dialog, and Notepad
  focus) running locally against the VM. CI runs the E2E suite
  runner-direct — the in-guest agent bound to loopback on a plain
  GitHub-hosted Windows runner — alongside unit and provider tests.
  Automating the _VM_ itself in CI is still deferred per D3:
  `.github/workflows/vm-smoke.yml`, manual-dispatch only, checks whether
  GitHub's larger Windows runners can host nested virtualization at all,
  ahead of ever depending on it.
- `xtask vm test` is audible by default now (real `OneCore` speech, real
  `WasapiSink`), and `--record` additionally captures the run as a video
  with audio to `artifacts/vm-recordings` — both shipped reality, not
  aspirational. See "Audio in the VM harness" below for the constraint that
  shapes both.
- Arbitration attribution, landed at the end of the milestone when the new
  E2E suite exposed its absence as intermittent failures: a UIA event's
  element is usually not a window itself, so
  `verbatim_uia::nearest_window_handle` (NVDA's `getNearestWindowHandle`
  mechanism, one `NormalizeElementBuildCache` round trip) resolves its
  nearest windowed ancestor, and both backends arbitrate the same window
  for the same logical element; the decision ladder itself never needed
  changing. A residual risk stands: the normalize call is a cross-process
  call made inline on the event callback thread (NVDA's own trade),
  contained by per-app outpost isolation, with a deadline-guarded query
  worker as the fallback if it misbehaves.
- The D9 outpost generalization, landed at the same time and for the same
  reason: one outpost per application, spawned on first foreground and kept
  alive in the background (so the cross-pid retarget path — and the
  WinEvent rebind gap it carried — no longer exists), per-pid respawn on
  death, idle retirement on a two-minute threshold (never the current
  foreground's outpost, never Core's own, which is pre-warmed at startup
  because a cold spawn provably races an immediately following keystroke),
  and Core-side gating so only the foreground application's outpost is
  heard. Foreground changes announce the new window and then its focused
  control — NVDA's model — with bounded, generation-checked retries
  absorbing slow-starting applications; that made application-switch
  announcements deterministic, verified by a dedicated multi-app E2E
  scenario plus five consecutive green suite runs.

Exit: a one-command local run boots the VM, deploys a build, runs E2E, and
reports speech assertions + latency numbers. The suite must include the M1
exit behavior as a scripted regression: Verbatim's own menu and its Speech
settings dialog read correctly — every menu item and dialog control
announced with name, role, value, and state on focus, plus value changes
while adjusting the rate slider (both directions) and the voice combo box
(changed and changed back) — with keypress-to-audio latency reported from
the same run. The regression drives the capture synthesizer in runner-direct
mode (whose Speech page offers a voice combo box and a rate slider but no
toggle at all, so it asserts value changes only, not a check-box state
change) and the real `OneCore` synthesizer against the VM, where it is now
audible by default; the reducer's checked and not-checked announcements are
covered by `verbatim-core`'s unit tests and, cross-process, by `mockapp`'s
scripted state-change events. A state-change assertion belongs in this E2E
regression too, the day a drivable synthesizer page offers a toggle.

### Audio in the VM harness

Hyper-V Gen2 VMs have no emulated sound card, so audio needs a virtual
device. VB-CABLE (VB-Audio Virtual Cable) is that device — not Scream,
which was tried first and fails to root-enumerate a device node under this
image's Secure Boot; VB-CABLE is validly Authenticode-signed (chains to a
trusted root) and installs headless under Secure Boot without issue. It
provides both a render endpoint speech plays to and a loopback capture
endpoint `cargo xtask vm test --record` records from.

The constraint that shapes the whole design: recording audio and a
connected RDP session are mutually exclusive, proven live. The moment an
RDP session connects, Windows replaces that session's audio with its own
"Remote Audio" endpoint and the VB-CABLE capture device becomes invisible
within that session — confirmed with two independent tools, ffmpeg and
SoX, both failing identically to open it while a session was connected. So
a VM run is either heard live over a connected session, or recorded
headless with `--record`, never both in the same run.

Two dead ends were tried and ruled out, on record here so neither is
retried: mirroring the VB-CABLE capture endpoint's audio out to RDP's
Remote Audio endpoint via the "Listen to this device" registry keys (the
property-store keys involved are protected — writes are denied even
running as SYSTEM — and re-enumerating the device to apply any change
wipes them again anyway); and a SoX-based forwarder relaying the cable's
audio to Remote Audio (SoX cannot open the cable in the RDP session for the
exact same reason ffmpeg cannot — the device is not visible there at all).

## M3 — Desktop usability core

Verbatim becomes usable as a daily driver for basic Windows navigation.
This milestone's two originally largest items — arbitration attribution
and the D9 outpost generalization — were finished early, at the end of M2
(see M2).

- Outpost hardening, continuing the M2 work: the recovery ladder beyond
  respawn. Call deadlines and thread abandonment (rungs 1 and 2) already
  existed; rung 3's kill-and-respawn now also covers a wedged-but-alive
  outpost, not just one that crashes. A dedicated heartbeat thread in the
  supervisor pings every live outpost every few seconds; each pong reports
  the outpost's current parked-thread count (rung 2's bounded garbage). An
  outpost that misses several consecutive pongs, or whose parked-thread
  count climbs past a small threshold, is killed (dropping its job handle
  triggers an immediate kernel-level kill, so no message has to reach an
  outpost that is not reliably answering) and respawned through the same
  generation-checked machinery respawn-on-crash already used, so a kill
  cannot race a retirement or an application exit. Risk R2 measured on the
  existing harness VM: a live outpost's working set is roughly 19 megabytes
  (18.9 and 19.6 in a two-outpost sample), and an outpost is ready within
  roughly 160 milliseconds of a cold spawn, with each additional concurrent
  outpost marginal (about 26 milliseconds). Nineteen megabytes per app is
  the number the working-set-on-low-end-devices concern turns on; the
  architecture's mitigations (idle retirement, shared code pages from the
  single outpost binary, and the option to consolidate low-traffic apps
  into one host later) address it, and there is deliberately no dedicated
  low-end VM profile — the goal is to be efficient outright, and behavior on
  weaker hardware gets investigated only if real users report problems.
  Remaining, as lower-priority polish carried forward: the stale-cache
  policy and WinEvent routing refinements.
  Residual E2E flakes: root-caused during this milestone, as promised
  here, from failing runs' flight recorders and stderr on a repeated
  fresh-restore repro loop. The "occasional missed announcement deep in a
  long tab-through-dialog sequence" was never a missed announcement:
  every control's focus announcement was present in the flight recorder,
  and the harness was matching assertions against the speech stream's
  audio-start follow-up frames — under a loaded synthesizer those arrive
  seconds late, interleaved with fresh queue-time frames, satisfying
  assertions with stale text. The collector now matches queue-time frames
  only. The "slow first launch on a cold guest" was the first menu popup
  of a session taking over two seconds to appear (GUI-process resource
  loading; the foreground grab itself took under twenty milliseconds,
  confirmed by permanent info-level logging on the popup path) while
  scenarios sent their first arrow key blind less than a hundred
  milliseconds after Verbatim+V; scenarios now wait for the popup's own
  announcement first, the same synchronization a listening user performs.
  The same investigation removed the bare "window" announcement heard on
  a cold first menu open: the outpost's announce-focus retry was reading
  the popup window before the platform named it, and now skips unnamed
  windows so a retry announces the named window instead.
- Structured utterances (D12), landed now before more speech features
  accrete: the reducer emits utterances as sequences of semantic spans —
  label, role, value, state, description, attribute-tagged text runs —
  never pre-flattened strings; a presentation stage at the end of the
  speech pipeline flattens spans to text through a default theme. This is
  what later makes M11's earcons and voice styling a theme swap rather
  than a rewrite; the `PlayEarcon` effect already exists in the model.
  Utterances also carry optional source-node metadata — role and bounding
  rectangle — so M11 positional audio themes (the audio-themes add-on
  family plays role sounds panned by the object's on-screen position)
  have their data without a pipeline change.
- Announce a focused list's selected item. Focus landing on a list currently
  speaks only the list's own name and role; the selected entry is not
  spoken, which is not how a screen reader should read a category list or a
  list box (a gap masked until M2's arbitration fix removed the spurious
  cross-backend events that were announcing it by accident). Generalized to
  selection changes inside a focused container — the same mechanism that
  reads Explorer tab switches.
- Object navigation, review cursor navigation, minimally and deliberately scoped so it does not silently
  inflate: navigate to parent, next and previous sibling, and first child;
  report the current object; the review cursor follows focus, with a
  command to return it to focus; navigate through the review cursor to read text; activate the current object. Enough to
  reach everything the tab order cannot. Gestures are NVDA's, with the
  Verbatim modifier in NVDA's place. A navigation that finds no neighbor
  speaks NVDA's edge message ("No next", "No previous", "No containing
  object", "No objects inside") rather than falling silent — an earlier
  revision stayed silent with an M11 earcon planned on top, but live
  testing found silence indistinguishable from a broken command, so the
  message is the behavior and the earcon becomes an addition. Windowed
  controls (a dialog's slider, check box, and so on, each its own window
  over MSAA) navigate the Win32 window hierarchy the way NVDA's
  Window/WindowRoot classes do, since plain MSAA answers their sibling and
  child navigation with the control's own scroll-bar and client pieces
  rather than the sibling controls a user moves between. The active layout is a proper global
  setting from the start: a `keyboard` section in `settings.toml` whose
  `layout` is either `desktop` (the default) or `laptop` — exposed only in
  the file in M3; its GUI surface arrives with M8's gesture-remapping
  work. Review-mode switching stays excluded (it arrives with screen
  review in M6). Object navigation sees the full tree, matching NVDA with
  its simple review mode off — the user's baseline, and a decision this
  milestone re-affirmed after briefly shipping NVDA's simple-review
  projection instead (reverted the same day: the baseline is the
  unfiltered tree). Spoken focus ancestry is filtered separately, exactly
  as NVDA filters it regardless of that setting: NVDA's
  `isPresentableFocusAncestor`, ported onto Verbatim's roles — layout
  elements (unknown and pane roles, textless static text, nameless and
  description-less windows, property pages, and groupings) plus list
  items, tree items, and editable text are crossed but never spoken as
  entered containers. The same testing round gave navigation NVDA's
  responsiveness: outposts cache the live UIA element behind every node
  (re-finding by runtime id was an unscoped desktop-wide search per
  navigation step, the "stuck" feel), and menus are announced from the
  `MenuPopupStart` WinEvent the moment they open rather than from the
  foreground-announce retry loop (over half a second of the Verbatim+V
  lag NVDA does not have).

  The object-navigation bindings, desktop then laptop:

  - Report current object: Verbatim+numpad5, Verbatim+shift+o, with
    NVDA's full press semantics: once reports it, twice spells it, three
    times copies its name and value to the clipboard. M3's spelling
    speaks bare characters; punctuation names and character descriptions
    arrive with M4's character table, and configurable symbol handling
    with M8.
  - Move to parent: Verbatim+numpad8, Verbatim+shift+upArrow.
  - Move to next sibling: Verbatim+numpad6, Verbatim+shift+rightArrow.
  - Move to previous sibling: Verbatim+numpad4, Verbatim+shift+leftArrow.
  - Move to first child: Verbatim+numpad2, Verbatim+shift+downArrow.
  - Move review cursor back to focus: Verbatim+numpadMinus,
    Verbatim+backspace.
  - Activate current object: Verbatim+numpadEnter, Verbatim+enter.

  The review-cursor text-reading bindings, desktop then laptop:

  - Top of review area: shift+numpad7, Verbatim+control+home.
  - Previous line: numpad7, Verbatim+upArrow.
  - Current line: numpad8, Verbatim+shift+period.
  - Next line: numpad9, Verbatim+downArrow.
  - Previous word: numpad4, Verbatim+control+leftArrow.
  - Current word: numpad5, Verbatim+control+period.
  - Next word: numpad6, Verbatim+control+rightArrow.
  - Start of line: shift+numpad1, Verbatim+home.
  - Previous character: numpad1, Verbatim+leftArrow.
  - Current character: numpad2, Verbatim+period.
  - Next character: numpad3, Verbatim+rightArrow.
  - End of line: shift+numpad3, Verbatim+end.
  - Bottom of review area: shift+numpad9, Verbatim+control+end.

  Also excluded from M3, beyond simple review: NVDA's review-mode
  next/previous (Verbatim+numpad7 and Verbatim+numpad1 — document and
  screen review land in M6), move focus to navigator object
  (Verbatim+shift+numpadMinus), say-all (M4), and the mouse commands.
- Windows shell support expressed as generic core policy, not per-app
  patches. NVDA's live Windows 11 Explorer fixes reduce almost entirely to
  capabilities Verbatim needs anyway: window-classification arbitration
  rules (the `isGoodUIAWindow` analog — taskbar, systray overflow, Task
  View, and the input switcher prefer UIA), the selection-change
  announcements above, generic UIA notification-event handling (snap
  layouts), and foreground-transition focus filtering (alt-tab noise
  suppression). The irreducibly Explorer-specific residue — duplicate-focus
  dedup on desktop icons, stripping directionality marks from date columns,
  tooltip dedup in the systray — is cosmetic and waits for the Tier A
  extension port, which is the first real test of the Wasm path. Recorded
  contingency: if an E2E scenario surfaces a quirk that is both blocking
  and inexpressible as generic policy, the fix is pulling a minimal
  extension host forward from M5, not a built-in quirk layer.

  Explorer E2E scenario: deliberately not in the automated suite, verified
  manually for now. The capability is proven live (reading a real Explorer
  folder window's file list over UIA — file name, list-item role, and
  positional info, with good-window arbitration keeping it on UIA), but an
  automated scenario for it is flaky in a way the other shell scenarios are
  not, and the flake is in the harness's interaction with the shell, not in
  Verbatim: a folder window opened through the shell (`start <folder>`) is
  created by the already-running `explorer.exe`, so unlike a freshly
  launched process such as Notepad or msinfo32 it does not reliably take and
  hold the foreground on a loaded, freshly-restored guest, and Verbatim's
  passive foreground announcement then has nothing stable to fire on. The
  other shell scenarios (msinfo32, object navigation, tree navigation,
  multi-outpost switch) already exercise the shell-navigation exit criterion
  on real surfaces, so this one is verified by hand rather than papered over
  with retries: with Verbatim running, open a folder of a few files in real
  Explorer (Windows+E, or any folder from the desktop or taskbar — opening it
  interactively is what gives it the foreground the automated harness cannot
  reliably arrange), and confirm Verbatim announces the folder window and then
  each file as a list item with its name and position in the set as you arrow
  through ("alpha.txt, list item, 1 of 3"), which is the UIA file list,
  list-item role, positional info, and good-window arbitration the scenario
  would have asserted. Automating it waits for the shell-foreground-timing
  work its own investigation would need, most naturally alongside the Tier A
  extension port that motivates the Explorer-specific cosmetic fixes anyway.

  Toggle controls (the Settings app's read-only WinUI toggles): landed as a
  `ToggleButton` role, matching the reference NVDA exactly. A WinUI
  `ToggleSwitch` reports as a UIA Button that exposes the Toggle pattern, so
  `verbatim-uia` reclassifies a Button-with-Toggle to `Role::ToggleButton`
  and maps its toggle-on state to `Pressed` rather than `Checked` (NVDA's
  `_get_role` and toggle-state branch); the reducer announces "not pressed"
  for a toggle button that is off, the same negated-state treatment check
  boxes get, and a live-off toggle's state change announces the negation
  too. A separate `Switch` role spoken "on"/"off" was considered (an earlier
  recorded intent) and dropped after checking the reference NVDA, whose UIA
  path has no switch role and announces these as toggle buttons; the
  maintainer confirmed the toggle-button wording. The mapping is verified
  cross-process through mockapp's real UIA client (a scripted toggle button
  round-trips to `ToggleButton` plus `Pressed`), and the reducer wording is
  unit-tested. The live Settings-app E2E scenario is deferred for the same
  reason as the Explorer scenario above: the Settings app is a
  broker-activated process that does not take keyboard focus reliably
  through the harness on a freshly-restored guest (an eight-tab probe into
  the Notifications page produced no announcements at all), so the toggle
  behavior is verified by hand — open Settings, tab to any toggle, and
  confirm it announces "<name> toggle button pressed" on and "not pressed"
  off.

  Start menu and search: the `start_menu` E2E scenario covers the reliable
  half — pressing the Windows key opens the Start/Search surface and
  Verbatim announces its search box. Unlike Explorer and the Settings app,
  the Start menu opens on a real key press and takes the foreground the
  ordinary way, so its opening announcement is stable (validated across
  repeated fresh-restore runs). Navigating the search *results* is
  deliberately not automated: typing a query switches Start to virtualized
  Web-content result panes that report selection rather than focus as the
  highlight moves, an async surface that does not settle predictably under
  the suite's pace. Reading the results is verified by hand — open Start,
  type a few letters, arrow through the results, and confirm each is
  announced as you land on it.
- Time and date command: Verbatim+F12 speaks the time, twice quickly for
  the date. A system tray and taskbar icons list replicating the
  systrayList NVDA add-on exactly, including its GUI: Verbatim+F11 opens
  the system tray list, pressed twice quickly the taskbar list; the dialog
  is a label over a single-selection list box of item names with four
  buttons — Left Click, Left Double Click, Right Click, and Cancel — where
  each click action moves the pointer to the center of the selected item's
  screen rectangle and injects the matching mouse events. Enumeration goes
  through the existing UIA client over the shell windows, not the add-on's
  per-Windows-build window-class walks, which predate usable UIA there.
  The list dialog is a reusable component — M6's elements list presents
  through the same one. Both are core features.
- Latency budget enforcement starts here: the pipeline budget via the
  capture synthesizer (deterministic, measures everything except
  synthesis), plus an end-to-end OneCore smoke number with a looser
  threshold. Landed, then amended by a recorded decision: the pipeline
  budget (`verbatim_e2e::latency::PIPELINE_BUDGET_MS`, the deterministic
  event-observed-to-speech-queued latency, synth-independent) is
  *reported, never asserted* — a prominent warning on a breach, plus every
  run summary's measured per-scenario maxima. It began as a hard
  per-scenario assertion, and flaked twice for the same non-code reason: a
  50 ms budget set from lightly loaded runs (1 to 8 ms typical) tripped at
  58 ms on a loaded run; raised to 200 ms, it tripped again at 277 ms
  during a full audible suite run, while a dedicated rerun of the
  identical build measured 3 ms. A wall-clock assertion inside a shared,
  variably loaded VM measures host scheduling, not Verbatim's code, so the
  assertion only manufactured flaky runs. A real pipeline regression still
  shows unmistakably in the always-reported maxima. The enforced budget
  returns in M8 with eSpeak's reference number on a controlled
  measurement. An earlier concern that the audible path
  never reached audio turned out not to reproduce: a substantial fraction
  of an audible run's utterances do reach audio (measured live), the rest
  interrupted before playback by the suite's fast pace — expected
  real-synth behavior, raised by `--paced`. The end-to-end OneCore smoke
  number is deliberately not enforced: observed audio latency ranges past
  1300 ms for a long utterance (synthesis time scales with text), too
  variable for any stable threshold, so per this milestone's recorded
  contingency it waits for eSpeak's reference number in M8. Every run's
  per-scenario summary reports the measured pipeline and audio maxima.
- Generic backend parity with NVDA, scoped to MSAA and UIA only: object
  presentation on focus (property order, spoken and negated state sets,
  description, positional info), the WinEvent and UIA event sets with
  NVDA's acceptance filtering, and the arbitration class lists. IA2
  interface acquisition (the `QueryService` seam left in `verbatim-ia2`)
  is deliberately deferred to M6, where the browsers that motivate it
  land; until then MSAA-only apps are read through plain MSAA.
- E2E scenarios: Explorer, Settings, Start menu, task switching, at least
  one MSAA-only legacy app;

- Cleanup / improvement of the E2e: currently everything runs together in one recording but this won't be feasible once we have more, we need to be able to group them and run all of them, or only a subset. Each recording should only cover one scenario for ease of debugging, scenarios should have before and after commands for setting up and teardown of state (e.g, open / close notepad)
- Deferred to M8: eSpeak NG, input help mode, and the configuration
  surfaces for gesture remapping, speech dictionaries, and symbol
  pronunciation. The underlying infrastructure is data-driven from the
  start — gestures bind through stable identifiers (the control plane
  already injects them by name), and structured utterance spans make
  dictionaries a per-span pipeline stage — so M8 adds configuration UI on
  top of it, not new architecture.

Exit: navigate the Windows shell fluently; review cursor and object
navigation work (confirmed live, including on the MSAA-backed settings
dialog after the window-hierarchy and edge-message fixes); the
capture-synth pipeline latency stays within budget in the VM harness —
measured at 1 to 8 milliseconds typically, reported per scenario, with the
hard assertion deferred to M8's controlled measurement after it proved to
track guest scheduling rather than code (recorded above); no reducer path
emits a pre-flattened utterance string (D12) — every spoken announcement,
including navigation edge messages, reaches the pipeline as semantic
spans.

## M4 — Text, editing, and terminals

- TextPattern support in the model; caret tracking, typed-character echo,
  word/line/character navigation; say-all with index-mark continuation.
- Text runs carry formatting attributes as utterance spans (spelling and
  grammar markers, and font/color where exposed), so M11's
  formatting-change sounds have data to act on.
- A minimal built-in character-description table (punctuation and symbol
  names): character navigation must say "comma" on a comma even though the
  configurable dictionary system waits until M8.
- Remote-ops integration (ancestor fetch on focus; terminal text ranges) —
  includes the ARM64 remote-ops verification (R3).
- Windows Terminal: diff-based output announcement with flood policy; the
  "cat a huge file" scenario is an E2E latency test.

Exit: Notepad editing and Terminal sessions are solid; terminal flood E2E
shows bounded latency and no hang.

## M5 — Extensions v1

- Component host with epoch preemption; WIT `verbatim:ext` v0: event
  subscription, tree queries, speech output, gestures, config, storage.
  Ratify the D6 runtime choice here: verify wasmtime on Windows ARM64 and
  measure host-call overhead against the extension-hook deadlines before
  building further on it.
- App-module activation keyed to processes; hot reload; capability manifests.
- Dogfood: at least one first-party app module ported out of the core into
  an extension — the Explorer cosmetic fixes deferred from M3 are the
  designated first candidate — and a trivial Wasm synth proves the
  `verbatim:synth` world.

Exit: an app module can be edited and hot-reloaded without restarting
Verbatim; capability denial is enforced and tested. This milestone opens the
app-module porting track (see the section after M15), which then runs
continuously alongside every later milestone.

## M6 — Browse mode, browsers, and the injection helper

- Document projection / incremental virtual buffer over the normalized tree;
  quick-nav keys, elements list (presented through the M3 list dialog),
  switching between focus and browse modes.
- IA2 lands here, deferred from M3 because browsers are what motivate it:
  interface acquisition (`IServiceProvider::QueryService` from the
  WinEvent's `IAccessible` to `IAccessible2` and the IA2 text, hypertext,
  and relation interfaces), the proxy/stub marshaling story from Rust, and
  IA2 roles and states preferred over MSAA in the normalized mapping.
  `mockapp`'s MSAA mode grows an IA2 answering path so the client stack is
  testable cross-process without a browser.
- Firefox and Chromium (Edge/Chrome) via IA2 as the primary source, with a
  UIA comparison where relevant.
- The D2 injection helper lands here, staged inside the milestone:
  out-of-process IA2 first, proving correctness, then the in-process helper
  for performance — IA2 call batching and virtual-buffer acceleration —
  including the x64/ARM64EC/x86 helper matrix and antivirus/signing
  considerations. NVDA has already proven that in-process access is what
  makes browsing fast enough, so this is scheduled work rather than a
  measure-first gate; the fixed-page-corpus measurement against NVDA
  remains as this milestone's exit verification.
- Screen review as a spatial projection of the normalized tree (D11):
  visible nodes ordered by bounding rectangle, grouped into visual lines,
  walked by the review cursor by line, word, and character. Extent-backed
  wherever a text interface exists — UIA TextPattern bounding rectangles
  and IA2 character extents give exact per-character geometry, batched
  through the new helper — with rectangle interpolation as the fallback for
  name-plus-rectangle elements. OCR joins as a second text source in M8 and
  the display model as a third in M14; the review commands and cursor stay
  the same throughout, only the source improves underneath.
- Interaction-before-full-render E2E on a very large page.
- Scan-mode generalization: the same projection over an ordinary app.

Exit: real browsing works day-to-day; large-page E2E passes; corpus
performance/parity report vs NVDA written, at (or consciously accepted
near) parity; screen review reads a modern app's screen correctly.

## M7 — Native synth host + Eloquence PoC

- Sandboxed native synth host process (AppContainer, job object, shared-memory
  PCM ring), arch-matched loading.
- Eloquence proof of concept meeting the same latency budget.

Exit: Eloquence speaks through Verbatim under sandbox; latency test green.

## M8 — Breadth: speech configurability, profiles, overlays, OCR, secure desktop

- eSpeak NG built-in synth (statically linked, x64 and ARM64) — the
  reference synth for the latency budget, whose enforcement tightens from
  the M3 capture-synth budget to the eSpeak reference number.
- Input help mode; the gesture-remapping configuration GUI; full
  pronunciation/symbol dictionaries and their configuration UI (the
  data-driven infrastructure exists from M3/M4).
- Configuration profiles (manual and triggered); localized UI shipped in at
  least two languages as proof.
- Focus highlight (DirectComposition overlay) and screen curtain (R6 check);
  overlay component designed to host a future magnifier.
- OCR capability (Windows.Media.Ocr) exposed to extensions; synthetic-subtree
  review of an image/window; OCR becomes screen review's second text source,
  for windows whose pixels contain text that no API exposes.
- Secure-desktop instance (`--secure`), AT registration, UIAccess/test-signing
  story in the VM (R5).
- Installer/updater skeleton.

Exit: sign-in and UAC prompts are read in the VM; curtain + highlight E2E;
eSpeak latency budget green.

## M9 — Logging and log viewer

Proper user-facing observability, distinct from (and built on) the
developer-facing flight recorder and tracing spans.

- User-facing log levels and categories; logging to file plus an in-memory
  ring; the existing tracing spans, collected stderr, and panic dumps
  absorbed into one coherent story.
- A log viewer window in Verbatim itself, readable with Verbatim — the
  viewer is its own dogfooding test.
- The commands NVDA users expect around it: open the log viewer, report the
  most recent error, cycle the log level at runtime.

Exit: a user can reproduce a bug, open the log viewer, and read what
happened, without touching developer tooling.

## M10 — Extension console

The equivalent of NVDA's Python console for extension development. Python
is an obvious fit for NVDA; with Wasm extensions the console is instead an
interactive interpreter that is itself an extension: an interpreter
compiled to Wasm, granted broad capabilities, evaluating against exactly
the `verbatim:ext` WIT API every extension uses. No privileged side-channel
API exists — if the console can do it, an extension can — so the console
doubles as a standing test that the API is ergonomic enough for
exploratory work. Convenience aliases in the NVDA console style (the
focused object, the navigator object) are pre-bound bindings over the same
calls, never separate host functions.

Design questions settled inside this milestone: which interpreter (QuickJS,
RustPython, or similar, compiled to a component), how epoch preemption
interacts with long-running evaluations, and whether the console UI lives
in the Verbatim GUI, in `verbatim-inspect`, or both.

Exit: from the console, inspect the focused object, walk its tree, speak,
and bind a gesture — against a live Verbatim, without restarting anything.

## M11 — Audio formatting: earcons and voice styling

The payoff for D12's structured utterances: presentation themes that map
semantic spans to sound, in the tradition of Emacspeak's audio formatting
and the audio-themes family of NVDA add-ons.

- A theme maps span semantics to presentation: a role can become an earcon
  plus shorter speech (a slider sound and "pitch 50" instead of "Pitch
  rate slider 50"); formatting attributes on text runs can become sounds
  (a spelling or syntax error under the cursor plays a sound rather than
  being spoken); capitals, quotes, and emphasis can become pitch or voice
  changes.
- The default theme reproduces plain speech exactly; switching themes is a
  runtime configuration change, no restart.
- Themes are data, and eventually extension-provided packages — giving the
  porting track a consumer in the audio-themes add-on family.

Exit: an earcon theme ships alongside the plain default; a scripted E2E
hears the slider earcon and the spelling-error sound; the plain theme's
output is identical to pre-M11 speech.

## M12 — Remote support

- Pairing/auth UX on the control plane; speech mirroring out, input in;
  secure-desktop and permission rules applied to remote sessions.

Exit: control a second Verbatim instance (between the VM and the host)
end-to-end.

## M13 — Java Access Bridge

JAB is a committed backend, deliberately last among the accessibility APIs
(D1): far fewer apps need it than UIA and MSAA/IA2.

- `verbatim-jab` client stack: the WindowsAccessBridge-64 C API and its
  event callbacks, running on outpost threads like the other backends,
  mapped into the normalized model behind the same arbitration.
- A small Java Swing fixture app joins `mockapp` duty for provider-level
  tests; E2E against a current Java IDE or LibreOffice-adjacent Java app.
- Unblocks porting-track Tier E (javaw, Eclipse).

Exit: a mainstream Swing application is readable and navigable end-to-end.

## M14 — Display model (gated)

A GDI display model in the NVDA tradition, deliberately last among the
text sources (D11), and opened by an explicit re-triage gate: test the
then-current PuTTY, SecureCRT, and Tera Term (and any other app that has
motivated this milestone by then) against stock Verbatim plus OCR, and
build only if the display model still earns its maintenance cost. By this
point everything it needs already exists, which is what keeps the
milestone cheap: the M6 injection helper is the delivery vehicle (the
display model is its second in-process client), synthetic nodes make its
output first-class model content, and the M4 diff announcer and flood
policy handle live terminal output.

- GDI text-output hooks (`ExtTextOutW` and family) in the injection helper;
  a per-window text model (chunk rectangles, baseline ordering,
  invalidation on redraw).
- A live-text diff source over that model — the `DisplayModelLiveText`
  equivalent — feeding the same announcement pipeline as Windows Terminal.
- Screen review gains the display model as its third, pixel-faithful text
  source for GDI apps; text-under-mouse works in apps with no text API.
- Unblocks the porting track's legacy terminal clients (PuTTY, SecureCRT,
  Tera Term).

Exit: the re-triage decision is recorded; if built, a PuTTY session is
readable with live output announcement and the existing screen-review
commands, with no fidelity regression anywhere else.

## M15 — Braille (deliberately last, D7)

- liblouis integration; `BrailleDisplay` trait implementations for common
  displays; braille viewer (on-screen virtual display) so development and
  E2E tests need no hardware; routing keys, cursor tethering, speech-braille
  sync via existing index marks.

Exit: braille viewer E2E scenarios pass; at least one hardware display
verified when hardware is available.

## The app-module porting track (runs from M5 onward)

Goal: eventually port all of NVDA's built-in app modules
(`nvda/source/appModules`, roughly 80) except those for software that no
longer exists or is unmaintained — Skype, Lync, MSN Messenger, Outlook
Express, Windows Live Mail, Winamp, Instantbird, Miranda, Lotus
Notes/Symphony, the legacy EdgeHTML modules, and similar; each skip is a
recorded case-by-case call.

Method: triage before porting. Many NVDA modules exist to patch quirks of
NVDA's pipeline or of APIs at the time they were written; for each module,
first test the app against Verbatim's stock UIA/IA2 handling and port only
the behavior that is still needed. Every port doubles as a test of the
extension API (per the API-growth rule: no host-API additions without a
consumer).

Tiers, gated by the capabilities each module needs:

- **Tier A — immediately after M5** (needs only extension API v0: events,
  tree queries, speech, gestures): the Windows shell and utility modules —
  Explorer (starting with the cosmetic fixes deferred from M3), Settings,
  Task Manager, Calculator, search, lock screen and logon UI, Open With,
  Notepad, Notepad++, VS Code, Poedit, Spotify, foobar2000, Audacity,
  1Password, basic Zoom and Teams behavior.
- **Tier B — the terminal clients, in two stages**: Windows Terminal
  workflows are core M4 behavior, and mintty is triaged on its own after M4
  (its ConPTY integration may make it readable stock). PuTTY, SecureCRT,
  and Tera Term draw their screens through GDI with no accessibility API at
  all, so they are gated on the M14 display model — with ssh from Windows
  Terminal as the recommended interim answer.
- **Tier C — after M6 browse mode** (web-content-hosting apps): WebView2
  hosts, WhatsApp, deeper Teams support, Thunderbird, Kindle and other
  readers.
- **Tier D — after the object-model bridge capability lands (planned around
  M8)**: Word, Excel, PowerPoint, Outlook, LibreOffice, Visual Studio.
  NVDA's Office modules rely heavily on COM object-model automation, which
  Wasm extensions cannot reach directly; this tier is gated on designing a
  capability-gated app object-model bridge in the WIT host API — the
  largest single API-growth item in the plan, and worth its own design pass
  for both surface and security.
- **Tier E — after the JAB backend lands (M13)**: javaw and Eclipse.

## Ongoing tracks (every milestone)

- **Latency**: budget tests run in E2E from M2 on; regressions fail the build.
- **Localization**: no hardcoded user-visible strings, ever; pseudo-locale
  test from M1.
- **Flight recorder into regression tests**: every reproduced field/VM bug
  lands as a replay test.
- **Extension API growth**: only via porting real app modules/add-ons;
  each addition needs a consumer.
- **Deferred**: CI automation for VM E2E (D3); revisit once local harness is
  stable — candidates are QEMU/KVM Win11 guests on Linux runners or
  self-hosted runners. `.github/workflows/vm-smoke.yml` (manual dispatch
  only) checks whether GitHub's larger Windows runners can host nested
  virtualization at all, a precondition for any of those candidates.
