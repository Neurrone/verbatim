# The manifests

An extension is described by two kinds of file: one `manifest.toml` for the extension, and one `module.toml` for each module it holds. Both are TOML. Anything Verbatim does not recognise is refused at install, with the key named, rather than ignored, so a misspelt key is an error you see at once.

These pages describe the manifests as designed for milestone M5. Once Verbatim's manifest types exist, this page is generated from them.

## The extension manifest

`manifest.toml` sits at the root of the `.vext` file.

| Key | Required | Type | What it is |
|---|---|---|---|
| `id` | yes | text | The extension's unique id, such as `example.notepad-tools`. It names the folder the extension is installed into and is how Verbatim refers to the extension everywhere. Letters, digits, dots and hyphens only. |
| `name` | yes | message id | The display name, as a message id in the extension's own strings (`locale/`). |
| `description` | yes | message id | What the extension does, shown in the install dialog and the Extensions dialog. |
| `author` | yes | text | Who made it. |
| `version` | yes | text | The extension's own version, such as `1.2.0`. |
| `api` | yes | text | The version of the extension API the extension was built against, such as `0.1`. The imports in your components are the authority; this key must agree with them. |
| `modules` | yes | list of text | The module folders in the extension. |

An extension carries its strings in `locale/<language>/*.ftl`, in the Fluent format, with English as the fallback.

```toml
id = "example.notepad-tools"
name = "notepad-tools-name"
description = "notepad-tools-description"
author = "A. Developer"
version = "1.0.0"
api = "0.1"
modules = ["notepad"]
```

## The module manifest

`module.toml` sits in each module folder, beside its component if it has one.

| Key | Required | Type | What it is |
|---|---|---|---|
| `kind` | yes | `"app"`, `"global"` or `"synthesizer"` | The module's kind ([Extensions, modules and kinds](../explanation/concepts.md)). |
| `component` | if the module has code | file name | The WebAssembly component, such as `notepad.wasm`. A module made only of rules has none. |
| `applications` | for an app module | list of text | The application identities the module claims, every name its application has gone by. |
| `reads` | no, global modules only | list of text | The applications a global module may read. |
| `sleep-mode` | no, app modules only | true or false | Starts the claimed applications in sleep mode, for applications that speak for themselves. |
| `[[rules]]` | no | tables | Rules ([Rules](rules.md)); not in a global module. |
| `[[hooks]]` | no | tables | Which hooks Verbatim calls, and for what ([Hooks](hooks.md)). |
| `[[commands]]` | no | tables | Commands and their gestures ([Commands and gestures](commands.md)). |

## What is refused

An extension is refused at install, with the reason shown in the install dialog, when:

- a key, rule property, action or event kind is not one these pages list;
- two modules of the extension claim the same application;
- an installed extension already claims one of its applications;
- a rule or a command id appears twice in one module, or one module binds the same gesture twice;
- a rule names a property of a backend other than the one it names;
- a component imports something the extension API does not offer.

The checks run again whenever Verbatim loads the extension, so an extension whose folder was changed after install is marked failed rather than loaded.
