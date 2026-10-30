# Ownership audit: output and configuration

Read-only audit, 2026-10-30, of main at 5797c075, against the rules of `architecture-study.md` and its answers file: one owner per fact, pure reducers with calls as effects, queries with exactly one answer, no back-channel carrying another process's state, lifecycles as typestates, ownership enforced at compile time. Per Dickson's added requirement, a change is proposed only with evidence: a commit or review finding it would have prevented, or a named race or confusion it removes. Everything else is marked "keep as is".

## In short

- The output side mostly complies already. Its defects were ordering defects between threads of one process, and each fix has held. Three mechanisms already do what the model asks:
  - D17 hands an utterance's ending from one owner to the next;
  - `FocusNow` is a one-way snapshot from the reducer, which is its only writer;
  - the GUI's `GuiThread` token is neither `Send` nor `Sync`, so its thread rule is checked at compile time.
- The model needs one addition. Core sends its children configuration that is not a query and not `NodesHeld`:
  - `Fetches`, which changes when the theme changes;
  - `SetBackendOverride`;
  - `--classic-uia` at spawn;
  - later, M5's rule tables and hot reload, and M8's profile-dependent settings.

  The study's rule "Core-to-process messages are only queries and `NodesHeld`" has to admit a third kind, configuration. Configuration belongs to Core, is versioned, travels one way, and is applied between events. It is never a copy of what Core believes about the screen.
- Four changes have evidence. Each is listed under its fact below with that evidence:
  - the configuration store's file writes on the reducer and synth threads;
  - the settings mirror written before the synthesizer answers;
  - the key-origin side table;
  - the ownership of an utterance's ending once M6.3 hands valid speech on again.
- The rules do not cover real-time audio. A rule for it is proposed at the end.

## Processes and threads

- verbatim.exe:
  - the keyboard hook thread;
  - the router;
  - the reducer;
  - the speech queue thread (`verbatim-speech-queue`);
  - the synth thread (`verbatim-synth`);
  - the audio thread (`verbatim-audio`, MMCSS);
  - Windows' endpoint-notification thread, which calls `DeviceNotices::notify`;
  - the GUI thread, which is the process's main thread and runs the wx loop;
  - the GUI event thread (`gui_event_loop`);
  - the control plane's threads, which also inject gestures;
  - the WAV recorder, through the audio tap on the audio thread.
- `verbatim-synth-host.exe`: one process per synthesizer in use, with one thread. It is owned by the `HostedSynth` that lives on the synth thread.
- No other process belongs to this area. The GUI is a thread in Core, not a process.

## Facts, owners and verdicts

1. The lanes and the order of speech.
   - Owner: the queue thread.
   - Writers reach it through one channel, in send order: the reducer's `Speak` and `DropExpired`, the hook's `Cancel(key)`, the settings host's `ApplySetting` and `SwitchSynth`, and the mixer's `Ended`.
   - Verdict: complies. Keep as is.

2. An utterance's ending (D17).
   - Owners, one at a time: the queue thread until dispatch, then the mixer.
   - Copy: the queue thread's `handed_on` list, which copies the mixer's set of registered utterances.
   - Defects:
     - 3ebb7be0: the woken driver returned and had its utterance finished as completed, because the job was cancelled before the mixer. Two threads could end the utterance.
     - 21fb0e51: `handed_on` was judged again while the mixer still owned the endings, so speech handed on later was stopped.
     - 7baa1ac6: the documents claimed an order between `utterance_ended` and `marks_cancelled`, which run on two threads.
   - Verdict: complies since those fixes. Keep the mechanism.
   - Compile-time rule: `Source::register` consumes an admission token, so no code path can end an utterance it no longer holds. 3ebb7be0 shows this would have prevented a defect.
   - M6.3 changes this fact. See the milestones.

3. Core's focus as speech sees it: `FocusNow` and each utterance's `FocusValidity`.
   - Owner: the reducer, the only writer, through `Effect::DropExpiredSpeech`. It travels on the same channel as `Speak`, so speech and the focus it is judged against are ordered by construction.
   - Defects:
     - 39b31fdf and 87044d03: waiting speech was judged at the wrong moment.
     - b94d2ea2: object identity was decided in Core and in the speech manager apart. It is now decided by one rule, `ObjectKey`.
   - Verdict: complies, as the answers file says. Keep as is.
   - The open parity work goes to M6.3: the critical loss of still-valid speech, and the menu-item clause.

4. Which key caused a piece of speech: the fence.
   - Owners:
     - the hook thread gives out key numbers through the static `KEY_SEQUENCE`;
     - the global `KEY_ORIGINS: Mutex<VecDeque<(TraceId, u64)>>` is written by the hook thread and the control plane;
     - the reducer reads it in `execute` (`verbatim_input_windows::key_of`).
   - Defect: bf4fbedb. The first key's speech was heard after the second key's cancel. Fixed by the fence.
   - Change, with evidence:
     - The hook takes a lock that the reducer also takes. `docs/design/communication.md` section 6.2 says the hook "never blocks on a lock".
     - A trace whose origin has left the 1,024-entry table loses its fence silently.
     - M15's braille keys would be a third writer.

     The change: carry the key number in the input the hook already posts, as a field of the fact, and delete the table. No incident has been recorded, so this is low priority.
   - Compile-time rule: the reducer's `Speak` effect carries its cause as a typed field, not a lookup.

5. Pause.
   - Owner: the queue thread applies it. `paused` is an atomic copy, written only after the mixer has been told.
   - Defect: 714d95d4, a test race. The pause reached the mixer through the queue thread while the audio came through the synth thread.
   - Verdict: complies. Keep as is.

6. The mixer's state: sources, positions, the device, endings and sounds.
   - Owner: the audio thread. Other threads only enqueue requests or write under the mixer lock: the synth thread, the queue thread, the reducer's `play_earcon`, and the GUI's `play_sound`.
   - Defect: 54e9506c. A reopen used the played position from the previous poll, a stale copy of the device's fact, so audio was heard twice.
   - Verdict: complies. Keep as is.
   - The lock is held across WASAPI calls (`device.write`, `start`, `stop`, `queued_frames`). That is accepted (communication section 8) and is covered by the real-time rule below.

7. Device availability.
   - Owner: `DeviceNotices`. The notification thread writes it, and the audio thread reads it and acts on it.
   - Defects: 2f198c9f and 67ca167f. These were behaviour defects, not ownership defects.
   - Verdict: keep as is.

8. The live synthesizer and its engine settings.
   - Owners: the synth thread owns the driver. The host process owns the engine's state. `HostedSynth`'s copy of the values changes only after the host answers `SettingApplied`, and it is replayed to a replacement host.
   - Defects:
     - fe5fa4eb: before frames were tagged with their utterance, a cancel that crossed a `Done` could cancel the next utterance.
     - 2bdfffb7: a host owed a message across cancels and was never judged hung. Echoes were silent, the synth thread stalled for good, and Verbatim's exit hung.
     - 53551109 and 3ebb7be0: cancellation was polled.
   - Verdict: complies. Keep as is.
   - The owed-message tracking of 2bdfffb7 is a lifecycle that a typestate fits: Idle, Owed(since), Draining(set). Encoding it needs no new evidence beyond that defect.

9. The settings dialog's mirror (`SettingsHost`'s `HostState`).
   - Owners:
     - the GUI thread writes `current` before the driver answers;
     - the synth thread writes in `switch_ended`;
     - `commit` persists `current`.
   - Defect: 0d791a13. After the asynchronous switch of 510b7f82, a commit or revert made during a switch disagreed with the synthesizer that became active. Fixed with the owed commit and revert.
   - Change, with evidence: when a driver refuses a value (`SynthCommand::SetSetting` only logs it), the mirror keeps the refused value. The dialog shows it, `commit` saves it, and the next start logs it and skips it. This is a concrete divergence; no incident has been recorded.

     The change: update the mirror from the synth thread's answer, as `HostedSynth` updates its own copy.

10. The active synthesizer's display name.
    - It is decided in two places: Core's registry, localized, and the host's `driver.display_name()`. The host process never loads the user's locale. `DriverState` takes the host's name after a switch.
    - Change at M8, when a second locale ships: take the name from Core's registry only, since words belong to Core. The evidence is the confusion this would remove: the list and the active name in different languages. There is no defect while only English exists.

11. The configuration store (`Arc<Mutex<ConfigStore>>`, and `settings.toml`).
    - Writers:
      - the GUI thread: the speech commit and the theme's `persist`;
      - the synth thread: `switch_ended`, then `commit`, then `persist_fn`;
      - the reducer thread: `save_reader_settings` on `Effect::SettingsChanged` and on a Terminal page change.
    - Each writer writes the whole file atomically under the lock.
    - Readers: the synth thread through `SavedSettingsFn`, the GUI's hosts, and the hook's `DecisionConfig` at startup.
    - No update is lost, because every writer writes the same in-memory value under one lock.
    - Change, with evidence: the reducer writes a file, and it waits for a lock that the GUI thread holds while writing a file. `communication.md` section 14 says the reducer never blocks or does file I/O. There is no defect yet.

      The change: one owner of the file, a saver off the path to which owners hand changed sections, and each section written by one owner:
      - speech by the settings host;
      - reader settings by the reducer;
      - the theme by its owner, which is the GUI until M8 and Core from then on.
    - Compile-time rule: a section writer is a capability, and only its owner's module can construct it.

12. Reader settings.
    - Owner: the reducer's `SrState`. The store holds a persisted copy.
    - The Terminal page reads the store and sends only deltas, which the reducer merges, so a Verbatim+5 pressed meanwhile is never undone (643e25cc).
    - Verdict: complies. This pattern is the model for every M4.3 setting. Keep as is.

13. The active theme.
    - Owners: `ThemeHandle`, which the GUI writes and which the queue thread and the reducer's earcons read; the reducer's fetches, through `Input::Fetches`; the outposts' `Fetches`; and the store.
    - The GUI writes the handle first and then posts the fetches, so an utterance can be presented with the new theme before the reducer fetches what that theme needs.
    - Evidence that this order matters: 6869c298 had to guarantee it at startup.
    - Keep as is until M8. Profiles change the theme on attention changes, which makes the race routine, so at M8 the theme becomes a fact the reducer owns.

14. The hook's `DecisionConfig`. Read once at startup. Keep as is until M4.3, which needs a live path. See the milestones.

15. Locale. `verbatim_i18n::loader()` is a `OnceLock` set at startup, which is single-writer by construction. Keep as is. Before M8, check NVDA's source for whether a language change needs a restart.

16. GUI state.
    - Owner: `GuiCore` on the GUI thread, behind the `GuiThread` token, with a tested lifecycle state machine.
    - Defects:
      - 76180512: a shell-item request whose guard thread never started was never answered, so the list could not open again until Verbatim restarted.
      - 7070a340: a remembered icon rectangle, a copy of the shell's state, was clicked stale. It was replaced by a re-find query.
    - Verdict: keep as is, with one addition. 76180512 is exactly what the study's responder that answers "gone" on drop prevents, so `ShellItemsHost::request` should take such a responder.

17. The hidden frame marker.
    - The GUI owns it, and the outposts read it as a window property. It is an identity mark set once, not state.
    - Verdict: keep as is. M8's overlay windows need the same mark.

18. Speech results back to the reducer.
    - `MarkReached` comes from the audio thread, `UtteranceCancelled` and `UtteranceFailed` from the queue thread, and `SpeechCancelled` from the hook. Each is posted to the one port.
    - Defects:
      - 6e303a61: speech cut off outside the reducer left terminal output waiting forever.
      - 7baa1ac6: a failure was reported as a cut.
    - Verdict: complies. Keep as is.

19. Only the reducer speaks (6e303a61). The one exception is a GUI sound preview, by decision (87b4f284). It is held by convention.
    - Compile-time rule: `SpeechManager::speak` takes a capability that only the reducer thread holds. Evidence: the defect of 6e303a61 came from speech outside the reducer.

## What the rules do not cover

- Real-time audio. The audio thread runs at MMCSS priority. Today:
  - it holds the mixer lock across WASAPI calls;
  - the synth thread, the queue thread, the reducer and the GUI take that same lock;
  - its listeners take the latency ledger's mutex;
  - it allocates `notes` on every pass.

  No defect has been recorded, and the full suite has no underruns. Proposed rule:
  - the audio thread never waits on a lock that a lower-priority thread holds across a blocking call;
  - it never does I/O except the device's own calls;
  - it allocates nothing proportional to work on its hot loop;
  - any change to it is measured on underruns, beside the wake-up and CPU rule.

  M6.3's awake silence and earcon re-mix add work to this thread, so the rule should be stated before that work.
- Configuration as a kind of message, as in the summary.
- Causal stamps. Ordering across threads that share no channel is settled by stamps, not by owners: the key fence's sequence numbers, and the host's utterance tags. The model should name this, and require the stamp to travel inside the fact (item 4).
- Shared external resources. Two Verbatim processes must not hold one engine instance, just as two must not connect to one application. The host's one-owner rule covers this today. M6's sharing breaks it (below).

## Milestones

- M4.3, settings and indications.
  - What it adds: live settings for the hook (key handling from other applications, the multiple-press timeout), the synth thread (leading-silence trimming), the presenter (capital pitch, "cap", sound volume following the voice), the audio thread (the output device) and the reducer (review following focus, tooltips, notifications, expired-focus cancelling, word segmentation). It also makes sound-only announcements into queued utterances.
  - Rules it must follow:
    - each setting has one consumer thread and reaches it as a delta merged by its owner, as with reader settings;
    - the hook reads an immutable snapshot that one writer publishes;
    - expired-focus cancelling is decided in the reducer, by whether it emits `DropExpiredSpeech`, not again in the speech manager;
    - an announcement is cut by a key according to its effect kind, which the reducer chooses once.
- M6, the native synth host and Eloquence.
  - What it adds: an x86 host, AppContainer launch, an adapter DLL whose engine callbacks run on engine threads, and Wasm synthesizers in the host. The architecture document also lets a host be held by several users: the active synthesizer, a settings dialog listing its voices, an extension.
  - Rules it must follow:
    - one owner per host process, a registry, with users holding leases;
    - a non-active synthesizer's voices are a query with exactly one answer, "gone" if the host dies;
    - an adapter's callbacks only write frames and never call back into the engine;
    - the ABI's cancel and is-cancelled are the existing token.
- M6.3, the speech and audio pipeline.
  - Fixing the critical loss means handing still-valid utterances on again. The queue thread must then keep each handed-on sequence until its ending, and D17's handoff splits in two: the queue thread owns the utterance's one ending, and the mixer owns only each attempt's ending. Without that split, a re-handed utterance would be registered twice and could end twice. The evidence is the roadmap's critical-bug entry together with 21fb0e51.
  - The earcon re-mix ties a sound's ending to its utterance, with the mixer as the one owner.
  - The awake period is configuration sent to the audio thread as a request.
  - The real-time rule applies.
- M8, speech configurability, profiles, overlays and OCR.
  - Profiles: the active profile is owned by Core, since its triggers are attention and say-all. A switch is applied atomically, as NVDA's `atomicProfileSwitch` does. Triggers are suspended while the settings dialog is focused, as NVDA does (`appModuleHandler.py` 428, `config/__init__.py` 950), so the dialog never edits a layer that changes under it.
  - The theme and synthesizer settings become reducer effects, ordered with `Speak` on the speech queue's channel.
  - The config saver of item 11 is needed before this work.
  - Automatic language switching: the synth thread owns each language's voice.
  - Localization: words only in Core (item 10).
  - The overlays (highlight, curtain) are a new Core thread written only by reducer effects (`SetHighlight`), and their windows carry the hidden-frame mark.
  - An OCR read is a query with one answer, and one owner gives ids to its synthetic nodes.
- M11, audio formatting.
  - Theme switching is runtime configuration owned by Core (item 13).
  - Positional sounds take position and pan from the utterance, decided in the presenter and applied in the mixer, with no new thread.
- M15, braille.
  - What it adds: a braille host process like the synth host, the reducer's `Braille` effect, and display keys arriving as inputs.
  - Rules it must follow:
    - cursor tethering and the braille content belong to the reducer;
    - display keys get key numbers carried in the input, through item 4's change, so a braille key fences speech as a keyboard key does;
    - speech and braille stay in step through the existing marks, which come from the audio thread.
