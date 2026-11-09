# A global module with a list dialog

In this tutorial you write a global module: a command that works everywhere, reads another application, shows a list dialog, and clicks what the user chooses. It lists the buttons on the taskbar, the way Verbatim's own system tray list does, with a second command on a double press.

This tutorial is a draft written against the API designed for milestone M5; you can follow it once M5 and its guest SDK are built.

## Declare the module

```toml
kind = "global"
reads = ["explorer.exe"]
component = "taskbar.wasm"

[[commands]]
id = "list-tray"
gestures = ["verbatim+shift+f11"]
description = "list-tray-description"

[[commands]]
id = "list-taskbar"
gestures = [{ keys = "verbatim+shift+f11", presses = 2 }]
description = "list-taskbar-description"
```

- `kind = "global"` makes the commands work in every application.
- `reads` lets the module read File Explorer, which draws the taskbar, and nothing else. The install dialog shows "File Explorer: reads it, clicks in it".
- Two commands share the keys: one press lists the tray, two presses the taskbar. Because both are declared, Verbatim holds the tray's dialog for the double-press interval, so a double press shows only the taskbar.

## Read the taskbar

On Windows 11, the tray icons and the taskbar buttons are the children of one window inside the taskbar, read through MSAA:

```rust
use verbatim_ext::{Backend, Node, find_window};

fn taskbar_input_site() -> Option<Node> {
    find_window(
        "explorer.exe",
        &["Shell_TrayWnd", "Windows.UI.Input.InputSite.WindowClass"],
        Backend::Msaa,
    )
    .ok()
    .flatten()
}

/// The tray icons: every child after the first.
fn tray_items() -> Vec<Node> {
    let Some(site) = taskbar_input_site() else { return Vec::new() };
    site.children().unwrap_or_default().into_iter().skip(1).collect()
}

/// The taskbar's buttons: the first child's children, after the Start button.
fn taskbar_items() -> Vec<Node> {
    let Some(site) = taskbar_input_site() else { return Vec::new() };
    let Ok(Some(buttons)) = site.first_child() else { return Vec::new() };
    buttons.children().unwrap_or_default().into_iter().skip(1).collect()
}
```

## Show the list and click the choice

```rust
use std::cell::RefCell;
use verbatim_ext::{Button, ClickButton, CommandId, DialogAnswer, DialogId, ListDialog,
                   Message, Node, Trigger, click, show_list_dialog, speak};

thread_local! {
    static ITEMS: RefCell<Vec<Node>> = RefCell::new(Vec::new());
}

fn on_command(command: CommandId, _trigger: Trigger) {
    let (items, title) = match command.as_str() {
        "list-tray" => (tray_items(), "tray-title"),
        _ => (taskbar_items(), "taskbar-title"),
    };
    let names = items.iter().map(|item| item.properties().name().unwrap_or_default()).collect();
    ITEMS.with(|kept| *kept.borrow_mut() = items);
    show_list_dialog(&ListDialog {
        title: Message::id(title),
        label: Message::id("items-label"),
        items: names,
        buttons: vec![
            Button::new(Message::id("left-click"), false),
            Button::new(Message::id("left-double-click"), true),
            Button::new(Message::id("right-click"), false),
        ],
    });
}

fn on_list_dialog(_dialog: DialogId, answer: DialogAnswer) {
    let DialogAnswer::Chosen { button, item } = answer else { return };
    let kind = [ClickButton::Left, ClickButton::LeftDouble, ClickButton::Right][button];
    ITEMS.with(|kept| {
        if let Some(node) = kept.borrow().get(item) {
            if click(node, kind).is_err() {
                speak(Message::id("item-gone"));
            }
        }
    });
}
```

- The module keeps the nodes it listed, so the click reaches the very button the user chose, wherever it is now.
- `show_list_dialog` called again while your dialog is open replaces its contents, so a second press updates the same window.
- Verbatim adds Cancel itself.

## What you learned

- A global module runs for the whole session, reads only what `reads` names, and its commands work everywhere.
- Commands can be bound to a number of presses, and Verbatim keeps a double press from flashing the first dialog.
- A module's state between hooks is its own memory, such as the nodes it listed.

Next, [package it](packaging.md) so others can install it.
