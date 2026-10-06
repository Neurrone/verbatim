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
3. Run `cargo xtask nvda capture` followed by the keys to press, for
   example `cargo xtask nvda capture tab tab insert+t`.

Each key is pressed separately. After each one, the capture waits until
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

To record NVDA reading Verbatim's own GUI, run Verbatim with test audio so
that it is silent (see the test-audio section of the tooling guide). Give
the two screen readers different modifier keys, using Verbatim's
share-modifier setting, which exists so that a second screen reader can
run behind Verbatim.

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
