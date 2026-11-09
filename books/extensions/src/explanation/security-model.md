# The security model

Users install your extension deliberately and trust you to do what it says. Verbatim does not try to make a trusted extension harmless: a translator has to receive what Verbatim speaks and send it to a service, and that is the feature. What Verbatim does is contain your extension, so that a bug, a compromised dependency or a stolen release cannot reach further than what the user approved, and tell the user accurately what your extension can do.

## The goals

The mechanisms on this page serve three goals:

- **Recovery.** The user can always get speech back, and knows which extension was at fault. An extension that stops responding is turned off ([Reliability](reliability.md)), and Verbatim can start with every extension off.
- **Containment.** Your extension never gains Verbatim's own reach. It runs in none of Verbatim's own processes, never has the rights Verbatim needs to read administrator windows and sign-in screens, and reaches only its own folders, the network through its network imports, and the devices Verbatim opens for it.
- **Honest consent.** The install dialog tells the user what your extension can do with their content, derived from the extension itself rather than claimed by it, and an update that adds a flow is approved again.

Two things are not goals. Verbatim does not keep content from an extension the user gave it to: once a user installs a translator, its author and its service can see everything Verbatim speaks. And Verbatim does not defend the install dialog against other programs the user runs, which could install an extension themselves.

## Where your code runs

Two separate boundaries contain your extension:

- **The extension sandbox** is WebAssembly's: your component can call only the functions it imports, and nothing else, whatever its code does.
- **Process isolation** is Windows': every extension runs in a process of its own, inside a Windows container, and all of your modules share it. No other extension's code is in your process, and none of yours is in Verbatim's. A crash or hang in your code, native code included, costs only your extension.

Native code runs outside the extension sandbox but inside process isolation, which is why the install dialog warns about it.

## Your capabilities are your imports

There are no permissions to request. A WebAssembly component's import list is a complete statement of every host function it can ever call, written by the compiler, so Verbatim reads your capabilities from it, and the install dialog's lines come from the same list. Two consequences:

- **Import only what you use.** A dependency that pulls in a network interface puts a network warning in your install dialog. Install every release yourself and read its install dialog before you publish it.
- **The import list is a ceiling.** Your code can change what it does after install, for example by following instructions it downloads, but it can never call an import it was not built with. An update that adds a flow, a new way to send data out, network host or device, or a new source of the user's content in an extension that can already send data out, is installed only after the user approves it again. A source is content your extension receives, such as an application's controls and text, all the text Verbatim speaks, or captured images.

The manifest carries only what an import cannot say: the hosts you connect to, the devices a braille driver drives, and, for native code, whether it uses the network.

## What you can reach

- **Your folders.** You read and write your data folder freely, up to its size cap, and read your module folders, which are read-only, so you cannot change your own code. Your file imports reach nothing else: every path is resolved inside your data folder, no import creates a link, and Windows grants your process your own folders and no other extension's or user's files, so a link someone else planted in your folder leads nowhere. On sign-in and administrator screens you get a scratch folder instead, deleted when Verbatim exits.
- **The network,** if you import a network interface: HTTP with every method, headers and streamed bodies, and WebSockets, to the hosts your manifest names, or to the host the user enters for an endpoint they choose. The import checks every host, so a WebAssembly module reaches only those; Windows can grant native code the network only as a whole, so a native module's install line says it can connect to any site. Raw sockets are added when an extension needs them.
- **The applications you claim,** for an app module, and the applications a global module's manifest lists in `reads`. Navigation stops at those applications' boundary: a parent, sibling or child that belongs to another application's window comes back as none, and so do the focus and foreground when another application has them. Verbatim enforces this in the process that reads the application, by which extension's process a request came from, never by anything your code says.
- **What your kind is given.** A speech-text stage receives every utterance; a recogniser receives the images Verbatim captures for it; a synthesizer receives the text to speak; a braille driver receives handles for the devices it names, which Verbatim, not your code, finds and opens.

From WebAssembly you cannot start processes, read the registry, call Windows directly or write Verbatim's settings; native code can call Windows, within what the container allows, which excludes starting processes, other processes and the user's files. No module can read another extension's data. Your commands may take a gesture Verbatim or another extension already uses, such as one the user relies on to stop speech; the install dialog lists every such conflict, so the user decides.

## What the user sees

The install dialog shows a warning wherever a source of the user's content meets a way out, for example "Can send the text Verbatim speaks to api.example.com". The line states what your extension can do, never what it does: Verbatim does not analyse which of your code sends what. If you send content only when the user asks, say so in your description, and consider confirming in your own interface each time. [Capabilities and the install dialog](../reference/capabilities.md) lists every line.

## Native code

Native code is for an engine or a vendor library that exists only as a binary, or for work that must call Windows directly. It comes in two forms:

- **A library your component calls.** A WebAssembly module of any kind may ship a DLL in its extension and call its functions through a narrow import that passes bytes to a named function; the DLL runs in your extension's process. This is the usual form, for calling a vendor's SDK from your module.
- **A whole native module,** for kinds whose interface is small: a synthesizer's adapter, and later a braille driver's, implements its kind's interface as a C ABI.

Native code runs in the same container as the rest of your extension and reaches the same folders, network and devices, and a crash in it costs only your extension. But it has no import list for Verbatim to read, so what it can do is bounded only by the container, the install dialog warns "Contains native code that runs outside Verbatim's extension sandbox", and you state in its manifest whether it uses the network. Prefer WebAssembly whenever you have the source: a component runs on x64 and ARM64 unchanged, and its imports tell the user exactly what it can do.

## Handling the user's content

Everything you receive belongs to the user: what is on their screen, what they type, often their email and documents.

- Send only what your feature needs, to the service your description names.
- Keep API keys in the secret store, never in your data folder or the log.
- Never write content to the log, which users attach to bug reports.

Verbatim does not review extensions, so your users' trust in you is the only review your extension gets.
