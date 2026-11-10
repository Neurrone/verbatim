//! The JSON Lines format: the header, event lines and the `behind` and
//! `registered` lines, each field in the contract's order.

use serde_json::{Value, json};

/// The format version a recording's header gives and [`crate::print`]
/// requires.
pub(crate) const FORMAT_VERSION: u32 = 1;

/// What one event's line holds, every field written, null when it has no
/// value.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct EventData {
    /// `uia_focus`, `msaa_focus` and so on.
    pub kind: &'static str,
    /// Whether the event is the target's.
    pub in_target: bool,
    /// The process.
    pub pid: Option<u32>,
    /// The window handle.
    pub window: Option<isize>,
    /// The runtime id.
    pub runtime_id: Option<Vec<i32>>,
    /// The name.
    pub name: Option<String>,
    /// The control type's name.
    pub control_type: Option<String>,
    /// The class.
    pub class: Option<String>,
    /// The automation id.
    pub automation_id: Option<String>,
    /// The framework.
    pub framework: Option<String>,
    /// A property change's property, or an automation event's name.
    pub property: Option<String>,
    /// A property change's new value, a structure change's type, or a
    /// notification's text.
    pub value: Option<Value>,
    /// Whether the element was the focus.
    pub is_focus: Option<bool>,
    /// An MSAA event's raw event id.
    pub event: Option<u32>,
    /// An MSAA event's `idObject`.
    pub object: Option<i32>,
    /// An MSAA event's `idChild`.
    pub child: Option<i32>,
}

/// A window handle as a line writes it.
pub(crate) fn window_text(window: isize) -> String {
    format!("0x{window:X}")
}

/// `fields`, in order, as one line of JSON.
fn object(fields: &[(&str, Value)]) -> String {
    let parts: Vec<String> = fields
        .iter()
        .map(|(name, value)| format!("{}:{value}", Value::from(*name)))
        .collect();
    format!("{{{}}}", parts.join(","))
}

/// The value of an optional field, null when it has none.
fn optional<T: Into<Value>>(value: Option<T>) -> Value {
    value.map_or(Value::Null, Into::into)
}

/// The header line.
pub(crate) fn header(started_utc: &str, windows_build: &str, target: &Value) -> String {
    object(&[
        ("format_version", json!(FORMAT_VERSION)),
        ("started_utc", json!(started_utc)),
        ("windows_build", json!(windows_build)),
        ("target", target.clone()),
    ])
}

/// An event's line, `t_us` first.
pub(crate) fn event_line(t_us: u64, data: &EventData) -> String {
    let runtime_id = data
        .runtime_id
        .as_ref()
        .map(|id| id.iter().map(i32::to_string).collect::<Vec<_>>().join("."));
    object(&[
        ("t_us", json!(t_us)),
        ("kind", json!(data.kind)),
        ("in_target", json!(data.in_target)),
        ("pid", optional(data.pid)),
        ("window", optional(data.window.map(window_text))),
        ("runtime_id", optional(runtime_id)),
        ("name", optional(data.name.clone())),
        ("control_type", optional(data.control_type.clone())),
        ("class", optional(data.class.clone())),
        ("automation_id", optional(data.automation_id.clone())),
        ("framework", optional(data.framework.clone())),
        ("property", optional(data.property.clone())),
        ("value", data.value.clone().unwrap_or(Value::Null)),
        ("is_focus", optional(data.is_focus)),
        ("event", optional(data.event)),
        ("object", optional(data.object)),
        ("child", optional(data.child)),
    ])
}

/// A line that delivery fell behind: `source` `msaa` when the event
/// arrived late after its Windows timestamp, `uia_focus` when a UI
/// Automation focus arrived late after its MSAA focus.
pub(crate) fn behind_line(t_us: u64, source: &str, late_ms: u64) -> String {
    object(&[
        ("t_us", json!(t_us)),
        ("kind", json!("behind")),
        ("source", json!(source)),
        ("late_ms", json!(late_ms)),
    ])
}

/// A line that `window`'s registration has completed.
pub(crate) fn registered_line(t_us: u64, window: isize) -> String {
    object(&[
        ("t_us", json!(t_us)),
        ("kind", json!("registered")),
        ("window", json!(window_text(window))),
    ])
}
