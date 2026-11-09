# A fix made of rules: Notepad's go-to-line box

In this tutorial you write a module that needs no code at all. Windows 11's Notepad reports its "Go to line" box as a dialog, so Verbatim says "dialog" where you would expect an edit field. One rule fixes it.

This tutorial is a draft written against the API designed for milestone M5; you can follow it once M5 is built.

## What you need

- Verbatim running in developer mode, so it watches the developer folder.
- Notepad.

## Find the control's properties

1. Open Notepad and press Control+G to open the go-to-line box.
2. Press Verbatim+F1. Verbatim opens the log viewer with a report on the focused control: each property, the backend it came from, and the module or rule that set it, if any.
3. Note the backend, `uia`, and the automation id, `LineNumberBox`. An automation id is the most stable way to recognise a control, since it does not change with the language or the text shown.

## Write the module

In the developer folder, make a folder `notepad-goto` holding one file, `module.toml`:

```toml
kind = "app"
applications = ["notepad.exe"]

[[rules]]
id = "goto-line-is-edit"
subject = "node"
backend = "uia"
match = { automation-id = "LineNumberBox" }
action = { set = { role = "edit" } }
```

- `kind` and `applications` make this an app module for Notepad.
- The rule matches the control by its automation id, under UI Automation, and sets its role.

## Try it

Save the file. Verbatim says "Loaded notepad-goto". Press Control+G in Notepad again, and Verbatim announces an edit field.

Press Verbatim+F1 again: the role now reads "edit, set by notepad-goto/goto-line-is-edit, was dialog from UIA".

## What you learned

- A rule is a matcher and an action, and Verbatim applies it without running any code of yours.
- Verbatim+F1 shows a control's properties and where each came from.
- The developer folder reloads a module the moment you save it.

Next, [a module with code](calculator-result.md) reads the application in response to an event.
