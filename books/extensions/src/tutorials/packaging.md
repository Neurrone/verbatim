# Packaging and installing an extension

In this tutorial you turn the modules from the earlier tutorials into an extension other people can install. A `.vext` file is a zip with a different name, as NVDA's add-ons are.

This tutorial is a draft written against the design for milestone M5.

## Lay out the folder

Make a folder for the extension, holding its manifest, its strings and one folder per module:

- `manifest.toml`
- `locale/en/strings.ftl`
- `notepad-goto/module.toml`
- `calculator/module.toml`
- `calculator/calculator.wasm`

`manifest.toml` names the extension and lists the module folders ([The manifests](../reference/manifests.md)):

```toml
id = "example.everyday-fixes"
name = "everyday-fixes-name"
description = "everyday-fixes-description"
author = "A. Developer"
version = "1.0.0"
api = "0.1"
modules = ["notepad-goto", "calculator"]
```

`locale/en/strings.ftl` holds the strings, in the Fluent format:

```
everyday-fixes-name = Everyday fixes
everyday-fixes-description = Reads Notepad's go-to-line box as an edit field, and speaks Calculator's whole result.
```

## Zip it

Zip the folder's contents, not the folder itself: `manifest.toml` must be at the top of the zip. Then rename the zip from `.zip` to `.vext`.

In File Explorer: open the folder, select everything in it with Control+A, open the context menu, choose Compress to, then ZIP file, and rename the result `everyday-fixes.vext`.

If the extension needs files you may not distribute, such as a speech engine's own files, the person installing it adds them to the folder before zipping it.

## Install it

Open the `.vext` file while Verbatim is running, or choose Install from file in the Extensions dialog. Verbatim shows the install dialog:

1. "Only install extensions from people you trust."
2. The name, version and author.
3. The description.
4. Applications: "Notepad: changes how Verbatim reads it" and "Calculator: changes how Verbatim reads it".
5. No commands and no warnings, since the extension has neither.

Read every line before you publish an extension: it is what your users will read, and an unexpected line, such as a warning that content can leave, means a dependency pulled in something you did not intend.

If Verbatim is not running when you open the file, a message says that Verbatim must be running to install extensions.

## What you learned

- A `.vext` is a zip of the extension folder, with `manifest.toml` at the top.
- Verbatim checks everything when the extension is installed, and the install dialog shows what it can do.
