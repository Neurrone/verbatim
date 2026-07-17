# verbatim-inspect

The developer CLI over the control plane. Deliberately not a child of Core
and in no job object; it attaches through the pipe like any client. Output
is plain text, one fact per line — no tables, no spinners — so it reads
well piped, redirected, or through a screen reader.

Every subcommand accepts a global `--connect <ADDRESS>` option: an address
of the form `tcp:HOST:PORT` selects TCP, anything else is treated as a
named-pipe path, and omitting it connects to the well-known local pipe —
the same three transports `verbatim-control`'s promoted `Client` exposes.

Subcommands: `status`; `watch-events`; `watch-speech`; `watch` (both
subscriptions on one connection, lines prefixed `event` or `speech`,
interleaved in arrival order so an event reads directly above the speech
it caused); `send-gesture`; `send-keys`; `latency --last N`; `dump-tree`
(prints the foreground application's accessibility tree from its
top-level window, one node per line, indented two spaces per depth level
and reusing the same role-and-name summary the event stream uses; a
trailing line notes when the outpost's depth or node-count cap truncated
the tree); `dump-recorder` (milestone M2: asks Core to write its flight
recorder's current contents to disk and prints the path it wrote to);
`quit`. Each speech line shows the queue-time delta since the triggering
event, and a follow-up line appears when audio actually starts, carrying
the true event-to-audio latency; an interrupted utterance simply never
gets the follow-up. Timestamps render as local wall-clock time
(`2026-07-14T10:42:32.158`, no zone suffix) via the Win32 conversion that
is correct across DST transitions.
