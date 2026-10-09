# The NVDA transcript

How to record what NVDA says during a scenario, so it can be compared
with what Verbatim says. The transcript is a research instrument. Use it
interactively to decide what Verbatim should say and to write Verbatim's
own end-to-end assertions by hand. Verbatim's tests never compare against
a transcript, and NVDA never runs in continuous integration. Where Verbatim
deliberately differs from NVDA, record the difference in
[the parity ledger](parity.md).

There are two parts:

- The add-on, `nvda-addon/verbatimTranscript.nvda-addon`, installed into
  NVDA. It records each speech sequence NVDA queues and serves those
  records over a local TCP connection.
- `cargo xtask nvda capture`, which presses keys through the agent's real
  OS input and prints what NVDA queued for speech after each key.

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
end-to-end run (see [the tooling guide](tooling.md)). Run it only when the
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
- `--front <image>[=<title>]` brings a window of that program, with a
  title containing the text if given, to the foreground through the
  agent, as the end-to-end suite does. With `*` for the program,
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
  closes (`docs/tooling.md`, "Recording what NVDA says").
- `--gesture <identifier>` sends a gesture such as `kb:verbatim+v` to
  the Verbatim running on this machine through its control pipe, so
  Verbatim's own commands can be used without pressing a modifier key
  NVDA also uses.
- `--type <text>` types the text through the agent's `TypeText`, as the
  terminal scenarios type their commands.

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
that it is silent (see the test-audio section of the tooling guide), open
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
