# Walkthroughs: three end-to-end narratives

[The crate guides](crates/readme.md) describe each crate in isolation; these walkthroughs
stitch the crates into the three journeys that cover most of the system.
They are written for a reviewer asking "how does it all connect?", with
pointers into the per-crate guides for depth. Kept
current as of milestone M3 (commit 0a40653).

## 1. The life of a focus change

The user presses Alt+Tab, or an application moves focus. What happens,
in order:

1. **The OS raises events.** The newly focused window's process raises a
   UIA focus-changed event and MSAA winevents (focus, foreground —
   [Windows and messages](explainers/windows-and-messages.md) for the mechanics).
2. **The focus listener catches them.** The dedicated listener process
   (`verbatim-outpost.exe --listener`, decision D13) holds the only
   desktop-global registrations. It never calls into the app: it reads
   the event's own payload (cached UIA properties, raw MSAA ids), stamps
   a trace id and an observation timestamp, and writes a `FocusFact` up
   its pipe (`run_listener`; [verbatim-outpost](crates/verbatim-outpost.md)).
3. **The supervisor routes the fact.** In Core, the supervisor's
   lifecycle owner maps the fact to the target application's pid. If the
   app has no outpost yet, one is launched (`--target-pid`) on a helper
   thread; facts arriving before its `Ready` are held in arrival order,
   one per object and kind, then released in that order (`route_fact`,
   `HeldFacts`). Facts reach the outpost through its writer thread.
4. **The outpost's worker handles it.** The per-app outpost queues the
   fact on its intake queue, with NVDA's limiter rules; its single worker
   takes entries in order, so a foreground change is reported before the
   control behind it. The worker decides the window's backend (a kept
   verdict, else a server-side-provider probe), so exactly one backend
   reports, acquires the node (a UIA focus is resolved with one
   `focused_element` call), walks its ancestor chain, and emits an `Event`
   (trace id, timestamp, backend, window facts, normalized event) back up
   the pipe; a foreground change arrives as a focus on the window. Every
   call runs under the watchdog's deadline; a hang abandons the worker and
   a replacement continues with the queue (recovery ladder rung 2).
5. **The reducer decides what to say.** Core feeds the event into the
   pure `reduce` in `verbatim-core`: acceptance against the attention
   record (a foreground change moves it), NVDA's window rules,
   focus-ancestry diff (speak newly entered presentable containers
   first), NVDA-ordered property announcement, duplicate suppression. Out come `Speak` effects carrying structured
   utterances (D12) and the trace id.
6. **Speech renders and plays.** `verbatim-speech`'s queue thread
   flattens the utterance through the theme, dispatches at Interrupt
   priority (cancelling anything in flight), and the synth thread
   drives the driver into the WASAPI sink (`verbatim-audio-wasapi`), which
   emits `audio_started` with the same trace id — closing the latency
   timeline that began at step 2's observation timestamp.

Failure paths to know: events from a window the system reports hung are
dropped unread, and a call that hangs past its deadline abandons the
worker, which a replacement takes over from; a wedged outpost is ended by the heartbeat (`wedge_decision`, rung 3)
and replaced if its application holds attention; the listener crashing
is replaced at once and the attention application is asked to report
again with a focus-now query to cover the gap.

## 2. The life of a command keystroke

The user presses Verbatim+numpad8 (report current object):

1. **The hook decides.** The `WH_KEYBOARD_LL` hook thread
   (`verbatim-input-windows`) runs `verbatim-input`'s pure
   `DecisionMachine`: the chord is
   bound, so the keys are swallowed and an `EmittedGesture` (with a
   repeat count for multi-press semantics and a fresh trace id) is
   `try_send`-ed to the app — never blocking on the hook thread.
2. **The router resolves the script.** `verbatim-app` maps the gesture
   through the layout's binding table to a `ScriptAction` and feeds a
   `Command` input to the reducer.
3. **The reducer acts on its cursors.** For report-current, repeat 0
   speaks the navigator object from state; repeat 1 spells; repeat 2
   copies via the shared clipboard helper. For a movement command the
   reducer instead emits a navigation `Fetch` effect tagged with a
   query id.
4. **The outpost answers the query.** Core sends the target's outpost
   a `Navigate` (parent/next/previous/first-child); the outpost runs
   the backend call on the deadline-guarded pool and replies with a
   `NavigateOutcome` — a snapshot, or first-class `NoNeighbor`, or
   `Gone`.
5. **The completion returns to the reducer**, matched by query id (an
   intervening focus event snaps the navigator but does not cancel the
   user's in-flight move): a snapshot moves the navigator and speaks
   it; `NoNeighbor` speaks the edge message; `Gone` re-seeds the
   navigator from focus and announces it.
6. **Speech**, as in walkthrough 1, at the priority the command chose.

## 3. The life of an E2E run

`cargo xtask vm test --scenario start_menu_search` (or the same suite locally
against a `verbatim-agent` on this machine):

1. **The harness reaches the agent.** `verbatim-e2e` reads
   `VERBATIM_E2E_ENDPOINT` and connects over TCP to `verbatim-agent`,
   which runs *inside the interactive session* of the guest (or
   locally) — it refuses to start in a non-interactive session, since
   injected input needs a real desktop ([Tooling](tooling.md)).
2. **`Scenario::launch` boots a real Verbatim**: writes a
   `settings.toml` selecting the capture synth (no audio device
   needed; `VERBATIM_TEST_AUDIO=null`), launches `verbatim.exe`
   through the agent with stderr redirected to a readable file, waits
   for the control plane to answer through the agent's byte-relay
   tunnel onto Verbatim's named pipe, and opens a second tunnel
   dedicated to speech collection.
3. **The body drives and asserts.** The scenario injects gestures and
   keystrokes (the control plane's key injection speaks the same key
   vocabulary as the hook), waits for expected speech frames —
   matching queue-time frames only, so double emission can't fake a
   pass — and asserts ordering and content. Latency timelines are
   fetched, reported, and recorded rather than hard-asserted
   (VM scheduling variance made budget assertions flaky; commit
   9853ca2).
4. **Teardown always runs**: body and teardown each run under
   `catch_unwind`; `Scenario`'s `Drop` kills every launched process
   even on panic; artifacts (speech timeline, Verbatim stderr,
   per-process outpost and listener logs, the reducer flight-recorder
   dump taken before the quit) are collected pass or fail into
   `target/e2e-artifacts/<scenario>/`, and a `ScenarioSummary` file is
   written — which is what `xtask vm test` reads for its summary,
   never stdout parsing.
5. **The VM variant** (`xtask vm test`) additionally restores the
   guest to its checkpoint, renews DHCP (commit 278289f), deploys the
   freshly built binaries, and runs scenarios one at a time. The
   flight-recorder dump means a failure can be replayed
   deterministically against the pure reducer afterward
   (`verbatim-core`'s replay fixtures).

The trap that costs the most time, restated from [Tooling](tooling.md):
everything in walkthrough 3 requires an unlocked interactive desktop;
on a locked one, injected input goes nowhere and scenarios hang or
mis-assert.
