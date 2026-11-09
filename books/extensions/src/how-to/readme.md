# How-to guides

Each guide gives the steps for one task. They are written as each topic's tooling exists; until then, this list says what each will cover.

## Finding your way

- Find a control's properties for a matcher: the focus report on Verbatim+F1, and `verbatim-inspect`.
- Work in the developer folder: turning developer mode on, where the folder is, what reloading says, and overriding your own installed extension while you work on its next version.
- Check what your extension can do before you publish it: install it and read the install dialog's Applications, Commands and Before you install lists.

## Changing how an application is read

- Rename a control, or change its role or states, with a node rule.
- Choose UI Automation or MSAA for a window with a window rule.
- Ignore a duplicate or spurious focus with a focus veto, and know whether it costs a read.
- Hear an application's notification when it is not in front, with an `accept` rule.
- Fix a control when the fix needs a read of its parent or children, with `on-node`.
- Provide the status bar.
- Apply a rule or a branch only on some Windows versions.

## Commands

- Add a command and its gesture, and handle a conflict with a gesture already in use.
- Give a gesture a different command on a double press.
- Bind one command to several keys and tell them apart with `trigger.declared`.
- Show a list dialog with your own buttons and act on the answer.

## Testing and debugging

- Test a module's decisions with unit tests, and its hooks with the test helper against a fake tree.
- Find out why a hook failed, from the log viewer.
- See what your hooks cost, in developer mode.
