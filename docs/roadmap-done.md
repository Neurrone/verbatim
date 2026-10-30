# Completed milestones

What each completed milestone, and each completed part of M4, delivered. The open work is in [the roadmap](roadmap.md); the commits that landed each item, and the evidence each exit criterion was checked against, are in git. Milestone numbers are stable identifiers; nothing here is renumbered.

## M0, foundations

The Cargo workspace with a skeleton for every crate in the architecture's crate map, `xtask`, and CI on GitHub-hosted Windows runners (build, Clippy, unit tests); tracing with a flight-recorder skeleton, and the localization framework wired in from the first user-visible string. Its exit was `cargo xtask ci` clean on both architectures. No technology spikes: UIA, MSAA and IA2, OneCore and accessible wxWidgets were known to work from Rust, and what was uncertain was settled inside the milestone that built it.

## M1, the self-voicing prototype

`verbatim.exe` with the Verbatim menu (settings and quit, localized) and a settings dialog whose speech tab picks the synthesizer, voice, rate, pitch and volume, with the configuration persisted. The real outpost architecture at its smallest: the supervisor spawns an outpost process, in a job object with inherited-handle IPC, for the focused application, Verbatim's own GUI included, and UIA focus, property and value events flow from it through the reducer and the speech pipeline to OneCore and WASAPI. A minimal MSAA client and NVDA-style arbitration per window, since wx dialogs are native Win32 controls read over MSAA, as NVDA reads its own. The keyboard hook with the Verbatim modifier and one gesture, Verbatim+V for the menu. Control plane v0 and `verbatim-inspect` (event and speech streams, gesture and key injection, latency queries, quit); latency traces from event to audio; the capture synthesizer and in-memory replay. A second instance replaces the running one, as NVDA's does, and the configuration is portable: `settings.toml` beside the executable holds the globals and the base profile, and `profiles` the named overlays. Its exit, every menu item and every settings control announced with name, role, value and state, and value changes while adjusting, now runs as the end-to-end scenario `m1_exit_regression`.

## M2, the test harness and VM

`mockapp`, which exposes the same scripted trees as UIA and as MSAA and IA2 providers to real client stacks cross-process, with the first tree-construction and arbitration tests; flight-recorder dumps to disk and reducer replay tests built from them; the control-plane surface the harness needs, starting with tree dumps; and `verbatim-agent`, the in-guest agent that drives it. The Hyper-V harness (`cargo xtask vm`, `docs/tooling/vm.md`) with a golden checkpoint, and the end-to-end suite running locally against the VM and in CI against a plain GitHub-hosted runner, the VM itself not automated in CI (D3). Two items planned for M3 landed here because the new suite exposed their absence: arbitration by the nearest windowed ancestor (`verbatim_uia::nearest_window_handle`, one `NormalizeElementBuildCache` round trip), so both backends arbitrate the same window for the same element; and D9, one outpost per application, spawned on first foreground, kept in the background, respawned per process on death, retired after two idle minutes (never the foreground's or Core's own, which is started at startup), with Core hearing only the foreground application's outpost, and a foreground change announcing the window and then its focus, as NVDA does. Its exit was a one-command local run that boots the VM, deploys, runs the suite with the M1 exit as a scripted regression, and reports speech assertions and latency.

## M3, desktop usability core

Verbatim became usable for basic Windows navigation.

- Outpost hardening: the recovery ladder's third rung kills and respawns a wedged outpost as well as a crashed one, from a supervisor heartbeat whose pong reports each outpost's abandoned-thread count. Risk R2 was measured on the harness VM: a live outpost's working set is about 19 MB, and an outpost is ready about 160 ms after a cold spawn, each further concurrent one adding about 26 ms. Low-end hardware gets no dedicated profile; behaviour there is investigated when users report it.
- Structured utterances (D12): the reducer emits sequences of semantic spans (label, role, value, state, description, attribute-tagged text), carrying the source node's role and rectangle, and a presentation stage flattens them through a theme; no reducer path emits a flattened string.
- A focused list's selected item announced, generalized to selection changes inside a focused container.
- Object navigation and the review cursor, minimally scoped, over the full tree as NVDA has it with simple review off, with spoken focus ancestry filtered as NVDA's `isPresentableFocusAncestor` filters it. A navigation that finds no neighbour speaks NVDA's edge message ("No next", "No previous", "No containing object", "No objects inside"), because silence sounded like a broken command. Windowed MSAA controls navigate the Win32 window hierarchy, as NVDA's window classes do. Outposts cache the live element behind each node, and menus are announced from `MenuPopupStart`. The layout is a global setting, `layout` in `settings.toml`'s `keyboard` section, `desktop` (the default) or `laptop`, set only in the file until M8. Review modes (M7), moving focus to the navigator object, say-all (M4) and the mouse commands were left out.
- Windows shell support as generic policy rather than per-application patches: window-class arbitration rules (the `isGoodUIAWindow` analog), the selection announcements above, UIA notification events, and filtering of focus noise across foreground changes. Explorer's cosmetic residue waits for the Tier A extension port, and a blocking quirk that generic policy cannot express pulls a minimal extension host forward from M5, never a built-in quirk layer.
- WinUI toggle switches read as NVDA reads them: a UIA button with the Toggle pattern is `Role::ToggleButton`, on is `Pressed`, and off is spoken "not pressed".
- The `start_menu` scenario: the Windows key opens Start and Verbatim announces its search box.
- Verbatim+F12 speaks the time, twice quickly the date; Verbatim+F11 opens the system tray list, twice quickly the taskbar list, replicating NVDA's systrayList add-on with its dialog (a list box of item names and Left Click, Left Double Click, Right Click and Cancel), enumerated over UIA. The list dialog is reusable; M7's elements list presents through it.
- Latency: the pipeline's event-observed-to-speech-queued time, through the capture synthesizer, is reported per scenario with a warning on a breach, never asserted, because a wall-clock assertion in a shared, variably loaded VM measured host scheduling rather than Verbatim's code; the enforced budget comes with M8's controlled measurement, and OneCore's audio latency, too variable for a threshold, is reported only.
- Generic backend parity with NVDA for MSAA and UIA: object presentation on focus, the WinEvent and UIA event sets with NVDA's acceptance filtering, and the arbitration class lists. IA2 interface acquisition waits for M7's browsers.
- Deferred to M8: input help, and the configuration surfaces for gesture remapping, speech dictionaries and symbol pronunciation, all of which sit on infrastructure that is already data-driven.

The object-navigation bindings, desktop then laptop (`crates/verbatim-input/src/scripts.rs` transcribes them in this order):

- Report current object: Verbatim+numpad5, Verbatim+shift+o, with NVDA's press semantics: once reports it, twice spells it, three times copies its name and value to the clipboard.
- Move to parent: Verbatim+numpad8, Verbatim+shift+upArrow.
- Move to next sibling: Verbatim+numpad6, Verbatim+shift+rightArrow.
- Move to previous sibling: Verbatim+numpad4, Verbatim+shift+leftArrow.
- Move to first child: Verbatim+numpad2, Verbatim+shift+downArrow.
- Move review cursor back to focus: Verbatim+numpadMinus, Verbatim+backspace.
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

## M4, the parts completed

M4 is not complete; its open work is in [the roadmap](roadmap.md). Code comments and the crate guides cite M4's text work by item number, from the plan of 2026-10-06:

1. Text foundations: segmentation and the text model in Core.
2. Reading text in the outposts: UIA's text pattern and the standard edit controls' messages.
3. Caret navigation and selection.
4. Typed-character and word echo, with the password rule for terminals.
5. The review cursor.
6. Say-all.
7. Formatting spans.
8. Themes and earcons.
9. Terminals.
10. The exit criteria.

### Opening work

UIA remote operations for the focus's ancestor walk (D15); the latency stage ledger, covering verbatim.exe's own stages and the listener's, the outposts' and the synthesizer host's hops; eSpeak NG as the built-in default synthesizer, built from the `third_party/espeak-ng` submodule and run in the synthesizer host; and the attention model in place of the foreground gate (D14), with the File Explorer and Settings scenarios that waited on it.

### The phase 6 sequence and stage A, the focus pipeline

A second Verbatim replaces the running one as NVDA's does; NVDA's event filtering in the outposts; cheaper reads. Then [the focus pipeline](design/focus-pipeline.md): time limits that hold; Core's inputs taken in arrival order, with NVDA's duplicate rule and background speech dropped across a foreground change; console windows as one application, every event for a console window going to the console host's outpost with the client process as identity; arrival order in the outposts with NVDA's MSAA limiter; MSAA focus, foreground and menu events in each outpost's own hook; the listener judging and reading each UIA focus from the event's own element, with NVDA's focus handler ported in its order and NVDA's base cache request (option (b), with no MSAA fallback while the listener's UIA focus is stalled, decided 2026-10-10); and the focus followed with one UIA event handler group. A foreground event for a window read through UIA is dropped and the window announced from the focus's ancestry, Control+Tab follows NVDA, and a reading thread ends with its application's outpost. The audits that followed added NVDA's handling of notifications, of the focus's own events, of UIA menu openings and selections, one duplicate test and one acceptance test per focus (verified unchanged; the tooltip setting and background progress bars went to M4.3, the `requestEvents` table to M5, NVDA's Chromium rule to M7, and a per-source cap on background events is unscheduled), foreground events tried newest first as NVDA's `pumpAll` takes them, the system caret's location change and show as a caret event on a UIA focus, a combo box's value from its selected item, and selection in a list the focus controls (`settings_search_suggestions`).

### The focus pipeline after stage A

The UIA probe asks once, as NVDA does; two spare outposts kept waiting and given to applications as they need one, and an outpost whose hook fails exits instead of saying `Ready`; the UIA focus change's call count revised to the calls NVDA's focus handler makes, 4 for a windowless sender and 3 for a window in the steady state (Dickson, 2026-10-16), with a menu item's legacy checked state read inside the focus's remote operation and NVDA's handling of a menu opening on the current focus ported; File Explorer's focus ancestors compared by element id, and by role and name only where ids cannot be compared, and a foreground window in front reported at once, shown or not; UIA events received before the listener's subscriptions are installed dropped, as NVDA drops them; say-all's caret move taken as the caret a later key starts from; and the harness's console host launched without the foreground right.

### The communication migration and stage B

The steps of [the communication design](design/communication.md), section 11: the baseline and JSON's cost, the platform floor and the speech-path scenario, the scheduling policy (`THREAD_PRIORITY_HIGHEST` for the threads that only move messages, and no power-throttling opt-out), the transport, the relay on its port, inline writes in the children, the harness's idle barrier through every child, the relay reading the outposts' pipes, the host protocol, the outpost's per-event costs, and the measurements; step 8 was not adopted and step 10 deferred. verbatim.exe's relay is met within 1 to 2 ms with events still routed through it. Stage B made the control-flood scenarios follow NVDA, compared Shift's pause with NVDA's (no change), corrected the parity entries on UIA focus and the limiter, and recorded the toast difference ([parity](parity/alerts.md), "Toasts"). The suite switches windows by bringing them forward, never with Alt+Tab.

### Holding caret keys

Caret keys in text fields and terminals are held as NVDA holds them, the one mechanism for both, with the keys pressed behind a held key kept in order and nothing held while an elevated window is in front ([holding caret keys](design/key-holding.md); Dickson, 2026-10-22). It closed the caret problems that shared one cause (Dickson, 2026-10-17), since the read before each key replaces Core's and the outpost's copies of the caret: select-then-copy followed by Right, and a selection reported after the next key, are no longer misjudged.

### M4.1, stability

M4.1 is complete (Dickson, 2026-10-21): Verbatim stopped crashing and stalling and the suite stopped failing by timing, with a week of Dickson's daily use without a crash or a stall and the full suite ten times clean as its exit. Its open follow-ups are in [the roadmap](roadmap.md), M4, "Follow-ups from M4.1". Settled with it:

- An MSAA event raised between a ready outpost's end and the listener resuming is lost; the gap is accepted (Dickson, 2026-10-21), being a few milliseconds of relay for an application whose outpost has just ended.
- `CoCancelCall` for MSAA calls is not built (Dickson, 2026-10-21): it ends a COM call but not a window-message acquisition, NVDA ends the entry on it and speaks nothing, and it costs about 12 microseconds on every COM call of the thread, so the worker's deadline and abandonment stay the bound (`experiments/cocancelcall-ends-an-msaa-com-call-but-not-a-window-message/readme.md`).
- Live reads of the focus's property events are struck: the ledger's "Name change on the focus" and "What an accepted MSAA change reads" already match NVDA (Dickson, 2026-10-20).
- The unit-test jobs run `cargo xtask warm-speech` before `cargo xtask ci`, because the Windows speech platform's first activation in a fresh runner session takes more than ten seconds, and upload `target/debug/logs` when they fail.
- `settings_search_suggestions` is local-only, because the runner's Settings raises no selection event for a first suggestion.
- A classic ancestor walk whose step from below the root fails, or finds no parent, reports the ancestors unknown, so Core keeps the previous chain (`docs/parity/focus-events.md`, "A focus whose element is gone before its read").
- No outpost connects to a new window after its probe, so a new File Explorer window's first item is queued within about 320 ms of its event ([the listener design](design/listener-without-connections.md), risk 11).
- The suggestions-closed sound played twice in Settings' search box matches NVDA (`docs/parity/changes-and-selection.md`, "Suggestions appearing and going away").

## M4.4, message types in their final home

The messages exchanged with the outposts and the listener are `verbatim_model::protocol`'s plain serde types, one enum per direction and role, each variant of a declared kind, with merging declared by the type and the supervisor's generations as typestates ([the messages build](design/ownership-messages.md)). It came before M5 because M5's extension API exposes those types.
