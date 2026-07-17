# Remote access

NVDA Remote — originally a third-party add-on — is now built into
NVDA as `source/_remoteClient/` (underscored: API still private).
It lets one NVDA user control another's machine, or receive support:
keystrokes travel one way, speech and braille travel back.

## Roles and topology

A session has a *leader* (the controller) and a *follower* (the
controlled machine) (`_remoteClient/session.py`,
`LeaderSession`/`FollowerSession`). Connection is via a *relay
server*: both sides connect out to a server (the community relay or
self-hosted; `server.py` contains a built-in relay implementation
NVDA can host) and join a named *channel* protected by a shared
key; direct connection mode exists where one side is the server.
`connectionInfo.py` encodes this as `nvdaremote://` URLs
(`urlHandler.py` registers the scheme).

## Protocol

`_remoteClient/protocol.py`: JSON messages, type-tagged
(`RemoteMessageType`), protocol version 2. The vocabulary tells you
exactly what is relayed: `key` (input events leader-to-follower),
`speak` / `cancel` / `pause_speech` / `tone` / `wave` / `index`
(the follower's *speech sequence and sound output* forwarded to the
leader — speech is re-spoken by the leader's own synth, not audio
streamed), `display` / `braille_input` / `set_braille_info`
(braille cells to the leader's display, braille keys back),
`set_clipboard_text` (clipboard push), `send_SAS` (Ctrl+Alt+Del on
the follower, which requires the follower side's uiAccess/service
cooperation), plus channel bookkeeping (`join`, `client_joined`, …)
and MOTD from the relay.

Transport (`transport.py`): TLS socket to the relay, a reader
thread dispatching inbound messages to registered handlers via
queued callbacks, reconnect logic (`ConnectorThread`), and
serialization in `serializer.py` (JSON with type registration).
`bridge.py` wires NVDA extension points to protocol messages — the
follower hooks its own speech pipeline (the post-manager filter
points; [Speech](speech.md)) and braille output and forwards them.

## Interception details

- On the leader, a "sending keys" mode captures all keyboard input
  (hooking into [Keyboard input](input.md)'s gesture layer above script resolution)
  and ships raw key events; the follower injects them with
  `SendInput` machinery (`localMachine.py`), gated by its own
  security posture.
- `cues.py` plays earcons for connect/disconnect and control
  handoff.
- Secure desktop: `_remoteClient/secureDesktop.py` bridges a
  follower's secure-desktop NVDA instance ([Secure mode](secure-mode.md)) into
  the session over a local IPC hop, so UAC prompts on the
  controlled machine remain readable remotely — the interaction of
  remoting with the secure desktop is the trickiest part of the
  whole feature and got its own module.

## Trust model

The design trusts the channel key: anyone with server address plus
key joins the channel with full control rights. Speech relayed as
*sequences* (not audio) means the leader hears the follower's
screen reader in the leader's own voice/rate, and message volume
stays tiny. All relaying is explicit opt-in per session; nothing
listens by default.
