# ADR 0001: Use Outpost Processes for Provider Isolation

## Status

Accepted.

## Context

Verbatim must avoid blocking when the foreground application hangs. UIA, MSAA, IA2, native APIs, and some synth DLLs can block or crash in ways that thread-level isolation cannot fully contain.

NVDA uses a combination of out-of-process Python, native helper code, UIA Remote Operations, and in-process techniques to solve performance and compatibility problems. Verbatim should learn from these techniques, but its primary safety boundary should be process isolation.

## Decision

Verbatim will use outpost processes for accessibility provider access. Each outpost is responsible for one application or provider scope. The default scope is one logical outpost per application, but Verbatim can split browsers, Electron apps, Office documents, embedded providers, tabs, renderer processes, or plugin scopes into separate outpost processes when traces show independent hang or latency domains. The core communicates with outposts over structured IPC with deadlines and trace IDs.

## Consequences

| Positive | Negative |
|---|---|
| Core remains responsive during provider hangs | More IPC complexity |
| Outpost crashes are recoverable | Provider object identity must be reconstructed |
| App-specific throttling is easier | More lifecycle management |
| VM and fake-provider testing can target boundaries | Requires careful trace correlation |

## Alternatives Considered

| Alternative | Reason rejected |
|---|---|
| Single process, many threads | A blocking call can still compromise the reader process |
| One global accessibility worker process | One hung application can block all provider access |
| Broad in-process injection first | Increases crash and security risk before measurements justify it |

## Follow-Up Requirements

| Requirement | Specification |
|---|---|
| COM lane model | `docs/architecture/com-threading.md` |
| Watchdog behavior | `docs/architecture/process-model.md` |
| Trace correlation | `docs/observability/trace-schema.md` |
| Phase gates | `docs/phases/README.md` |
