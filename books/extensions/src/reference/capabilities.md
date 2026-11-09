# Capabilities and the install dialog

A capability is a group of host functions a component imports. Verbatim derives every capability from the component's import list, never from a declaration, and the install dialog's contents come from the same list. [The security model](../explanation/security-model.md) explains why.

This page describes the install dialog as designed for milestone M5, with the capabilities later milestones add.

## The install dialog

The dialog is made to be read with a screen reader, one control at a time, in this order:

1. The line "Only install extensions from people you trust."
2. Read-only fields for the extension's name, version and author.
3. Its description, in a read-only text field.
4. **Applications**, a list with the columns Application and What it does: one row for each application the extension touches, such as "Notepad: changes how Verbatim reads it" or "File Explorer: reads it, clicks in it". A global module's commands add an "Everywhere" row, and a synthesizer a row "Adds the speech synthesizer <name>".
5. **Commands**, a list with the columns Command, Gesture, Where and Conflict: each command with its default gesture, where it works, and the command it replaces when its gesture is already bound.
6. **Before you install**, a list shown only when it has something in it: the warnings below.
7. Install and Cancel.

The Extensions dialog shows an installed extension's details with the same lists.

## What each capability shows

| Capability | What it gives | In the install dialog |
|---|---|---|
| Reading controls | The controls and text of an app module's applications, or of a global module's `reads` | "Changes how Verbatim reads it" for an app module's application; "reads it" for a global module's |
| Speech and sound | Speaking, speaking a control, cancelling speech, tones, a message the user can browse | Nothing |
| Commands | Gestures, and passing a key through to the application | Each command in the Commands list |
| List dialogs | Showing a list with buttons, and its answer | Nothing |
| The mouse | Clicking a control | "clicks in it" for the application |
| The system | The Windows version and the formatted time and date | Nothing |
| Log | Writing to Verbatim's log under the module's id | Nothing |
| Data folder (later) | Reading and writing the extension's own folder, up to its size cap | Nothing |
| Network (later) | HTTP and WebSockets to the hosts the manifest names or a host the user enters; for native code, any host | A warning for each source of the user's content the extension has (below) |
| Speech text (later) | Every utterance, for a speech-text stage | "Changes what Verbatim says before it is spoken" |
| Synthesizer | The text to speak | "Adds the speech synthesizer <name>" |
| Captured images (later) | The images Verbatim captures for a recogniser | Settled with the recognition interfaces |
| Braille devices (later) | Handles for the displays the manifest names, opened by Verbatim | "Connects to <braille display>" |
| Native code | A DLL that runs in the extension's process | A warning: "Contains native code that runs outside Verbatim's extension sandbox." |

Whether a module uses rules, hooks or both is not shown: it changes how the application is read either way. In developer mode, the Extensions dialog shows each module's rules, hooks and imports in full.

## Warnings: where content can leave

Where a source of the user's content meets a way to send data out, "Before you install" shows one warning for each source, naming the hosts:

- "Can send what it reads from Notepad to api.example.com."
- "Can send the text Verbatim speaks to api.example.com."
- "the address you enter" replaces the hosts for an endpoint the user chooses, and "any site" for native code that uses the network.

A warning states what the extension can do, not what it does.

## What the manifest declares

Only what an import cannot carry:

- the hosts the network imports may reach;
- the devices a braille driver drives, by USB identifiers and protocol and by Bluetooth name;
- for a native module, whether it uses the network.

## Updates

An update that adds a way for content to leave, a new network host or device, or a new source of the user's content in an extension that can already send data out, is installed only after the user approves it again. The update dialog lists only what is new.

## Refused

A refusal replaces the dialog's contents with one message and a Close button:

- "Can't install Example Fixes: it changes Notepad, and so does Notepad Tools, which is already installed. Uninstall Notepad Tools first."
- "Can't install Example Fixes: it needs a feature Verbatim doesn't offer extensions (wasi:cli/environment)."
- "Can't install Example Fixes: it contains two parts that both change Notepad."

These are never available, and a component that imports one is refused: raw UI Automation or COM, calls inside other processes, other processes' memory, threads, the registry, files outside the extension's own folders, starting processes, and every WASI interface. An extension whose native code is built only for x86 may contain no other module, since the process that runs 32-bit code under emulation has no WebAssembly runtime; ship any other module as a second extension.

## Not settled yet

The clipboard, a timer, reading speech history, global modules reading every application, and how a recogniser that sends images to a service is consented to, are designed with the imports that bring them.
