# The security model

Users install your extension deliberately and trust you to do what it says. Verbatim does not try to make a trusted extension harmless: a translator has to receive what Verbatim speaks and send it to a service, and that is the feature. What Verbatim does is contain your extension, so that a bug, a compromised dependency or a stolen release cannot reach further than what the user approved, and tell the user accurately what your extension can do.

## The goals

The mechanisms on this page serve three goals:

- **Recovery.** The user can always get speech back, and knows which extension was at fault. An extension that stops responding is turned off ([Reliability](reliability.md)), and Verbatim can start with every extension off.
- **Containment.** Your extension never gains Verbatim's own reach. It runs in none of Verbatim's own processes, never has the rights Verbatim needs to read administrator windows and sign-in screens, and reaches only its own folders, the network through its network imports, and the devices Verbatim opens for it.
- **Honest consent.** The install dialog tells the user what your extension can do with their content, derived from the extension itself rather than claimed by it, and an update that can do more is approved again.

Two things are not goals. Verbatim does not keep content from an extension the user gave it to: once a user installs a translator, its author and its service can see everything Verbatim speaks. And Verbatim does not defend the install dialog against other programs the user runs, which could install an extension themselves.

## Where your code runs

Every installed extension runs in a process of its own, inside a Windows container, and all of your modules share it. No other extension's code is in your process, and none of yours is in Verbatim's. A crash or hang in your code, native code included, costs only your extension.

## Your capabilities are your imports

There are no permissions to request. A WebAssembly component's import list is a complete statement of every host function it can ever call, written by the compiler, so Verbatim reads your capabilities from it, and the install dialog's lines come from the same list. Two consequences:

- **Import only what you use.** A dependency that pulls in a network interface puts a network warning in your install dialog. The packaging tool prints the capabilities it derived before it builds a `.vext`; read that list on every release.
- **The import list is a ceiling.** Your code can change what it does after install, for example by following instructions it downloads, but it can never call an import it was not built with. An update that adds an import, a network host or a device is installed only after the user approves it again.

The manifest carries only what an import cannot say: the hosts you connect to, the devices a braille driver drives, and, for native code, whether it uses the network.

## What you can reach

- **Your data folder.** You read and write it freely, up to its size cap. It is the only part of the file system you can reach: every path is resolved inside it, no import creates a link, and Windows itself refuses anything outside it, so a link someone else planted in your folder leads nowhere. Your module folders are read-only, so you cannot change your own code. On sign-in and administrator screens you get a scratch folder instead, deleted when Verbatim exits.
- **The network,** if you import a network interface: HTTP with every method, headers and streamed bodies, and WebSockets, to the hosts your manifest names, or to the host the user enters for an endpoint they choose. Raw sockets are added when an extension needs them.
- **The applications you claim,** for an app module. Navigation stops at your application's boundary: a parent, sibling or child that belongs to another application's window comes back as none, and so do the focus and foreground when another application has them.
- **What your kind is given.** A speech-text stage receives every utterance; a recogniser receives the images Verbatim captures for it; a synthesizer receives the text to speak; a braille driver receives handles for the devices it names, which Verbatim, not your code, finds and opens.

You cannot start processes, read the registry, call Windows directly from WebAssembly, write Verbatim's settings, read another extension's data, or bind the gestures that let the user recover: cancelling speech, sleep mode, the Verbatim menu, quitting, and the screen curtain.

## What the user sees

The install dialog shows a warning wherever a source of the user's content meets a way out, for example "Receives all the text Verbatim speaks, and can connect to api.example.com". The line states what your extension can do, never what it does: Verbatim does not analyse which of your code sends what. If you send content only when the user asks, say so in your description, and consider confirming in your own interface each time. [Capabilities and the install dialog](../reference/capabilities.md) lists every line.

## Native code

Any kind of module may contain native code instead of a WebAssembly component, for an engine or a vendor library that exists only as a binary, or for work that must call Windows directly. A WebAssembly component cannot call a DLL, so a native module implements its kind's interface as a C ABI, as a synthesizer's adapter does; each kind's C ABI is defined once a native module of that kind needs it.

Native code runs in the same container as the rest of your extension and reaches the same folders, network and devices, and a crash in it costs only your extension. But it has no import list for Verbatim to read, so what it can do is bounded only by the container, it carries the strongest warning in the install dialog, and you state in its manifest whether it uses the network. Prefer WebAssembly whenever you have the source: a component runs on x64 and ARM64 unchanged, and its imports tell the user exactly what it can do.

## Handling the user's content

Everything you receive belongs to the user: what is on their screen, what they type, often their email and documents.

- Send only what your feature needs, to the service your description names.
- Keep API keys in the secret store, never in your data folder or the log.
- Never write content to the log, which users attach to bug reports.

Verbatim does not review extensions, so your users' trust in you is the only review your extension gets.
