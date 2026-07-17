# Completed milestones (archive)

The full scope and exit-criteria evidence of milestones already
completed, moved verbatim out of [the roadmap](roadmap.md) so the
active plan stays short. Milestone numbers are stable identifiers;
nothing here is renumbered.

M1's exit behavior was verified live during development — the menu and
every settings-dialog control announced with name, role, value, and
state through a real outpost, with end-to-end latency timelines — and
the repeatable, scripted form of that verification now runs as
`crates/verbatim-e2e`'s `m1_exit_regression` test: locally
runner-direct, in CI runner-direct on plain GitHub-hosted Windows
runners, and locally against the Hyper-V VM harness via
`cargo xtask vm test`. See [the tooling guide](tooling.md) and
[the VM harness guide](vm.md) for how to run any of these by hand.

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
  Remaining, carried forward: the stale-cache policy (lower-priority
  polish). The focus-listener outpost (decision D13, recorded in
  `docs/architecture.md` section 1 after M3's live debugging traced the
  announce-poll's whole failure family to the first-focus spawn race) was
  implemented at the start of M4 as suggested here, before caret tracking
  and typed-character echo build more behavior on top of the announcement
  plumbing it reshaped: all seven scenarios green on fresh restores plus
  ten consecutive clean cold-start runs of the two announce-race
  reproducers (`start_menu` and `m1_exit_regression`), with the menu-open
  announcement measured at roughly 100 milliseconds, at the fast end of
  the pre-D13 warm range.
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
