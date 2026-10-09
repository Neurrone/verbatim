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

### How the outpost finds new lines

Written down on 2026-10-06 in answer to Dickson's question of how the
outpost knows which lines to fetch when lines are redrawn or the
scrollback overflows.

The outpost keeps an anchor: a text range at the start of the last line
it read, plus the text of that line and of the line before it (a
two-line fingerprint, so a run of identical lines is less likely to
match by accident). A range is never trusted on its own, because once
the scrollback is full the terminal keeps a range's row number while the
text moves up beneath it. On each change, one remote program:

1. Reads the text at the anchor and the line before it, and compares
   them with the fingerprint.
2. If they match, the anchor is still where it was. The program counts
   the lines from the anchor to the end of the text (moving a range
   endpoint by line, which returns the count without sending text), and
   reads only the lines that will be spoken: all of them up to the
   backlog limit of 30, or the last 30 with the rest counted as
   skipped. The anchor's own line is compared character by character,
   so a prompt that grew, or a progress bar rewritten in place, speaks
   only what changed.
3. If they do not match, the text moved or changed under the anchor.
   The program searches upward from the anchor's row, a bounded number
   of lines, for the fingerprint. Found, the distance is how far the
   text scrolled; the anchor moves there and step 2 continues. This is
   the overflowing scrollback case, where the oldest lines are discarded
   and everything shifts up.
4. If the fingerprint is not found within the search, the anchor's line
   is gone: the screen was cleared, a full-screen program switched to
   the alternate screen, the line was rewritten, or more output arrived
   than the search covers. The outpost then compares the visible screen
   with the last screen it read, by line, and speaks the lines that are
   new, as NVDA does for screens that scroll (#12974). A new anchor is
   set at the end of what was read.

What is spoken in each case:

- A redraw that changes nothing (a full repaint with the same text)
  matches in step 2 with nothing new, so nothing is spoken.
- A line rewritten in place speaks only its changed characters; rapid
  rewrites within the collapse window speak once.
- A flood that overflowed the scrollback past the anchor (step 4)
  cannot know exactly how many lines went by, because the discarded
  lines are gone. The outpost then says "skipped lines" without a number
  and speaks the last 30. The exact count is spoken whenever it is known
  (steps 2 and 3).

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

## UIA remote operations

For design with Dickson before the run. Researched 2026-10-06, with a
throwaway probe run on this machine (Windows 11 26200, x64) against the
taskbar and Windows Terminal; "measured" and "verified" below refer to
that probe.

### What the platform offers

- The API is in Windows itself: the WinRT class
  `Windows.UI.UIAutomation.Core.CoreAutomationRemoteOperation`, with
  `AutomationRemoteOperationResult`. NVDA calls it through a thin C++/WinRT
  shim (`nvdaHelper/UIARemote/lowLevel.cpp`) and dropped Microsoft's
  `microsoft-ui-uiautomation` library in 2024 (NVDA commit `7e31f30f5`)
  as unmaintained and slow to build. The `windows` crate exposes the
  class behind its `UI_UIAutomation_Core` feature; it activates here.
- A client imports elements and text ranges as operands (an
  `IUIAutomationElement` casts to the WinRT `AutomationElement` by
  QueryInterface, verified), runs a bytecode program with `Execute`, and
  reads back the registers it asked for. One `Execute` is one
  cross-process round trip; the program runs inside the provider's
  process.
- The bytecode framing is documented on Microsoft Learn; the layout of
  each instruction is not, so the references are Microsoft's MIT-licensed
  `RemoteOperationInstructions.h` and NVDA's
  `source/UIAHandler/_remoteOps/lowLevel.py` and `instructions/`. Opcodes
  cover registers, integers, strings, booleans, arrays, string maps,
  comparison, loops and branches, property reads, navigation, pattern and
  text-range methods, and, since NVDA commit `9fccec044`, cache requests
  populated inside the provider.
- Support depends on the provider's process, so `IsOpcodeSupported`
  answers only after an import (verified).
- Results: a status (success, malformed bytecode, instruction limit
  exceeded, unhandled exception, execution failure), the failing
  instruction's index, an extended HRESULT, and any requested registers
  computed before a failure.

### What the probe found

- **Client-side proxies cannot run programs.** Importing an element
  served by the MSAA proxy inside the client's own process (Windows
  Terminal's caption buttons) fails with E_UNEXPECTED. Server-side
  providers import, including the taskbar's proxy, which runs in
  Explorer. So the classic path stays as a fallback.
- **The remote walk stops at the process's top-level window.** It crosses
  child windows within the process, and returns nothing for the desktop
  root.
- **Timings** (release build): a remote ancestor walk of 1 level took
  0.33 ms, 2 levels 0.61 ms, 5 levels 2.3 ms, against 3.6 to 3.8 ms for the
  classic walk of 6 levels. `GetFocusedElement` alone costs about 1.5 ms.
  Ancestors that are window-host elements cost about 0.6 ms each even
  inside a program, which suggests extra calls for those proxies
  (unverified). Against the taskbar's server-side proxy, remote and
  classic were close (2.3 ms against 2.6 ms). Counting 42 terminal lines
  to the end and reading the last 5 took 0.48 ms remotely.
- **Runtime ids compare remotely.** The "walk until a known ancestor"
  pattern works: the known runtime ids go in as strings in a string map,
  and each step turns the ancestor's runtime id into a string and checks
  the map (verified).
- **Verified in the run against mockapp's `stall` command:** `Execute`
  does not honour the UIA connection timeout, and neither does a classic
  call on an element already fetched; both are bounded by UIA's
  transaction timeout (20 seconds by default), which is process-wide (the
  last value set through any client applies to all), and a run that
  times out ends with an execution failure carrying `UIA_E_TIMEOUT`. So
  the worker can call `Execute` as it makes classic calls. Against a
  provider whose process has gone, `Execute` fails at once with
  `UIA_E_ELEMENTNOTAVAILABLE`. The instruction limit's value is still
  unmeasured. The details are in `docs/crates/verbatim-uia-rops.md`,
  which also records that a remotely filled cache stores defaults where a
  local one stores "not supported", which `verbatim-uia`'s mapping and
  the remote program now handle.

### Proposed design

The crate stays Windows-specific (GPL tier), so porting from NVDA's
Python framework is allowed. Three layers:

1. **The program builder.** NVDA's framework
   (`source/UIAHandler/_remoteOps/`, about 4,300 lines of Python) has
   three parts: the instruction table (about 100 opcodes), typed remote
   values with methods (element, text range, string, integer, boolean,
   array, string map, cache request) and structured control flow (if,
   while, for each, try and catch) that compute jump offsets, and a local
   emulator of the program machine used for its tests. Verbatim ports the
   first two whole, since terminals, text attribute runs, and M6's buffer
   fetches will use most of them: every opcode in the table with a unit
   test of its exact bytes, and a typed builder in which each register
   has a Rust type for what it holds, so a program that compares an
   element with an integer does not compile. It does not port the
   emulator, since its tests run against mockapp's real provider, nor the
   Python operator overloading; a failing instruction is mapped back to
   the Rust line that emitted it with `#[track_caller]`, as NVDA maps it
   to the Python line. Building a program costs microseconds, so programs
   are built per call, with that call's values as literals.
2. **Execution.** `Operation` imports the elements or text ranges, runs
   the program, maps the status to a Rust error (with the failing
   instruction's index), and converts the requested registers to Rust
   values, or to `IUIAutomationElement` with the populated cache, which
   the existing snapshot code in `verbatim-uia` reads unchanged. Every
   `Execute` passes through the call counter of step 2, as one round
   trip.
3. **Algorithms.** Named operations the outpost calls, each with a
   classic implementation behind the same function signature:
   - `focus_ancestry(element, known_runtime_ids, depth_limit)`: the
     element's ancestors, nearest first, each with Verbatim's cached
     property set, stopping at the first ancestor whose runtime id is
     known (and saying which), plus a list's or tab control's selected
     child.
   - `terminal_tail(anchor, lines_wanted)`: the anchor line's text, the
     number of lines from the anchor to the end, and the text of the last
     lines, for M4's terminals.

   The classic implementation is today's walk. It is the fallback, the
   baseline for the before-and-after measurement, and the reference in
   tests: against mockapp, both implementations must return the same
   ancestors with the same properties.

Why some elements cannot run programs: UIA has two kinds of provider.
A native one runs inside the application (Windows Terminal's text
control, WinUI, Office). For an application or control with no native
UIA, only MSAA or plain Win32, UIA builds a proxy provider inside the
client, that is inside Verbatim's own outpost, which translates each UIA
request into MSAA calls to the application. A remote program runs in the
provider's process, and for a client-side proxy that is the outpost
itself, so there is nothing to run remotely and the import fails
(E_UNEXPECTED). Windows Terminal's caption buttons are such elements:
standard window-frame controls that UIA proxies, though the terminal's
text is native. Verbatim's arbitration already sends windows without a
native provider to the MSAA outpost path, so this affects only proxied
parts of windows read through UIA: window frames, and Win32 child windows
inside otherwise native applications.

Fallback, so a failure costs one attempt, not one per event:

- Whether a window's provider is native is already known: arbitration
  probes it (`UiaHasServerSideProvider`) and keeps the verdict for the
  window's lifetime. A window without a native provider uses the classic
  implementation without trying. An import that fails anyway marks the
  window the same way, for its lifetime, since a window's provider does
  not change.
- An `Execute` that fails runs the classic implementation for that call
  and is logged with the instruction index; repeated failures for one
  window mark it like a failed import.
- A program that exceeds the instruction limit is retried once with a
  smaller depth limit, then falls back.

The two-round-trip target for a steady-state UIA focus change: the focus
event arrives with its cache already filled, so `GetFocusedElement` is
not needed; the outpost imports the event's sender. Then:

- Option A: one round trip for the live `HasKeyboardFocus` check that
  NVDA now makes, and one `Execute` for the ancestry.
- Option B: the program reads `HasKeyboardFocus` first and returns early
  if it is false, so the whole focus change is one round trip.

Option B was agreed on 2026-10-06: it meets the target with one round
trip to spare.

The program reads raw-view parents, which is the view the classic walk
uses (`RawViewWalker` in `verbatim-uia`'s client), so the two return the
same tree.

Where remote operations are used: for every operation that would
otherwise take several round trips (the focus ancestry, terminal reads,
and later text attribute runs and M6's buffer fetches), whenever the
window's provider is native. Not for a single read that is already one
round trip, since a program cannot make it cheaper, and never for event
subscriptions, which are not calls. Each choice is justified by the
operation ledger's counts. A developer setting in `settings.toml`
(`uia.remote_operations`, on by default) forces the classic path, for
diagnosing a provider and for the before-and-after measurements.

Dependencies: the `UI_UIAutomation` and `UI_UIAutomation_Core` features
of `windows`, and the `windows-collections` crate (the WinRT vector type
moved there), which means rerunning hakari.

Corrections this research makes: `docs/architecture.md`'s remote
operations bullet and the crate's doc comment both say Verbatim wraps
Microsoft's `microsoft-ui-uiautomation` library; it uses the API in
Windows directly, as NVDA now does. `docs/nvda/uia-remote-ops.md` links
to a `docs/RemoteOperations.md` in that repository which does not exist.


## The autonomous run

Agreed scope (2026-10-06): steps 1 to 3 and all of M4, after the UIA
remote operations crate is designed with Dickson. The run works on
`main`, commits each independently reviewable change with `cargo xtask
ci` passing (x64; ARM64 left to CI), verifies behavior live (desktop
takeover allowed), compares with NVDA through the transcript tool where
NVDA has the behavior, and reports at the end. Where the design leaves a
choice open, the run takes the reading the design most directly
supports, records it here, and lists it in the final report.

### How the run is verified

Four kinds of check, and videos:

- **Unit tests**, in `cargo xtask ci` and so in GitHub Actions: pure logic
  in the crate that owns it, mostly reducer tests in
  `crates/verbatim-core/tests/reduce.rs` driven by inputs and asserting
  effects.
- **Cross-process tests against mockapp**, also in CI: a real outpost in
  the test process against mockapp's real UIA and MSAA providers, in the
  shape of `crates/mockapp/tests/slow_application.rs`. Exact call counts
  and provider hits are asserted here.
- **End-to-end scenarios**, run locally through the agent with a real
  Verbatim and real applications. Each new behavior gets a scenario; the
  whole suite must pass three consecutive runs at the end of each step.
- **NVDA comparison**: where NVDA has the behavior, the scenario's keys are
  captured with the transcript tool first, and the scenario's assertions
  are written from the capture; a deliberate difference is recorded in
  `docs/parity.md`.
- **Videos**: `cargo xtask demo <scenario>` records a scenario, so every
  video is also a passing test. Videos showing the settings dialog are
  re-recorded after the GUI port.

Step 1:

- Scenarios: `explorer_folder_window` (open a folder, arrow through its
  items, open a subfolder and go back), `settings_system_page` (replaced `settings_toggle` on 2026-10-06: read-only, and the same on GitHub's Windows Server runner; open a page of
  the Settings app, Tab to a toggle, switch it, hear the new state), and
  `start_menu_search` extended to type a query and arrow through the
  results.
- Videos: `explorer-folder-window`, `settings-system-page`, and
  `start-menu-search` re-recorded.

Step 2:

- Unit: the counting-allocator test (a focus event and a navigation step
  allocate the same bytes against a small and a large state, and a
  snapshot's cost does not grow with the state; M4 adds a caret move and
  a terminal line); the flight recorder and latency ledger stay within
  their byte bounds when fed large texts; replay from a snapshot gives
  the recorded effects; the existing reducer tests pass unchanged after
  the change to `&mut SrState`; the exact bytes of every remote operations
  instruction; status mapping; `verbatim-inspect latency`'s output.
- mockapp: exact client calls and provider hits for each operation in the
  ledger on each backend (UIA focus cold and steady, into a list, MSAA
  focus, an object navigation step); the remote and classic
  `focus_ancestry` return identical ancestors and properties on several
  trees (deep, a list with a selected item, stopping at a known
  ancestor); a focus event whose element no longer has focus is dropped;
  a stalled provider (mockapp's `stall`) times out the remote call or,
  if it does not, the finding changes the design before anything builds
  on it; a UIA menu item reports its legacy checked state; and the
  headline assertion, one round trip for a steady-state UIA focus
  change.
- End-to-end: every scenario saves its calibration and the stage ratios
  with its artifacts; the existing suite passes, which shows the remote
  walk did not change what is spoken.
- Videos: none; this step changes nothing audible.

Step 3:

- Unit: the key routing (Enter on Cancel cancels, Enter on Apply applies,
  Enter elsewhere is OK, Enter on the synthesizer name opens Change,
  Control+Tab and Control+S work from any control), the dialog
  lifecycle, and the settings model.
- Build: the wxWidgets build in CI on x64 and ARM64.
- End-to-end: the existing GUI scenarios unchanged, plus
  `settings_dialog_keys` (change the rate, Tab to Cancel, press Enter,
  reopen, the rate is unchanged; Control+S applies; Control+Tab from
  inside the Speech panel) and `tray_list` (the tray and taskbar list
  dialog, which no scenario covers today).
- NVDA comparison: NVDA's reading of the menu and settings dialog before
  and after the port.
- Videos: `settings-dialog-keys`, and the existing settings videos
  re-recorded (`tabbing-through-settings`, `rapid-tabbing-in-settings`,
  `object-navigation-in-settings`, `switch-to-onecore`,
  `notepad-and-verbatim-menu`).

M4:

- Unit, text crate: grapheme clusters (an emoji sequence, combining
  accents, Hangul, an Indic conjunct, surrogate pairs), words (English
  with punctuation, the whitespace-run rule, Chinese through `jieba-rs`,
  Thai through ICU4X), sentences, cell widths, and trailing padding
  trimmed by White_Space.
- Unit, reducer: what is spoken for each caret key (line, word,
  character, Home, End, Backspace, Delete); "selected" and "unselected";
  character and word echo; the password rule; every review command,
  including the column kept in a grid and in ordinary text, and
  Verbatim+F9 and F10; say-all chunks and continuation after an index
  mark; the flood backlog (output under 30 lines whole across several
  batches, "skipped N lines" and the last 30 beyond, newer output never
  cancelling older); Verbatim+5; theme resolution (profile, base, theme,
  default), a missing sound spoken instead, off skipping the fetch; the
  presentation stage's words and sound items.
- Unit, outpost: the anchored diff on simulated buffers (appended lines,
  a line rewritten in place, the scrollback full and shifting, the screen
  cleared, the alternate screen, padding).
- Unit, audio and speech: an immediate sound mixed over speech; a sound
  in the speech stream starting at its place and cancelled with its
  utterance; a theme package loaded from TOML with a missing sound
  reported.
- mockapp, which grows a UIA text provider and a Win32 edit control
  fixture: lines, words, and characters read; caret events; an
  unsupported unit reported; movement stopping at the document's ends;
  remote and classic `terminal_tail` agreeing; exact counts for typed
  character echo, arrowing through text, and a terminal output line.
- End-to-end, each compared with NVDA where NVDA has the behavior:
  - `notepad_editing`: typing with character and word echo, arrowing by
    character, word, and line, selecting with Shift, deleting.
  - `notepad_review_cursor`: review by line, word, and character; the
    column kept through a text table; Verbatim+F9 and F10 copying a range,
    checked by pasting it.
  - `notepad_say_all`: say-all by sentence, interrupted by a key, the
    caret left where speech stopped.
  - `spelling_error_sound`: a misspelled word read in Notepad plays the
    spelling sound at its place in the speech.
  - `windows_terminal_commands` and `conhost_commands`: commands and their
    output, typed echo, and a `Read-Host -AsSecureString` prompt whose
    typing is never spoken.
  - `terminal_flood`: ten thousand lines give "skipped N lines" and the
    last 30, Verbatim+5 silences and restores output, Verbatim stays
    responsive, and the wall-time ratio is reported.
  - `terminal_review_grid`: the review cursor down a column of a text
    table in a terminal, through shorter lines.
  - `theme_panel`: described under videos.
- Videos:
  - `notepad-editing`, `notepad-review-cursor` (including copying with
    Verbatim+F9 and F10), and `notepad-say-all`.
  - `windows-terminal` and `conhost`: commands, output, and the silent
    password prompt.
  - `terminal-flood` and `terminal-review-grid`.
  - `sounds-in-use`: the default theme's sounds in their places, such as
    the spelling sound while reading and the skipped-lines sound.
  - `theme-panel`: open the settings dialog and the Theme panel; arrow
    through the theme list hearing the preview; in the indications tree,
    two or three entries from each category (a role, a state, a property,
    a text format, a structure item, an event); on one of them, go through
    its controls: change "Report as", play a sound from the Sound list,
    press Preview, press Reset; then Cancel, and hear that nothing
    changed.

### Terminal end-to-end scenarios

The terminal scenarios share a setup, so their results do not depend on
the user's own terminal settings or on timing:

- **Their own windows.** Each scenario starts its own window with a title
  unique to the run: Windows Terminal with `wt.exe -w new --title <title>
  --size 120,30`, and the console host with `conhost.exe`, its size set
  by `mode con cols=120 lines=30`. The fixed size fixes line wrapping. The
  scenario finds, brings forward, and closes its window by that title,
  never by class, so the user's own terminals are never touched.
- **A fixed shell.** PowerShell started with `-NoProfile -NoLogo` and a
  one-word prompt set at startup, or `cmd` with `prompt $G`, so the
  prompt's text is known and assertions do not depend on the working
  directory or a user's profile.
- **Scripts written by the agent.** Anything a scenario runs (a flood, a
  text table, a password prompt) is a small script the agent writes into
  the run's directory with `WriteFile` before the window opens, so the
  typed command is short and the output is exactly known.
- **Typing text.** A new agent request, `TypeText`, types a string as
  real key presses: each character is mapped to its key and Shift state
  in the active keyboard layout (`VkKeyScanEx`), so Verbatim's keyboard
  hook sees ordinary typing, which typed-character echo needs. A
  character the layout cannot type fails the request before any key is
  sent. `SendKeys` stays for named keys.
- **Assertions on speech.** As in every scenario, on the speech frames
  from Verbatim's control plane: what was spoken, in what order, and when.

The password scenario (`windows_terminal_commands`, repeated in
`conhost_commands`):

1. Type `echo hello` and Enter: each typed character is spoken, then the
   output "hello", then the prompt. This first shows that echo works in
   this window, so the silence that follows means something.
2. Run the written script, which calls
   `Read-Host -AsSecureString "Password"`: "Password" is spoken.
3. Type `secret` and Enter. The terminal shows nothing for these keys,
   so with "speak passwords" off (the default) Verbatim speaks none of
   them: no speech frame between the prompt and the next output contains
   any of the typed characters or the word.
4. The script then prints "done": it is spoken, which shows Verbatim did
   not go silent for another reason.
5. The same steps with "speak passwords" on: the characters are spoken.

The flood scenario (`terminal_flood`):

1. Run a written script that prints ten thousand numbered lines ("flood
   line 1" to "flood line 10000") as fast as the shell can, then the
   prompt returns. Ten thousand lines also exceed Windows Terminal's
   default scrollback of 9,001 lines, so the oldest lines are discarded
   during the run, which exercises the anchor's invalidation.
2. Assertions: the spoken lines are in increasing order with no line
   twice; "skipped N lines" is spoken; the last 30 lines and the prompt
   are spoken; nothing is spoken out of order after the prompt. N itself
   is not asserted exactly, because how many lines are spoken before the
   backlog fills depends on the speech rate; the assertion is that the
   skipped count plus the lines spoken accounts for every line.
3. Responsiveness: during the flood, the scenario presses a Verbatim
   command (report the title) and asserts it is answered within a bound,
   and the control plane's status answers throughout, so a hang fails the
   scenario rather than stalling it.
4. Verbatim+5 turns output reporting off: a second flood speaks nothing
   but the confirmation; Verbatim+5 again turns it back on, and the next
   command's output is spoken.
5. The wall-time ratio for the M4 exit criterion is computed from the
   trace stages and the run's calibration, and saved with the
   artifacts.

`terminal_review_grid` prints a written text table whose column 10 is
the start of a column on every row, with some rows shorter than 10
characters, then moves the review cursor down through it: every step
lands on column 10, reading the cell there or "blank".

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

### Caret responsiveness (added with Dickson on 2026-10-06)

A UIA caret move costs 9 calls today (NVDA's equivalent is about 12 at
the least, and 20 to 40 with formatting). Remote operations are used
whenever they are available, so the caret is read through one remote
operation, with formatting (M4 item 7) fetched in the same round trip
rather than as extra calls. For awareness, not as a gate:

- Verbatim logs, for every caret report, the time from the caret key or
  caret event being received to its speech being queued, by stage.
- A one-off measurement, not a permanent tool, times a key injected by
  the agent to the first audible speech, for Down Arrow, Right Arrow,
  and Control+Right Arrow, in Windows 11 Notepad (UIA) and in a standard
  Win32 edit control (MSAA and window messages), with Verbatim's remote
  operations on and off and with NVDA, all on eSpeak NG.

### Unsafe code (added with Dickson on 2026-10-06)

No dedicated safety review of the crates' `unsafe` code has been done:
about 470 sites across the Windows crates, most of them `windows` crate COM
calls marked `unsafe` only because the bindings are generated. In order:

1. Lints, before more code is written: `#![forbid(unsafe_code)]` on every
   crate with no `unsafe` today (the platform-neutral crates among them),
   and `clippy::undocumented_unsafe_blocks` across the workspace, adding
   the missing `SAFETY` comments.
2. Safe wrappers for UIA and MSAA in `verbatim-uia` and `verbatim-ia2`:
   small newtypes whose methods hold one documented `unsafe` call each, so
   the outpost's logic and the remote operations algorithms are safe
   code. Done with step 2's remote operations wiring, which touches the
   same code.
3. A safety audit of what remains (step 2b, after step 2 and before step
   3): raw pointers, `SAFEARRAY` and `VARIANT` handling, COM apartments
   and agile references, callbacks and reentrancy, cross-process
   `SendMessage`, window-handle reuse, and the eSpeak and WASAPI FFI, with
   a written report and fixes; then `clippy::multiple_unsafe_ops_per_block`.

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
   The review commands in M4, NVDA's desktop keys first, laptop keys in
   parentheses (from `globalCommands.py`):
   - Lines: previous, current, next, on numpad 7, 8, 9 (Verbatim+Up
     Arrow, Verbatim+Shift+Period, Verbatim+Down Arrow); current line
     pressed twice spells it.
   - Words: numpad 4, 5, 6 (Verbatim+Control+Left Arrow,
     Verbatim+Control+Period, Verbatim+Control+Right Arrow); current word
     pressed twice spells it.
   - Characters: numpad 1, 2, 3 (Verbatim+Left Arrow, Verbatim+Period,
     Verbatim+Right Arrow); current character pressed twice gives its
     description from the character table, three times its numeric value.
   - Top and bottom: Shift+numpad 7 and 9 (Verbatim+Control+Home and End).
   - Start and end of line: Shift+numpad 1 and 3 (Verbatim+Home and End).
   - Previous and next page: Verbatim+Page Up and Page Down
     (Verbatim+Shift+Page Up and Page Down).
   - Say all from the review cursor: numpad Plus (Verbatim+Shift+A).
   - Start and end of the selection: Verbatim+Alt+Home and End.
   - Select and copy (requested by Dickson): Verbatim+F9 marks the start;
     Verbatim+Shift+F9 moves the review cursor to that mark; Verbatim+F10
     once selects from the mark to the review cursor where the text
     supports selection, twice copies that text to the clipboard. With no
     mark, Verbatim+F10 says "No start marker set".
   - The review cursor follows the caret, toggled by Verbatim+6 ("caret
     moves review cursor").
   - The review cursor's location and the caret's location
     (Verbatim+numpad Delete, Verbatim+Delete on laptops).
   Not in M4: switching review modes (Verbatim+numpad 7 and 1). M4 has
   only object review, the text of the navigator object; document review
   needs browse mode and screen review needs the screen projection, both
   M6. Object navigation, already implemented, is unchanged.
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

Choices the run made for item 8 on the Windows side, where the design
left them open:

- With "play sounds during say all" off, an indication say-all reads
  plays no sound and is spoken instead, so nothing is lost.
- The exit sound is waited for at most two seconds; a replacing instance
  waits four for the old one to go.
- The error sound plays for every error logged, as in NVDA's test
  versions; "application not responding" plays once per stall, when the
  outpost's watchdog abandons a query, until the outpost answers again.
- The outposts skip what is off in their UIA reads and remote focus walk
  and in their MSAA reads; the UIA event subscriptions' cache requests
  are fixed when registered and still ask for everything. Text formatting
  is read only for the indications that are on (item 7, below).
- On the Theme page, changes to indications are held until Apply or OK
  saves them, and Cancel drops them; file operations (New, Rename,
  Import, Export, Remove, and Browse for a sound) act on the themes
  folder at once, and Export writes the theme as saved. Remove is also
  unavailable for the theme the base settings use. The dialog title
  names the category rather than a profile until M8 activates profiles.
- The Theme page's controls are laid out with the find field and tree on
  the left and the selected indication's controls on the right, between
  the theme's settings above and the theme buttons below; the tab order
  follows the design's list.


Choices the run made for item 7 and the caret's remote operation, where
the design left them open (2026-10-07):

- The caret is read by one entry point, `verbatim_uia_rops::caret_read`,
  remote or classic with the fallback of the focus walk. Every read of a
  caret key's wait is the whole read (the caret, the evidence, the line,
  the unit, and the formatting), one round trip remotely, so a wait that
  finds nothing costs one round trip per read and the read that finds
  evidence is the answer. The classic reads do the same work call by
  call. A run that failed because the provider is gone or timed out is
  not repeated classically.
- Formatting is read for what is spoken: the line after a focus, and a
  caret key's character, word, or line; not a paragraph or a page (NVDA
  reads no spelling errors when moving by paragraph), not the report
  after a typed character, and not yet for review commands or say-all.
- Bold, italic, and underline are one new indication, "font attributes"
  (`font-attributes`), off by default as NVDA's font attributes setting
  is; strikethrough is not read.
- Colors are named in the outpost, by NVDA's hue, saturation, and
  brightness names, in English.
- The standard edit controls report no formatting: their message for it
  (`EM_GETCHARFORMAT`) reports the selection's format, so reading a
  character's means moving the selection, three messages and a visible
  change per stretch, as NVDA does; not cheap, so left out.
- Where NVDA's default and the default theme differ: NVDA reports
  spelling errors by speech alone by default; the default theme also
  plays the error sound. Recorded in `docs/parity.md` for a decision.

Choices the run made for item 9, terminals, where the design left them
open:

- The anchor's fingerprint counts as found in place when the line before
  the anchor is unchanged; the anchor's own line is then compared with
  what it held. A rewrite of that line under a blank line is not trusted,
  and the screen is read afresh instead. The upward search covers 256
  lines, and a line that was the text's last matches with the line break
  it gains once more text follows.
- A line rewritten in place speaks from the start of the word where it
  first differs, not only the changed characters, so "progress 50%" says
  "50%"; a line that only got shorter speaks nothing.
- A read that finds nothing new after the anchor also reads the screen
  afresh and compares it line by line, so a full-screen program's redraw
  of a line above the anchor is spoken. "skipped lines" without a count is
  said when no line of the fresh screen kept its place and the text holds
  more lines than were read.
- Typed characters held for a terminal are echoed when the terminal shows
  them at the end of its line, and that text is not spoken again as
  output (in place of NVDA's rule dropping any one-character change). A
  line that grew by something else drops the held typing unspoken, so a
  password prompt's asterisks are spoken but the password never is.
- The backlog is kept in Core: two utterances of output are handed to
  speech ahead of playback, each starting with an index mark, and the rest
  wait in Core's state, where the "30 and 30" limits trim them. Anything
  that cuts speech off drops the backlog.
- Both limits are kept between 1 and 100, and an outpost reads at most as
  many lines per change as the larger keeps.
- Verbatim+5 says "report new output on" and "report new output off".
- The terminal end-to-end scenarios have no report-title command to press
  during the flood (Verbatim has none yet), so the responsiveness check
  presses Verbatim+5. The flood's wall-time ratio is the flood script's
  own elapsed time with output reported, divided by its time with output
  reporting off, both measured by the script in the same window, since the
  suite has no calibration of trace stages yet.

What the first live runs against Windows Terminal and the console host
found, on 2026-10-06, and what changed:

- The console host is read through UIA on Windows 11: its provider reports
  text formatting, as arbitration checks. Its text area's focus comes from
  the host's process, while `GetWindowThreadProcessId` names the console's
  client as the window's owner, and inside a remote operation its provider
  gives no native window handle, so the outpost finds the window by the
  classic walk.
- The agent gave its own standard handles to every program it started;
  `conhost.exe` takes inherited handles as a pseudoconsole's and opens no
  window. The agent now starts programs with none, and lets each take the
  foreground, as a user's launch would.
- A prompt's trailing space cannot be told from padding, so what a line
  gained can start with white space that was already there;
  `LineChange::uncertain` says how much, and Core matches typing after it.
- Ranges keep their rows while a full scrollback scrolls beneath them,
  between any two calls. Lines are therefore read in one call, a read is
  checked to have held still and set aside when it did not (when the text
  scrolled beneath it, saying lines were skipped and starting again from
  it), the last line is found from the walk that counts, and a
  half-written last line is found again grown.
- The console scenarios set the scrollback to 9,001 lines, since `mode
  con` cuts it to 30. The wall-time ratio is measured on a fourth flood
  against the third, both into a full scrollback: a flood filling an
  empty one while it is read is slower in the console host whether its
  output is reported or not.
### After M4 (scheduled with Dickson on 2026-10-07)

In this order, once M4's remaining items (formatting spans, the caret
remote operation, the Terminal page, the demonstrations) are merged:

1. NVDA comparison captures for every M4 feature, taken with `cargo
   xtask nvda capture` (working material, not kept in the repository),
   compared with what Verbatim says for the same steps, each difference
   either fixed or recorded in `docs/parity.md` as intended, and turned
   into end-to-end and unit tests that pin the behavior down. This is
   how Verbatim is shown to behave correctly, so it comes first: editing in
   Notepad (caret by character, word, and line; selection; typed echo;
   Backspace and Delete), the review cursor (including the column
   difference), say-all, terminal output and typing in Windows Terminal
   and the console host, the password prompt, a flood, and the settings
   dialog's pages.
2. Edit fields under object navigation and report current object read
   their text, as on focus, rather than their whole value.
3. A dialog's own text (a message box's question) is read when the
   dialog opens.
4. A state change of an object that has already lost the focus is not
   spoken (the stray "unavailable" from Reset).
5. The clipboard write frees its memory when it fails, and opens the
   clipboard with an owner window.
6. The GUI bridge's GUI-thread rule enforced by its types rather than
   stated in a comment.
7. A tray icon re-found before it is clicked.
8. `cargo xtask demo` respects `CARGO_TARGET_DIR`.
9. This document brought up to date with the run's decisions and each
   step's outcome.

## Outcome (2026-10-07)

What the run delivered, step by step, with the decisions taken along the
way. The code, its crate guides, `docs/parity.md`, and
`docs/performance.md` hold the details; this records what was decided and
why.

### Step 1: NVDA as the reference

- The transcript add-on and `cargo xtask nvda capture` exist, now with
  `--verbatim` (Verbatim's own speech for the same steps), `--type`, and
  key steps joined by commas. Captures are working material, never kept
  in the repository (Dickson, 2026-10-07): they drive exploratory testing
  and the tests that pin behavior down.
- The comparison against NVDA for every M4 feature found and fixed, among
  others: selection wording ("hello selected"), NATO spellings ("Alfa",
  "Xray"), the review cursor reaching the empty last line, the console
  host's "Text Area" name, "multi line", tree levels from 0 and their
  placement, report current object's second and third press, a dialog's
  own text, and the settings dialog keeping one node across title changes.

### Step 2: counts, memory, remote operations, unsafe code

- Exact cross-process call counts are pinned for both the remote and the
  classic path (`crates/mockapp/tests/call_counts.rs`). Remote operations
  are used whenever available (Dickson, 2026-10-06): a steady-state UIA
  focus costs 2 calls, a UIA caret move or caret report 1 (from 9), with
  the theme's formatting attributes in the same round trip, and every poll
  of the caret wait 1.
- Caret responsiveness was measured once, from an injected key to the
  first audible sample, against NVDA on the same eSpeak NG settings.
  Medians in Windows 11 Notepad: Verbatim 21 to 24 ms, NVDA 31 to 48 ms;
  in a Win32 edit control, Verbatim 19 to 23 ms, NVDA 29 to 46 ms. The
  latency log line now starts at the keyboard hook and names every stage.
- The UIA and MSAA clients read through safe wrappers; every crate without
  unsafe code forbids it; `undocumented_unsafe_blocks` and
  `multiple_unsafe_ops_per_block` are on. The audit
  (`safety-audit-2026-10-06.md`) found 1 high, 3 medium, and 26 low
  issues; all are fixed except two lows declined with reasons there.
- Verbatim survives output it cannot write (a full disk under redirected
  output), which had aborted it.

### Step 3: the GUI port

- wxDragon and libclang are gone; a small C++ wxWidgets layer behind a
  cxx bridge holds the windows, with all logic and strings in Rust. The
  bridge's GUI-thread rule is enforced by a token type. The port needed
  `/EHsc` (exceptions changed `wxApp`'s vtable) and the RelWithDebInfo
  configuration requested explicitly for the ARM64 build.
- The settings dialog has Speech, Theme, and Terminal pages; Enter on a
  focused button activates it, and Control+S and Control+Tab work from any
  control.

### M4

All ten items are in: the text model and segmentation (`verbatim-text`),
reading text through UIA and the standard edit controls, caret navigation
and selection, typing echo (only in edit controls by default, as NVDA),
the review cursor with its kept column, say-all, formatting spans, themes
and earcons with NVDA's sounds, and terminals (an anchored diff as a
remote operation, the 30-and-30 flood policy, Verbatim+5). The end-to-end
suite holds 24 scenarios that run the same locally and on GitHub's
Windows Server runners; where a machine lacks something (Windows 11
Notepad's spell checker), the suite reads the same speech from mockapp
and the real case is a demonstration. Demonstrations, separate from the
tests, are recorded into `videos/demos/`; test recordings are kept in
`videos/tests/`.

### Open questions for Dickson

- Whether line breaks are named as NVDA names them ("carriage return"),
  where Verbatim says "blank" or nothing.
- Whether the Terminal page applies its changes live, like the Speech and
  Theme pages, or on Apply, as built.
- Whether the default theme's spelling error also plays the error sound,
  where NVDA only speaks it.
- Whether the say-all demonstration's Windows Forms text box is
  acceptable for reading by sentence.

### Decisions and work scheduled on 2026-10-07

- Settings pages wait for Apply, OK, or Control+S; Speech and Theme are
  the exceptions and apply as they change, since their changes are heard
  at once. The Terminal page, and every page added later, waits for
  Apply.
- Line breaks are named as NVDA names them ("carriage return", "line
  feed"); Verbatim+V says "Verbatim" and then "Context menu", as NVDA
  does; the default theme plays the error sound for a spelling error.
- Everything a demonstration shows is also covered by an end-to-end test,
  unless that is impossible, which is brought to Dickson to decide.
- Scheduled, in order:
  1. A systematic parity sweep, so bugs stop being found by accident:
     for every NVDA command and speech rule Verbatim implements, NVDA and
     Verbatim are compared with `cargo xtask nvda capture --verbatim` at
     the boundaries (document ends, empty lines, lines of only white
     space, punctuation-only words, selections across lines, and each
     control type: UIA and MSAA, edit controls, terminals, lists, trees,
     dialogs), and every difference is fixed with a test or recorded in
     `docs/parity.md`.
  2. What text attributes terminals expose: every UIA text attribute
     and annotation that Windows Terminal and the console host report
     (fonts, colors foreground and background, bold, italic, underline
     styles, strikethrough, links, and anything else), surveyed live and
     written down, then the useful ones reported through the formatting
     stage, links among them. The same survey for Windows 11 Notepad:
     every attribute and annotation it exposes that Verbatim does not yet
     read. Dickson chooses which attributes are queried, from the survey:
     what is fetched stays driven by the theme's indications (an
     indication set to off is never fetched), and the survey measures the
     cost of each attribute and of querying all of them, in calls and in
     time, within the caret's one remote operation and in the classic
     fallback, so the choice is made knowing the price. Language is
     part of the survey: terminal output switches voice only if the
     terminal itself reports a language per line (the Culture
     attribute), checked once for the whole read and line by line only
     when that answer is mixed. Otherwise terminal output is spoken in
     one language, as NVDA speaks it; Verbatim does not guess the
     language from the text (Dickson, 2026-10-07).
  3. A typed space at the end of a terminal's line is spoken when typed,
     using the outpost's knowledge of which trailing white space is real.
  4. The terminal flood: why reporting slowed the console host 5.68 times
     under load, and the run whose speech did not account for every line,
     by measuring what Verbatim does during a flood: each read's
     duration, its calls and how long each takes, how often it reads,
     and how long the provider is blocked.
  5. A dialog's text through MSAA costs 14 calls: measure its wall-clock
     cost against NVDA's, then reduce it.
  6. An audit of demonstrations against end-to-end coverage.
  7. Remote operations everywhere they help (Dickson, 2026-10-07): every
     UIA path that makes a sequence of calls (the review cursor's
     commands, say-all, which reads several pieces ahead per call, object
     navigation, report focus, a dialog's text, selection reads, and any
     other the audit finds) runs as a named remote operation behind one
     entry point with the classic fallback, with call counts pinned on
     both paths and the wall-clock gain measured. NVDA uses remote
     operations only for browse mode's heading search and some Word
     operations.
  8. Local-only scenarios (Dickson, 2026-10-07): scenarios that need
     Windows 11 Notepad (its spell checker and its UIA document) are
     marked local-only in the registry; the GitHub e2e job skips them
     through an explicit setting in its configuration, and every local
     run includes them. `notepad_spelling_errors` becomes such a test
     again rather than a demonstration.
  9. GitHub's runner installs Windows Terminal (Dickson, 2026-10-07), so
     the terminal scenarios run against it there as well as here.
  10. Say-all batches by count (Dickson, 2026-10-07): read 20 lines or
      sentences at a time, and read the next 20 once fewer than 10 remain
      to be spoken, replacing the estimate of speaking time. Measured
      against Windows 11 Notepad itself (time per batch, gaps between
      pieces, calls), with remote operations on and off and against NVDA,
      which reads one piece at a time as the previous one starts.
  11. The instruction limit (Dickson, 2026-10-07): measure UIA's
      unpublished cap (NVDA's local emulator of remote operations assumes
      10,000) and count what each of Verbatim's programs executes at
      typical and worst-case sizes, to confirm they stay well under it.
      Programs are not made resumable: their work is bounded, and
      resuming is left until something like Word's whole-document
      searches needs it. The terminal's upward search (256 lines, never
      approved) is reconsidered with these numbers, and with UIA's
      `FindText`, which searches a range inside the provider in one call
      (and is also a remote-operation instruction), if Windows Terminal
      and the console host implement it; Dickson decides the bound.
  12. MSAA call counts (Dickson, 2026-10-07): measure the wall-clock cost
      of MSAA paths against NVDA's (a cold MSAA focus, 30 calls; a focus
      into a message box, 44 with its text; navigation and list moves),
      and reduce them: reading static text through window messages,
      reusing what the focus walk already fetched, and anything else the
      measurements point to.

  13. Newer UIA (Dickson, 2026-10-07: UIA support must be as robust and
      performant as possible). Not used by Verbatim today, though NVDA
      uses most of them:
      - `IUIAutomation6`: `CoalesceEvents` (UIA filters duplicate events
        before they reach the client; NVDA enables it) and
        `ConnectionRecoveryBehavior` (UIA adjusts its timeouts for a
        provider that stops responding; NVDA enables it), and event
        handler groups (many registrations in one call).
      - `IUIAutomationTextRange3`: `GetAttributeValues` (every attribute
        for a range in one call, for the classic path),
        `GetEnclosingElementBuildCache` and `GetChildrenBuildCache` (the
        element or embedded objects with their cache, in one call).
      - `SelectionPattern2` (first, last, and current selected item and
        the count without fetching the whole selection).
      - The active text position changed event, and `FindText` (item 11).
      Dickson decided to use them (2026-10-07). All predate Windows 11
      (TextPattern2 and the annotation, link, and style attributes:
      Windows 8; TextRange3: Windows 10 1703; SelectionPattern2: 1709;
      IUIAutomation6 and the active text position event: 1809; remote
      operations: build 20348), so Verbatim assumes they exist and has
      no fallback for older Windows. Fallbacks remain only where an
      application decides: a provider that does not implement a newer
      pattern, and a window with no server-side provider, where remote
      operations cannot run.
      NVDA uses them all except `TextPattern2.GetCaretRange` (it infers
      the caret from the selection; Verbatim already uses
      `GetCaretRange` where supported), so they are proven. Edge cases
      NVDA handles that Verbatim must too: a missing `SelectionPattern2`
      or `IUIAutomationTextRange3` falls back to the older pattern; an
      attribute read that fails is "not supported", and a mixed value is
      handled apart; a selection container that raises (Qt) or is null
      (Outlook's attachment list) is "none" without cutting focus speech
      short. NVDA also registers some property events only for the focus
      and its ancestors ("selective" registration, automatic by default)
      and rate-limits events itself ("enhanced event processing", on by
      default, with a feature flag). Verbatim already has the rate
      limiting: the listener queues each event without calling into the
      application, with NVDA's limiter rule (one waiting fact per element
      and kind), and each outpost limits its batches per application
      thread. Selective registration is adopted with no setting
      (Dickson, 2026-10-07). NVDA's automatic choice uses it from
      Windows 11 22H2, and Verbatim's minimum is now 24H2, the oldest
      version Microsoft still supports (Dickson, 2026-10-07: versions out
      of support are not supported, which removes fallbacks and branches
      for them), so it is unconditional.
      Each is weighed by measurement and adopted where it helps; the
      text attribute survey (item 2) covers the full list of UIA text
      attributes, including annotations, link, culture, font weight,
      colors, decoration styles, sub- and superscript, hidden and
      read-only text, style names, and paragraph spacing.

  14. Text range conformance (Dickson, 2026-10-07): audit Verbatim's
      text-range code against Microsoft's guidance ("Understanding
      Performance Issues When Using the Text and TextRange Control
      Patterns", "Using IUIAutomationTextRange", the text units and
      embedded objects topics), fixing or adopting with tests:
      `GetText` with a length limit where a range is unbounded (the
      terminal's line-bounded reads keep -1, which Dickson judged fine);
      how an unsupported unit is detected, since providers silently use
      the next larger unit; ranges after the text changes; the mixed and
      not-supported attribute tokens on both paths; `FindAttribute` to
      find formatting or spelling errors in one call; embedded objects
      (`GetChildren`, `RangeFromChild`, virtualized items); hidden text
      (`IsHidden`); `GetVisibleRanges`; annotation objects and
      `RangeFromAnnotation`; and `ShowContextMenu` for autocorrect and
      IME candidates.

The order from here: the say-all change (item 10), then the terminal
measurements (items 2 and 4), then the MSAA call counts (item 12, which
item 5 is part of), with the instruction-limit measurements (item 11)
alongside as build slots allow.

## Test design decisions (2026-10-07)

An audit of the tests found shortcuts throughout; Dickson's standard is that each test targets one fixed application and one code path and asserts one exact behaviour, with no alternatives that depend on the machine. Decisions so far:

- Editing scenarios exist twice: against a Windows Forms text box (MSAA and the edit control's messages, run everywhere) and against Windows 11 Notepad (UIA, local-only). Spelling errors and other formatting have tests of their own.
- Terminal scenarios exist twice: in Windows Terminal and in the console host, each strict, with the window's owning program asserted.
- `explorer_folder_window` keeps its own folder (deleted at the end) but asserts exact speech; Explorer's window announcement on GitHub's runner is to be understood and asserted exactly, not tolerated.
- `settings_system_page` is acceptable; if the list can be focused directly rather than by Tabbing, do that.
- `start_menu_search` is removed: without IAccessible2 support it tests nothing useful.
- `switch_to_onecore` switches to Microsoft David and requires that exact voice; if GitHub's runner lacks it, that is found out there.
- Each step gets a response-time budget, from event to queued speech and to audio, set from measurements on this machine.
- The flood policy changes (Dickson, 2026-10-07): the first "Lines spoken in full" lines of a burst are spoken whole, never cut short by output that arrives while they are spoken. Only when that group has been spoken does Verbatim look at what arrived meanwhile: if it is more than the limit, it says "skipped N lines" for all but the newest "Last lines to speak" and speaks those; it repeats the same decision after each group until the output stops. A flood that ends while the first group is spoken is therefore heard as its first lines, one exact "skipped N lines", and its last lines, which the flood test asserts exactly; a separate test covers a flood that overflows the terminal's scrollback.

## Explorer focus identity (found 2026-10-07)

A failed `explorer_folder_window` run showed three faults, queued for one fix:

- Explorer can give a new focus the UIA runtime id of an element that has died (going back from a subfolder, "Inner" took "delta.txt"'s id). The outpost's registry then hands Core the old node id, and Core, which treats a matching id as the same focus, stays silent. NVDA drops a focus event as a duplicate only when the old element still has keyboard focus when read live; the outpost does that check, reissuing a node id when the old element no longer has focus, inside the existing remote enrichment.
- After going back, NVDA says "Inner 1 of 4"; Verbatim says "Inner not selected 1 of 4" and then "selected", because it builds the focus from the event's cached states, read before Explorer selected the item.
- The first focus in a new Explorer window waited 2.6 seconds in the outpost's queue behind reads Explorer was slow to answer while building the window, against a 200 ms budget.

Fixed on 2026-10-07, each with mockapp and Core tests and not yet run against Explorer itself:

- The outpost reads, in the focus's remote operation, whether the element a runtime id's node stands for still has the keyboard focus, and gives the id a new node when it does not or cannot be read (`docs/parity.md`, "Duplicate focus suppression"). A held element that is gone fails a whole program before it runs, so the program is run once more without it.
- NVDA's focus event carries only its base cache; its states, value, and details are fetched when the focus is handled. The outpost now takes those from the focused element it reads when it handles the focus, at no extra call (`docs/parity.md`, "How an outpost turns events into focus reports").
- A focus change now goes before the events of other objects queued ahead of it, in the batch and in the batch in progress (`docs/crates/verbatim-outpost.md`, the queue). Reproduced in mockapp with every provider call answered 20 ms late: the focus waited about 610 ms behind ten selections before, and about 0.1 ms after (`docs/performance.md`, "A focus behind other objects' events").

## Terminal decisions (2026-10-07, after the measurements)

- The outpost keeps reading a terminal while output reporting is off: typed echo depends on those reads.
- The console host's slowdown under a flood (about twice as long, half of it the console host's own events) is compared with NVDA's on the same flood before anything else is decided; no minimum interval between reads.
- The anchor search always uses FindText, with no line bound, for a predictable cost (about 4 ms remotely), replacing the 256-line walk.
- Text attributes are fetched generically, never by application: Verbatim fetches the attributes the theme's indications ask for, among those the focused text control supports. Support is learned from the control itself, since UIA answers an unsupported attribute with its "not supported" value, and remembered for that control. Hidden text (`IsHidden`) is never fetched. New indications for themes: background colour, strikethrough, the kind of underline, font size (headings) and bullet style. Links and spelling errors come through the same mechanism: spelling errors through the annotation types attribute, links through the link attribute wherever an application supports it.
- The anchor search's worst case is measured in time, not only instructions: each FindText inside a program costs about 3 ms, so failed matches add up. Each terminal is tested for whether its FindText matches text spanning a line break; where it does, the search looks for the line above and the anchor's line together, one call finding the exact pair. Where it does not (Windows Terminal, as measured), it searches for the more distinctive of the two lines (not blank, then the longer). At most 20 matches are checked before the search gives up (Dickson, 2026-10-07; it was 64).
- Full-screen programs (Dickson, 2026-10-07): when the terminal is read afresh, the whole visible screen is compared with the one last seen, not only the newest 30 lines; the flood policy's limit applies only to what is spoken. A single changed line anywhere on a tall screen is therefore always spoken, and a screen where more lines changed than the limit gets "skipped N lines" and the last ones. Whether the terminal shows its alternate screen is to be detected from the text itself (the document starting where the visible range starts: no history above the screen), verified live in both terminals. Tested end to end with a script that switches to the alternate screen and draws and changes 40 rows, and in mockapp.
- Reading terminal output on demand (Dickson, 2026-10-07). The outpost reads a terminal's new output at once while Core has room for it, and only marks it changed while Core's output queue is full. Core asks for new output when it hands the last line of a group to speech, so the answer is ready before that line ends; the flood test measures the gap before the skipped-lines announcement. When the anchor's line has left a full history, Verbatim says "skipped more than N lines", N being the history's size, known once its line count stops growing, less the lines about to be spoken. Dropping the console host's text-change subscription during a flood was considered and rejected as too risky (Dickson, 2026-10-07): unlike on-demand reading, a missed transition would leave the terminal silent with no event to recover from. On-demand reading is modelled as an explicit state machine, a pure transition function over every state and event (text changed, a read done, Core having room, a key cancelling speech, focus lost and regained, output reporting toggled, the setting changed, an outpost restart), each transition tested in Core or the outpost and in mockapp, with end-to-end tests of Control during a flood and of leaving and returning during one. A cancel tells the outpost to move its anchor to the end without speaking, as NVDA drops what is pending. The flood setting may exceed the terminal's history, degrading to NVDA's behaviour (everything read) for anyone who wants it: when the anchor's line has left the history and no count is possible, Verbatim says "skipped lines" without a number rather than a meaningless one, and Core's waiting output is bounded by bytes rather than lines, so a very high setting cannot exhaust memory. Reads, slowdown and the gap are measured before and after in both terminals.
- Every terminal line counts as a line, blank and whitespace-only lines included (Dickson, 2026-10-07): "skipped N lines" counts every line of output, whether the outpost never read it or Core cut it, so it matches what the review cursor shows. Whether a blank line is spoken is a separate matter (it is not).

## Terminal risks found by studying NVDA (2026-10-07)

A read-only study of NVDA's terminal code found these risks in Verbatim's design, each to be fixed with a test meeting docs/testing.md, in the terminal work package with on-demand reading and the full-screen fix:

- A typed space is not echoed until a visible character follows (trailing padding hides it).
- Typing above the document's last line (a status bar on the bottom row, nano's help rows) is not echoed, and the whole edited line is spoken on every keystroke.
- A password can leak: any rewrite of the anchor's line while typing is held (a clock in a status bar during a password prompt) speaks the held characters; Control+C and Escape do not forget held typing, as NVDA's Tab, Control+C, Control+D and Control+Break do.
- Mid-line edits under a blank or missing line above (after `cls`) are spoken as the whole line, without echo.
- A key press can let one or two lines through: terminal output already in Core's channel is handed to speech after the cancel.
- The console's keyboard layout is taken from the client process, not the console host's thread (NVDA's #10113).
- Resizing a terminal, which reflows its rows, may speak a screenful.
- Typing past the right margin arrives as a new line and stops matching held typing.
- A rewrite split across two reads speaks a fragment ("oading 51%").
- A scrollback full of identical lines goes silent.
- The legacy console (no UIA text) and terminals without UIA are silent.

Decisions (Dickson, 2026-10-07):

- A line whose only change is a single symbol replaced in place (a spinner) is silent; the line is spoken once it changes to text.
- Inline prediction ghost text (PSReadLine, fish, zsh-autosuggestions) is spoken, since users need it to use those features; typing that matches it must still be echoed.
- Lines made only of symbols (vim's "~" rows, box-drawing rules) are spoken as they are for now, not skipped (Dickson, 2026-10-08, reversing the day before): punctuation processing (M8) decides how they are spoken, and the roadmap's M8 entry says so.
- Pausing speech with Shift: a paused queue does not drain, so Core does not ask for more output, and resuming lets it drain and ask again; on-demand reading needs no special case for it.
- Long lines and the memory bound (Dickson, 2026-10-07): the per-line 4 KB cut is removed, so a line is spoken whole, as NVDA speaks it. Core's waiting terminal output is bounded at 10 MB in total, so extremely long output is not cut; if the bound is ever reached, the oldest waiting lines join the skipped count. A single line larger than the whole bound is cut on a grapheme boundary and Verbatim says it was cut. Long lines must be read correctly end to end: a line far longer than the terminal's width, one that keeps growing past any earlier size, and change detection on such a line, each tested exactly, with reads sized to fit the outpost's message limit.
- A limit on outpost messages (Dickson, 2026-10-07): messages between Core and an outpost, newline-delimited JSON over anonymous pipes, had no size limit, so an enormous line from an application could make the reader allocate without bound. Each message is now limited to 32 MB in both directions (room for 10 MB of terminal output and JSON's escaping); the outpost splits a large terminal read across messages so it never reaches the limit, and a reader that receives a larger message treats the other end as failed (an outpost is restarted, as after a crash) rather than reading on. Tested: the split, the limit, and the rejection.

## MSAA audit against NVDA (2026-10-07)

A read-only audit compared Verbatim's MSAA handling with NVDA's. The tree view focus race (Verbatim announcing a tree view before its item, because the item's focus event is still on its way) is fixed by asking a child-0 focus's accFocus and reporting the focused child, NVDA's outcome reached deterministically. The rest form an MSAA work package, each matched to NVDA with a test meeting docs/testing.md, unless docs/parity.md records a deliberate difference, which is then brought to Dickson:

- Windows Forms tree views get none of the SysTreeView32 handling (class names are compared raw, not normalized as NVDA does): no position, flat navigation, no expanded count.
- Progress bars: NVDA beeps on value changes of any progress bar in the foreground; Verbatim never emits its progress indication.
- Group boxes as context, and multi-column list view item names ("content; Header: content"), already recorded as gaps.
- Tree view check boxes read from the item's state image.
- Help balloons through the show event.
- Owner-drawn controls' display text (needs a display model; M14).
- Description changes, and state changes on the focus's ancestors.
- Verbatim speaks MSAA selections, and a focused list's selected child, where NVDA does not.
- Tree items outside SysTreeView32: NVDA keeps a non-numeric value and takes no level from it.
- A focused list view group header.
- MSAA roles and states with no mapping (IP address, clock, grip and others; busy indicator; traversed and linked).
- Application-specific support (IME candidate lists, Scintilla editors, Qt containers' focus redirect) is deferred until Wasm extensions exist (Dickson, 2026-10-07).
- Lower likelihood: alert-role objects, repeating accFocus until it settles, the caret show event, focus events separated by other events in one batch, and the focused-state check reading ancestors' states as last read.

MSAA calls Verbatim makes more often than NVDA, for the call-count reduction: accRole twice per child-0 focus; a rejected focus read in full before the focused-state check; a repeated focus read in full; full snapshots for events on objects that are not the focus; tree item sibling counts on every read, ancestors included; expanded child counts on every state change; selected-child reads NVDA does not make.
- Selection lists drawn in a terminal (Dickson, 2026-10-08): tested with PowerShell scripts run in the terminal, not with a real application. One script draws a five-item list and moves its ">" marker by rewriting only the two characters; another redraws the whole list region on each move, as Ink-based programs such as Claude Code do. Each is a separate scenario in both terminals. NVDA's reading of each script is captured first to establish the expected speech, and Verbatim is then made to match it.

## Terminal reading by diffing the screen (Dickson, 2026-10-08)

The anchored append read with an in-place screen comparison as fallback is replaced. Every read fetches the visible screen and diffs it by line against the last screen seen, with padding stripped and wrapped rows joined into logical lines; what was inserted is spoken, never what was only deleted (NVDA's rule), and a changed line speaks from the start of the word that changed. The anchor remains only to count lines that scrolled into the history between reads, for floods, "skipped N lines" and on-demand reading. This replaces the full-screen fallback and covers selection lists, output above a fixed footer, scrolling up in full-screen programs, messages printed above a prompt, resizing, deletion-only changes and several changes in one region. Every end-to-end script's NVDA reading is captured first to set the expected speech.

### The tests

Unit tests:
- The screen diff, a pure function: a table of exact cases (insertion at the end, in the middle and above a footer; deletion only; scrolling up and down; rows rewrapped by a resize; repeated identical lines; several changes on one line; the changed-word rule; padding of spaces and other white space; wide CJK characters, combining marks and tabs), plus property tests over random edits.
- The on-demand reading state machine: every state and event pair.
- Core: the flood policy, blank lines counted, the 10 MB bound, long lines, typing reconciliation (held typing matched against what a line gained), symbol-only lines, the spinner rule, terminal output arriving after a key's cancel, and "skipped more than N lines".
- The outpost: counting lines that scrolled into a full history, its overflow, and splitting a large read across messages under the 32 MB limit.

mockapp tests, a scripted terminal through the real UIA code with remote operations on and off: the two paths agree, exact call counts, FindText behaviour, alternate-screen detection, the message limit, and each on-demand transition through a real outpost.

End-to-end scenarios, each a PowerShell script run in the terminal, written as separate code for Windows Terminal and for the console host, with exact speech:
- Output: commands and multi-line output with blank lines; short output ("y", "OK"); a line far wider than the terminal; a long line that keeps growing; symbol-only lines spoken as they are.
- Typing: character and word echo; a typed space at the end of a line; editing mid-line after `cls`; typing past the right margin; tab completion; a password prompt with no echo and one echoing asterisks; a clock in a footer ticking during a password (no leak); Control+C and Escape clearing held typing; inline prediction ghost text; a keyboard layout switched inside the console.
- Redraws: a progress bar rewritten in place; a spinner; a selection list moved by two characters; a selection list redrawn whole; output above a fixed footer; a message printed above the prompt while typing; several changes in a status block; deletion-only changes; a resize.
- Full screen: a 40-row alternate screen opened, a row near the top and one near the bottom changed, scrolled down and up by a line, and closed; a cleared screen.
- Floods: within the history, asserted exactly; overflowing the history ("skipped more than N lines"); Control during a flood; leaving and returning during one; pausing and resuming with Shift; output reporting toggled; a flood of identical lines; a raised flood setting.
- The review cursor over the screen grid.

Further decisions (Dickson, 2026-10-08):

- The legacy console (no UIA text) is not supported or tested: every supported Windows has the console host with UIA, and most have Windows Terminal. The NVDA study's risk about it is dropped.
- The changed-word rule must work in every language. Today it finds a word's start by the last whitespace before the change, so in Chinese, Japanese or Thai, which have no spaces, it speaks from the start of the line, and it compares characters rather than graphemes. It is to use verbatim-text's word segmentation (ICU's, with dictionaries for scripts without spaces) and grapheme boundaries, with unit cases in those scripts.
- Floods and the screen diff. The anchor becomes the screen's top row as last read, found again by its text. How far it moved up is how many new lines arrived since the last read; the diff of the visible screens gives the ones still visible (in-place updates, such as a program's footer redrawn, are found by the same diff and are not counted as new lines), and the rest are the lines that went by unread, spoken as "skipped N lines". When the old top row has left a full history, the count is "more than N". A program that redraws its region after a flood is therefore read as the flood's new lines, counted, plus whatever the redraw inserted. End-to-end: a flood above a fixed footer that redraws during and after it, asserted exactly.

Outcome of the terminal package (2026-10-08):

- The screen is read and diffed as decided: one remote program per read (the visible screen, its top two rows as the anchor, how far the text scrolled, the rows that went by unread up to the flood policy's limit, and the caret), a pure diff (`terminal::screen`), and on-demand reading as a pure state machine (`terminal::reading`), each with unit tests, and mockapp tests of the read's exact costs and of each on-demand transition through a real outpost. The anchor is found at a range kept for it while nothing was discarded, otherwise by `FindText`, padded in the console host, whose `FindText` matches padding; neither terminal's `FindText` can search across a line break (measured).
- The flood's intermittent "flood line 31". The hypothesis was refuted by the outpost's timeline: the outpost read all 2,001 lines within 1.5 seconds, and Core's group ended early because the first line's mark was reached before the second line had arrived. Groups now end at closing marks. conhost_flood and windows_terminal_flood each passed ten runs in a row.
- In the console host, UIA's text changes stopped reaching the outpost in the middle of a 12,000-line write in 2 of 14 runs, after a few reads of about 0.12 seconds, while the console host kept writing and raising its other `WinEvents`; why is not known. The console's own update `WinEvents` are now its terminal's text changes as well, merged per window, and a change observed before the last read began is covered by it (a deliberate difference from NVDA, recorded in `docs/parity.md`). conhost_scrollback_overflow then passed ten runs in a row.
- Windows Terminal takes a write of 12,000 lines whole before its first text change reaches a client in most runs, so the single write has no one correct spoken form; `windows_terminal_scrollback_overflow` writes the burst's first 30 lines on their own and the rest while their group plays (the reasons and NVDA's capture are in the scenario's module).
- The NVDA study's risks: the password leak (a rewrite echoes held typing only when what it gained is the typing), Control+C, Control+D, Control+Break and Escape forgetting held typing (they type no text, so they come as `Input::ClearingKey`), a typed space at a line's end and typing over ghost text (echoed by the caret's move), typing above a status line (the caret's line is the changed line), mid-line edits after `cls`, typing past the right margin, a rewrite read half done ("51%", not "oading 51%"), and the console host's keyboard layout (its own thread, from its `WinEvents`, NVDA's issue 10113). A key press letting lines through is fixed by `SpeechCancelled`'s time. Resizing and a scrollback full of identical lines whose history is full are not fixed: the first needs a live check of how each terminal reflows its rows, and the second cannot be counted from text alone.
- What a key did: Backspace and Control+Backspace speak the text removed, from evidence, so the classic edit control's Control+Backspace, which inserts DEL, says nothing; any other key that types no text (`CaretMotion::Other`) speaks text it removed at the caret, a new line, or by where the caret landed. A caret key's answer read after a later key was pressed is not spoken for it, and a terminal's line redrawn by a caret key is the key's own, not output.
- Done since (2026-10-09): NVDA captures for the typing scenarios, the selection-list scripts ("Selection lists in a terminal" below), the full-screen alternate screen and the long-line scenarios, each with NVDA captured first.
- Done since (2026-10-09): the review cursor after new output and the flood above a redrawing footer, each with NVDA captured first.
- Done since (2026-10-09): tabs and two windows of each terminal, with NVDA captured again first (`conhost_two_windows`, `windows_terminal_two_windows`, `windows_terminal_tabs`; the tab difference is in `docs/parity.md`).

### Selection lists in a terminal (Dickson, 2026-10-09)

Superseded the same day by "Terminal line keys as NVDA has them" below:
the rules for a finished redraw, the output held while a line key's
watch was open and the kept earlier screens are removed. What follows is
the record of what was built and why it was taken out; the wrapped
Escape at its end stands, judged from the terminal's memory alone.

When Up or Down Arrow makes a program rewrite other lines and the caret is
not on a changed line, Verbatim speaks the line that gained the marker,
a deliberate difference from NVDA recorded in `docs/parity.md`. Reading a
redraw half done is a bug under any rule: the console host's marker
script said "> banana" three times, its watch answered by the caret
moving to the line the marker was leaving, before the program had written
anything.

No event says a program has finished redrawing. The console host raises a
console update event for every write and moves its caret as each write
asks, and both terminals raise their UIA events for whatever their
renderer last painted, so every event can come from a redraw under way.
The evidence is therefore the screen itself, compared with the screen
before the key: the outpost reads the terminal on each of its text and
caret changes while the key's watch is open, and a read answers the key
only when it shows a finished effect. A line gained text that another
line lost: the marker moved, whichever was written first, and every such
line is spoken, top to bottom, the caret's line among them when it is
one (Dickson, 2026-10-09; it was the first such line alone), unless more
lines lost that text than gained it, a marker erased and not yet drawn
again, which is a redraw under way. A program that finishes moving one
marker before it starts erasing the next is answered with the first
alone, since nothing on the screen says another is coming. The caret's line gained
text: a line recalled from history. The caret's line was cut short with
nothing else changed: a shorter line recalled. The caret moved to the next
or previous line and no line only lost text: an editor's caret. A read
showing only text removed (the marker erased and not yet drawn again), or
the caret moved elsewhere over unchanged lines, is a redraw under way and
answers nothing. While the watch is open the terminal's memory stays at
the screen before the key, so no read under way is spoken as output
either, and the answering read's output leaves out the line the answer
speaks. A watch that ends unanswered (the next caret key, or its
ten-second bound) keeps the screen before the key, so what the key did is
then spoken as output.

Scenarios, each in both terminals, with exact speech:
`windows_terminal_marker_list`, `conhost_marker_list`,
`windows_terminal_redrawn_list` and `conhost_redrawn_list`, and, for a
key that moves two markers, `windows_terminal_two_markers` and
`conhost_two_markers`.

Escape on a typed line that had wrapped onto a second row (2026-10-09)
says the text removed, as on a line of one row. The caret's line, as both
terminals' text pattern gives it, is one row, so the caret's line before
the key (the second row) and after it (the prompt's row) do not show the
removal, and the caret going up a row was spoken by the landing rule as
the new line ("ready>"). The screen's text, though, gives a line that
wrapped whole (measured in both terminals). When a key judged by where
the caret landed takes the caret off its row, the outpost compares the
screen before the key with the screen now: when the caret is on the same
line of the screen and that line was cut short, the rest of it is the
text removed (`CaretReply::removed`); while the next line still shows
that rest, the redraw is under way and the watch stays open. The screen
before the key is the newest of the last few reads that ended before it
was pressed, since the program's answer can be read more than once before
the key's request reaches the outpost. Both typing scenarios end with this
case.

## Decisions to confirm with Dickson

Made while Dickson was away (2026-10-09).

Confirmed by Dickson (2026-10-09):

- Selection lists: the rules above for when a redraw has finished. A list
  marked only by colour, with no marker in its text, changes no text, so
  Up and Down in it say nothing; a key that changes nothing (Up at the top
  of a list, or with no history) says nothing, as in a text field.
- Selection lists: output the program writes while a line key's watch is
  open is held until the key is answered, or its watch ends, and is then
  spoken.
- The outpost keeps the screens of the last eight reads of a terminal
  for finding the screen before a key; a key whose request arrives after
  more reads than that is judged by the caret alone.
- The footer flood redraws its footer unchanged during the flood and
  changed once after it, so its speech is the same however the reads
  fall: a footer changed during a flood is heard, or not, as the reads
  happen to find it. NVDA was captured with this script and, first, with
  a footer changing every twenty lines.
- Selection lists, changed (confirmed, being built): every line that
  gains a marker another line lost is spoken, top to bottom, not only the
  first. The marker rule is
  tried live against list-drawing programs already installed on the test
  machine; if any shows a false match, the rule is removed and Verbatim
  speaks the caret's line, as NVDA does.

To be confirmed:

- Every line that gained a marker (2026-10-09): each line is spoken as
  an utterance of its own, as lines of output are. A read in which more
  lines lost a marker's text than gained it is a redraw under way, since
  without that rule a read made after the first marker had moved, and
  before the second was erased, answered the key with the first line
  alone. A program that moves one marker completely before it erases the
  next is still answered with the first line alone, and the rest is
  spoken as output; nothing on the screen says another marker is coming.
  The two-marker scenarios' script erases both old markers before it
  draws the new ones.
- A failed speech assertion's trace (2026-10-09): the failing step's
  trace ID is the one carried by the speech frame of the utterance the
  assertion failed on, or, when the expected utterance never came, by the
  newest utterance the step heard; a step that heard nothing has no trace
  to show. The assertion unwinds without the panic hook, so its message
  is printed once, in full, after the logs are collected.

- The review cursor after new output: the plan's test read the same
  line after output written below it, but the review cursor follows the
  caret by default, as NVDA's does, and both NVDA and Verbatim, captured
  live, move it to the prompt the output leaves the caret on. The
  scenarios pin that default; the case of the review cursor staying on
  its line needs following turned off, which is not tested.
- The blank lines a burst of output starts with are not counted as lines
  (a cleared screen's rows above a footer used up the first group of
  thirty), an exception to "every line counts". Checked in the coherence
  review (Dickson, 2026-10-09), which removes the exception (decision 1
  below).
- The full-screen scenarios use the terminal's own height (30 rows in
  both test windows) rather than the 40 rows the plan named, so the
  alternate screen is exactly the window: the script draws as many rows as
  the window has.
- System Information without the foreground right: the minimize before
  the restore is in the agent's `SetForeground`, not the harness's
  `bring_forward`, so every caller (launches, Notepad's documents, the
  tab closing at cleanup) brings a window forward the same way and the
  protocol needs no minimize request. A window already in front is left
  as it is. The wait for the minimize is bounded at five seconds, well
  inside the client's read timeout.
- System Information: its title is not the run's own, so the scenario
  checks no such window is open (`Scenario::require_absent`, now public)
  before `launch_titled`, as `launch_target` did.
- `set_foreground` still reads `GetForegroundWindow` straight after
  `SetForegroundWindow`; on the old path that read the foreground mid-change.
  Left as it is, since the new path restores the window before the read;
  waiting for the foreground event instead is a follow-up. Done the same
  day: it waits on window events, the foreground event among them, for the
  window to be in front, bounded at five seconds, and no longer takes
  `SetForegroundWindow`'s `false` as final, since a window restored from
  minimized can still be taking the foreground.
- The focus's element after one follow-up read: a selection event of the
  focus supplies it by reading the focused element once, as a focus event
  does, since the listener's selection fact carries no element. A caret or
  text event cannot supply it: those subscriptions follow the focus's
  element, so none arrives while it is unknown, and no subscription was
  added for that case.
- `hello_version_mismatch_is_refused`'s reset: fixed at the launch, by
  giving a child with a capture file a handle list naming only that file,
  rather than by making the server's socket duplicates non-inheritable,
  which would leave every other inheritable handle in the agent, and a
  window between the duplicate and the flag change, for a concurrent
  launch to take. The regression test asserts the peer reads the end of
  the stream within a 30-second bound while the child runs.
- The console host's flood (`conhost_scrollback_overflow` and its
  `_during_group` twin failing about one run in two, after the terminal
  package's second part): a screen read whose view moved is trusted when
  the old screen's last line is still on its row, as it was or grown,
  rather than never; the footer flood's case, a footer drawn a row lower
  over a row the flood then wrote, is still set aside. And a read whose
  text grew between its walks to the text's end is no longer set aside:
  the shift lies within the rows the text grew by, and moving from the
  anchor finds it exactly, halving that range each step (at most 14
  steps for 9,000 rows, a few hundred instructions within the remote
  operation's limit). Both checks set aside nearly every read during a
  console host flood, which then went unread until it ended. The count
  of "skipped more than N lines" still comes from the screen's lines, not
  its rows, so a read made after a flood ends and before the prompt is
  written counts one more; it is left as it is, since Core's own request
  at the end of a group comes after the prompt in both scenarios.
- Windows still opened in front: the Settings page now opens minimized
  and inactive and is brought forward (`launch_titled`), and the agent's
  `Present` condition no longer counts a cloaked window, so the Settings
  app's closed, cloaked window is never taken for the new one. File
  Explorer's folder window still opens in front with the agent's right,
  against the decision of 2026-10-08, because nothing else was heard:
  File Explorer raises its only foreground event as its window is
  created, before it is shown and titled, and when Windows refuses it then
  (the agent lacking the right), or the window opened minimized, the
  window becomes the foreground with no event when it is restored and
  brought forward later (measured with an out-of-context `WinEvent` hook
  after a zero-pixel mouse move from another process: minimize, restore,
  `SetForegroundWindow` and `SwitchToThisWindow` all left it in front
  silently). Opening the folder through the desktop's shell
  (`IShellDispatch2::ShellExecute` on the desktop's folder view, which runs
  in the shell's process while the desktop holds the foreground) did bring
  it forward with its foreground event, and NVDA, captured that way, said
  "<folder> - File Explorer", "Items View list", "Inner not selected 1 of
  4"; but Verbatim, reading the window's title at the event, said "File
  Explorer", the title before File Explorer sets the folder's, where NVDA
  reads it about 100 ms later as it speaks. That approach was set aside
  rather than build on a second difference; reading a foreground window's
  name only once it has been shown, or after its title settles, is a
  follow-up.
- Tabs and two windows of each terminal: NVDA was captured again, since
  the earlier captures were gone. The earlier findings did not reproduce:
  after Control+Shift+Tab Verbatim said the tab's terminal and its line,
  with no "blank", no late "window" and no typing lost, and the capture's
  `--front` to a second Windows Terminal window worked (after
  `SetForeground` began waiting for the foreground); NVDA leaves a
  console's title out of its terminal as Verbatim does. The one
  difference left, NVDA saying "list" and the tab as Control+Tab moves the
  focus through it, is recorded as deliberate in `docs/parity.md` rather
  than matched: it is a race between Windows Terminal moving the focus on
  and the reader judging the focus event, which NVDA wins by judging it on
  its event thread; matching it would mean judging focus events in
  Verbatim's listener as they arrive, a change to every application's
  focus handling.
- The tabs scenario opens its second tab from the command line
  (`WindowsTerminal.exe -w 0 new-tab`) once the first tab's window is
  announced, since a window opened with both tabs is announced with
  whichever tab's title Windows Terminal reports first. NVDA was captured
  on that step too. Its Windows Terminal turns `confirmCloseAllTabs` off,
  the one setting changed from the release's defaults, so cleanup can
  close the window by its title. The second tab is titled with the first
  tab's title and " two", so that title names the window whichever tab is
  in front, and `Scenario::expect_exit_at_cleanup` now finds the window
  whose title the given title contains.
- The NVDA captures of Windows Terminal launched the harness's portable
  copy's `WindowsTerminal.exe` directly, as the scenarios do, never
  `wt.exe`, so the owner's Windows Terminal was never involved.
- The Settings page's "Windows isn't activated" banner was read once in
  five `settings_system_page` runs in place of the search box's "blank";
  left as it is and reported.

Made while carrying out "Terminal line keys as NVDA has them" (2026-10-09,
to be confirmed):

- The list scenarios' scripts now write each move in one write (escape
  sequences, with the console's processing of them turned on as the
  full-screen script does): NVDA, captured with the earlier scripts, read
  their cell-by-cell writes part-way and said different things from run
  to run, so no exact test could say what NVDA says. NVDA was captured
  again with the new scripts in both terminals: "greater banana",
  "greater cherry", "greater banana" for the marker that the caret
  follows, every run, and "blank" for each key of the list redrawn with
  the caret below it.
- The redrawn list's scenarios assert Verbatim's own speech there, the
  line that gained the marker as output ("> banana"), not NVDA's "blank":
  matching it would take reversing two deliberate differences, a key
  that does not move the caret being silent and a rewritten line being
  spoken from the word that changed. Recorded in `docs/parity.md`.
- The two-marker scenarios are deleted. With line keys answered by the
  caret's line and the rest spoken as output, they tested the marker
  scenario's answer and the redrawn scenario's output together, and the
  order of those two depends on whether the program's write or Core's
  request reaches the outpost first.
- A line's change belongs to the caret key that changed it only once the
  key's answer has said what it did (`key_owns_line`), not from the key
  press: a key that changes the caret's line without moving the caret
  (PSReadLine's menu after its first move) would otherwise have its
  change dropped as the key's and never spoken, its watch ending without
  evidence.
- The caret key's watch is checked before the console host's own update
  events are read, as it already was for UIA's text changes, and again
  after any read of a terminal that read its caret: the read's caret
  counts as read, so the change that caused it would show the watch
  nothing, and Windows Terminal raised no caret event when PSReadLine's
  menu moved its caret after the text.
- On a screen with no history above it, before a read and after it, how
  far the text scrolled is found from the text (`alternate_scroll`): the
  range kept at the top row stays on its row while a pager scrolls the
  text through it, so it always said 0. The diff's alignment of a scroll
  was already right in most cases through its longest common run of
  lines; what the shift corrects is what each row is remembered to have
  said.
- The wrapped Escape, which judged a key against the newest of the last
  eight screens read before it, now has only the terminal's memory, when
  the read behind it ended before the key; otherwise the key is judged
  by the caret alone, as for a key whose request came after more reads
  than were kept.
- The pager scenarios run a script pager on the alternate screen
  (`pager.ps1`), written in one write a move, rather than Git's `less`,
  which a test machine need not have and which writes a move in pieces in
  the console host.

Made while setting Windows' screen reader flag (2026-10-09, to be
confirmed):

- The flag is saved in the user's profile and every window told
  (`SPIF_UPDATEINIFILE | SPIF_SENDCHANGE`), as NVDA does, so a Verbatim
  that crashes or is ended from outside leaves it set until a screen
  reader next clears it, as NVDA's crash does. Not saving it would clear
  it at the next sign-in, at the cost of differing from NVDA.
- It is set after a running instance has been replaced and cleared before
  the startup mutex is released, so the replacing instance's flag is
  never cleared by the instance it replaced. NVDA's `--no-sr-flag` has no
  Verbatim option yet.
- The exits that clear it besides a normal one: a failed start, a panic
  unwinding through `main`, the console's control events (Control+C,
  Control+Break, the console closed) through a console control handler,
  and the session ending, through a hidden window of its own that
  handles `WM_ENDSESSION`, since none of the GUI's windows clears it
  first. Checked live: a normal quit and Control+Break clear it, and the
  process ended from outside leaves it set. Closing the console window
  and signing out were not tried live: a console window opened for a
  test can be handed to the owner's Windows Terminal, and signing out
  ends the session the run is in.
- With the flag set, Windows PowerShell prints a notice that it leaves
  `PSReadLine` out as its interactive host starts, before the shell runs
  its script. The terminal scenarios now bring a window forward only once
  its shell has written its process id, so the notice is on screen before
  the terminal is announced and is not new output; a second Windows
  Terminal window or tab, which opens in front at once, runs its shell
  with `-NonInteractive`, which leaves the notice out (the password prompt
  that needs an interactive host is in a first window). NVDA, captured
  with a tab opened the same way, spoke the notice as output when its
  read of the tab came before it was printed, and missed it otherwise.

Made while fixing blank lines and rows in the screen read (coherence
decision 1 and the rows fix, 2026-10-09, to be confirmed):

- A blank line on a row the old screen had not written to yet, the rows
  just below its last line, is not inserted, as the decision says; it
  follows that a blank line a program prints onto such a row (`echo ""`
  on a screen not yet full) is not counted either, where one printed as
  the screen scrolls, or read from the history, is. Nothing in the text
  tells a blank row a line feed passed through from one a program jumped
  over.
- A line's rows are worked out from its cells and the terminal's width,
  its top row's cells padding included: the console host pads every row
  to the width, so a wrapped line's text is a whole number of rows, and a
  line whose text is shorter takes the rows its cells fill. Wide
  characters count two cells (`verbatim_text::cell_width`).
- A wrapped line straddling the screen's top after a scroll is taken as
  what is left of it, the new screen's first line, so it is neither new
  nor a rewrite.
- When the screen scrolled away whole and the row the old screen's last
  line was on now holds something other than that line, as it was or
  grown, that row is the first new line, not a change of the old last
  line: Core put such a change in place of the newest line still waiting,
  which dropped one line of a flood above a footer (the footer flood said
  "skipped 40 lines" in place of 41 in both terminals once the blank rows
  above its footer were no longer counted).
- The order Core takes a key's report and an outpost's message in is
  left as it is: the reducer picks either channel when both wait, so a
  terminal's echo of a typed character is now and then read as output
  before the typed character is known (seen in `conhost_typing` and
  `conhost_flood`, once each in about thirty runs). Preferring the key's
  channel fixed that but let a key overtake older output (the flood's
  prompt, read before "report new output" was turned on, was spoken after
  it); ordering them by when each was queued is a follow-up.

Made while fixing the trust rule's footer case (2026-10-09, to be
confirmed):

- A console host read whose view moved while it was read is trusted
  whenever its anchor was found. When the old screen's last line is no
  longer on its row and the read's lines do not end with it, it is taken
  as a footer redrawn below the rows the read gives, and remembered as
  the screen's last line: the rows read are still rows the console host
  showed, and only the footer is outside them. Before, such a read was
  set aside, and a flood above a footer, during which the view moves
  under nearly every read, went unread until it ended. A read whose range
  was taken after the footer was redrawn, the view moving again only
  later, ends with the footer, and is diffed as it is.
- `conhost_footer_overflow` writes its flood a line at a time until the
  scenario has heard the first line, then the rest to line 12,000 in one
  write, so the overflow lands while Core holds the terminal and the
  count is exact; the script waits for the go file through a file-system
  watcher that sets an event, which the writing loop checks without
  waiting.

Made while raising the flood limit (2026-10-09, to be confirmed):

- Both terminal limits now go up to 10,000 lines, the first round number
  above either terminal's history (9,031 lines in Windows Terminal, 9,001
  in the console host), so "Lines spoken in full" can cover everything a
  terminal still holds. The sliders keep their steps, one line for the
  arrow keys and ten for Page Up and Page Down, so reaching the top from
  the default takes many presses; End reaches it at once.

Made while removing a new tab's extra "blank" (2026-10-09, to be
confirmed):

- NVDA's source reads the caret's line for any focused terminal and
  says "blank" for an empty one, yet in Windows Terminal it said no line
  for a new window or tab in 17 of 19 captures, and in the console host
  "blank" in all 14; returning to a tab with a prompt, it said the
  prompt in some captures and nothing in others. The cause in NVDA was
  not found, and looks like timing. Verbatim matches the usual result for
  what is new: a Windows Terminal window or tab whose screen is all
  blank as it takes the focus says no line, every other terminal focus
  its caret's line, the console host's "blank" included. The outpost
  sends this as `NoText` before the focus's caret, the answer for a focus
  with nothing to read, so Core needs no new message.
- A Windows Terminal tab whose screen was cleared and is all blank is
  treated the same as a new one when it takes the focus.
- Closing a tab, NVDA said the remaining tab's terminal without its line
  in its capture; Verbatim keeps saying the line, as NVDA does when its
  read of the line is not lost.

Made while writing the dropped end-to-end tests (2026-10-09, to be
confirmed):

- Up Arrow and a character typed at once: Verbatim reads the line's
  change once, the recalled command and the character in it, and says
  it as the change ("echo onex"), without the character's own echo,
  where NVDA says "x" and then the line. `*_up_typing` asserts it.
- Shift pause and resume: speech reports no pause, so the scenarios
  assert only that pausing and resuming during a flood cuts nothing off
  and loses nothing.
- A Windows Terminal window whose shell printed its PSReadLine notice
  before the window took the focus says "blank" for its caret's blank
  row: the rule of no line covers only a screen with nothing on it.

## Language audit (2026-10-08)

A read-only audit found these, ranked by how likely a user of the language is to hit them; each is fixed with exact tests in a language work package, together with the terminal's changed-word rule:

1. The flat review cursor (objects without a text pattern: list items, buttons, labels) walks by code point and by white space: Hindi vowel signs and viramas stop alone, emoji modifiers and flags split, and Chinese, Japanese and Thai names are one word. Walk with verbatim-text's graphemes and words, as text-pattern review does.
2. Backspace and Control+Backspace speak a prediction (the grapheme or word before the caret), not what was deleted; Windows edit controls delete one code point inside Indic and Thai clusters, and the control's word break can differ from ICU's. Derive the deleted text from evidence, comparing the line before and after, falling back to the prediction only when the lines are not comparable. This is the same evidence rule as the terminal's Backspace.
3. The terminal's changed-word rule (already recorded).
4. Typing echo with Windows' built-in text-service keyboards outside Chinese, Japanese and Korean (Vietnamese Telex and VNI, Indic Phonetic) echoes the raw keys, not the composed text.
5. The plain Win32 edit control's word unit is white-space only.
6. Language runs are flattened to one language per line, caret speech carries none, and the language never reaches the synthesizer: OneCore's SSML always uses the current voice's language. Automatic language switching is recorded as "not yet" in docs/parity.md.
7. Left-to-right and right-to-left marks in names and values (Explorer's dates, the clock) are not stripped, and are read as silent characters.
8. Say-all's sentence ends know only ". ! ?": add Chinese and Japanese full-width marks, Hindi danda, Arabic and Urdu marks, Armenian, Ethiopic, and closing marks such as » „“ 」.
9. Typed-word echo ends a word at combining marks (virama, Thai tone marks, ZWNJ): word characters are letters, marks and numbers.
10. A zero-width space counts as a word.
11. Capitals and character names fail on decomposed text: normalize each grapheme to NFC first.
12. ANSI rich edit windows are decoded by the window's flag rather than by the buffer, as NVDA does.

Lower likelihood: kanji-only Japanese segmented as Chinese, and Han outside the BMP; locale-free word rules ("EU:n"); text chunks cut mid-surrogate or mid-grapheme; East Asian Ambiguous cell widths; Cherokee as capitals; Turkish casing for a future table; the ideographic space unnamed.

Decisions on the language audit (Dickson, 2026-10-08): automatic language switching (item 6) is on the roadmap under M8, not in this package; with a text-service keyboard outside Chinese, Japanese and Korean active (item 4), Verbatim stays silent instead of echoing raw keys, until composed echo arrives with the D2 injection helper (M6, also on the roadmap).

## What a key did to the text (Dickson, 2026-10-08)

One rule replaces per-key deletion handling (the caret-history prediction for Backspace and Delete) in text fields and terminals alike. Verbatim keeps a copy of the line being edited, its text as the application exposes it (padding included) and the caret. After any key, once its effect is evident (a text change, a caret move, or the application answering), the outpost reads the line and caret again; the key's effect is the difference. Text removed at the caret, on the caret's line, with the caret still on that line, is spoken: Backspace, Delete (corrected by coherence decision 2 of 2026-10-09: Delete speaks what it leaves at the caret, as NVDA does, and is not among these keys), Control+Backspace, Control+W, Control+U, Escape clearing a line, a cut, in any language. Text added goes to typing echo or output; a caret that only moved is caret movement. A key that is not a movement key, leaving the text unchanged but moving the caret left over white space, deleted that white space (a trailing space in a padded terminal). The read after one key is the copy before the next, so no key is held. Keys pressed faster than the application answers are judged together and their combined effect is spoken once. The guard keeps a selection list's marker moving between lines from being spoken as a deletion.

Caret moves the application makes itself: the outpost reads the caret on every caret event and on every text change, in both terminals (the console host raises caret events on its own schedule, so the read on each text change is what keeps its copy current). Core tracks such a caret, and the review cursor follows it, but an application's own caret move in a terminal is not spoken, as NVDA does not use caret events in terminals.

The text-edit-changed and changes events: checked live in both terminals. If a terminal raises them and they report edits correctly in live tests, that terminal uses them, through a separate path chosen by the terminal's control class and covered by end-to-end tests that would show if it stopped working; no heuristic in code decides at run time. Otherwise the rule above is used.

Caret moves from keys Verbatim does not know (Dickson, 2026-10-08), such as bash's Control+A and Control+E and Alt+B and Alt+F: when the user pressed a key that is not one of Verbatim's caret keys and the caret moved with no text change, Verbatim speaks by where the caret landed, not by the key. To another line: the new line. By exactly one character: that character. To the start or end of the line: the character at the caret, as Home and End speak. Otherwise, at the start of a word, or just after its end: the whole word. Inside a word: the whole word and then the character at the caret ("test e"). A caret move the program makes on its own, with no key pressed, stays silent. Verbatim's own caret keys keep their exact units. Each rule has its own end-to-end script, in both terminals.

Units for caret moves (Dickson, 2026-10-08): every key goes through the same path (the evidence wait, the read of the line and caret, and the judgement of text removed, text added or caret moved). When the caret moved, the unit comes from a small fixed table for the standard Windows editing keys (the arrows, Home, End, Page Up and Page Down, and Control with those), whose meaning is the same in every Windows application and which gives NVDA's behaviour; every other key uses the landing rule above. The rule applies to all editable text, text fields and terminals alike, since Windows 11 Notepad, Win32 edit controls and the terminals all report only that text changed or the caret moved, not what the edit was; the live check of the text-edit-changed and changes events covers Windows 11 Notepad as well as the terminals.

The guard, stated by text positions rather than by lines (2026-10-08): a removal is spoken when the removed text was adjacent to the caret before the key and the caret afterwards sits where the removed text began. This covers Backspace at the start of a line, which removes the line break and joins two lines (spoken as the line break, as NVDA names it), and Delete at the end of one, while still excluding a selection list's marker moving between lines. The text compared is the caret's line together with the lines next to it, in the control's own offsets, so a join or split is seen whole. Win32 edit and rich edit controls (read through their window messages, with the caret's location-change and the value-change events as evidence) use the same rule as UIA text and terminals.


Outcome of the work package (2026-10-08), each recorded in `docs/parity.md` and tested in mockapp against real comctl32 controls where one was needed: class names are normalized for every MSAA class rule, so Windows Forms tree views get the tree view handling; progress bars are indicated off the focus; group boxes are context; report view items are named by their columns; tree view check states come from state images; help balloons are spoken from their show events; description changes, and state changes on the focus's ancestors, are spoken; tree items outside a tree view control keep a value that is no number; and the missing roles and states are mapped. The call counts listed above are cut and pinned (`docs/performance.md`). Verbatim's speaking of MSAA selections and a focused list's selected child is a recorded deliberate difference, left for Dickson. Still open: a focused list view group header, which needs an end-to-end scenario to test against, NVDA's repeated `accFocus`, the caret's show event, focus events separated by other events in a batch, and alert-role objects. Application-specific support (IME candidate lists, Scintilla, Qt) is deferred until extensions.

MSAA selections (Dickson, 2026-10-08): Verbatim keeps speaking MSAA selection changes and a focused list's selected item ("Fruits list Apple" where NVDA says "Fruits list"), the deliberate difference recorded in docs/parity.md since M3.

Start-up speech and budgets (Dickson, 2026-10-08): Verbatim drops its spoken start message and keeps the start sound, as NVDA speaks none, so the first speech is always the foreground's announcement (whether the message was cut off depended on how fast an outpost started). Enforcing response-time budgets is deferred; the harness keeps recording the latency distribution per kind of step.

Decisions of 2026-10-08 (afternoon), Dickson:

- Caret keys become event-driven: the outpost never blocks waiting for a caret move. A key that does not move the caret is silent (a deliberate difference from NVDA, which reads the unit after its 100 ms wait), so no deadline and no "already at destination" logic are needed. Every blocking wait in the outposts is audited and removed.
- Performance tests check exact counts (calls, instructions) everywhere. Response-time budgets are enforced by default in local runs, and an explicit setting turns enforcement off on GitHub's runners, which record the numbers instead. No self-hosted runner.
- Every mockapp test that does not need the foreground runs on its own isolated desktop; the rest run only when nothing else uses the desktop.
- Terminal scenarios add tab switching, closing a tab, two Windows Terminal windows and two console host windows.
- Parity changes are checked against a live NVDA capture of the same control before merging; every claim about NVDA cites NVDA's source; changes near a recorded deliberate difference are reviewed for interactions with it. A behaviour-based MSAA audit follows: live NVDA captures against a matrix of standard controls, compared exactly with Verbatim.
- The latency outlier in "mixer and device" (about one utterance in eight spends 40 to 80 ms there) is investigated: the mixer stops the audio device when speech is cut off, and restarting it is the likely cost.

## The review cursor on objects that no longer exist (2026-10-08)

A read-only study found that Verbatim's review commands can speak stale content. Most current-unit review commands (current line, word and character, line ends, moves within the line, Top and Bottom) answer from Core's cached line without asking the application; flat review (objects without a text pattern), report object, say-all from review and copy answer from the node's snapshot; and a Gone answer from the outpost is dropped silently, leaving the navigator on the dead node. Nothing reports an element's removal. In the common case a focus change moves the navigator off a closed tab or dialog, but when the navigator was object-navigated away from the focus, or a key comes between the close and the focus event, the dead object's old text is read. NVDA reads live on every review command, so a dead object gives silence or "blank", never stale text.

Fixes, each with exact tests (reducer, mockapp and end to end in both terminals and Windows 11 Notepad):
- Review commands read live from the application, as NVDA's do: the current unit re-reads at the review position, and flat review and report object re-read the node first (a new fetch of the node itself). This also fixes a stored line going stale in a live terminal whose line was rewritten.
- A Gone answer for the navigator's node re-seeds the navigator from the focus and announces it; if the dead node is the focus itself, the navigator is cleared and "No navigator object" is said (NVDA's wording).
- The outpost reports held MSAA nodes it forgets when their window is destroyed, as direct evidence of a death.
- Verbatim has no "review follows focus" setting where NVDA has one (on by default); recorded in docs/parity.md for the owner (in "Navigator follows focus; review follows navigator", since 2026-10-09; the setting is part of the settings package).

## Audio start-up latency and scheduling (2026-10-08)

A read-only study refuted the guess that restarting the audio device costs 40 to 80 ms: the slow "mixer and device" figures all come from end-to-end runs with no real device (`VERBATIM_TEST_AUDIO=null`), where stopping and starting cost nothing. They have two causes. Speech that starts while an earcon is playing waits behind the roughly 40 ms of earcon already queued for the device (every start-up's first announcement, and any speech starting during an earcon); and back-to-back utterances are a measurement artefact, the ledger counting the tail of earlier speech as "mixer and device".

Proposed, for Dickson:
- Speech starting while other audio is queued is mixed in at once (the queue discarded and re-mixed, as a cancel already does), saving about 35 to 40 ms.
- The audio stream keeps running after speech, writing silence, for an awake period (NVDA keeps the device awake 30 seconds by default, with a setting, because Bluetooth, USB, HDMI and virtual-machine endpoints otherwise clip or delay the next speech); cancel keeps Stop and Reset, as NVDA does, since shared mode has no other way to discard queued audio.
- Every Verbatim process opts out of Windows' power throttling (EcoQoS), so a background screen reader is not moved to efficiency cores or slowed; the keyboard hook thread runs at high priority; the speech queue and synthesis threads above normal. NVDA sets none of this.
- The latency ledger counts waiting behind earlier speech until that speech has ended.
- The end-to-end suite never exercises the real audio device; an audible run, or a scenario on the real device, is needed to measure endpoint wake-up, and IAudioClock or the stream latency would expose the endpoint's own delay.

Audio decisions (Dickson, 2026-10-08): the audio stream stays awake after speech for a configurable period, 30 seconds by default as in NVDA; the thread and process priority recommendations are adopted; the latency ledger stops counting the tail of earlier speech as "mixer and device". Earcons and speech mix together on the same stream with no delay: speech that starts while an earcon plays is mixed into audio not yet played, re-mixing what was already handed to the device; an indication's sound and its speech start together, and cancelling an utterance cancels both. Verbatim gets NVDA's settings (an inventory against NVDA's configuration is being made).

The review cursor in a terminal (Dickson, 2026-10-08): typing must not move the review cursor, and when new output scrolls the terminal, the review cursor stays on the same logical line, not the same row. Today the review cursor follows the caret when a caret event reaches its node (so typing moves it), and its position is a text range, which a full scrollback keeps on its row while the text moves beneath it. To be designed with the screen diff, which knows how far the text scrolled, and tested end to end in both terminals: review a line five from the bottom, type a command, run it so output scrolls, and read the same line.

Which stands (note of 2026-10-09): the two paragraphs after this one,
dated later the same day, replace it. "Further" makes the review cursor
follow the caret by default, as NVDA's does, so typing does move it unless
following is turned off; "Revised" leaves the review position to the
provider instead of following the logical line by scroll distance.

Further (Dickson, 2026-10-08): the review cursor follows the caret by default, as NVDA's does, with NVDA's setting to turn it off. Sounds without speech are normal (a theme can set any indication to sound only), so a sound-only announcement is a first-class announcement: queued, timed, measured and cancelled exactly like a spoken one; the start-up sound becomes one rather than playing outside the announcement system.

## NVDA's settings in Verbatim (inventory 2026-10-08)

Of NVDA's 242 user-visible settings, Verbatim has 18, has an equivalent with a difference for 25 (mostly theme indications standing in for NVDA's checkboxes, per D12), lacks 14 whose behaviour already exists, and lacks 177 whose feature is not built yet (M4 6, M6 33, M8 30, M12 9, M15 29, the Office porting track 5, unscheduled 65); 8 do not apply. The settings package builds now: the 14 missing settings for behaviour Verbatim already has (capital pitch change, OneCore's pause after punctuation, the audio output device, sounds' volume following the voice, the audio device awake time, handling keys from other applications, the multiple key press timeout, review cursor following focus, reporting tooltips, reporting notifications, the word segmentation standard, cancelling speech for expired focus events, trimming leading silence), dialog controls for the nine settings that exist only in settings.toml, and the three small M4 features (say "cap" before capitals, delayed character descriptions, the spelling-error sound while typing). NVDA's caret movement timeout has no counterpart once caret keys become event-driven. Each setting for a feature not yet built lands with its milestone.

Revised (Dickson, 2026-10-08): the review cursor's position is left to the provider, with no bookkeeping of Verbatim's own. With the history not yet full, a UIA range stays on its text as new lines are written below it, which is the case that matters; with it full, staying on the same row is acceptable; when the review line itself is rewritten, the position stays where the provider keeps it, as in NVDA. This replaces following the logical line by scroll distance. An end-to-end test in both terminals pins the case that matters: review a line still on screen, write new output below it without filling the history, and read the same line. Status cues (the start-up and exit sounds, progress beeps, error sounds) always play through and are not cut off by keys; content announcements are. Spoken progress comes every 10 percent, as NVDA's does. The error sound plays in every build for now.

Status cues settled (Dickson, 2026-10-08): every indication in the theme tree's Events group (start and exit, error, application not responding, browse mode and focus mode, suggestions opened and closed, progress) plays through key presses; every indication in the other groups is content and is cut off. It is built-in behaviour defined by the group, not shown in the GUI, and the group keeps the name "Events".

Follow-ups decided (2026-10-08): the outpost's messages to Core are merged by object and kind while they wait, always, as the intake and the supervisor already merge incoming events and as NVDA's limiters do; terminal output is combined under the flood policy and answers to Core's requests are kept in order; sending never blocks, and nothing is sent while a lock is held, so the watchdog can never be stalled behind a full queue. Switching synthesizer in the settings dialog becomes asynchronous: the dialog never waits on the GUI thread, says the switch is under way, and reports the result; the time limit sits where the synthesizer host is waited for, and a switch that does not answer in time fails and keeps the previous synthesizer. An open caret watch does not hold the idle barrier.

Outcome of the follow-ups (2026-10-08): the outpost's writer queue never waits and merges what waits by node and event kind, a terminal's output combined under the flood policy's limits, while answers, notifications, and faults keep their order; the queue numbers the messages that carry node ids as Core counts them, so merging cannot release a node Core has seen. Publishing claims the result under the watch lock and queues it after releasing the lock, and the watchdog queues its `Abandoned` answer outside the lock too. The supervisor's side already never waited; the audit is in `docs/crates/verbatim-outpost.md`, "Messages to Core". The Select Synthesizer dialog starts the switch and returns at once, says "Switching synthesizer" (NVDA has no wording for this, since its switch blocks), closes once the new synthesizer is active, and on failure shows NVDA's "Could not load the ... synthesizer." box titled "Synthesizer Error" and stays open, as NVDA's does; the only time limit is the synthesizer host's own, `HANG_TIMEOUT`, after which the previous synthesizer stays. The open caret watch already stayed out of the idle barrier (a passive request, since commit 0ba555c). Related fix: the supervisor holds each application's process open while it has an outpost or a crash history for it, so a pid can no longer name another process, and it never starts or replaces an outpost for an application that has exited; such an outpost ends as "target exited", and a request for one is answered `NotWatched`, which the app uses to stop waiting.

Memory growth tracking (Dickson, 2026-10-08): a scenario of about 25 seconds runs in every suite: ten iterations of opening a Windows Forms text box window, typing and editing in it, reviewing a line and a word, closing it, and Tabbing through five controls of Verbatim's settings dialog. After two warm-up iterations, the counts Verbatim reports through the control plane (held nodes, text anchors, outposts, queued speech, cache sizes; a new status addition) must return to exactly the same values after every iteration, and each process's private memory may grow from iteration 3 to 10 by no more than a small bound, enforced locally and recorded on GitHub, whose results are reported to Dickson after the push.

Keys and notes (2026-10-08): only keys passed through to the application make a caret note. Keys Verbatim consumes as its own commands (review cursor and other Verbatim gestures) never reach the application and make no note; a caret move a Verbatim command causes itself (moving the caret to the review cursor, say-all) is spoken by that command's own rules, not by the landing rule.

## Windows Terminal crash of 2026-10-08

The owner's Windows Terminal crashed (access violation in UIAutomationCore.dll's error-reporting code, `wil::details::ThreadFailureCallbackHolder::GetThreadContext`) while the suite's terminal windows shared its process. Fixed so far: the suite now runs a pinned portable Windows Terminal (1.24.12741.0) as a process of its own, locally and on GitHub. Not reproduced in about 640 kills of Verbatim and outposts mid-read in the isolated copy. Decisions (Dickson): the suite keeps running on the owner's desktop; Verbatim's outposts and listener shut down cleanly (a shutdown message, event handlers removed and calls finished before exiting, killing only after a time limit), since killing a UIA client mid-call is the leading suspect and also happens to users whenever Verbatim exits; programs the harness starts that need no visible console start without one, so Windows never hands their console to the owner's terminal.

Done (2026-10-08): the supervisor sends every outpost and the listener a shutdown message whenever it ends one (Verbatim exiting, a child replaced or restarted, an outpost retired or ended because its application exited, which the outpost now reports); the child takes no new work, lets the call or remote operation in progress finish, removes its UIA handlers and `WinEvent` hooks, releases what it holds, leaves COM, and ends itself with exit code 0. It is killed through its job only after 21 seconds, longer than UIA's transaction timeout (20 seconds), and every kill is logged with its reason (`docs/crates/verbatim-outpost.md`, "Shutting down"). Every program the agent and the harness start without a console title has no console window (`CREATE_NO_WINDOW`), the console host scenarios start `conhost.exe` explicitly, and every scenario fails if a Windows Terminal window the agent did not launch appears, or if an outpost was killed or left running when Verbatim exited.

## Live caret event checks (2026-10-08)

Checked live on the development machine's interactive desktop (unlocked, on the console), with a scratch probe: an out-of-context `SetWinEventHook` over all events, filtered to the target window, and UIA handlers on the target window's subtree for text selection changed, text changed, text-edit-changed of every kind, changes, notifications, active text position, value, name and keyboard-focus property changes, and focus. Keys were injected with `SendInput`, 0.9 to 1.2 seconds apart. Then a debug Verbatim (00b33fe, eSpeak NG into the silent device, outposts logging at debug) ran against the same targets, read through `verbatim-inspect watch`, the outpost logs and the flight recorder. Keys: Left, Right, Up, Down, Control+Left and Control+Right, Home, End, Backspace, Delete, Control+Backspace, a typed character, and keys that cannot move the caret (Left at the start, Right at the end, Down with no later history).

The Windows Forms text box (the harness's classic edit control, `WindowsForms10.EDIT`):
- Every caret move raises `EVENT_OBJECT_LOCATIONCHANGE` for `OBJID_CARET` within 1 to 5 ms of the key, inside a caret hide and show pair, followed by `EVENT_OBJECT_TEXTSELECTIONCHANGED` on the client. Up and Down add the system's capture start and end events around them. So the classic edit does raise caret events on the normal desktop; the earlier silence did not come from the control.
- A text change raises `EVENT_OBJECT_VALUECHANGE` on the horizontal and vertical scroll bars and then on the client, with the caret's location change between them, all within 1 to 8 ms. Delete moves no caret, so its evidence is the value change and a caret hide and show, with no location change.
- UIA (the Win32 edit proxy) raises only a value property change, about 15 ms after the key, with an empty value: no text selection changed, text changed, text-edit-changed or changes event. The focused element has no text pattern.
- A key that cannot move the caret raises nothing at all.
- Control+Backspace deletes nothing: the control inserts a DEL control character (U+007F) and the caret moves one to the right.

Windows 11 Notepad (`RichEditD2DPT`):
- Each caret move raises, within 2 to 5 ms, the caret's destroy, create, location change and show (the caret is recreated on every move), `EVENT_OBJECT_TEXTSELECTIONCHANGED`, and UIA text selection changed.
- A text change adds `EVENT_OBJECT_VALUECHANGE`, UIA text changed, and a UIA value property change carrying the whole document, within 5 to 10 ms. Delete raises UIA text selection changed too, although the caret does not move.
- No text-edit-changed, changes, notification or active-text-position event is raised for any key.
- A key that cannot move the caret raises no location change and no UIA event.

The console host (`conhost.exe` running Windows PowerShell without PSReadLine):
- Typed text raises `EVENT_CONSOLE_UPDATE_SIMPLE` and `EVENT_OBJECT_VALUECHANGE` per character, and UIA text changed, within 1 to 5 ms; deletions and history recall raise `EVENT_CONSOLE_UPDATE_REGION` the same way.
- The caret's events (UIA text selection changed, `EVENT_OBJECT_LOCATIONCHANGE` for `OBJID_CARET`, `EVENT_CONSOLE_CARET`, `EVENT_OBJECT_TEXTSELECTIONCHANGED`) come together but late, anywhere from 10 to about 520 ms after the key (measured 10, 52, 110, 154, 158, 214, 251, 258, 303, 463, 470 and 514 ms), consistent with the console host raising them from its cursor blink timer. Delete raised a caret location change and a console caret event 514 ms after the key though the caret had not moved.
- UIA's bridge also raises WinEvents numbered as IAccessible2's (caret moved, text inserted, removed and updated, text selection changed) alongside.
- No text-edit-changed or changes event; a key that cannot move the caret raises nothing.

Windows Terminal (the harness's portable 1.24.12741.0):
- No caret WinEvents and no console events at all. The terminal control raises UIA text selection changed 30 to 55 ms after a caret key, then four or five more within about 20 ms; a text change raises UIA text changed (four in a burst), a notification (`TerminalTextOutput`, carrying the redrawn text, such as "cho hello worl" after Delete), and bridged value and text selection WinEvents on the input site window.
- Delete raises text changed and a notification but no text selection changed.
- No text-edit-changed or changes event; a key that cannot move the caret raises nothing.

Text-edit-changed and changes events are therefore unusable in all four targets, since none raises them for any key: the rule of "What a key did to the text" applies everywhere, and neither terminal gets a separate path.

Focus on launch or restore (no Alt+Tab): the text box's launch raises the foreground event, then `EVENT_OBJECT_FOCUS` on the edit, then the caret's create, show and location change; Notepad's, the focus and the caret's create and location change; the console host's, UIA focus on its text area, with the caret's events once it is in the foreground; Windows Terminal's, UIA focus only. A Notepad started from a background process did not take the foreground and was brought forward with `SetForegroundWindow`. Verbatim's focus read found the caret's line in every target, after a launch and after minimizing and restoring, with the one Notepad exception below.

What Verbatim said:
- The text box: every caret key spoke as NVDA would (the character, word or line, "carriage return" at End, the deleted character for Backspace, the new character at the caret for Delete). Control+Backspace said "hello", a word it had not deleted, since the control inserted a DEL character instead.
- Notepad: correct in one run. In the other, the focus read at launch took 1.0 seconds, the caret read after it answered no text, and Verbatim spoke the whole document as the focus's value; every caret key in that document was then silent, each answered at once with no text, because the outpost keeps a failed text pattern fetch as "no text" for the node's lifetime (`uia_source` in `text_reads.rs`). After minimizing and restoring, the new node read correctly. In the run that worked, the focus was spoken 3.1 seconds after its event.
- Both terminals: every caret key spoke correctly, answered at its first check (the UIA caret is already current although the console host's caret events come late). Delete, Control+Backspace and Up also had the redrawn line spoken as output after the key's own speech: "c" then "cho hello worl", "cho" then "hello worl", "ready> echo one" then "echo one".
- Keys that cannot move the caret were silent everywhere. Their watch stayed open until the next caret key replaced it or, with no further key, ended silently at the 10 second bound.

Watches answered by a later key's evidence:
- In both terminals, Down with no later history left its watch open, and Escape, which is not a caret key, cleared the line 1.2 seconds later; Down's watch took that change as its evidence and spoke "ready>", 1.2 seconds late.
- In the text box, Left at the start, then typing "z", then Backspace: the typed character answered Left's watch, and the Backspace was silent. The flight recorder shows the Backspace's watch answered at its first check, read in the same millisecond as the key, with the caret's offset from before the key against the text from after it, and Core spoke nothing. Without the Left first, the same Backspace spoke "z". Reproduced twice.

Both come from a watch for a key that did nothing staying open for whatever changes next. No caret key in any target was silent for want of an event, and no watch for a key that did something ended at the bound.

## Notepad at launch and the outposts' loose ends (2026-10-08)

Outcome of the follow-up to "Live caret event checks" above, and of three loose ends in the outposts and the settings dialog.

A failed text read at Notepad's launch. The outpost logs of that run were lost with the build directory, and later launches with a debug Verbatim (eleven by hand, with and without a late activation, and the Notepad scenarios) all read correctly, so the cause is reasoned from the code, the run's speech, and earlier findings, not seen:
- The focus read took 1.0 seconds, exactly `FOCUS_READ_WAIT`, the connection timeout the outpost sets for its read of the focused element. With a short timeout, UIA answers the focused element read of a Windows 11 Notepad that is still starting with its own stand-in for the window, a nameless edit read through the window proxy, instead of the "Text editor" document (`docs/parity.md`, the 2026-10-02 entry on reading Notepad's focused element). Notepad's text area is a window of its own, so the stand-in has the document's runtime id (`[42, <window>]`): the outpost took it as the focus's element. The stand-in has no text pattern, so the caret report after the focus answered `NoText`, and its value, read live, is all of the document's text, which Core spoke as the focus's value. That answer was kept for the node's life, so every caret key in the window answered at once and said nothing.
- The latency line of that announcement, which has no listener stage, is not evidence of the path the focus took: the focus and its caret report share a trace, and the ledger keeps the stages of the last message received for a trace, the caret report's. Every Notepad launch shows the same line (found in the end-to-end runs of 2026-10-08, whose outposts logged a direct read). That is left as it is and reported.
- A UIA provider that fails the pattern request reaches the client exactly as one with no pattern (mockapp's new `refuse-text` shows it: the fetch answers no pattern, not an error), so a provider that is not ready answers "no text pattern" too.
- Speaking the value of a focus that has no text pattern is what NVDA does (`docs/parity.md`, "A text pattern missing at the focus"); what was wrong is keeping the answer, and keeping the stand-in as the node's element.
- Fixed: an answer of no text pattern is kept only until the node is next reported as the focus, or raises a caret or text event, which only an element with text does, and the element that raised that event then becomes the node's, so a stand-in kept for it is replaced by the document's own element; a caret key's watch on such a node stays open for that event instead of being answered at once, so the first key after the provider is ready is spoken; a fetch that fails with an error is not kept at all. A focus whose text cannot be read now sends nothing rather than `NoText`, so Core never speaks a document's value for a read that failed, and a focus reported from its event alone has its caret and text events followed, and its caret reported, once its element is found. Test: mockapp's `a_text_pattern_missing_at_the_focus_is_read_at_the_next_caret_event`.

The focus announcement 1.0 and 3.1 seconds after its event. In the run above, the second went to the focused element read waiting out its one-second timeout on Notepad, which was still starting. In the other run the reads answered, the slowest being Notepad's own: the focused element read may take up to a second and the ancestors up to two (`ENRICHMENT_BUDGET`), so 3.1 seconds is Notepad answering slowly while it starts, not time spent in Verbatim; the outpost logs that would split it were lost, and none of the later launches was slow (the focus read took 17 milliseconds in the Notepad scenarios). One thing is changed where Verbatim was stricter than NVDA: when the focused element read names another element of the application, NVDA still accepts a UIA focus event whose own element has the keyboard focus, read live, and the outpost held such a focus back and read again up to three times. For an element that is a window of its own, as Notepad's text area is, the outpost now reads that window's element and reports the focus at once when it is the event's element and has the keyboard focus. Test: mockapp's `a_windowed_focus_is_reported_though_the_focused_element_read_answers_a_stand_in`. A windowless element is still held back and looked for again up to three times; replacing those reads with waiting for the application's next focus event is left open.

`outpost_crash_recovery` failing in setup. The outpost's start was slowed deliberately, every new `verbatim-outpost.exe` suspended for three seconds as it started (a scratch script, `NtSuspendProcess`), and so was mockapp's, by two seconds: four runs passed, the facts held while the outpost started all released in order. Input injected from another process just before the scenario reproduced a setup failure at once: "the window ... did not take the foreground within 30s (Windows did not let the agent allow its launch to take the foreground)". Windows raises the foreground event for the new window, so its outpost starts and says it is ready, but keeps the previous window in front, and Verbatim, like NVDA, says nothing for a window that never became the foreground. That is the harness's known limit (`docs/tooling.md`, "Windows' foreground lock keeps launched applications behind"): the agent may allow a launch the foreground only while it injected the last input, and this scenario launches its window before it sends any key. The failed run's 60-second build left the most time for other input on the shared desktop. No race in Verbatim was found.

An application whose outposts crashed repeatedly. After the third crash within a minute the supervisor stops replacing its outpost until the next foreground change to it; the app, told of the crash, had marked the application's focus as wanted, and nothing ever answered, so the idle barrier held for ever. The supervisor now sends `OutpostMessage::NotWatched` after that crash and for every request to start an outpost meanwhile, and the app drops the wanted focus. Test: `an_application_whose_outposts_crashed_repeatedly_is_reported_not_watched` in `crates/verbatim-outpost/tests/target_gone.rs`.

The settings dialog during a synthesizer switch. NVDA's switch blocks its GUI thread, so OK and Cancel cannot be pressed during one (`docs/parity.md`, "Settings dialog during a synthesizer switch", with NVDA's source lines). Verbatim's does not block, and OK saved whichever synthesizer was active at that moment. Now a commit during a switch saves the active synthesizer at once and the new one once it has started, and a revert waits for the switch and restores the previous synthesizer's values only if it failed, so they never reach the new one. Tests: `a_commit_during_a_switch_saves_the_synth_the_switch_started` and `a_revert_during_a_switch_that_fails_restores_the_committed_values` in `verbatim-speech`.

The end-to-end runs of these fixes. All seven related scenarios passed, but the Notepad ones only after two harness failures, both fixed:
- `notepad_editing` once failed in its launch with its own document's Notepad window left restored after the taskbar's Minimize All. No artifacts were kept from a launch failure; the likeliest cause, not seen, is that Minimize All skips a window the taskbar has not taken in yet, and the document is opened moments before. The agent now sends Minimize All and waits for the taskbar to handle it, logs at info each window still restored then (class, image, title, and its process's age), and minimizes it directly.
- The harness ended an earlier run's leftovers before it closed Notepad's harness tabs, so a Notepad ended by its handle kept its tab and opened it again in the next scenario without its deleted document, behind a "Cannot find the file" dialog that blocked closing it, and the next two Notepad scenarios failed at cleanup. Tabs are now closed as tabs first, a launch that fails after opening its document closes it and deletes it, and a tab that will not close fails the launch with the dialog named.
- The outpost now logs at info each caret or text event whose element replaces the element of a node that answered no text pattern. In the three Notepad scenarios run afterwards, neither this line nor the agent's minimize line appeared, so the stand-in element and Minimize All's missed window both remain unconfirmed.

## Ignoring the owner's Windows Terminal (2026-10-08)

During end-to-end runs on the owner's desktop, a UIA focus fact from the owner's own Windows Terminal (the process hosting their Claude Code session) was routed, the supervisor started an outpost for it, and the outpost handled the fact. The owner decided to exclude that Windows Terminal from test runs. The agent now launches Verbatim naming every Windows Terminal outside its own jobs, and the console hosts they run, in `VERBATIM_IGNORE_PIDS`; Verbatim and the agent hold those processes open, so a pid can never name another process meanwhile. For them, the listener drops every fact before routing it, the supervisor starts no outpost, the shell never asks one for its focus, and a focus in one of their windows never reaches the reducer. Real users set nothing and ignore nothing. Details in `docs/tooling.md`.

What still touches an ignored process, and why it cannot be avoided without losing focus tracking:
- The listener's desktop-wide UIA registrations (the focus changed handler, and the event handler group for element selected, menu opened, and notifications) are global by nature: UIA offers no way to leave a process out of them. While any client is registered, UI Automation's provider-side code in Windows Terminal's own process handles each event Windows Terminal raises, and for a client that registered with a cache request, which the listener's registrations do, it reads the requested properties there and ships them with the event: the name, control type, class, automation id, framework, process id, keyboard focus, and the few other base properties. The listener's callback itself makes no call; the reads run inside Windows Terminal, done by UI Automation on the client's behalf. Registering and unregistering also has UI Automation tell each process with a UIA provider that a client is listening. Dropping the cache request would not remove this work, since the listener would then have to fetch the properties itself with a real cross-process call. Only not registering desktop-wide would, and that would lose focus tracking for every UIA application, so nothing more is done. Any other UIA client registered desktop-wide, such as NVDA or Narrator, causes the same work.
- The listener's global `WinEvent` hooks are out of context: Windows posts the events to the listener's thread, and nothing runs in Windows Terminal's process for them. The `GetWindowThreadProcessId` read that names an event's process is a local call.
- A UIA tree walk that steps from a top-level window to its siblings, which object navigation does from a top-level window and the tree dump does not, can reach an ignored process's window through UIA's desktop root. No scenario in the suite navigates past a top-level window today; such a step would be made by the outpost of the application being navigated, and is not guarded.

## Test isolation and the foreground lock (2026-10-08)

Outcome of the work package on the shared desktop, the caret watch test's failure, the held UIA focus, the harness's foreground-lock failure, and the latency ledger.

Every mockapp test runs on a desktop of its own. All eighteen test binaries go through `tests/common/harness.rs`'s isolated runner, `msaa_tree` and `slow_application` included, which had used libtest; none needed the foreground or the keyboard focus, so none is kept to an explicit command or guarded against a running end-to-end suite. verbatim-outpost's one unit test that made windows makes message-only windows now. Checked with a scratch probe, an out-of-context `WinEvent` hook logging every top-level window created or shown by a process whose executable is under the worktree's `target` or is `mockapp.exe`, on the desktop each run was started from: `cargo test -p mockapp` (128 tests: 110 in its eighteen test binaries and 18 unit tests) and `cargo xtask ci`, both started from a scratch desktop, logged no mockapp window; the only windows logged were that unit test's, now gone (`cargo test -p verbatim-outpost --lib` then logged none).

`caret_watch`'s `a_key_that_moves_nothing_is_answered_with_nothing` failed once under load with "the outpost said more: ... CaretMoved ... offset: 1" at its final settle. mockapp acknowledges `caret-event` once the call that raised the event has returned; the outpost's handlers receive it later, on UI Automation's threads, and once more as the `WinEvent` UIA raises alongside it, which reached the outpost first in every traced run, 0.2 to 0.4 ms ahead. `settle` covers only what has reached the outpost, so the first step's event, still on its way, could answer the second key's watch, and the second step's event, arriving after that answer, was reported as a caret move no assertion expected. The outpost now tells an observer each event its own handlers take in (`Outpost::observe_heard`), and every caret step waits to hear both routes of its event before it settles. No other test that settles had the gap: the rest raise MSAA events, whose later events from the same hook are their evidence, or wait for the reply the event causes.

A held UIA focus. A focus fact whose element no longer had the keyboard focus was held back and the focused element read again up to three times. That was a retry, and mostly a dead one: the follow-up read only while the held fact named the focus last reported, so a fact for any other element was dropped at its first follow-up without a read. The fact is now dropped as NVDA drops it: `shouldAllowUIAFocusEvent` reads the sender's `currentHasKeyboardFocus` (`nvda/source/NVDAObjects/UIA/__init__.py`, lines 1632 to 1637), and the focus handler returns without queuing the focus when it is false (`nvda/source/UIAHandler/__init__.py`, lines 948 to 953); the application's next focus event reports where the focus is. Test: mockapp's `a_windowless_focus_that_lost_the_keyboard_focus_waits_for_the_next_focus_event` (one focused-element read where there were four). The one case where NVDA and Verbatim now differ is a windowless element that has the focus while the focused-element read answers a stand-in: NVDA reads the event's own element and accepts it, and Verbatim, which cannot reach that element, waits for the next focus event. The follow-up for a focus reported from its event alone, whose element was not found in time, still reads up to three times; it is the same kind of retry and is left open. Outcome (2026-10-09): that follow-up now reads once. When it finds nothing, or another element, nothing reads again, and the focus's element comes from its own next event, keyed by its runtime id. A focus event already supplied it: its handling reads the focused element afresh, keeps the element under the focus's node, and moves the subscriptions to it, and Core takes the repeated focus on the same node silently. A selection event did not: the listener's selection fact carries no element. It now reads the focused element once, as a focus event does, when its runtime id is the focus's and the focus has no element, and keeps it when it is the focus's. The Notepad package's replacement of a stand-in on a caret or text event does not reach this case: it replaces the element of a node that answered no text pattern, and a focus with no element has its caret, text, and property subscriptions listening nowhere, so none of those events comes until its element is found. Tests: mockapp's `a_focus_whose_element_was_not_found_is_followed_from_its_next_focus_event` (two focused-element reads for the focus where there were four, then the next focus event's read, then the focus's rename heard and reported) and `a_selection_of_a_focus_whose_element_was_not_found_finds_its_element` (fails with the selection's read removed).

The harness's foreground-lock failure. Root cause: the agent lets a launch take the foreground with `AllowSetForegroundWindow`, which Windows grants only to a process that may set the foreground itself; at a scenario's start the agent's only ground is having injected the last input, since it is neither the foreground process nor started by it, there is a foreground window (the desktop's), and this machine's foreground lock never expires (`SPI_GETFOREGROUNDLOCKTIMEOUT` reads 2147483647 ms). Any input from elsewhere, a person's or another agent's `SendInput`, takes the ground away. Reproduced at once with a freshly started agent, and again after one zero-pixel mouse move injected from a scratch process: `outpost_crash_recovery` failed in setup with "Windows did not let the agent allow its launch to take the foreground". The windows a scenario opens through `Scenario::launch_titled` (mockapp and the Windows Forms text box) now open minimized and inactive and are then restored and set as the foreground, as Notepad's documents already were, injecting nothing. After the same foreign input, `outpost_crash_recovery` then passed three times (the control run on the old path failed), and `text_box_editing`, `second_application_and_verbatim_menu`, `spelling_errors`, `object_navigation_over_uia`, and `notepad_editing` passed. msinfo32 could not be brought forward that way after foreign input (why is not known), so `system_information_tree` keeps the old launch and passes whenever the agent's input came last; so do the console host and Windows Terminal scenarios, which open their windows in front. Making those reliable too would need one of: finding why msinfo32's restore is refused; setting the foreground lock timeout to Windows' default or to zero on the test machines (a machine setting, for Dickson to decide); giving the agent UI Access, which may set the foreground freely but needs a signed executable in a protected folder; or keeping the desktop exclusive during a run, which the desktop turns between agents already do.

The latency ledger kept the stages of the last message received on a trace, mixed with the first message's receipt and reduction, so a text focus's line showed its caret report's outpost stages. It now keeps one message's own stages: the message whose reduction first asked for speech on the trace, fixed on Core's thread as the speech is handed to the pipeline, since the pipeline queues it on its own thread, by which time the caret report may have arrived. Test: `a_line_keeps_the_stages_of_the_message_its_speech_came_from` in `verbatim-app`'s `latency.rs`.

Outcome for the terminals (2026-10-08): the console host and Windows Terminal scenarios open their windows minimized and inactive and bring them forward as `launch_titled` does. Both honor the launch's `SW_SHOWMINNOACTIVE`: the console host for the console window it creates, and Windows Terminal for its first window, which the harness found minimized when it appeared (`minimized` in the agent's window report), so no Windows Terminal option was needed. After one zero-pixel mouse move injected from a scratch process, `conhost_short_output` and `windows_terminal_short_output` failed in setup on the old path ("Windows did not let the agent allow its launch to take the foreground") and passed on the new one.

Decision (Dickson, 2026-10-08): the machine's foreground lock timeout stays as it is. The remaining launches (System Information, the console host and Windows Terminal) are made to work the new way too, without the agent's foreground right, starting with finding why msinfo32's restore is refused.

Outcome for System Information (2026-10-09): msinfo32's restore was never refused; it was never minimized. msinfo32 ignores the minimized show state it is launched with and opens its window restored and inactive, and the agent's `SetForeground` restored only a minimized window, so it went straight to `SetForegroundWindow`, which a restored background window takes only while the agent has the foreground right (Dickson, live). The agent's `SetForeground` now minimizes a window that is restored and not already in front, waits on window events until it is minimized, and then restores it and sets it as the foreground, logging each such window at info; `system_information_tree` launches msinfo32 through `Scenario::launch_titled`, after `Scenario::require_absent` checks no System Information window is open, and `Scenario::launch_target` and `require_launched_in_front` are gone. The console host and Windows Terminal launches (`launch_console`, `launch_owning_window`) already went through the same `bring_forward` and so through the same `SetForeground`. Evidence, each after one zero-pixel mouse move injected from a scratch process: with msinfo32's window restored and inactive behind a scratch window, the agent's `SetForeground` answered `false` on the old code in three of three tries and `true` on the new in three of three, the new agent logging the minimize each time. On the old code the window took the foreground a moment after the `false` answer, so on this machine the old failure was most likely `set_foreground` reading `GetForegroundWindow` while the change was still under way rather than a refusal; a window restored from minimized is in front before the read. `system_information_tree` passed five times of five after such a move. Every `conhost_` and `windows_terminal_` scenario was run once after one: all 36 came forward and passed setup, and 34 passed; `conhost_scrollback_overflow` and `conhost_scrollback_overflow_during_group` failed in their flood's speech ("skipped more than 8972 lines", where 8971 and the lines after were expected) and both passed when run again, so they are the console host's intermittent flood, not the launch. In those five runs msinfo32 took the foreground by itself as it opened, the desktop being in front (a scratch probe saw the same with no Verbatim running), so they did not need the new path; the direct `SetForeground` comparison is what shows it works. Two launches still opened their windows in front and needed the agent's right: `Scenario::open_folder` (File Explorer) and `Scenario::open_settings_page` (Settings). The Settings page has since been changed to open minimized and inactive and be brought forward ("Decisions to confirm with Dickson", "Windows still opened in front"), so only File Explorer's launch still needs the right.

## Coherence review decisions (Dickson, 2026-10-09)

A read-only review of phase 6 against the dated decisions, docs/parity.md and docs/testing.md found drift; Dickson decided, each as recommended:

1. Blank lines at the start of a burst: the exception (`burst_written`, e07438d) is removed and its cause fixed in the outpost, where rows not yet written to are dropped from a screen read and come back as inserted blank lines when a footer is drawn below them. The 2026-10-07 rule stands: every line counts, blank ones included.
2. Delete speaks what it leaves at the caret, as NVDA does and as the code and docs/parity.md already have it. The 2026-10-08 "what a key did" decision listed Delete among the keys that speak the text removed by mistake; that list no longer includes Delete.
3. "Lines spoken in full" may exceed the terminal's history, as decided on 2026-10-07; the cap of 100 is raised above the history size, the 10 MB bound protecting memory.
4. Control+Tab announces the tab ("list", the tab's name, "1 of 2") as NVDA does; the recorded difference is withdrawn.
5. Kept as deliberate differences: Escape forgetting typing not yet echoed, and closing a full-screen program speaking only what followed it. Fixed: a new tab's extra "blank", which NVDA does not say.
6. Response-time budgets: the afternoon decision of 2026-10-08 stands. They are enforced by default in local runs, and recorded but not enforced on GitHub's runners.
7. The waits not driven by events (the outpost's 10 ms foreground check for up to 250 ms, the watchdog's 500 ms foreground check, the one follow-up focus read): measure whether the foreground event comes before or after the window is in front, then remove every wait the measurement shows is not needed.
8. The two-window scenarios switch windows with Alt+Tab, as a user does, so they test activating a window already open rather than restoring a minimized one.

Fixes needing no decision, also from the review: File Explorer is announced once its window is shown and titled, and launched without the foreground right; a second Verbatim waits for the first one's clean shutdown instead of ending it after 4 seconds; rows and lines are no longer mixed in the terminal screen read; tests for Up followed by typing, a line key during a flood, and UIA text fields read by parts; the end-to-end tests decided on but dropped (Control during a flood, leaving and returning during one, closing a tab, Shift pause and resume, a flood of identical lines, a raised flood setting, a full-screen redraw larger than the flood limit); and a documentation pass over docs/architecture.md, docs/roadmap.md and docs/testing.md, with the decisions not yet built added to the roadmap.

## Terminal line keys as NVDA has them (Dickson, 2026-10-09)

Live trials in programs already installed (git log's pager, less, vim, tig, Microsoft Edit, gh's prompts, PSReadLine's MenuComplete, nano) found the marker rule matching by accident (git log's Up spoke 15 lines where NVDA says ":" and the new line), and the per-key machinery behind it failing in real programs: line keys silent in tig, less and vim, because output was held waiting for a finished redraw that no read showed; redraws answered half done in the console host. Dickson decided that Up and Down in terminals work as NVDA's do, reversing the selection-list difference and the confirmed decisions on held output and the eight kept screens:

- The marker rule, the line-key watch's judgement of a finished redraw (`terminal::keys`), output held while a line key's watch is open, and the kept earlier screens are removed.
- When the caret moves to another line, that line is spoken, as in a text field. Screen changes are spoken at once as output, through the screen diff.
- The screen diff recognises a full-screen program scrolling (on the alternate screen as on the main one), so a pager's line move speaks the new line, not every row.
- docs/parity.md records the selection lists as matching NVDA.

Verbatim also sets Windows' screen reader flag while it runs and clears it when it exits, as NVDA does (`nvda.pyw` 285 and 286, 308 and 309), so programs that change their behaviour for a screen reader, such as PowerShell's PSReadLine, behave the same under both.

## Foreground events against the foreground window (2026-10-09)

Decision 7 of the coherence review asked for the order of foreground events against `GetForegroundWindow` to be measured, and every wait the measurement shows is not needed to be removed.

How it was measured. A scratch probe installed out-of-context `WinEvent` hooks, as the listener does, for the foreground, focus, create, show, hide, name, minimize and cloak events of top-level windows, and logged at each foreground event whether `GetForegroundWindow` already named the event's window, and if not, when it did. A second scratch program logged UIA focus events. The probe launched and switched windows itself: console windows started through `conhost.exe` (launched minimized, restored and set as the foreground as the agent does, and switched with Alt+Tab), File Explorer folder windows (launched minimized and normally), Notepad and msinfo32. Separately, the listener and the outposts logged each foreground event and how long the intake's hold waited, over one run each of ten scenarios (`explorer_folder_window`, `system_information_tree`, `conhost_two_windows`, `windows_terminal_two_windows`, `notepad_say_all`, `settings_system_page`, `menu_and_settings_dialog`, `second_application_and_verbatim_menu`, `windows_terminal_tabs`, `text_box_editing`).

What it showed:

- The probe: every system foreground event reached the hook once `GetForegroundWindow` already named its window, over 37 restores with `SetForegroundWindow`, 27 Alt+Tab switches and 28 launches. Each `SetForegroundWindow` after a restore had made the window the foreground window by the time it returned (37 of 37).
- The events whose window was not in front came in two kinds. The Alt+Tab switcher's staging windows (`ForegroundStaging`), already passed when their events arrived, never came in front. File Explorer raises a foreground event of its own as it creates a folder window, titled "File Explorer", hidden, 140 to 404 ms before the window is in front (14 of 14 launches); the system's own event follows once it is, with the folder's title already set.
- The scenarios: 25 foreground events, 23 with the window already in front at the listener. Of the outposts' 20 holds, 17 found the window in front at once; the others waited for File Explorer's early event (129 ms), and for Core's hidden frame (14 ms, and once the full 250 ms), whose foreground the outpost drops anyway.
- No focus follow-up read (`resolve_focus_later`) ran in those runs: no UIA focus was reported from its event alone.

What was removed:

- The intake's hold of a batch with a foreground change, 250 ms at most with a check every 10 ms. The worker drops a foreground fact whose window is not the foreground window when it handles it, as NVDA's `processForegroundWinEvent` drops it after at most two core cycles of deferral; File Explorer's early event is dropped and its window reported from the system's event.
- The watchdog's check every 500 ms whether the user had left a slow window for another thread of its application. It checks when the grace ends and again whenever a foreground fact reaches the outpost, which is the evidence of the move.
- The agent's wait after `SetForegroundWindow`: the answer is read when the call returns.
- The one follow-up focus read (`resolve_focus_later`), with the event-driven discovery of a focus's element that replaces it ("A focus followed from its own events", below). No run of the ten scenarios needed it: none reported a UIA focus from its event alone.

One older observation disagrees: on 2026-10-05 an observer saw Notepad's foreground event arrive about 130 ms before Notepad was in front, after Alt+Tab from the desktop. It did not reproduce in these measurements, and the related scenarios pass without the hold; a foreground event that does come early is now dropped, as NVDA drops one that stays early past its two deferrals, and Notepad's focus event still reports the window's focus.

## File Explorer opened without the foreground right (2026-10-09)

The coherence review's fix: File Explorer is announced once its window is shown and titled, and launched without the agent's foreground right.

What was measured, with the scratch probe of "Foreground events against the foreground window" and a UIA focus logger:

- File Explorer creates a folder window hidden, titled "File Explorer", raises a foreground event of its own while the window is not yet in front, sets the folder's title about 125 ms later, takes the foreground (the system's foreground event, the window still hidden), and shows the window about 240 ms after that. The desktop's shell, which holds the foreground as every scenario starts, gives the new window the foreground whether the agent allows it or not.
- Launched minimized (`SW_SHOWMINNOACTIVE`), the folder window still takes the foreground, hidden, and is then shown minimized while it is the foreground window. Restored, it raises no foreground event and no focus event (4 of 4 with the UIA focus logger), and its file list has no keyboard focus: NVDA, captured live, said nothing as it was restored and nothing for Down Arrow or Up Arrow. Giving the desktop the foreground before restoring it, so the window would be activated anew, needs the foreground right the launch is meant to do without.

What was done:

- An outpost reports a foreground window that is in front but not yet shown once it is shown: its show event (a new `WinEventKind::WindowShown`, the process-scoped `EVENT_OBJECT_SHOW` of a top-level window) brings the report, its name read then, so the name is the one the window is shown with. A focus inside the window that comes first has the window reported just before it, since Core does not announce a window reported after a focus inside it. A newer foreground change replaces one still waiting.
- `Scenario::open_folder` launches File Explorer with the agent's new `withhold_foreground`, which keeps the agent from allowing the launch the foreground, and brings the window forward as every launch is brought forward: it is already in front when the shell opened it so, and is restored from minimized and set as the foreground when it opened behind. It is not launched minimized, against the item's wording, for the reason above (to confirm with Dickson).
- In the runs since, the folder window's outpost, started by Explorer's early foreground event, handled one foreground fact for the window and dropped it as not in front, as NVDA drops such an event; why the system's own event did not follow as a second fact is not established (it may be folded into the first while the outpost starts, one fact per object and kind, or handled while `GetForegroundWindow` names Explorer's tab window, `ShellTabWindowClass`, a child of the folder window that the measurement saw in front for a moment as Explorer built the window). It is a follow-up. The window was announced as the first ancestor of the file list's focus, read once the window was shown, with the folder's title. The speech is the same either way: "<folder> - File Explorer", "Items View list", "Inner not selected 1 of 4", as NVDA said it when the folder was opened in front.

## The two-window scenarios switch with Alt+Tab (2026-10-09)

Decision 8 of the coherence review. `conhost_two_windows`, `windows_terminal_two_windows`, `conhost_leave_flood` and `windows_terminal_leave_flood` switch between their two windows with Alt+Tab (`Scenario::switch_with_alt_tab`), which presses it only once the agent's report shows that the window just behind the foreground window, in Z order, is the scenario's other window, restored, so the keys never reach a window the scenario did not open, the owner's Windows Terminal above all. `docs/testing.md` says so: Alt+Tab is still never used to bring a window forward, only as the input of a scenario about switching windows.

NVDA, captured live with two console windows and with two Windows Terminal windows, three switches each: "<title> window", then "terminal" with the caret's line in the console host, and "<title> terminal" in Windows Terminal, whose line NVDA left out in two of the three switches; nothing for the switcher itself. Verbatim says the same, the line always, as decided on 2026-10-09 for a terminal taking the focus.

The first runs found Verbatim saying "pane" before the window: Explorer's Alt+Tab staging window (`ForegroundStaging`), topmost and nameless, took the focus on the way. NVDA's File Explorer app module ignores focus on that window and two other shell transition windows (`appModules/explorer.py` lines 443 to 449); the outpost now drops a focus or foreground fact on them too (`docs/parity.md`, "The shell's staging windows").

## Control+Tab announces the tab (2026-10-09)

Decision 4 of the coherence review. NVDA, captured live with two tabs of the harness's Windows Terminal: Control+Shift+Tab queued "list", "<title> 1 of 2", "<title> terminal"; Control+Tab queued "list", "<title> 1 of 2" (the tab it left), "<title> two 2 of 2", "<title> two terminal", with no cancel between them, and NVDA culls a focus's speech once the focus has moved on, so less than that is heard.

What Verbatim did: the outpost dropped the tab's UIA focus, because by the time it handled the fact the terminal had the keyboard focus (`LiveFocus::Elsewhere`), though the fact's cached states, read as the listener received the event, said the tab had it, which is what NVDA judges by. Fixed in three parts:

- A UIA focus whose event said it had the keyboard focus, which another element of the application has taken since, is reported as the event said, its ancestors read without requiring the focus (`FocusQuery::require_focus`), its states read as the outpost handles it, as NVDA reads them as it speaks, and it is not followed by the focus-following subscriptions. One whose element is found nowhere is gone and is dropped: File Explorer's "Working on it..." placeholder takes the focus for a moment as a folder opens, and was otherwise said before the folder's first item. The console host's window's own focus stays dropped, as NVDA refuses it.
- Its element is found in its window by its runtime id, else by the name and position the event gave it: Windows Terminal raises its tabs' focus events from elements that are not in its tree (measured: the event named runtime id 17, the tree's tabs had 9 and 10; managed UIA showed the event's own element a `ListItem` whose parent is the tab list, whose parent is the terminal's container, where the tree's tab has the tab control above the list).
- Every UIA focus of a batch is handled in turn, oldest first, as NVDA's UIA handler queues every focus event it accepts; the outpost had handled only the newest, so whether a tab was announced depended on whether its event arrived in a batch of its own. MSAA's focus events are still tried newest first, as NVDA's MSAA handler does.

What Verbatim says now (`windows_terminal_tabs`): Control+Shift+Tab "tab control", "list", "<title> 1 of 2" (culled once the terminal has the focus), "<title> terminal", the line; Control+Tab "tab control", "list", "<title> not selected 1 of 2" and "<title> two 2 of 2" (both culled), "<title> two terminal", the line. Decisions to confirm with Dickson: the "tab control" NVDA does not say, which comes from the tab found standing in for the event's element; and the tab left saying "not selected", read once the selection has moved. Two earlier tries were set aside: the event's own states made the tab moved to say "not selected", its state as its event was raised, and its selection then came as "selected" whenever Windows Terminal raised it before the terminal's focus, which it does in some runs and not in others. `windows_terminal_close_tab` says the same as before.

## A focus followed from its own events (2026-10-09)

A UIA focus whose element the outpost could not read in time is reported from its event alone. Its element then came only from a follow-up read, queued at once (the last of the waits the coherence review's decision 7 named), or from the focus's next focus or selection event; until then its property, caret and text subscriptions listened nowhere, so a focus that raised neither event, such as a button whose name changed, was never followed. Now the subscriptions listen in the focus's top-level window while its element is unknown (`Worker::follow_focus_window`, the application's own top-level windows when no window is known), and the first event the focus raises itself brings its element, kept under the focus's node and followed from then on (`Worker::adopt_focus_element`); the other elements' events are dropped, as the subscriptions follow the focus alone. The follow-up read is removed: it was a second try with nothing to say the answer had changed, and none of the ten measured scenarios needed it. Test: mockapp's `a_focus_whose_element_was_not_found_is_followed_from_its_own_changes`, which fails with the subscriptions left listening nowhere; the two tests of a focus whose element was not found read the focused element once fewer.

## Decisions to confirm with Dickson (coherence fixes, 2026-10-09)

Made while carrying out the coherence review's fixes, each recorded in its section above:

1. File Explorer is opened without the agent's foreground right but not minimized ("File Explorer opened without the foreground right"): launched minimized, its folder window takes the foreground while minimized and, restored, has no keyboard focus in its file list, in NVDA as in Verbatim.
2. "Read once shown and titled" is read as "once shown": a foreground window in front but not yet shown is reported at its show event, its name read then. File Explorer sets the folder's title before it shows the window (measured), so the title it is shown with is the folder's. Waiting for a title as well would leave a window that is shown untitled never announced.
3. A second Verbatim waits for the first one's process to exit with no limit; a first Verbatim whose teardown never finishes keeps it waiting until it is ended by hand.
4. Control+Tab: Verbatim says "tab control" before "list", and "not selected" for the tab Control+Tab leaves (culled); NVDA says neither ("Control+Tab announces the tab").
5. Every UIA focus of a batch is now handled in turn, oldest first, as NVDA's UIA handler queues them, rather than only the newest; a UIA focus that has moved on since its event is reported unless its element is gone. This reaches every UIA application, not only Windows Terminal.
6. The outpost drops focus and foreground facts on the shell's staging windows, ported from NVDA's File Explorer app module, found when Alt+Tab said "pane" ("The two-window scenarios switch with Alt+Tab").
7. The leave-during-a-flood scenarios count as two-window scenarios and switch with Alt+Tab too.
8. The 250 ms foreground hold is removed against one older observation of Notepad's foreground event arriving 130 ms early, which the new measurements did not reproduce ("Foreground events against the foreground window").
