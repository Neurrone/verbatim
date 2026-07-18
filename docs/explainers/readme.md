# Windows background explainers

This folder teaches the Windows-specific background needed to work on Verbatim
and to read `docs/nvda`. It is written for an experienced systems programmer
who has not worked against the Win32 API before. Each file explains a Windows
technology on its own terms; how NVDA drives that technology lives in
`docs/nvda`, and how Verbatim uses it lives in [Architecture](../architecture.md) and
[the crate guides](../crates/readme.md).

These files describe stable, documented Windows behavior, so they cite
Microsoft Learn pages rather than source code. When a claim is about
undocumented or folkloric behavior (there is some of that in accessibility
work), the file says so explicitly.

## Reading order

Read in this order the first time; later, use each file as a reference.

1. [The accessibility landscape](accessibility-landscape.md) — orientation: the three accessibility APIs,
   who implements which, and the bridges between them. Read this first; it
   makes every other file make sense.
2. [COM](com.md) — the Component Object Model: interfaces, apartments, marshaling.
   The single biggest prerequisite; MSAA, IA2, and UIA are all COM APIs.
3. [Windows and messages](windows-and-messages.md) — window handles, window classes, message loops,
   hooks, and winevents. Explains why a hung app can hang its callers.
4. [MSAA](msaa.md) — Microsoft Active Accessibility, the oldest API.
5. [IA2](ia2.md) — IAccessible2, the extension that made MSAA good enough for
   browsers.
6. [UIA](uia.md) — UI Automation, the modern API, both client and provider sides.
7. [ARIA](aria.md) — the web-authored semantics layer that browsers map
   into IA2 and UIA: roles, states, live regions, and the mapping
   documents. For readers who have not done web work.
8. [The Java Access Bridge](java-access-bridge.md) — how Java
   applications become readable at all: the bridge architecture, its C
   API, and its manual reference lifetime.
9. [Windows IPC](ipc.md) — the inter-process communication primitives used by screen
   readers: named pipes, shared memory, events, and MS-RPC.
10. [Rust concurrency](rust-concurrency.md) — the Rust-side concurrency vocabulary Verbatim
    is written in (threads, channels, arc-swap, RAII guards) and how it
    meets the Windows primitives. Verbatim-specific, unlike its siblings.
11. [Processes and security](processes-and-security.md) — integrity levels, UIPI, uiAccess, the secure
    desktop, and AppContainer.
12. [Input and text](input-and-text.md) — virtual keys, scan codes, keyboard layouts, and
    enough IME/TSF to understand typed-character echo.
13. [Audio](audio.md) — WASAPI concepts: shared-mode rendering, latency,
    cancellation, sessions, and ducking.
14. [Speech APIs](speech-apis.md) — the Windows speech synthesis
    landscape: OneCore, SAPI 5 and SAPI 4 in implementable depth, and
    embedded engines.
