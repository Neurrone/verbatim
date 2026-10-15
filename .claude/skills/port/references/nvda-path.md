# What to determine about an NVDA causal path

Read during phase 2, and give this list to every research subagent. Trace the
complete causal path that produces the behavior; do not stop at the most
obvious callback, class, or handler, because a function name that sounds
authoritative is not proof that the path has been understood. Search callers
and consumers.

As applicable, determine:

- where the raw input or platform event originates;
- registrations, hooks, callbacks, RPC, or injected sources involved;
- early filtering;
- event acceptance;
- ordering, limiting, deduplication, and coalescing;
- thread assumptions;
- process assumptions;
- synchronous cross-process calls;
- object construction or lookup;
- queue-time state changes;
- execution-time state changes;
- focus, navigator, foreground, or document state affected;
- handler dispatch and overrides;
- downstream consumers;
- speech, braille, sound, or other observable output;
- cancellation or supersession behavior;
- stale-event behavior;
- failure and timeout behavior;
- application-specific exceptions;
- relevant configuration;
- reasons for surprising behavior when they can be established.

Mark each finding as established from source, established from a live
capture, or a hypothesis. An inference does not become a parity requirement
without evidence.

Return the causal path as prose with source citations (NVDA file and
function) and the preserve, replace, omit or defer, and unresolved list from
phase 3. Do not return file contents; the parent needs the path and the
classification, not the code.
