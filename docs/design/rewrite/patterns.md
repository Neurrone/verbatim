# Back on track: plan from the four architecture reviews (2026-11-28)

Approved by Dickson ("Proceed", 2026-11-28): architectural review, mechanical checks, the review-follow ruling, the desktop postPopup port.

## Dickson, 2026-11-28: not incremental

Each build removes one pattern class across the whole codebase, not a sample: Say for every shell phrase; test constructors for every record with literals banned; sealing for every state record; every known copy; every flag lifecycle as one enum; every per-owner map moved into its owner's record (sweep running); every read failure parsed once at the boundary; one shell-step function; single-feature queue hooks removed (audit running); the 22 untriaged findings triaged. Follow becomes a standing request (Dickson approved; record in ADR 0003). Ledger corrections in ledger-corrections.md.

## Order (earlier draft)

1. Checks that fail the build (worktree builder): shared-state ratchet; state-field ledger with categories, existing copies listed as known-copy debt that may only shrink.
2. Desktop postPopup port; review-follow design then build (main-tree builder).
3. Remove known copies in small behaviour-neutral commits, each shrinking the ledger:
   - Terminal, in the reviewer's order: memory's review anchor; places keyed by Core's anchor number; marks carry their line; marking at Enter into the rules; settings and focus passed with each input; entry_read typed; text crate's types in Core; (last, optional) rules' screen as the memory's row payload. About 430-530 lines.
   - Core: delete Kept.foreground (write-only); focus's process and window held once (Place owns, accessor); listener Stage in the record replacing Queue.readers; Follows.named into Application; run_executed carried in its entry.
   - Shell: one generic Say input replacing per-phrase GuiEvent/Input/Core arms (cuts a phrase from 9 files in 6 crates to 4 in 3); synth mirror rewritten after each job, deleting take_values_changed and values_changed; DeviceNotices folded into the audio thread's Outcome; GUI lifecycle one enum owning the panels; app status map read from Core's record.
   - Readers: listener uia_installed folded into Readiness; outpost reducer phases one enum; supervisor Generation from Core's Stage.
4. Cut files per change: one test constructor per state/message type with literals banned outside it (a copied-field removal touched 22 test files); one shell-step function shared by app, mockapp and desk; COM misses parsed once at the uia/ia2 boundary.

## Needs Dickson

- Outpost's Context::named becoming a standing request (ADR 0003 line).
- syn as an xtask dependency (not needed now: text scan).
- Terminal size: the 14x against NVDA comes from his ruled behaviours (row memory, marks, places, lingering, flood policy, key window), not copies; only he can trade behaviours for size.
- CLAUDE.md shortening once rules live in checks; process: no local e2e by default, parallel builders.

## Owner-record sweep (2026-11-28, at 7e981e21): 33 entries break the rule
- Leak: Core BackendCache.probed (focus/backend.rs:52) never pruned when a destroy is lost.
- Duplicate records of one owner (9): Core Queue.readers vs Record; app LiveOutposts and status map vs Core Record vs supervisor records; supervisor frames Stream.position vs app Live.position; supervisor records/apps two maps; spare_children vs Spares.waiting; supervisor Writers vs child records; listener one application in six collections (check_threads records, checking wakes, tells Applications, kept elements, schedule running/exited, reading wakes); uia registry reverse+elements; speech handed_on vs marked.
- One outpost node in seven side maps (store/mod.rs:178-198 + TextFacts.nodes): target the registry's per-node record.
- Outpost shell generations + parked: one map with a worker-state enum.
- Core per-owner side state cleared by hand: Follows.named, Terminals.models, TerminalFocus.focused, say_all, navigator, start_marker, Versions.last token, Versions.postponed, table late/recovery, ListenerView.
- control server Registry + Connections: one record per connection.
- 10 hand-written clean-up functions mark it: inbox::reader_ended, entries::reader_ended, entries::window_destroyed, outposts::application_exited, app on_outpost_ended, supervisor application_exited, listener ApplicationExited handler + checking::application_exited, outpost store::forget_nodes, worker::forget. Target: each disappears into the owner record's drop.

## Queue hooks (pending Dickson's approval): remove watched (ignored set to the outpost record); merge started + foreground_marker; fold exempt_from_cap into Place::Msaa; joining into wanted; dropped_at_arrival into arrival; Queue::timers into the menus' facts; delete Place::Other.ends_run and MsaaClass::Hide; keep ended_at_foreground separate.

## Root enumeration (2026-11-28, at 7e981e21)
- Ledger walker only follows types in the root's own crate: make it follow fields into other workspace crates. Loop-local thread state (saver.rs:131 settings; uia subscribe.rs:1062-1066 followers/unremoved/queue) needs a struct first. C++ GUI cannot be a root: its copies go by moving the lifecycle to Rust.
- 56 missing roots (ready-to-paste list in the agent report; key ones): app ReducerThread, ControlHandlersConfig, LatencyLedger, AppThemeHost, AppTerminalHost, SpeechSettings, Saver, key_reports Ports, ShellSpeechEvents; core Runtime, ListenerView, FlightRecorder, Spares, Liveness; supervisor Supervisor, Frames; speech QueueThread, SynthThread, SettingsHost; audio AudioLoop, Mixer, Fallback, WavRecorder, WasapiDevice; input DecisionMachine, HookState, ForegroundView, InputHook; control ControlServer; listener EventThreadState, ReadingThread, ListenerKept (replaces KeptElements), Schedule, CheckThreads, ForegroundWatch; outpost Shell, Outpost, Client, Charge, TextFacts; uia Registrar, Registration, Follower, NodeIdRegistry; ia2 EventThread, HookSet, NodeIdRegistry; uia-rops ProgramCache; synth Host, EspeakSynth, OneCoreSynth; process IgnoredProcesses, PortQueue, Sender, ChildIo.
- New copies: ignored set held 3x (SrState.ignored, Owner.ignored, Outgoing._ignored); harness::Parts readiness flags copy Core's record + `signalled` flag lifecycle; saver's whole Settings copies each section owner (and AppThemeHost.section is a third theme copy); QueueThread.paused vs mixer SourceShared.paused; QueueThread.focus_now copies Core's focus; AudioLoop.format/underruns mirrored into Shared atomics, running/ran_dry flags; supervisor Writers vs Child.writer; Frames position vs Live.position; control Registry vs Connections; uia UIA_READY + MTA_USAGE one lifecycle; ia2 DESKTOP_NAME_HOOK (0 sentinel) and MARKER_CALLBACK copy HookSet; ia2 registry Facts.event vs Kept.key (check); DecisionMachine.held_modifiers vs keys_down, repeat_count vs last_gesture; HookState.desktop_switches copies ForegroundView; outpost RequestContext.text copies Store.text; store ranges/anchor_nodes vs TextFacts.nodes; gui.cpp g_wake_window copies Shell.frame.
- Flag lifecycles: outpost Client uia+failed; synth-host Commands current/cancelled/ended/clean_end; ForegroundWatch started/console_events/unnamed_console; Schedule.stopping, CheckThreads.stopping; control SpeechHistory None for two states; ia2 ThreadTimer.id 0 sentinel.
- Snapshots to file as ordered-snapshot: ForegroundView, FlightRecorder.
