//! The async form's logic: each entry's handling read top to bottom, its
//! requests awaited in line, each answered in its own type.

use std::future::Future;
use std::pin::Pin;

use super::super::harness::{COMMAND_DUE_US, Effect, FOCUS_DUE_US, WATCH_MS, Window, focus_speech};
use super::driver::{Cx, Entry};
use super::request::{
    Caret, CaretPosition, CompleteFocus, Completion, Element, ElementRead, Line, Outcome, Watch,
};

/// The function that handles `entry` at its turn.
pub(super) fn start(cx: Cx, entry: &Entry) -> Pin<Box<dyn Future<Output = ()>>> {
    match *entry {
        Entry::Focus { app, window, token } => Box::pin(focus(cx, app, window, token)),
        Entry::CaretKey {
            key, app, window, ..
        } => Box::pin(caret_key(cx, key, app, window)),
    }
}

/// A UI Automation focus: two requests chained under one due time, then
/// spoken.
async fn focus(mut cx: Cx, app: u32, window: Window, token: u32) {
    let due_us = cx.facts().now_us().saturating_add(FOCUS_DUE_US);
    let element = cx.ask(ElementRead { token }, window, due_us).await;
    let Outcome::Result(Element { name, route }) = element else {
        return;
    };
    let completion = cx.ask(CompleteFocus { app, route }, window, due_us).await;
    let Outcome::Result(Completion { role }) = completion else {
        return;
    };
    cx.emit(Effect::Speak(focus_speech(&name, &role)));
}

/// A caret key: its starting point, the key sent on, the watch, the line.
async fn caret_key(mut cx: Cx, key: u32, app: u32, window: Window) {
    if cx.facts().silent() {
        cx.emit(Effect::SendKey(key));
        return;
    }
    let due_us = cx.facts().now_us().saturating_add(COMMAND_DUE_US);
    let start = cx.ask(Caret { app }, window, due_us).await;
    cx.emit(Effect::SendKey(key));
    let Outcome::Result(CaretPosition) = start else {
        return;
    };
    if cx.facts().silent() || cx.facts().focus_waiting() {
        return;
    }
    let watch = Watch {
        app,
        within_ms: WATCH_MS,
    };
    if let Outcome::Result(Line { text }) = cx.ask(watch, window, due_us).await {
        cx.emit(Effect::Speak(text));
    }
}
