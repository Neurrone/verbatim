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
- Lines made only of symbols (vim's "~" rows, box-drawing rules) are skipped until symbol processing exists, as a stand-in for NVDA's default symbol level; the code says so in a comment, and the roadmap's M8 entry revisits it so that such lines then follow the user's symbol level.
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
