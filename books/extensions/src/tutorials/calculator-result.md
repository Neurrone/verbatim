# A module with code: Calculator's result

In this tutorial you write a module with a hook. When Calculator shows a result, it raises a notification whose text can be cut short, for example when you paste a number. Your module reads the result from the display instead and speaks it, as NVDA's Calculator module does.

This tutorial is a draft written against the API designed for milestone M5; you can follow it once M5 and its guest SDK are built.

## What you need

- Rust, with the `wasm32-unknown-unknown` target.
- The Verbatim guest SDK crate.
- Verbatim running in developer mode.

## Declare the hook

Make a folder `calculator` in the developer folder, with this `module.toml`:

```toml
kind = "app"
applications = ["calculatorapp.exe", "calculator.exe"]
component = "calculator.wasm"

[[hooks]]
hook = "on-event"
kinds = ["notification"]
backend = "uia"
match = { activity-id = "DisplayUpdated" }
```

`applications` lists both names Calculator's executable has had. The hook is registered only for Calculator's `DisplayUpdated` notification, so Verbatim calls your code for nothing else.

## Write the hook

Make a library crate for the module, depending on the guest SDK, and implement `on-event`:

```rust
use verbatim_ext::{Event, Matcher, Message, Node, Verdict, cancel_speech, speak};

const RESULT_IDS: [&str; 2] = ["CalculatorResults", "CalculatorAlwaysOnTopResults"];

fn on_event(node: Node, _event: Event) -> Verdict {
    let Ok(Some(window)) = node.window() else {
        return Verdict::Pass;
    };
    let display = Matcher::uia_class_name("LandmarkTarget");
    let Ok(Some(results)) = window.find_descendant(&display, 8) else {
        return Verdict::Pass;
    };
    let Ok(children) = results.children() else {
        return Verdict::Pass;
    };
    for child in children {
        let properties = child.properties();
        if properties.automation_id().is_some_and(|id| RESULT_IDS.contains(&id)) {
            cancel_speech();
            speak(Message::text(properties.name().unwrap_or_default()));
            return Verdict::Drop;
        }
    }
    Verdict::Pass
}
```

- Every read can fail, for example if Calculator closed in the meantime. Passing the event on lets Verbatim speak the notification as it would without you.
- `cancel_speech` handles a paste, which raises two notifications: the second one's result replaces the first's.
- `Drop` stops Verbatim from also speaking the notification's own text.

## Build and try it

Build the crate for `wasm32-unknown-unknown`, turn it into a component, and copy `calculator.wasm` into the module folder beside `module.toml`. Verbatim says "Loaded calculator". In Calculator, type 12.34, copy it, paste it, and press Enter: Verbatim speaks the whole result.

## What you learned

- A hook is registered for exactly the events it handles.
- Reads return nodes, which you navigate as a tree, and every read can fail.
- A verdict decides whether Verbatim's own handling carries on.

Next, [a global module](global-module.md) works across applications, with commands and a dialog.
