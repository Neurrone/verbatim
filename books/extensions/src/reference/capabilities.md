# Capabilities and the install dialog

A capability is a group of host functions a component imports. Verbatim derives every capability from the component's import list, never from a declaration, and the install dialog's lines come from the same list. [The security model](../explanation/security-model.md) explains why.

## Capabilities

| Capability | What it gives | Shown at install |
|---|---|---|
| Node reading | An app module's controls in its own applications: properties, parents, children and siblings, finding a descendant or ancestor, the window, a control's whole text, and the current focus and foreground. Navigation stops at the application's boundary. | "Reads the controls and text of" the named applications, for every app module |
| Output | Speaking, cancelling speech while the module's application has the user's attention, tones, and a message the user can browse | No |
| Commands | Passing a key through to the application | Each command and its default gesture, one per line |
| Process facts | The application's executable, path, product version, bitness and console client | No |
| Settings | A read-only view of Verbatim's reading settings | No |
| Log | Writing to Verbatim's log under the module's id | No |
| Data folder | Reading and writing the extension's own folder, up to its size cap | No |
| Network | HTTP with every method, headers and streamed bodies, and WebSockets, to the hosts the manifest names or a host the user enters; for native code, any host | Yes, as a flow with each source (below) |
| Speech text | Every utterance, for a speech-text stage, and the text to speak, for a synthesizer | For a stage, "Changes what Verbatim says before it is spoken"; for a synthesizer, "Provides a speech synthesizer, which receives all the text Verbatim speaks"; and a flow if the extension can connect |
| Captured images | The images Verbatim captures for a recogniser | Settled with the recognition interfaces |
| Devices | Handles for the braille displays the manifest names, opened by Verbatim | "Drives" the named display |
| Native code | A DLL in place of a component, in a module of any kind, implementing its kind's C ABI | "Runs native code in a restricted process of its own", shown first, with the module's other lines |

Exported hooks also produce a line: "Runs its own code when something happens in" the named applications. A module made only of rules is shown as "Changes how Verbatim reads" the named applications "with N rules, with no code of its own".

## Flows

Where a source of the user's content meets a way out, the dialog shows one warning line for each source, naming the hosts:

- "Receives all the text Verbatim speaks, and can connect to api.example.com."
- "Reads the controls and text of Notepad, and can connect to api.example.com."
- "the address you enter" replaces the hosts for an endpoint the user chooses, and "any site" for native code that uses the network.

The line states what the extension can do, not what it does.

## What the manifest declares

Only what an import cannot carry:

- the hosts the network imports may reach;
- the devices a braille driver drives, by USB identifiers and protocol and by Bluetooth name;
- for a native module, whether it uses the network.

## Updates

An update that adds a way to send data out, a network host or a device is installed only after the user approves the new lines, which the update dialog lists on their own.

## Refused

These are never available, and a component that imports one is refused at install, with the import named: raw UI Automation or COM, calls inside other processes, other processes' memory, threads, the registry, files outside the extension's own folders, starting processes, and every WASI interface outside the file and network imports. A module may not bind cancelling speech, sleep mode, the Verbatim menu, quitting, or the screen curtain and its status.

## Not settled yet

The clipboard, a timer, the mouse, reading speech history, global modules' access to every application, and how a recogniser that sends images to a service is consented to, are designed with the imports that bring them.
