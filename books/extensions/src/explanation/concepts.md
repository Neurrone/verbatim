# Extensions, modules and kinds

## Extensions and modules

An **extension** is what a user installs: one `.vext` file, a zip holding an extension manifest, locale files for its strings, and one or more module folders. It is the unit of installation, version, capabilities and ordering, and the equivalent of an NVDA add-on.

A **module** is one contribution inside an extension. It is a folder holding a `module.toml` manifest and, unless it is made of rules only, a WebAssembly component. Every module has a **kind**:

- An **app module** is bound to one or more applications and active while one of them is running. Most of NVDA's add-ons are app modules.
- A **global module** works across applications, like NVDA's global plugins.
- A **speech-text stage** takes the text of each utterance before it is spoken and returns new text, for translation, language detection or rewriting.
- A **synthesizer** speaks: it receives what Verbatim says and returns audio.
- A **recogniser** receives an image Verbatim captured, of an object, a window or the screen, and returns text, word positions or a description.
- A **braille driver** talks to a braille display.

One extension can hold several modules, of the same kind or of different kinds.

## Built-ins

Verbatim's own support for applications such as Explorer, Notepad and Windows Terminal is written as modules too, called **built-ins**, against the same API you use. A built-in ships with Verbatim, is not listed among the user's extensions, and cannot be disabled by the user. Because built-ins use the same API, anything Verbatim does for an application through a built-in, an extension can do too.

## How a module changes Verbatim

A module has three ways to change how Verbatim behaves, from cheapest to most powerful:

- **Declarations** are data in the manifest: the applications an app module claims, its commands and their default gestures, sleep mode for its applications.
- **Rules** are declarations of the form "match this, do that": a matcher over a control's properties, such as its role, class or automation id, and an action, such as giving it a new role or name, or dropping an event. Verbatim applies rules itself without calling your code, so a module made only of rules costs nothing per event beyond a table lookup. Every rule has an id, which the user and you see wherever it took effect.
- **Hooks** are functions your component exports and Verbatim calls when something happens: an event, a control appearing, one of your commands. A hook runs with a deadline ([Reliability](reliability.md)).

The functions your component calls are its **imports**: reading controls, speaking, playing tones, logging, and later the network and files. Which groups of imports a component uses are its **capabilities** ([The security model](security-model.md)).

## Binding to applications

An app module claims applications by **application identity**: normally the executable's name, such as `notepad`. For a console window it is the program running in the console, not the console host; for applications hosted by `wwahost.exe` or `msedgewebview2.exe` it is the hosted application. A module lists every name its application has gone by, such as an executable renamed between Windows versions.

There is one instance of your module per running process of a claimed application, so two windows of one application in two processes are two instances with separate state. Your instance's memory is its state; it lasts as long as the process and is lost when your module is reloaded or fails.

A module may claim Verbatim's own executables, to change how Verbatim's own windows are read.

## Where your code runs

Every installed extension runs in a process of its own, inside a Windows container, and all of its modules share that process. Your reads of an application are requests that the process reading that application answers, within that application. A crash or hang in your code costs only your extension; [The security model](security-model.md) explains what the container allows.

Built-ins run inside Verbatim's own processes instead, since they are part of Verbatim and run on the busiest paths. The API is the same either way.

## Layers and who wins

Verbatim applies three layers, in order: installed extensions, built-ins, then Verbatim's core behaviour. An installed extension's module overrides a built-in for the same application by design, and installed extensions are ordered among themselves, newest highest.

Within one event, every layer's rules apply first, lowest layer first, so the highest layer's rule wins a property two layers set; then hooks run from the highest layer down, each seeing the control as the layers above left it, until one ends the chain. For commands, the highest layer that binds a gesture gets it.

Nothing is hidden: for every property it changes, Verbatim records which module and rule set it, the focus report shows each property with its source, and the log names the module and rule that dropped an event.
