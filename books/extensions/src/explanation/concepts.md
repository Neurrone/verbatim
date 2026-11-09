# Extensions, modules and kinds

## Extensions and modules

An **extension** is what a user installs: one `.vext` file, a zip holding an extension manifest, locale files for its strings, and one or more module folders. It is the unit of installation, version, capabilities and ordering, and the equivalent of an NVDA add-on.

A **module** is one contribution inside an extension. It is a folder holding a `module.toml` manifest and, unless it is made of rules only, a WebAssembly component. Every module has a **kind**:

- An **app module** is bound to one or more applications and active while one of them is running. Its commands work while one of its applications is in front.
- A **global module** is bound to no application: it runs for the whole session, its commands work everywhere, and it may read the applications its manifest lists. It is the equivalent of NVDA's global plugins.
- A **speech-text stage** takes the text of each utterance before it is spoken and returns new text, for translation, language detection or rewriting.
- A **synthesizer** speaks: it receives what Verbatim says and returns audio.
- A **recogniser** receives an image Verbatim captured, of an object, a window or the screen, and returns text, word positions or a description.
- A **braille driver** talks to a braille display.

One extension can hold several modules, of the same kind or of different kinds, but no two of its modules may claim the same application.

App modules, global modules and synthesizers come first, in milestones M5 and M6; the other kinds follow.

## Built-ins

Verbatim's own support for applications such as Explorer, Notepad and Windows Terminal is written as modules too, called **built-ins**, against the same API you use, and so are its own speech engines. A built-in ships with Verbatim, is not listed among the user's extensions, and cannot be disabled by the user. Because built-ins use the same API, anything Verbatim does for an application through a built-in, an extension can do too.

## How a module changes Verbatim

A module has three ways to change how Verbatim behaves, from cheapest to most powerful:

- **Declarations** are data in the manifest: the applications an app module claims, the applications a global module reads, its commands and their default gestures, sleep mode for its applications ([The manifests](../reference/manifests.md)).
- **Rules** are declarations of the form "match this, do that": a matcher over a control's properties, such as its role, class or automation id, and an action, such as giving it a new role or name, or dropping an event. Verbatim applies rules itself without calling your code, so a module made only of rules costs nothing per event beyond a table lookup. Every rule has an id, which the user and you see wherever it took effect ([Rules](../reference/rules.md)).
- **Hooks** are functions your component exports and Verbatim calls when something happens: an event, a control appearing, one of your commands. A hook runs with a deadline ([Hooks](../reference/hooks.md), [Reliability](reliability.md)).

The functions your component calls are its **imports**: reading controls, speaking, playing tones, showing a list dialog, clicking, logging, and later the network and files ([Imports](../reference/imports.md)). Which groups of imports a component uses are its **capabilities** ([The security model](security-model.md)).

## Binding to applications

An app module claims applications by **application identity**: normally the executable's name, such as `notepad`. For a console window it is the program running in the console, not the console host; for applications hosted by `wwahost.exe` or `msedgewebview2.exe` it is the hosted application. A module lists every name its application has gone by, such as an executable renamed between Windows versions.

There is one instance of your module per running process of a claimed application, so two windows of one application in two processes are two instances with separate state. Your instance's memory is its state; it lasts as long as the process and is lost when your module is reloaded or fails.

A module may claim Verbatim's own executables, to change how Verbatim's own windows are read.

## Where your code runs

Every extension runs in a process of its own, inside a Windows container, and all of its modules share that process; Verbatim's built-ins are one such extension, and its two speech engines are two more. Your reads of an application are requests that the process reading that application answers, within that application. A crash or hang in your code costs only your extension; [The security model](security-model.md) explains what the container allows.

Inside the process, each application your module serves, and each global module, has its own thread, so an application that is slow to answer holds up only your hooks for that application. Modules you are developing, loaded from the developer folder, run as installed extensions do. The API is the same everywhere.

## Layers and who wins

At most one installed extension may change a given application, on top of Verbatim's built-in for it: installing a second extension that claims an application already claimed is refused, with the extension that claims it named. So for any application there are at most three layers, applied in this order: a module from the developer folder or the installed extension, then the built-in, then Verbatim's core behaviour. An installed extension builds on the built-in by design.

A module in the developer folder that claims an application an installed extension claims replaces that extension's module for that application only, while it is there, so you can work on the next version of your own extension with the current one installed. Removing it from the developer folder brings the installed one back.

Within one event, every layer's rules apply first, lowest layer first, so the highest layer's rule wins a property two layers set; then hooks run from the highest layer down, each seeing the control as the layers above left it, until one ends the chain. For commands, the highest layer that binds a gesture gets it.

Nothing is hidden: for every property it changes, Verbatim records which module and rule set it, the focus report shows each property with its source, and the log names the module and rule that dropped an event.
