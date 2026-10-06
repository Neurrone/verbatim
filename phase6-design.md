# Phase 6 design, proposed 2026-10-06

A proposal for discussion with Dickson before an autonomous run. It covers
the three phase 6 steps in the handoff (`handoff-2026-09-02.md`, "Phase 6")
in the agreed order, then milestone M4. Each section ends with the
questions that need an answer before the work starts. Nothing here is
decided until Dickson agrees to it; once agreed, the decisions move into
`docs/architecture.md`, `docs/roadmap.md`, and the crate guides as the work
lands, and this file is retired like the handoff.

Decisions already made on 2026-10-06:

- The ARM64 remote-operations verification (R3) is deferred; M4 is built
  and verified on x64 only.
- Performance and memory are enforced in CI through counts and bytes,
  never through time. Wall-clock ratios are measured and reported by the
  end-to-end suite, which runs locally, and are not CI gates.
- The order of work is the NVDA transcript, then counts and memory, then
  the GUI port, then the rest of M4.
- The real-application scenarios run in the default end-to-end suite.
- The unbounded collections found by the survey are left alone for now;
  whether they matter in practice is not known.
- Terminal diffing happens in the outpost.
- Review cursor commands are part of M4.
- Typed-character echo is not switched off per program. Verbatim never
  guesses the program running inside a terminal from its window title, as
  Terminal Access does; an extension could.
- Both the Windows console host (conhost) and Windows Terminal are
  supported.
- `reduce` mutates the state in place (see "Core's state").
- The wxWidgets build time in CI is accepted, with caching.

## Step 1: the NVDA transcript

### Done

The add-on, `nvda-addon/verbatimTranscript.nvda-addon`, is committed,
installed in Dickson's NVDA, and verified live in Notepad
(`docs/nvda-transcript.md`). The agent has a `SendKeys` request, and
`cargo xtask nvda capture` presses keys and prints what NVDA queued after
each one.

### Proposed remaining work

1. Capture NVDA on the three deferred real-application scenarios, whose
   by-hand recipes are in `docs/roadmap-done.md`: an Explorer folder
   window, a Settings-app toggle, and Start-menu search results. Then run
   the same keys against Verbatim, compare, and either fix Verbatim or
   record the difference in `docs/parity.md` as intentional.
2. Automate each of the three as an end-to-end scenario with assertions
   written by hand from the transcript. These are the live test of the
   attention model on real applications, which the handoff names as the
   purpose of this step.
3. Capture NVDA reading Verbatim's own menu and settings dialog, as the
   expected readings for the GUI port (step 3). This needs Verbatim in
   test-audio mode and the share-modifier setting, so the two readers use
   different modifier keys. Also capture what NVDA does when Enter is
   pressed on Cancel and on Apply in its own settings dialog, which
   decides the expected behavior for audit item 7.

A capture takes the desktop over, like any live run, so this work happens
only during an agreed autonomous run or with Dickson present.

## Step 2: counts and memory, enforced in CI

### What the code looks like today

From the survey of 2026-10-06, which corrects parts of the handoff:

- The latency ledger (`crates/verbatim-app/src/latency.rs`) records times
  per trace and stage. The stage breakdown reaches only a log line; the
  control plane's `LatencyRecord` carries three times, and
  `verbatim-inspect latency` prints those three.
- There is no single wrapper around cross-process calls. The UIA calls
  are in `verbatim-uia`'s `client.rs` and a few other files; the MSAA
  calls are spread through `verbatim-ia2`'s `acquire.rs`. There is no
  IAccessible2 code yet, so the handoff's "IA2 wrapper" means the MSAA
  one. All calls run on each outpost's single worker thread.
- `verbatim-uia-rops` is a nine-line skeleton; no remote operation exists.
- A steady-state UIA focus change against mockapp costs about 2 or 3
  cross-process calls today, plus 3 for a list's selected child. An MSAA
  focus change costs about 9 calls per ancestor hop.
- Core's state has no node map, so the handoff's "hundred-thousand-node
  state" does not exist yet; what grows today is the focus ancestor chain.
  `reduce` clones the whole state per input.
- No global allocator exists anywhere in the workspace.
- Unbounded collections found: the arbitration cache (entries removed only
  on an MSAA window-destroy event), the listener's urgent queue, the
  intake's waiting and batch maps, the request table (no timeout), the
  shell's focus-now set, the UIA registry (no counterpart to the MSAA
  registry's `forget_window`, already noted in phase 5). The flight
  recorder and latency ledger are bounded by entry count but hold text of
  any length.

### The design

Everything CI enforces is a count that cannot vary from run to run.

1. **The operation ledger**, a new `docs/performance.md`. For each
   operation it states the minimum number of cross-process calls per
   backend, the number today, and the target. The operations are a focus
   change (UIA and MSAA, cold and steady state), arrowing through a list,
   an object-navigation step, an interrupt, and, once M4 builds them,
   typed-character echo and a terminal output line. It also defines cold
   and warm, and how a cancelled trace is counted.
2. **A call counter in the UIA and MSAA wrappers.** Each cross-process
   call goes through a small counting helper that increments a
   thread-local count, by kind of call. The outpost worker reads and
   resets it around each entry it processes and sends the counts with the
   event's timing. The ledger stores them per trace, the control plane's
   `LatencyRecord` gains the stages and counts, and
   `verbatim-inspect latency` prints each stage as time, count, and ratio
   to the floor in the operation ledger.
3. **A hit counter in mockapp's providers**, one atomic per provider
   method, read by the test through a synchronous window message. The
   tests built on `tests/slow_application.rs`'s shape, a real outpost in
   the test process driving a real mockapp, assert both counts exactly:
   the client-side call count and the provider-side hits, for each ledger
   operation on each backend. These tests already run in `cargo xtask ci`.
   An exact assertion is a ratchet: a regression fails CI, and an
   improvement must update the number deliberately, in the same commit as
   the ledger document.
4. **Core's allocation invariant.** A test binary in `verbatim-core` with
   a counting global allocator, counting per thread so the parallel test
   runner does not interfere. It asserts that a reduce step allocates the
   same number of bytes for a small state and a large one, for a focus
   event and a navigation step today, and for a caret move and a terminal
   line once M4 adds them. Today "large" means a deep ancestor chain and
   held navigator; M4's text model makes the document the axis that
   matters.
5. **The representation decision**, needed before M4's text model lands.
   Proposed: `reduce` takes the state by mutable reference and changes it
   in place, instead of cloning it. Replay stays deterministic, because
   determinism needs only no I/O and no clocks, and the flight recorder
   already records inputs rather than states. Large immutable parts, such
   as a document's text, are shared behind `Arc`. This removes the
   whole-state clone, so the invariant holds by construction rather than
   by keeping the state small.
6. **Bounded memory for text.** The flight recorder and latency ledger
   become bounded by bytes rather than entries, with a test that feeds
   them large texts, since M4's events carry text bodies. The shell's
   per-event clone for the control plane moves after the subscriber
   check. The other unbounded collections are left alone (decided above).
7. **Calibration in the end-to-end suite.** Each scenario measures the
   cost of one cross-process call against its own target application in
   the same run, and reports each stage's time as a ratio to that floor.
   The report is printed and saved with the artifacts; it is not a gate.

### Questions

- How Core stores its state: see "Core's state" below.

## Step 3: the GUI port

### What exists

`crates/verbatim-gui` is 2,599 lines. The parts that survive in Rust are
`plan.rs`, `shell_items.rs`, `hidden_frame.rs`, `foreground.rs` (except one
handle conversion), and `tray_list::click`. The widget code (`lib.rs`'s
frame, tray, and menu; `dialog.rs`; `list_dialog.rs`) is rewritten in C++.
Loadstone's C++ GUI is implemented (`app/src/main.cpp`, 1,254 lines) and
is the reference for the cxx bridge, the typed page models, the callbacks
class, marshalling with `CallAfter` and an alive flag, and the
`wxAccessible` subclass. The build shape is not copied (decision 5 of the
handoff): Rust keeps `main`.

### The design

- **Build.** `crates/verbatim-gui/build.rs` downloads wxWidgets 3.3.3
  (the version wxDragon pins today) and checks its hash, then builds only
  the base and core libraries statically with CMake. On x64 it uses Ninja
  with `/MD` and RelWithDebInfo; on ARM64 it uses the Visual Studio
  generator with the `host=ARM64` toolset, as `verbatim-synth-espeak`
  already does. `cxx_build` compiles a small `cpp/` directory against it.
  The recipe is adapted from wxdragon-sys's build script, not rewritten.
  The wx build goes to a fixed directory keyed by version and
  architecture, and CI caches it, because a build inside the crate's own
  output directory would be redone on every CI run. Removing wxDragon
  also removes bindgen, so libclang and all its plumbing go away.
- **The bridge.** Strings are resolved in Rust through typed message
  functions, so C++ never sees a Fluent id and every id is checked at
  compile time. Shared structs carry the page models (the Speech page and
  its controls, a list dialog). C++ calls an opaque Rust `GuiCore` for
  menu choices, key routing, setting changes, commit, revert, list
  activation, and close. Rust calls C++ to run the loop, wake it, show the
  menu, open or close a dialog, and shut down. `GuiHandle` holds a real
  channel sender plus the wake call, replacing today's global queue.
- **Reentrancy.** Menus and modal dialogs run nested event loops that call
  back into Rust, the class of bug `lib.rs` already documents. `GuiCore`
  is never borrowed mutably across a call into C++.
- **Audit item 7.** The cause is that today's Enter handler is attached
  to every button and always commits and closes, and that wxDragon offers
  no dialog-wide key hook, so the shortcuts never fire inside the Speech
  panel. The port uses one `wxEVT_CHAR_HOOK` on the dialog, which sees
  keys from every child, and asks a pure Rust routing function what to do:
  Control+Tab and Control+Shift+Tab change category from anywhere,
  Control+S applies, Enter on a focused button activates that button,
  Enter on the synthesizer name opens Change, and Enter elsewhere is OK.
  NVDA's dialog sends every Enter to OK, so the transcript from step 1
  decides whether Verbatim's Enter-on-Cancel behavior is an intentional
  difference.
- **Order.** First, on wxDragon still, extract the pure Rust parts (key
  routing, the dialog lifecycle, the settings model) with tests, and put
  the real sender behind `GuiHandle`. Then prove the build script in
  isolation. Then switch in one commit, since two static wxWidgets copies
  cannot link into one binary: frame, tray, and menu; the settings dialog;
  the list dialog; then remove wxDragon and libclang.
- **Verification.** The existing GUI scenarios (`menu_and_settings_dialog`,
  `switch_to_onecore`, `object_navigation_in_settings`,
  `rapid_tabbing_in_settings`, `synth_host_crash_recovery`,
  `notepad_and_verbatim_menu`) pass before and after, plus a new scenario
  for item 7: change the rate, Tab to Cancel, press Enter, reopen, and
  check that the rate was reverted. The NVDA capture of the GUI from step
  1 is compared before and after.

### Questions

- Decision D4 says a Rust-owned callbacks object drives the dialogs; in
  this design, C++ calls into Rust and Rust calls C++ functions. The D4
  wording would be amended to match.

## M4: text, editing, and terminals

### Scope from the roadmap

The roadmap's M4 (`docs/roadmap.md`) is:

- Opening work: the latency stage ledger and UIA remote operations for the
  focus ancestor walk. The ledger is step 2 above.
- TextPattern support in the model; caret tracking, typed-character echo,
  and character, word, and line navigation; say-all with index-mark
  continuation.
- Text runs carry formatting attributes as utterance spans (spelling and
  grammar errors, font and color where exposed), so M11 has data for
  formatting sounds.
- A minimal character-description table, so character navigation says
  "comma" on a comma.
- Remote operations for the ancestor fetch on focus and for terminal text
  ranges. The ARM64 verification is deferred (decided above).
- Windows Terminal: announcing new output from a diff, with a flood
  policy; "cat a huge file" is an end-to-end latency test.

Exit: editing in Notepad and Terminal sessions is solid, and the terminal
flood test shows bounded latency and no hang. The remote-operations exit
criterion is two round trips per steady-state UIA focus change against
mockapp, and a terminal flood wall-time ratio under two.

### What Terminal Access for NVDA teaches

`../Terminal-Access-for-NVDA` is a large NVDA add-on (about 16,000 lines of
Python, GPL-3.0-or-later) layered over NVDA's own terminal support. It is
useful as a catalogue of problems and policies, not as code to port; its
2,000 tests run only against a mocked NVDA. Ideas worth adopting, written
afresh from prose for the reducer:

- A flood policy by burst size. It speaks small bursts in full (up to 3
  lines), the tail of medium bursts after a tone (4 to 20 lines), and a
  count for large ones. Blank lines are dropped.
- A caret event that follows output within a short window is treated as
  output, not as navigation.
- Runs of a repeated symbol are condensed ("4 dash").
- Lines are located by content or offset, never by counting newlines,
  because wrapped and padded rows make UNIT_LINE disagree with the text.

Edge cases M4 must handle, several of which the add-on gets wrong:

- **Passwords.** NVDA holds typed characters in a terminal until the
  screen changes, so an unechoed password prompt is never spoken. The
  add-on bypasses this. Verbatim must follow NVDA, with an end-to-end test
  using a `Read-Host -AsSecureString` prompt.
- **Scrollback limit.** A diff by line position breaks once the scrollback
  is full and the top lines are discarded. Diff against the tail, or
  against the visible ranges.
- **Two sources of output.** Windows Terminal can report output through
  UIA notifications as well as text changes. Use one, or output is spoken
  twice. NVDA defaults to diffing.
- **Large reads.** A 5,000-line walk one line at a time froze NVDA. Large
  reads go through remote operations and are capped.
- **Alternate screen.** Comparing ranges across a switch to or from a
  full-screen program fails with an error.
- **Tabs.** Several Windows Terminal tabs share one window handle.
- **Tab completion** must not disable the filter that stops typed
  characters being echoed twice.

### Proposed breakdown

Each part ends with live verification and its counts in the operation
ledger.

1. **Remote operations for the focus ancestor walk** in
   `verbatim-uia-rops`: one call returns the focused element's ancestors
   up to the first one Core already knows, and the selected child of a
   list. Target: two round trips per steady-state UIA focus change against
   mockapp, asserted exactly.
2. **The text model in Core**: a caret position, the current line and its
   neighbors, and selection, in the representation decided in step 2. The
   outpost reads text through TextPattern for UIA and through edit-control
   messages for plain Win32 edit controls (Notepad's classic control, if
   the current Notepad still exposes one).
3. **Caret tracking and navigation reporting**: arrow keys, Home, End,
   word and paragraph movement, Backspace and Delete, and selection
   changes ("selected", "unselected"), following NVDA's wait-for-evidence
   rules in `docs/nvda/editable-text-and-terminals.md`. Plus the
   character-description table.
4. **Typed-character echo**, with NVDA's character and word echo settings
   and its password rule.
5. **The review cursor**: reading the current line, word, and character
   without moving the caret, and moving the review position, with NVDA's
   key layout (`docs/nvda/review-modes.md`).
6. **Say-all** with index marks, continuing from where speech stopped,
   following `docs/nvda/speech.md`.
7. **Formatting spans** on text runs: spelling and grammar errors, and
   font and color where exposed. Data only; nothing is spoken about
   formatting until M11.
8. **Terminals**: Windows Terminal and the console host (conhost), both
   through UIA. The two share one UIA implementation, Microsoft's terminal
   code base, and conhost's is complete from Windows 11 22H2, which every
   supported Windows 11 release now includes, so one code path serves
   both and NVDA's legacy console support (console APIs, reading only the
   visible screen) is not needed. New output is detected by diffing in the
   outpost, not by Windows Terminal's UIA notifications: NVDA still
   defaults to diffing because its notification mode speaks passwords and
   misreports output over 1,000 characters, and conhost has no
   notifications at all. Reads go through remote operations. The flood
   policy is configured on a new Terminal settings panel, which needs the
   GUI port. The terminal flood end-to-end test measures bounded latency.

### Questions

- The Terminal settings panel, agreed 2026-10-06, with both limits at 30
  (see "The flood policy, reconsidered"):
  - "Report new output", a checkbox, on by default, with a command to
    toggle it, Verbatim+5, NVDA's key for its "report dynamic content
    changes" toggle. It silences a noisy window for a while without
    leaving the terminal. NVDA's setting covers all dynamic content, live
    regions included; Verbatim's covers terminals until M6 adds live
    regions, when it widens to match.
  - "Lines spoken in full", a number, default 30: up to this many lines
    of output waiting to be spoken are all spoken.
  - "Last lines to speak", a number, default 30: when more are waiting,
    the older ones are replaced by "skipped N lines" and the last lines
    are kept. This is the only
    behavior for longer output for now, so it is not a choice. NVDA tried
    a cap and reverted it (see "The NVDA update"); Verbatim's differs in
    that nothing is lost, since every line stays reachable with the
    review cursor.
  - "Speak passwords", a checkbox, off by default, NVDA's "speak passwords
    in all enhanced terminals": typed characters are held until the
    terminal shows them.

  Fixed behavior, not settings: blank lines are dropped, and newer
  output never cancels older output still waiting to be spoken. NVDA's default speaks every new line of a batch, up
  to the last 100, and a newer batch cancels whatever of the previous one
  is still unspoken, as does any speech cancellation; a change of a
  single character is ignored as probably typed. Terminal Access's policy
  (full, tail with a tone, count) is the alternative.

## Terminal output: notifications or diffing

Measured on this machine on 2026-10-06, with Windows Terminal 1.24 and the
Windows 11 console host, through a UIA client written for the purpose
(not committed):

- **Trailing whitespace is the terminals' doing, not NVDA's.** Both
  terminals (they share one UIA implementation) pad every line with
  spaces to the window's width, in the document text, the visible range,
  and the line unit alike. After 5,000 lines in a 120-column Windows
  Terminal, the document text was 610,244 characters, of which 526,330
  (86 percent) were padding.
- **NVDA's diffing reads the whole buffer on every change.** For these
  terminals NVDA's text is the whole document (`behaviors.py`,
  `LiveText._getText` with `POSITION_ALL`; it is bounded to the visible
  screen only for older consoles, in `winConsoleUIA.py`), and it diffs
  it character by character (`diffHandler.py`, diff-match-patch), once
  per text-change event. Reading 610 KB took 6 ms here, before any diff;
  the scrollback holds up to about 9,000 lines, and the read and the diff
  repeat for every change during output. That is the cost that makes
  NVDA slow in busy terminals, and it comes from what NVDA reads, not
  from diffing itself.
- **Windows Terminal sends output notifications; the console host sent
  none.** During a 5,000-line flood Windows Terminal raised 86
  notifications with the activity id `TerminalTextOutput` within about
  830 ms, each about 1,000 characters of the raw output with no padding,
  cut at arbitrary points, often in the middle of a line or a word. The
  console host raised no notifications during a 2,000-line flood, so
  notifications cannot be the only mechanism.

How each would work in Verbatim:

- **Notifications** (Windows Terminal only). The outpost receives output
  as it is written and never reads the buffer. It must join the chunks
  back into lines, since they are cut anywhere, waiting briefly for the
  end of a partial line. Every character written arrives, so the count
  for "skipped N lines" is exact. But the notifications carry what was
  written, not what the screen shows: a program that rewrites a line in
  place (a progress bar) or redraws the screen (`less`, `vim`) produces a
  stream of redrawn text, and echoed typing arrives as output, so it must
  be reconciled with typed-character echo; NVDA's notification mode
  speaks passwords for that reason. Background tabs notify too.
- **Diffing** (both terminals). The outpost reads text when the terminal
  reports a change, and finds the new lines. Verbatim avoids NVDA's cost
  by never reading the whole buffer: the outpost keeps a text range
  anchored at the end of the text it last read, and on a change it asks
  how many lines lie between that anchor and the end of the document
  (moving a range endpoint by line returns the count without
  transferring text), then reads only the lines it will speak: all of
  them for a short burst, the last few for a flood. The cost is a few
  round trips per change, independent of the scrollback's size. The
  trailing padding is stripped in the outpost, and text-change events
  that arrive while a read is in progress are coalesced into one more
  read. The edge cases are where the anchor stops meaning "the end of
  what was read": the scrollback is full and its oldest lines are
  discarded, the screen is cleared, or a full-screen program switches
  buffers. In those cases the outpost falls back to comparing the visible
  screen with the one it read last.

### Why notifications exist, and what went wrong with them

From NVDA's history and the Windows Terminal repository, researched
2026-10-06:

- Windows Terminal added output notifications in 2022
  (microsoft/terminal#12358) to spare screen readers the cost of diffing
  the buffer on every text change, after NVDA froze under console floods
  (nvaccess/nvda#11002). NVDA's author of console support hoped they
  would avoid text-change floods altogether.
- NVDA shipped them only behind a flag (nvaccess/nvda#14047), still off by
  default; the tracking issue (#13781) is open. The problems, none fixed
  upstream:
  - Windows Terminal cuts each frame's output into 1,000-character pieces
    because of a speech API limit, and since its pass-through console
    mode the pieces follow how the program writes, so a word can arrive
    in two halves; the terminal cannot know whether more text is coming
    and its maintainers decline to add timeouts.
  - Everything a program prints counts as new, including a prompt redrawn
    in place, with no position and no deletions.
  - Windows Terminal filters echoed typing by matching recent key presses
    in upper case, so shifted punctuation is spoken twice and passwords
    leak; one release announced the whole command line on every key.
  - Output from an unfocused tab is discarded, not delivered later.
  - There is no way to detect support; only Windows Terminal's control
    raises them, and the console host never will.
- NVDA's diffing troubles were what it read, not diffing as such: the
  whole buffer every change, and thousands of blank rows on older
  consoles (#14689). The flood freezes were fixed by coalescing events in
  C++ (#14888). Diffing only the visible screen made output choppy when
  it scrolled (#12974), so NVDA compares by line there.
- NVDA's cap on flood output was reverted because skipped output made
  terminals unusable (#20888, #20898).

Lessons applied to the design:

- The outpost needs a diff engine whatever else it does, because the
  console host has no notifications, so notifications are not used.
  Windows Terminal's notifications are blocked while diffing, or output
  is reported twice.
- The anchor is the start of the last line read, not the end of the
  text: the last line is where prompts, progress bars, and line editing
  rewrite in place, and it is compared character by character so only
  what changed is spoken. Lines above it are compared by line.
- A range does not report its own invalidation: once the scrollback is
  full, ranges keep their row numbers while the text moves up beneath
  them. The outpost keeps the anchor line's text and checks it on every
  read, and treats an error comparing ranges (the alternate screen) as
  invalidation.
- Trailing blank lines and padding are trimmed before counting or
  speaking, everywhere terminal text is used: new output, reading a
  line, say-all, and the review cursor's line and word reading. The rule
  is language-independent: trailing characters with Unicode's White_Space
  property are removed, not only ASCII spaces, and no word or language
  rules are involved; spaces inside a line are kept. The review cursor
  can still stand on a blank cell past the text (see M4's review
  cursor), but reading the line does not speak the padding.
- Text-change events stay the trigger and the gate for the password
  rule, coalesced in the outpost with no fixed delay. Windows Terminal
  has missed text-change events before (microsoft/terminal#10911), so its
  notification may serve purely as an extra signal that something
  changed, never as text.
- The fallback for an invalidated anchor compares the visible screen by
  line.

### Full-screen programs and the alternate screen

The detection floated earlier, treating a document no larger than the
screen as the alternate screen, would misfire: a new terminal with a few
lines of output, or one just cleared, also has a document no larger than
the screen, and would go silent. Some full-screen programs also draw on
the main screen (`less -X`). With diffing it is not needed: a full-screen
program's redraw is a change on the visible screen, compared by line, so
only lines that changed are spoken.

### The flood policy, reconsidered

What NVDA users complained about (nvaccess/nvda#20888, reported against
2026.3 beta 2): ordinary, short output stopped being read. `git pull`
sometimes spoke only the shell prompt instead of "Already up to date",
and `git log` spoke only its last line instead of reading from the top.
The cause was not the 100-line cap itself but the rule that came with
it: each batch of new lines was spoken from a generator, and a newer
batch cancelled whatever of the previous one was still unspoken. Output
often reaches the screen in several batches (the result, then the
prompt), so the prompt's batch cancelled the result. NVDA reverted both
changes in the beta (#20898); its reason was that output was skipped and
users could not read it, and it would accept another attempt only with "a
way of safely pumping the text that avoids performance issues but also
avoids skipping useful content".

NVDA's behavior now: every line of every batch is queued for speech in
order, at normal priority, with no cap and nothing superseded by newer
output. What stops it is the user: any key press except Shift cancels all
speech, queued terminal output included (the "speech interrupt for typed
characters" setting, on by default; Shift pauses instead). So a flood is
spoken until the user presses a key.

What this means for Verbatim:

- Newer output must never cancel older output still waiting to be
  spoken; the rule proposed earlier ("a newer burst cancels whatever of
  the older one is still unspoken") is dropped. It is exactly what broke
  NVDA's beta.
- A cap per batch would also misfire, since one command's output arrives
  in several batches. The cap applies instead to the backlog: lines are
  queued in order as they arrive, and when more lines are waiting than
  the limit, the oldest waiting lines are replaced by "skipped N lines",
  keeping the most recent ones. Output shorter than the limit is never
  touched, however many batches it arrives in.
- The limit is the decision. With a limit of 5, a 20-line `git log`
  becomes "skipped 15 lines" and its last five lines, which is the
  experience NVDA's users rejected. A limit around a screenful or more
  keeps ordinary command output whole and still bounds a flood.

### Generic, not an app module

NVDA's support for the console host and Windows Terminal is in its core,
not in app modules: an overlay class chosen by the control's UIA class
name (`TermControl`, and `WPFTermControl` for the terminal embedded in
Visual Studio) or the console's window class
(`NVDAObjects/UIA/__init__.py`, `winConsoleUIA.py`); only other terminals
such as Tera Term get app modules. Verbatim does the same: terminal
behavior is a generic behavior keyed by the control, so the terminal
embedded in Visual Studio gets it too, and app modules (M5) adjust only
one application's quirks. Architecture section 7's sentence placing
"Terminal-specific behavior" in app modules is amended accordingly.

## Earcons

Brought forward from M11 into this milestone at Dickson's request on
2026-10-06. The M11 themes, which map any span to a sound by
configuration, stay in M11; this milestone builds the mechanism and the
first sounds.

### What exists

- `Effect::PlayEarcon(Earcon)` is defined, with one variant,
  `AppNotResponding`, but nothing emits or executes it.
- D17 designed the speech sequence so that a sound item can join it: a
  sound placed in an utterance starts when playback reaches it, overlaps
  the speech that follows, and is cancelled with its utterance.
- D16 requires every sound to be rendered as PCM through Verbatim's
  mixer, so recordings and the end-to-end suite capture it.
- NVDA plays sounds in two ways (`docs/nvda/sonification.md`): in the
  speech stream, positioned between words and cancelled with the speech
  (spelling errors while reading, capital-letter beeps); and immediately,
  outside the speech queue, never cancelled by speech (mode switches,
  progress bars, start and exit).

### Proposal

- **Two ways to play, as in NVDA.** A sound item in the speech sequence
  for sounds tied to words, and an immediate sound,
  `Effect::PlayEarcon`, played on its own mixer source, for state changes
  that must not wait for speech.
- **Sound sources.** Tones generated in code (a sine at a frequency and
  duration, with a short fade so it does not click), which cover beeps
  and progress tones, plus short recorded sounds stored as WAV files in
  the repository and decoded once at startup. Sounds are resampled to the
  mixer's format when it opens.
- **Settings.** The Theme panel, below.
- **First sounds in this milestone**: the terminal's "skipped N lines"
  cue, the application-not-responding cue (already defined), spelling and
  grammar errors while reading text (from M4's formatting spans), a
  progress-bar tone, and the capital-letter beep for character
  navigation (an NVDA option).
- **Testing.** The control plane's speech stream and the end-to-end
  suite's speech collector report a sound as an item in the stream, such
  as "sound: spelling error", so scenarios assert sounds like words.

### Sounds

Agreed 2026-10-06: NVDA's sounds for now (`nvda/source/waves`). They are
data, not code, so the provenance rule for code does not apply to them;
they live in a top-level `sounds/` directory shared by every platform,
with a note of their origin and licence (NVDA distributes them under its
GPL), and are installed beside the program. The platform-neutral code
only loads files from that directory, so a macOS build would use the same
sounds. Generated tones cover beeps and progress tones.

### Research: how others expose sounds

Research
of 2026-10-06 covered JAWS's Speech and Sounds Manager, NVDA's Audio
Themes and Unspoken add-ons, Earcons and Speech Rules, Emacspeak,
VoiceOver, and Narrator.

What the others do:

- JAWS: a scheme maps control types, states, text attributes, fonts,
  colors, indentation, and HTML elements to one behavior each (say
  nothing, speak replacement text in a voice, play a sound, speak the
  text in a voice or language). Schemes are chosen per application, cycled
  with a key, and stored as INI files. Complaints: hard to discover (a
  dialog of ten tabs and wizards); one behavior per item, so a sound for
  "check box" and a sound for "checked" conflict; voice aliases must
  exist in every voice profile; editing a scheme silently copies it as
  "modified". It has a training mode that plays the sound and then says
  the word.
- NVDA add-ons map roles to sound files only, as a folder of files named
  after the role, may leave roles out and fall back to the default,
  normalize loudness, preview a theme as you arrow through the list, and
  all grew an option for sounds during say-all.
- Emacspeak maps text properties to voice changes defined as relative
  overlays on the current voice, independent of the synthesizer; sounds
  are a separate layer. VoiceOver lets each item speak, change pitch, or
  play a tone, and bundles settings per application as "activities".
  Narrator has one checkbox, "play sounds instead of announcements", for
  five cues.

### Themes: one model for verbosity, speech, and sounds

Revised 2026-10-06 with Dickson. A theme is a complete collection of
indications and their settings: for every kind of thing Verbatim can
report, whether it is reported, and if so how, with the sounds, words,
and voice styles that go with it. It is not audio-only: it decides what
is spoken for a user who wants speech alone, and braille joins it in
M15. So the name is not "audio theme": it is "theme" (agreed
2026-10-06), the name D12 already uses.

NVDA's precedent: "Report spelling errors" takes speech, sound, both, or
off (and braille, as a flag set, `reportSpellingErrors2` with
`ReportSpellingErrors` in `config/configFlags.py`), and "line
indentation" takes speech, tones, both, or off (`reportLineIndentation`).
Its other verbosity settings are on-or-off checkboxes. Verbatim
generalizes this to every indication.

The parts:

- **The indication catalogue**, defined in code: every kind of thing
  Verbatim can report, with a stable id and a category. Roles ("link",
  "heading level 2"), states and their negations ("checked", "not
  checked"), properties (description, position "3 of 7", shortcut key),
  text attributes (spelling error, bold, font change), structure
  (entering a list, leaving a table, blank line, indentation, skipped
  lines), and events (browse or focus mode, not responding, error,
  progress, suggestions). It grows with the features: this milestone has
  what Verbatim reports today plus M4's text and terminal indications.
- **A theme**: for each indication in the catalogue, how it is reported
  (off, speech, sound, or speech and sound; braille joins the set in
  M15), the sound and its gain, replacement words if any, and a voice
  style if any; plus the theme's own name, description, and overall
  gain.
- **No base chains** (agreed 2026-10-06). The built-in default theme is
  complete. Every other theme stores only the indications where it
  differs from the default, and anything it does not mention falls back
  to the default theme, including indications that later versions of
  Verbatim add to the catalogue. A theme never builds on another user
  theme, so changing or removing one theme never changes another, and a
  shared theme is self-contained.
- **The default theme** uses speech and sounds, matching NVDA's defaults:
  everything is spoken as NVDA speaks it, and sounds play where NVDA
  plays them by default (the browse and focus mode sounds, suggestions
  appearing and disappearing, errors, start and exit), plus Verbatim's
  own cues (application not responding, skipped terminal lines). A theme
  without sounds is added when braille arrives.

How "sound only" without a sound works: a theme says both how an
indication is reported and which sound it uses, so a theme that sets an
indication to sound only names its sound. The gap appears only when the
two come apart: the sound file is missing or cannot be decoded, or the
user chooses sound only for an
indication that has no sound. Loading a theme reports these as problems
(listed in the theme panel and the log), and at playback an indication
whose sound is unavailable is spoken instead, so information is never
dropped by accident. Off remains the only way to remove an indication.

Where it applies in the pipeline: the reducer keeps putting every fact
into the utterance as typed spans (D12), and the presentation stage at
the end of the speech pipeline turns each span into words, a sound, both,
or nothing, as the active theme says. The reducer consults the theme in
one case, to skip fetching information that is set to off, such as
descriptions, so off also saves the cross-process call (agreed
2026-10-06). Sounds for spans are placed in the speech stream and
cancelled with it; sounds for events play at once (see "Earcons").

### Themes and configuration profiles

The division: a theme says how things are presented; a profile says
which theme applies, along with everything else a profile can change.

- Profiles exist in `verbatim-config` as sparse overlays on the base
  settings; activating them, by hand or per application, is M8's work.
- The base settings name the theme in use. A profile may name a
  different one, such as a proofreading theme for a word processor or a
  quieter theme for a terminal; a profile that names none uses the base
  settings' theme. Sound volume and the say-all and learning checkboxes
  are ordinary settings that a profile may also change.
- Profiles do not hold per-indication settings. To present something
  differently in one application, the user makes a theme for it (usually
  "New theme based on this", which copies the current theme's settings,
  then changes a few indications) and
  selects it in that application's profile. That keeps one place to look
  for how something is presented: the theme.
- Editing a theme changes that theme everywhere it is used. The built-in
  themes cannot be changed in place; editing one asks for a name and
  creates a new theme based on it, explicitly rather than JAWS's silent
  "modified" copy. User themes are edited in place.
- Themes are packages in the user's themes folder, referred to by id from
  the settings and profiles.

### The settings dialog

One "Theme" panel in the settings dialog's category list, replacing the
two panels proposed earlier:

1. "Theme", a combo box listing the installed themes, the default first.
   Moving through the list applies each theme at once, so the next thing
   spoken uses it; Cancel restores the theme the dialog opened with. The
   dialog title names the profile being edited, as NVDA's does, and the
   choice is saved to that profile.
2. "Description", a read-only text field: the theme's author,
   description, and any problems found loading it (a missing sound).
3. "Sound volume", a slider from 0 to 100, relative to speech; moving it
   plays a short sample.
4. "Play sounds during say all", a checkbox, on by default.
5. "Also speak indications that play a sound", a checkbox, off by
   default: JAWS's training mode, for learning a theme's sounds.
6. "Indications", a tree view of the selected theme. Top-level items are
   the categories (Roles, States, Properties, Text formatting, Structure,
   Events); their children are the indications. Each indication's name
   summarizes its setting, such as "Link: speech and sound", "Checked:
   sound (check.wav)", or "Description: off", so arrowing through the
   tree reads the whole theme; an indication that differs from the
   default theme ends with "changed". A "Find" field above the tree filters it.
7. Below the tree, the selected indication's settings:
   - "Report as", a combo box: off, speech, sound, speech and sound.
   - "Sound", a combo box: none, each sound in the theme, and "Browse..."
     to add a sound file to the theme. Space on the combo box plays the
     selected sound.
   - "Words", an edit field: the words spoken, empty for the default.
   - "Voice", a combo box: default, or one of the theme's voice styles.
   - "Preview", a button that speaks a sample through the theme, such as
     a link inside a sentence, so the sound is heard in context.
   - "Reset", a button that returns the indication to the default
     theme's setting.
   Controls that do not apply are disabled: with "Report as" off or
   speech, "Sound" is disabled; with sound only, "Words" and "Voice" are
   disabled. Changing a built-in theme's indication first asks for a name
   for the new theme, as above.
8. Buttons: "New theme based on this...", "Rename...", "Import...",
   "Export...", and "Remove" (with a confirmation; unavailable for the
   built-in themes and for a theme a profile still uses).

Changes apply at once and Cancel reverts them, like the Speech panel.
Enter activates OK, except on a button, which activates the button, as
the GUI port's key routing does everywhere.

Today's NVDA-style checkboxes (report descriptions, report position
information, and the M4 formatting settings) are not separate settings:
they are indications in the theme. The Speech panel keeps the
synthesizer and voice settings.

### Packaging

A theme is a directory: a TOML manifest (id, name, author, description,
version, gain, voice styles, and the indications where it differs from
the default theme) and its
sound files. It is shared as that directory zipped. Import installs one;
Export writes one. The built-in themes ship in the top-level `sounds/`
and `themes/` directories, shared by every platform.

### Scope

This milestone: the catalogue for what Verbatim reports plus M4's
additions, the default theme with NVDA's sounds, the Theme panel as
described including new, import, and export, and the theme named by the
base settings and, once M8 activates them, by profiles. M11 keeps voice
styling beyond relative pitch, rate, and volume, and themes provided by
extensions.

## Core's state

### What Core has to store, now and through M6

- **Today**: the focus context (the focused node's snapshot and its
  ancestor chain), the navigator, the attention record, the latest focus
  and navigation, the foreground, and a query counter. All small; the
  largest part is the ancestor chain.
- **M4, edit fields**: the caret and selection of the focused field, and
  the text that speech decisions compare against (the line or word at the
  caret before a key, for Delete and selection changes). Not the whole
  document: like NVDA, Verbatim asks the provider for the unit it needs
  each time (the line after Down Arrow, the word after Control+Right), so
  the outpost reads it and Core receives it with the caret event.
- **M4, say-all**: a reading position and a bounded lookahead of chunks
  already sent to speech, fetched from the outpost as speech proceeds.
- **M4, terminals**: nothing per line. The outpost holds the previous text
  and diffs it (decided above); Core receives the new lines and keeps
  only the flood state (what is queued, what a newer batch supersedes).
- **M4, the review cursor**: a position, as a reference to a node and a
  position within its text that the outpost resolves; text is fetched
  per command.
- **M6, browse mode**: the virtual buffer, text plus a node map for a
  whole document, which can be megabytes for a large page. This is the
  only state that is large by nature.

### The constraint that decides the representation

The flight recorder keeps a bounded window of recent inputs. To replay
that window, the state at the start of the window must be recorded too,
because the inputs that built it have already been dropped. So the
recorder needs a snapshot of the whole state at the start of each window,
and taking that snapshot must be cheap even once a virtual buffer is in
the state. Today the recorder records inputs only, so a replay starts
from an empty state; that works now only because today's state is
rebuilt quickly by the next focus event.

### Agreed: mutation in place

AccessKit's consumer (`../accesskit/accesskit_consumer/src/tree.rs`) is
not immutable either: it keeps two copies of the tree, applies each update
in place to the newer one, reports changes by comparing old and new
versions of only the changed nodes, then patches the older copy with just
those nodes. The cost is proportional to the change. Verbatim's reducer
does not need the second copy, because it decides its effects from the
input and the current state; where a decision needs a previous value (the
previous selection, the previous focus), the state keeps that value
explicitly, as `last_selection` already does.

- `reduce` takes `&mut SrState` and returns its effects, changing the
  state in place instead of cloning it. A reduce step then costs nothing
  for the parts of the state it does not touch. Replay stays
  deterministic, since determinism needs only that `reduce` does no I/O
  and reads no clock.
- Small control state stays in plain structs and fields.
- Anything large or growing is held in structures whose clone is cheap
  and shares memory: text in a rope whose nodes are reference counted,
  so a clone is constant time and an edit copies only the path to the
  changed leaf; node maps in a persistent map with the same property.
  Until M6 the only such data is the text Core keeps for an edit field,
  which is small, so this rule mostly constrains M6.
- The flight recorder takes a snapshot of the state each time its window
  advances past a checkpoint, which the cheap clone makes affordable, so
  every recorded window can be replayed from its start.
- The counting-allocator test asserts that a reduce step allocates the
  same number of bytes for a small and a large state, and that taking a
  snapshot allocates a bounded number of bytes whatever the state's size.

Whether the M6 virtual buffer lives in Core at all, or in the outpost
with Core querying it (NVDA keeps its buffers inside the browser's
process and queries them), is an M6 decision. The proposal above holds
either way.

## Internationalization in the text model

Text in Core is UTF-8; Windows text is UTF-16. Decisions needed at design
time, because they shape the model's types:

- **Positions are the provider's, not Core's.** UIA positions are text
  ranges, and Win32 edit controls count UTF-16 code units. Core never does
  arithmetic on provider positions; it holds them as opaque references
  that the outpost resolves. Where Core slices text it received (the word
  at a caret within a line), it uses offsets into its own UTF-8 string,
  and the outpost converts between the two at the boundary.
- **A character is a grapheme cluster.** Character navigation and echo
  must treat an emoji sequence, a letter with combining accents, a Hangul
  syllable, or an Indic conjunct as one character, and never split a
  surrogate pair. Providers' character units are not reliable here, so
  Verbatim segments with Unicode's grapheme rules (UAX 29) in a
  platform-neutral crate.
- **Words depend on the language.** Chinese, Japanese, and Thai do not
  separate words with spaces. NVDA uses ICU's break iterator (which
  Windows ships), cppjieba's dictionary segmenter for Chinese, and
  Uniscribe where it must match a Win32 edit control's own word movement.
  Verbatim does the same, with platform-neutral Rust equivalents so the
  segmentation can live outside the Windows crates: ICU4X's segmenter
  (Unicode's word rules plus dictionaries for Chinese and Japanese and
  for Thai, Lao, Khmer, and Burmese) and `jieba-rs` for Chinese. Where the
  application moves the caret itself (Control+Right in an edit field),
  the word spoken is the provider's word unit, so speech matches where
  the caret went; Verbatim's own segmentation is for text it walks itself,
  such as the review cursor.
- **Text carries its language.** UIA exposes a culture per text run. Text
  runs carry it as a span, so speech can switch synthesizer language and
  the character table can choose its language.
- **The character-description table is per language.** NVDA has a table
  of symbol names and a table of character descriptions per locale. M4
  ships English only, keyed by locale in `verbatim-i18n`, so other
  languages are data, not code changes. NVDA's translations are never
  imported (the provenance rule).
- **Lines.** A line can end with CR LF, LF, CR, or the Unicode line and
  paragraph separators, and a soft-wrapped line has no break at all. Lines
  come from the provider's line unit; Core never splits text on newlines
  to find lines.
- **Terminals count cells, not characters.** East Asian wide characters
  take two cells, so a column is not a character index. The outpost's
  diff works on text, not columns.
- **Input methods.** With an IME, as used for Chinese and Japanese input,
  one keypress does not produce one character. M4's echo must not assume
  it does; announcing composition and candidates is a later milestone.

## The NVDA update of 2026-10-06

The `nvda` submodule moved from `92942556f` to `663e4b679` (NVDA master,
2027.1 development, 218 commits). The review's findings, by where each
goes:

Done on 2026-10-06:

- Windows Terminal's top-level window class is treated as UIA before any
  probe, as NVDA now does (its window reports no server-side provider;
  the XAML island child that hosts its content does).
- The transcript add-on declares 2027.1 as tested, since NVDA 2027.1
  refuses add-ons tested before it.
- The NVDA reference docs under `docs/nvda/` and the parity ledger were
  updated for the new pin.

For the autonomous run, since they affect behavior Verbatim already has:

- **Live focus check.** NVDA's UIA focus acceptance now reads the
  sender's `HasKeyboardFocus` live rather than from the event's cache,
  because stale intermediate focus events slipped through once events
  carried richer caches (a list container announced before its item).
  Verbatim gates on the cached value. Proposed: the listener reads the
  property live and drops a focus event whose sender no longer has
  keyboard focus, one cross-process call, which the operation ledger
  counts.
- **UIA menu items' checked state.** A UIA menu item that is not
  checkable falls back to its legacy MSAA checked state, as NVDA now does.

For M4, folded into its parts:

- **Flood policy: NVDA tried a cap and reverted it.** NVDA capped
  terminal output at 100 lines with a beep for skipped lines, then
  reverted it (commit `0e48954c8`) because users could not read the
  skipped output, and it now speaks every line. Verbatim's "skipped N
  lines" policy must therefore never lose content: every line stays in
  the terminal's buffer and reachable with the review cursor, the limits
  are settings, and the defaults deserve thought (see the question
  below).
- **Caret timing in Windows Terminal.** NVDA waits three times longer for
  caret evidence there (300 ms rather than 100 ms), because over SSH the
  UIA caret moves late.
- **Terminal text units.** Windows Terminal's paragraph, page, and
  document units span the whole buffer, so review, mouse, and say-all
  never expand to them in a terminal; line is the largest unit used.
- **Remote operations can fill caches.** New instructions build a cache
  request and populate it inside the provider, so one remote call can
  return the focus ancestors with their properties cached. NVDA verified
  this against classic caches across 33 ancestors. This is the design for
  step 2's ancestor walk. A remotely filled cache stores default values
  where a classic cache marks a property as not supported, so the walk
  asks whether patterns are available rather than trusting defaults.
- **Say-all reads by sentence.** NVDA's Speech panel gained "Say all
  reads by", with sentence (the default, where the text can be split into
  sentences), paragraph, or line. Sentences come from Unicode's sentence
  rules over the paragraph; UIA has no sentence unit, so UIA text is read
  by line. Verbatim does the same, with ICU4X for sentences. Say-all also
  keeps the display on while reading, controlled by a setting that is on
  by default.
- **Whitespace in word movement.** A run of spaces and tabs forms one
  word segment with what follows, so the caret does not stop inside it.
  Verbatim's segmenter applies the same rule.
- **Unsupported text units** are reported as unsupported, and movement
  stops at the document's ends rather than wrapping.
- **Symbol names.** NVDA corrected several English symbol names (such as
  "superscript minus" and "three eighths"); the character table uses the
  corrected names.
- **Interrupting speech** with typed characters or Enter applies to all
  speech, say-all included, as NVDA's user guide now states.

For step 3, the GUI port:

- NVDA moved to wxWidgets 3.3.3, the version the port downloads. From
  3.3.2, wxWidgets fires its own accessibility event when a check list box
  item is toggled, and its own check list box accessible reports no
  children and no position, so NVDA supplies its own. Any check list box
  in Verbatim's GUI gets its own accessible and must not notify twice.

For later milestones, recorded in `docs/roadmap.md` under M6 and M8:
browse-mode additions, copying the last spoken text, dictionary rule
style, OCR capture, and menu item locations in 32-bit applications.

## The autonomous run

Agreed scope (2026-10-06): steps 1 to 3 and all of M4, after the UIA
remote operations crate is designed with Dickson. The run works on
`main`, commits each independently reviewable change with `cargo xtask
ci` passing (x64; ARM64 left to CI), verifies behavior live (desktop
takeover allowed), compares with NVDA through the transcript tool where
NVDA has the behavior, and reports at the end. Where the design leaves a
choice open, the run takes the reading the design most directly
supports, records it here, and lists it in the final report.

### Step 1: NVDA as the reference

1. Capture NVDA on the three deferred scenarios (an Explorer folder
   window, a Settings toggle, Start-menu search results), following the
   recipes in `docs/roadmap-done.md`. Only the Start menu has a scenario
   today, `start_menu_search`, and it asserts only that the search box is
   announced; navigating the results, Explorer, and Settings have none.
   Then run the same keys against Verbatim;
   fix differences in Verbatim, or record them in `docs/parity.md` when
   they are intentional.
2. Automate them as end-to-end scenarios in the default suite (extending
   `start_menu_search` to the results), with assertions written by hand
   from the captures.
3. Capture NVDA reading Verbatim's menu and settings dialog (Verbatim in
   test-audio mode, with the share-modifier setting so the two readers use
   different keys), committed as a reference transcript for step 3.

### Step 2: counts, memory, and remote operations

1. `docs/performance.md`: the operation ledger, with the minimum and
   current cross-process call counts for each operation and backend, and
   the definitions of cold, warm, and cancelled.
2. A counting helper around every cross-process call in `verbatim-uia`
   and `verbatim-ia2`; the outpost worker reads and resets it per entry;
   the counts travel with the event's timing to the latency ledger, the
   control plane's `LatencyRecord` (which also gains the stages), and
   `verbatim-inspect latency`, which reports each stage as time, count,
   and ratio to floor.
3. Hit counters in mockapp's providers, read by a synchronous window
   message, and tests that assert the client call count and the provider
   hits exactly for each ledger operation on each backend.
4. Core: `reduce` takes `&mut SrState`; the flight recorder takes state
   snapshots at checkpoints so every recorded window replays; a counting
   allocator test binary in `verbatim-core` asserts allocation per step
   is independent of the state's size; the flight recorder and latency
   ledger are bounded by bytes; the shell's per-event clone for the
   control plane moves after the subscriber check.
5. Calibration in the end-to-end suite: each scenario measures one
   cross-process call against its target and reports stage times as
   ratios to it.
6. From the NVDA update: the live `HasKeyboardFocus` check on UIA focus
   events (counted in the ledger), and the legacy checked-state fallback
   for UIA menu items.
7. `verbatim-uia-rops` as designed with Dickson (see "UIA remote
   operations"), and the focus ancestor walk through it, keeping the
   current walk as the fallback. The counts are recorded before and
   after; the exit criterion is two round trips per steady-state UIA
   focus change against mockapp, asserted exactly.

### Step 3: the GUI port

1. On wxDragon still: extract the pure Rust parts (key routing, the
   dialog lifecycle, the settings model) with tests, and put a real
   channel sender behind `GuiHandle`.
2. `verbatim-gui/build.rs`: wxWidgets 3.3.3, base and core only, static,
   adapted from wxdragon-sys's recipe, into a fixed directory per version
   and architecture; a CI cache step for it.
3. The cxx bridge and the C++ layer, switched in one commit: frame, tray,
   and menu; the settings dialog with the dialog-wide key hook (audit item
   7); the list dialog; check list boxes with their own accessible. In
   the settings dialog, Enter on a focused button activates that button,
   so Enter on Cancel cancels (decided 2026-10-06). NVDA's settings
   dialogs send every Enter to OK, even on Cancel
   (`gui/settingsDialogs.py`, `_enterActivatesOk_ctrlSActivatesApply`),
   so this is recorded in `docs/parity.md` as an intentional difference.
4. Remove wxDragon, bindgen, and the libclang plumbing; update the crate
   guide, the tooling guide, CLAUDE.md's libclang note, and D4's wording.
5. Verify: the existing GUI scenarios, a new scenario for item 7, and the
   NVDA captures of the GUI compared before and after.

### M4

In this order, since later parts build on earlier ones:

1. **Text foundations.** A platform-neutral text crate with grapheme,
   word (ICU4X, with `jieba-rs` for Chinese, and the whitespace-run
   rule), and sentence segmentation; the text model in Core's state;
   language carried on text runs.
2. **Reading text in the outpost.** UIA TextPattern, and the standard
   Win32 edit and rich edit controls through their window messages (as
   NVDA's `EditTextInfo` does), which covers edit fields in applications
   Verbatim otherwise reads through MSAA; MSAA itself has no text
   interface, so a custom control known only through MSAA reports its
   value, as in NVDA, until IA2 (M6) and the display model (M14); caret and selection events; text sent to Core with the caret
   event; unsupported units reported as such, movement stopping at
   document ends.
3. **Caret navigation and selection**, following NVDA's wait-for-evidence
   rules (`docs/nvda/editable-text-and-terminals.md`), with the provider's
   word unit where the application moves the caret; "selected" and
   "unselected"; the character-description table (English, keyed by
   locale, with NVDA's corrected symbol names).
4. **Typed-character and word echo**, with NVDA's settings, the password
   rule for terminals, and interruption of all speech by typing and
   Enter.
5. **The review cursor**: NVDA's review commands and key layout, with
   one intentional difference (decided 2026-10-06, recorded in
   `docs/parity.md`): moving to the next or previous line keeps the
   column, where NVDA moves to the start of the line
   (`globalCommands.py`, `script_review_nextLine`). This makes columns of
   a text table, or the map of a text-based game, readable line by line.
   In a terminal the screen is a grid, so the column is a cell column:
   wide characters, such as Chinese, Japanese, and Korean ones, take two
   cells by Unicode's East Asian Width property, and a position past the
   end of a line's text is a blank cell, read as "blank", so moving down
   from column 10 always lands on column 10. In other text, the column is
   counted in characters (grapheme clusters); a shorter line puts the
   cursor at its end, and the column is remembered, so the next longer
   line returns to it, as editors do with their caret.
6. **Say-all** with index marks, the "Say all reads by" setting
   (sentence by default where the text can be split into sentences, line
   for UIA), and the display kept on while reading.
7. **Formatting spans**: spelling and grammar errors, font and color
   where exposed.
8. **Themes and earcons**: the indication catalogue; the default theme
   with NVDA's sounds in the top-level `sounds/` directory; the
   presentation stage resolving each span through the theme; sounds in
   the speech stream and immediate sounds on their own mixer source; the
   Theme panel with new, import, and export; the reducer skipping fetches
   for indications set to off.
9. **Terminals**: generic terminal behavior keyed by the control (Windows
   Terminal, the embedded `WPFTermControl`, conhost); the anchored diff in
   the outpost, through remote operations; the backlog flood policy (30
   and 30); "Report new output" with Verbatim+5; the Terminal panel; the
   password rule; the longer caret wait in Windows Terminal. Verbatim
   already subscribes to UIA notification events and speaks them, so the
   generic handler ignores Windows Terminal's output notifications
   (activity id `TerminalTextOutput`) from terminal controls; otherwise
   every line would be spoken twice.
10. **Exit**: Notepad editing and Terminal end-to-end scenarios, the
    terminal flood scenario showing bounded latency and no hang, the
    remote-operations count, and the flood wall-time ratio under two.

