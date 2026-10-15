# The NVDA transcript

How to record what NVDA says during a scenario, so it can be compared
with what Verbatim says. The transcript is a research instrument. Use it
interactively to decide what Verbatim should say and to write Verbatim's
own end-to-end assertions by hand. Verbatim's tests never compare against
a transcript, and NVDA never runs in continuous integration. Where Verbatim
deliberately differs from NVDA, record the difference in
[the parity ledger](parity/readme.md).

There are three parts:

- The add-on, `nvda-addon/verbatimTranscript.nvda-addon`, installed into
  NVDA. It records each speech sequence NVDA queues and serves those
  records over a local TCP connection.
- `cargo xtask nvda capture`, which presses keys through the agent's real
  OS input and prints what NVDA queued for speech after each key.
- `cargo xtask nvda compare`, which runs one end-to-end scenario against
  NVDA and reports, step by step, what the scenario expects Verbatim to
  say beside what NVDA said (see "Comparing a scenario" below).

Use a capture to explore: to find what NVDA says for keys you choose,
before a scenario exists, or for a step a scenario cannot take. Use a
comparison to check a scenario that exists against NVDA as a whole: every
step it takes, in its own order and with its own setup, so you see where
its expectations and NVDA part, and whether each place they part is a
difference the ledger records.

## Installing the add-on

Install the committed package,
`nvda-addon/verbatimTranscript.nvda-addon`, like any NVDA add-on. Open
the file from File Explorer, or choose "Install from external source" in
NVDA's Add-on Store, confirm the installation, and restart NVDA when it
asks. The add-on targets NVDA 2026.1 and later.

The add-on does not change what NVDA says or how it sounds. It only
registers listeners on two of NVDA's speech extension points, the ones
NVDA's own Remote Access feature uses. It listens on 127.0.0.1 only, on
port 44100 plus the Windows session id of the NVDA that loaded it, so an
NVDA in a Remote Desktop session and another on the console each get
their own port. It accepts one client at a time. It records nothing until
a client connects and clears its buffer when the client disconnects, so
while no capture is running, it keeps none of your speech. It is
safe to leave installed for everyday use.

## Capturing a scenario

Capturing presses real keys, so it takes over the desktop like a local
end-to-end run (see [the tooling guide](tooling/e2e.md)). Run it only when the
person at the machine is away or has agreed.

1. Start the agent in the same session as the NVDA you want to record:
   `target\debug\verbatim-agent.exe --bind-address 127.0.0.1 --port 44001`.
   The capture asks the agent for its session id to find NVDA's port.
2. Put the application under test in the state the scenario starts from.
3. Run `cargo xtask nvda capture` followed by the steps to take, for
   example `cargo xtask nvda capture tab tab insert+t`.

A step is a key to press, or one of these:

- `--launch <program>`, followed by any number of `--arg <argument>`,
  starts a program through the agent, so its first announcement is
  recorded too; `--launch explorer.exe --arg ms-settings:clipboard`
  opens a page of the Settings app.
- `--launch-without-foreground <program>`, with its `--arg`s, starts the
  program as the harness opens a File Explorer folder
  (`Scenario::open_folder`, through the agent's
  `launch_without_foreground_right`): its window opens as the program
  opens it, with the agent's right to the foreground withheld, for a
  `--front` to bring it forward as the harness does.
- `--launch-minimized <program>`, with its `--arg`s, starts the program
  exactly as the end-to-end harness starts a terminal
  (`Scenario::launch_owning_window`, through the agent's
  `launch_minimized`): its first window opens minimized and inactive
  (`SW_SHOWMINNOACTIVE`), and the agent's right to the foreground is not
  withheld. The harness then waits for the window's shell to write its
  process id and brings the window forward; `--wait-file <path>` waits,
  through the agent, for a file to exist, and `--front` brings the
  window forward as the harness does.
- `--front <image>[=<title>]` brings a window of that program, with a
  title containing the text if given, to the foreground through the
  agent, as the end-to-end suite does; a window named by its title is
  waited for until it is present first, as the suite waits for it. With `*` for the program,
  `--front *=<title>` brings forward whichever program's window has that
  title, for a window whose program is not obvious, such as a console
  window, which belongs to the console host or to the shell it runs
  depending on how it was started. If no such window takes the
  foreground, the capture stops with an error. Just before every key,
  each key of a comma-joined batch on its own, and every character
  typed, the capture checks that the window in front is the window the
  last `--front` brought forward (or, before any, the one in front when
  the capture started), by its handle, and that Windows does not judge it
  not responding, and stops with an error otherwise, so its keys never
  reach another window; after a `--launch`, a `--front` must name the
  launched program's window before any key. It checks once more at its
  end and fails when the window is no longer in front and responding,
  since keys it has not read would go to the window behind it when it
  closes (`docs/tooling/nvda.md`, "Recording what NVDA says").
- `--gesture <identifier>` sends a gesture such as `kb:verbatim+v` to
  the Verbatim running on this machine through its control pipe, so
  Verbatim's own commands can be used without pressing a modifier key
  NVDA also uses.
- `--type <text>` types the text through the agent's `TypeText`, as the
  terminal scenarios type their commands.
- `--hold <key>` presses one key, such as `control`, down through the
  agent's `PressKey` and keeps it down, so the keys of the steps after
  it are pressed with it, as a person holds Control through Windows
  Terminal's tab switcher; `--release <key>` releases it, through
  `ReleaseKey`. A key still held when the capture ends, or stops on an
  error, is released then, and the agent also releases every key a
  connection holds when the connection ends, so a modifier never stays
  down. For example, `--hold control shift+tab --release control` presses
  Shift+Tab with Control held, and records what is said before Control
  is released and after.

A capture that compares a scenario's opening, the burst of focus events a
window raises as it opens, must launch the program exactly as the
scenario does: how a window opens, behind or already in front, decides
which of those events are accepted, by NVDA as by Verbatim. A window
opened in front with `--launch` while the scenario's opens minimized
gives a different burst, and the comparison says nothing. For a terminal
scenario that is `--launch-minimized` with the scenario's own arguments,
`--wait-file` on its shell's process id file, and `--front` on its title.

A key step can also name several keys joined by commas, such as
`numpad8,numpad8`, pressed in one batch, so that both screen readers
count it as a double press.

Steps run one at a time, in order. After each one, the capture waits until
NVDA has queued no new speech for one second, which is the settling rule
of NVDA's own system tests, and then prints what was queued. Each line
gives the time since the key was sent, the speech priority, and the
text. A line reading `cancel` marks a point where NVDA cancelled speech.
The text is flattened the way NVDA's own system tests do it, joining the
sequence's strings and dropping commands such as pitch changes, so it can
be compared directly with Verbatim's speech frames.

Key names are the vocabulary of the control plane's `SendKeys`: a
plus-joined combination such as `shift+tab`, where the modifiers can be
`control`, `shift`, `alt`, `leftwindows`, `rightwindows`, and the
screen-reader modifiers `insert`, `numpadinsert`, and `capslock`.

The options are:

- `--agent <address>`, the agent to press keys through, by default
  `127.0.0.1:44001`.
- `--quiet-ms <ms>`, how long NVDA must stay quiet before a key counts as
  settled, by default 1000.
- `--timeout-ms <ms>`, the longest wait after any one key, by default
  10000.
- `--verbatim`, to record the Verbatim running on this machine instead,
  through its control pipe, with NVDA closed. The same steps then give
  Verbatim's transcript in the same form: each utterance as it is queued,
  and a line for each one cut off before it was heard in full. Run
  Verbatim with test audio and eSpeak NG, as the end-to-end suite does,
  and NVDA with eSpeak NG too, so that the two are compared under the same
  settings.
- `--json`, to print one JSON object per line, for a script comparing
  the two transcripts, since spoken text can itself hold a line break.
  Each step gives an object with its index (`step`) and its `label`, and
  each entry after it an object with the same `step`, its time since the
  step was sent (`ms`), its `kind`, and its `text`: `speech`, with the
  priority or utterance id as its `tag`; `event`, such as NVDA's cancel;
  `ended`, a Verbatim utterance cut off or failed, with `how` and `tag`;
  `sound`; and `note`, the capture's own remark, such as speech that did
  not settle, which has no `ms`.

To record NVDA reading Verbatim's own GUI, run Verbatim with test audio so
that it is silent (see "Test-audio mode" in [the end-to-end guide](tooling/e2e.md)), open
its menu with `--gesture kb:verbatim+v`, and move through the menu and
dialogs with plain keys, which both screen readers let through.

Captures are working material, not kept in the repository: they serve
exploratory testing, comparing what Verbatim says with what NVDA says,
and writing the end-to-end and unit tests that pin the behavior down.

Two traps found on 2026-10-06. NVDA's automatic update check can open an
"NVDA Update" dialog that takes the foreground, and every key then goes
to it, so check the foreground (the agent's `ForegroundInfo`) before a
capture. And a Settings app page can take a few seconds to appear after
`--launch`; give `--front` a long enough `--timeout-ms` before pressing
keys meant for it.

## Comparing a scenario

`cargo xtask nvda compare <scenario>` runs one end-to-end scenario's own
setup and body, unchanged, with NVDA running instead of Verbatim. Every
speech assertion records what it expects instead of checking it, and the
run writes a report, `compare.txt` in the scenario's artifacts folder
(`target/e2e-artifacts/<scenario>`), and prints it. It is a research
instrument, not a test: it asserts nothing, never runs in continuous
integration, and passes unless the harness itself fails, such as a
window that does not come to the front. It takes over the desktop like
any local end-to-end run.

To run it:

1. Start NVDA, with the add-on installed, in the session you will run in,
   and close Verbatim.
2. Start the agent in the same session:
   `target\debug\verbatim-agent.exe --bind-address 127.0.0.1 --port 44001`.
3. Run `cargo xtask nvda compare text_box_review_cursor`, for example.
   `--agent <address>` names another agent, and `--quiet-ms <ms>` sets
   the quiet period, by default 1000 and never less than 500.

The command checks the agent is reachable, deletes the report an earlier
run left, then runs `cargo test -p verbatim-e2e --test e2e -- --ignored
--exact <scenario> --test-threads=1` with `VERBATIM_E2E_ENDPOINT` set to
the agent, `VERBATIM_E2E_COMPARE` set to the scenario's name, and
`VERBATIM_E2E_QUIET_MS` when given, and prints the report if the run
wrote one. Those variables with the usual `cargo test` command do the
same. `VERBATIM_E2E_COMPARE` selects compare mode only for the scenario
it names: any other scenario run while it is set fails at once as a
harness error naming both, so a suite run with the variable left set
fails rather than passing without comparing or testing anything.

A step runs from one input, a key, typed text, or a gesture, or one
action of the harness's that changes what is in front, such as launching
a window, bringing one forward, opening a folder, focusing an element, or
writing the file that lets a terminal's shell show its first prompt, to
the next. A terminal scenario's start therefore comes as three steps: the
desktop, the window, and the prompt. Every speech assertion waits until
NVDA has queued nothing for the quiet period, the capture's settling
rule, so what the scenario does after it, such as writing a file a shell
waits for, does not run ahead of NVDA. Before each input or action, and
at the end, the harness waits the same way, unless NVDA was already found
quiet and nothing has happened since, and takes what NVDA said in the
step.

Two things keep the waiting from changing what NVDA hears. First, an
assertion that speech has started, or has been queued, for the next
input to interrupt waits only until NVDA has queued speech in the step,
not for the quiet period, since that input is meant to follow at once, as
it follows the start of Verbatim's speech in a normal run: the second
press of Verbatim+F10, for example, must come within
NVDA's multi-press timeout, 500 ms, for NVDA to copy rather than select
again. The check before that next input does not wait either, and the
two steps are compared as one unit: the report shows both inputs,
separated by a semicolon, with one verdict and a note, since NVDA's
speech for a rapid double press cannot be told apart press by press.
Second, the quiet period is never less than that multi-press timeout: a
`--quiet-ms` below 500 is raised to 500, and the report's header says so,
so the inputs of two separate steps always reach NVDA more than the
timeout apart and are never taken for a repeated press.

A gesture is pressed as keys, the Verbatim modifier as Insert, NVDA's
modifier: `kb:verbatim+v` as `insert+v`, and `kb:numpad8` as `numpad8`.
Whatever NVDA does with them is what the step records. The report starts
with the scenario's name, the date, NVDA's version, the `nvda`
submodule's pin, and the quiet period; it says "pin differs from the
installed build" when the commit the NVDA version names is not the pin,
since the expectations come from captures and the ported code from the
pin. Then, for each step: its input; "Verbatim:" and the texts the
scenario expects, one per line, with an utterance that is nothing but a
sound marked "not compared", since the add-on records no sounds; "NVDA:"
and what NVDA queued, one per line, with "cancel" where it cancelled
speech; any declared deviation and note; on a step that differs, the two
strings compared, "Verbatim compared:" and "NVDA compared:"; and one
verdict. A step with an input is always reported, even when neither side
said anything.

A step is judged on normalized text: on the Verbatim side each sound,
`sound:` and its name, is taken out wherever it is in an utterance, such
as the spelling-error sound before "spelling error h", and the rest of
the utterance kept; on each side, cancels and empty entries are dropped,
the utterances are joined with single spaces, and every run of white
space becomes one space, since the add-on records NVDA's speech sequence
parts, joined with double spaces, rather than where an utterance ends, so
neither boundaries nor spacing are evidence of a difference in what is
heard. The verdicts are:

- "same": the two normalized strings are equal.
- "differs": they differ, and the scenario declares no
  difference there.
- "declared deviation: <entry>": they differ, and the
  scenario declares the step a deliberate difference, naming the parity
  ledger's entry.
- "declared, but NVDA agrees: <entry>": the scenario declares a
  difference, but NVDA said the same; worth a look, since the difference
  may have gone, or the declaration may be on the wrong step.

A gesture with the Verbatim modifier is judged like any other step:
`kb:verbatim+f9`, for example, is pressed as `insert+f9`, "same" when
NVDA's command says what Verbatim's does and "differs" otherwise.

A summary at the end counts each verdict.

For the comparison to say anything, NVDA must run with the settings the
scenarios' expectations were captured with: eSpeak NG as its
synthesizer, as the suite runs Verbatim with eSpeak NG, and its symbol
level and verbosity settings at NVDA's defaults. The quiet period and
the text compared do not depend on the voice's rate. Check the pin
before a series of comparisons, as before a capture.

Some scenarios cannot be compared, and compare mode refuses them, saying
why: those that end Verbatim's own processes (`outpost_crash_recovery`,
`synth_host_crash_recovery`); those that read Verbatim's status
(`switch_to_onecore`, `second_application_and_verbatim_menu`); those
that take speech until a given line or until Verbatim is idle
(`rapid_tabbing_in_settings`, the `*_control_flood` scenarios); those that
drive Verbatim's own menu and settings dialog, which do not exist without
Verbatim (`menu_and_settings_dialog`, `object_navigation_in_settings`,
`settings_dialog_keys`, `theme_panel`, `terminal_settings_page`); and the
demonstrations and the measure set. Where a scenario waits for one of
Verbatim's events, such as Core receiving a terminal's caret on its
prompt, compare mode has no such evidence and relies on the quiet period
each assertion waits for instead.

## Changing the add-on

The source is under `nvda-addon/src`: `manifest.ini` and the global
plugin `globalPlugins/verbatimTranscript.py`, whose module comment
specifies the wire protocol. After changing it, raise the version in the
manifest, run `cargo xtask nvda build` to rewrite the package, and commit
both. The package is built so the same source always produces the same
bytes, and a unit test in `xtask` fails when the committed package does
not match the source, so `cargo xtask ci` catches a forgotten rebuild.
When a new NVDA release breaks add-on compatibility, raise
`lastTestedNVDAVersion` in the manifest.

If the add-on is ever unavailable, NVDA's own log is a fallback: with the
`speechManager` debug logging category enabled, `nvda.log` records every
queued speech sequence.
