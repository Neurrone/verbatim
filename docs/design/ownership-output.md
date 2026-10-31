# The output build

Status: built on 2026-10-31. Designed against main at 98b0388d, with its adversarial review's findings (verdict: achievable) and Dickson's decisions of the same day. It applies [the ownership model](ownership.md)'s rules "One owner per fact" and the audio thread's part of "Who may block on whom" to the settings file, the settings dialog's mirror of the synthesizer, the mixer and the WAV recorder, and takes the synthesizer's display name from Core's registry. It ran beside the outpost's reducer build and shared no code with it. The refactor preserves what is heard, and this build changed no speech. The mechanism is in the crate guides of `verbatim-config`, `verbatim-speech`, `verbatim-audio`, `verbatim-audio-wasapi` and `verbatim-app` (`docs/crates/`); this document keeps the problems, the choices and the limits.

It replaced two recently built mechanisms and kept their behaviour. The settings host's owed commit and revert during a synthesizer switch (0d791a13) went, because a commit and a revert now run on the synth thread in order behind the switch, which gives the same outcome. The mixer's lock-free cancel by generation and atomic pause are the mixer half of the communication design's step 10, which was not adopted for latency (`docs/design/communication.md`, section 8); Dickson adopted real-time audio on ownership grounds on 2026-10-30.

## The problems

- The settings file had three writers, each writing the whole file under the store's lock: the GUI thread (the speech commit, the theme), the synth thread (the commit owed after a switch) and the reducer (reader settings after a toggle key). A slow disk or an antivirus scan in the GUI's write made the reducer wait on the lock, and every later key's speech wait behind it (`docs/design/communication.md`, section 6.6).
- The dialog's mirror of the synthesizer's settings kept a value the driver refused: the dialog showed it, the commit saved it, and the next start refused it again. A commit also read the mirror on the GUI thread, so one made before the synth thread's answer saved the refused value even after the mirror was corrected from the answer. NVDA saves the driver's own values, read when it saves (`nvda/source/autoSettingsUtils/autoSettings.py` line 163).
- The audio thread, at MMCSS priority, held the mixer's lock across WASAPI calls, and the synth, queue, reducer and GUI threads took the same lock. A normal-priority holder pre-empted inside it made the audio thread miss its period; a WASAPI call blocked by a device change stalled the reducer's earcon and with it all speech; the latency ledger's mutex, held by a control-plane reader, stalled the audio thread's listener. The mixer's cancel, pause and fail also blocked their callers until the audio thread carried them out, and the thread allocated on every pass.
- The synthesizer's display name came from the host process, which never loads the user's locale, so after a second locale ships the list would show Core's localized names and the active name would read in English.

## What was chosen, and why

- One saver thread owns `settings.toml`, with one non-`Clone` writer per section (speech, reader, theme) handed to the section's one owner. A writer posts its section to a completion port, which never waits; the saver writes the newest of each section once per batch. Reason: no thread on the path waits on a file, and one writer means no lost update between sections. The speech section's owner is the synth thread, since it already reads the section at every start and must commit after the driver's answers; the reader section's is the reducer, which publishes a view for the Terminal page; the theme section's is the GUI thread.
- The synth thread alone writes the mirror, reads every value back from the driver after each change, and answers with all of them; the Speech page numbers its changes, takes from an answer only the settings with no later change outstanding, and snaps a refused or clamped control back to the driver's value. A commit and a revert are queue events carried out on the synth thread in order behind every change and switch, and a commit saves what the driver has. Reason: one writer, and what is saved is what the driver holds, as NVDA saves.
- The audio thread owns the mixer's state with no lock. Each producing thread has a bounded single-producer lane (`rtrb`) built before the thread starts; the cancel is a generation and the pause a flag, set by the queue thread and read by the audio thread each pass; one pass drains the lanes, then reads the generations, then fires events, then mixes. Records going out (playback events, underruns, everything the thread is done with that owns memory) leave through a ring to a delivery thread at `THREAD_PRIORITY_HIGHEST`, which runs the listeners, logs and frees. Per-pass buffers are allocated when the device opens, and the WAV tap pushes into preallocated rings. Reason: a lock on the audio thread is a priority inversion on every producer, and a request that waits couples a producer to the device.
- The display name comes from Core's registry by the driver's id.

Alternatives weighed:

- Updating the mirror from the synth thread's answer alone: it does not stop a commit made before the answer from saving the refused value, so the commit moved to the synth thread.
- A hand-written park handshake or flags for waking the synth thread: the review found it racy with two writers of the wanted room. Dickson decided on plain events (2026-10-31): a level-set room event the synth thread waits on in a loop, so no wake can be lost and nobody stores a want.
- Keeping the settings host's owed commit and revert: replaced, as above.

## Departures from the design as built

- Pushes wake the audio thread only when the lane was empty (4d5c2c52), not after every push, because a record already in the lane means the audio thread has yet to take it and its pass takes the new one with it. The room event is set when a pass took any record and left the lane at most half full, and the delivery thread wakes the audio thread after a drain that took at least half the ring; reading whether a ring was full when a drain began lost wakes to pushes and pops made meanwhile.
- A format change no longer strands an utterance whose registration was still in its lane: it fails with "the audio device changed format", the delivery thread writing the reason so the audio thread allocates nothing. A speech lane's finish is always sent.
- A device format beyond the stores' reserve, which keeps a device period's margin, is muted until the device reopens. A WASAPI device that cannot make an event refuses to open, so the mixer plays silence rather than take a lock while playing to it.
- The exit's save deadline is its own constant, `EXIT_SAVE_TIMEOUT`, beside the exit sound's.

## Limitations

- The audio thread's wake-ups rose under key load, against the rule not to raise wake-ups on the speech path: `measure_keys_notepad` traced 5,832 before and 11,420 after the wake change, and the cause of the rest is unknown (`docs/performance.md`, "Threads after the output build"; `docs/roadmap.md`). The delivery thread adds about three wakes per utterance and one per mark, which used to happen on the audio thread; a mark and an ending reach the reducer and the queue one hop later.
- A change is on disk a write after it is made, and a kill in between loses it. A commit waits on the synth thread, so a host that has stopped answering delays it up to its 10-second limit; at exit, a commit still unwritten after the 2-second bound is lost and logged. Exit takes at worst 4 seconds after the GUI loop ends.
- Earcons beyond 256 waiting in a lane are dropped and logged; the lane fills only when the audio thread has stalled.
- A device format beyond 8 channels at 192 kHz is refused by the mixer and speech is muted until the device reopens.
- The races of the wakes and of the cancel generation's ordering cannot be produced by a test; the crate guides of `verbatim-audio` and `verbatim-speech` state the rules for a reviewer: a cancel generation is raised before the token is cancelled, a pass drains, then reads the generations, then fires, and an event is set after every push to an empty lane, every pass that frees room and every pass that pushed records.
- The audio rule of the ownership check (`xtask/src/ownership.rs`) reads text, so a method that allocates inside an allowed type is a review rule, held by the allocation test. The delivery thread's listeners still take the ledger's and the control server's locks, now off the audio thread.
- After a driver refuses or clamps a value, the changed control snaps back at once, where NVDA leaves it as set (`gui/settingsDialogs.py` lines 1564 to 1566). NVDA's refresh of the other controls after a voice change (lines 1601 to 1619) is a roadmap follow-up.
- Verbatim writes the file on every change; NVDA writes it on exit or on its save command, which this build did not change.

## Dickson's decisions (2026-10-31)

- Wakes are plain events, with no hand-written flags or fences.
- The 2-second exit bound stays. Settling cancels speech first, then posts its query, so only a host that has stopped answering can lose a commit, and the loss is logged.
- The synth thread owns the speech section, and a commit saves the driver's values.
- After a driver refuses or clamps a value, the changed control snaps back to the driver's value at once, a deliberate departure from NVDA.
- A full earcon lane drops the earcon and logs it.
