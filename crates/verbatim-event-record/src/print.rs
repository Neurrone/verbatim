//! `print`: a recording as sentences, reading only the file.

use std::path::Path;

use serde_json::Value;

use crate::line::FORMAT_VERSION;

/// `file` as sentences, one per line after the header.
///
/// # Errors
///
/// Returns a message if the file cannot be read, is no recording, or has
/// another format version.
pub(crate) fn print(file: &Path) -> Result<String, String> {
    let text = std::fs::read_to_string(file)
        .map_err(|error| format!("could not read {}: {error}", file.display()))?;
    sentences(&text)
}

/// The sentences of a recording's text.
fn sentences(text: &str) -> Result<String, String> {
    let mut lines = text.lines();
    let header = parse(lines.next().ok_or("the recording is empty")?)?;
    match header.get("format_version").and_then(Value::as_u64) {
        Some(version) if version == u64::from(FORMAT_VERSION) => {}
        Some(version) => {
            return Err(format!(
                "this recording has format version {version}; this program reads version {FORMAT_VERSION}"
            ));
        }
        None => return Err("the recording has no format version".to_owned()),
    }
    let mut out = String::new();
    for line in lines {
        out.push_str(&sentence(&parse(line)?));
        out.push('\n');
    }
    Ok(out)
}

fn parse(line: &str) -> Result<Value, String> {
    serde_json::from_str(line).map_err(|error| format!("a line is not JSON: {error}"))
}

/// A line's text field.
fn text<'a>(line: &'a Value, name: &str) -> Option<&'a str> {
    line.get(name).and_then(Value::as_str)
}

/// `At 0.048 seconds`, for the line's arrival.
fn at(line: &Value) -> String {
    let t_us = line.get("t_us").and_then(Value::as_u64).unwrap_or(0);
    format!(
        "At {}.{:03} seconds",
        t_us / 1_000_000,
        t_us % 1_000_000 / 1000
    )
}

/// One line as a sentence.
fn sentence(line: &Value) -> String {
    let kind = text(line, "kind").unwrap_or("");
    let at = at(line);
    match kind {
        "behind" => {
            let late = line.get("late_ms").and_then(Value::as_u64).unwrap_or(0);
            if text(line, "source") == Some("msaa") {
                format!("{at}, an MSAA event arrived {late} milliseconds behind.")
            } else {
                format!("{at}, UI Automation focus arrived {late} milliseconds behind.")
            }
        }
        "registered" => format!(
            "{at}, the registration of window {} completed.",
            text(line, "window").unwrap_or("unknown")
        ),
        "uia_focus" => format!("{at}, UI Automation focus{}.", subject(line)),
        "uia_property" => {
            let property = text(line, "property").unwrap_or("unknown");
            let value = value(line);
            format!(
                "{at}, UI Automation property {property} changed to {value}{}.",
                subject(line)
            )
        }
        "uia_structure" => {
            format!(
                "{at}, UI Automation structure changed, {}{}.",
                value(line),
                subject(line)
            )
        }
        "uia_event" => format!(
            "{at}, UI Automation event {}{}.",
            text(line, "property").unwrap_or("unknown"),
            subject(line)
        ),
        _ if kind.starts_with("msaa_") => msaa(&at, kind, line),
        _ => format!("{at}, {kind}."),
    }
}

/// An MSAA line as a sentence: its window, its class and its object.
fn msaa(at: &str, kind: &str, line: &Value) -> String {
    let event = kind.trim_start_matches("msaa_");
    let window = text(line, "window").unwrap_or("no window");
    let class = text(line, "class").map_or(String::new(), |class| format!(", class {class}"));
    let number = |name: &str| line.get(name).and_then(Value::as_i64).unwrap_or(0);
    format!(
        "{at}, MSAA {event} on window {window}{class}, object {}, child {}, event {}{}{}.",
        number("object"),
        number("child"),
        number("event"),
        process(line),
        outside(line)
    )
}

/// `, on Document, Text editor, in process 4120`, for a UI Automation line.
fn subject(line: &Value) -> String {
    let control = text(line, "control_type");
    let name = text(line, "name");
    let on = match (control, name) {
        (Some(control), Some(name)) => format!(" on {control}, {name}"),
        (Some(control), None) => format!(" on {control}"),
        (None, Some(name)) => format!(" on {name}"),
        (None, None) => String::new(),
    };
    format!("{on}{}{}", process(line), outside(line))
}

/// `, in process 4120`, when the line names one.
fn process(line: &Value) -> String {
    line.get("pid")
        .and_then(Value::as_u64)
        .map_or(String::new(), |pid| format!(", in process {pid}"))
}

/// `, outside the target`, for a line of another application.
fn outside(line: &Value) -> &'static str {
    if line.get("in_target").and_then(Value::as_bool) == Some(false) {
        ", outside the target"
    } else {
        ""
    }
}

/// A line's `value` as a sentence says it.
fn value(line: &Value) -> String {
    match line.get("value") {
        Some(Value::String(text)) => text.clone(),
        None | Some(Value::Null) => "nothing".to_owned(),
        Some(other) => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = r#"{"format_version":1,"started_utc":"2026-11-10T15:30:00Z","windows_build":"26300.1","target":{"pid":4120,"window":null}}"#;

    #[test]
    fn each_kind_of_line_reads_as_its_sentence() {
        let lines = [
            r#"{"t_us":48211,"kind":"uia_focus","in_target":true,"pid":4120,"window":"0x3A0F2","runtime_id":"42.238","name":"Text editor","control_type":"Document","class":"RichEditD2DPT","automation_id":null,"framework":"Win32","property":null,"value":null,"is_focus":true,"event":null,"object":null,"child":null}"#,
            r#"{"t_us":51004,"kind":"uia_property","in_target":true,"pid":4120,"window":null,"runtime_id":null,"name":"Text editor","control_type":"Document","class":null,"automation_id":null,"framework":null,"property":"Name","value":"Untitled","is_focus":false,"event":null,"object":null,"child":null}"#,
            r#"{"t_us":60000,"kind":"uia_structure","in_target":true,"pid":4120,"window":null,"runtime_id":null,"name":null,"control_type":"Pane","class":null,"automation_id":null,"framework":null,"property":null,"value":"ChildAdded","is_focus":false,"event":null,"object":null,"child":null}"#,
            r#"{"t_us":70000,"kind":"uia_event","in_target":true,"pid":4120,"window":null,"runtime_id":null,"name":"File","control_type":"Menu","class":null,"automation_id":null,"framework":null,"property":"MenuOpened","value":null,"is_focus":false,"event":null,"object":null,"child":null}"#,
            r#"{"t_us":730112,"kind":"msaa_focus","in_target":false,"pid":812,"window":"0x1A","runtime_id":null,"name":null,"control_type":null,"class":"Edit","automation_id":null,"framework":null,"property":null,"value":null,"is_focus":null,"event":32773,"object":-4,"child":0}"#,
            r#"{"t_us":1204000,"kind":"behind","source":"uia_focus","late_ms":682}"#,
            r#"{"t_us":1300000,"kind":"behind","source":"msaa","late_ms":150}"#,
            r#"{"t_us":1400000,"kind":"registered","window":"0x3A0F2"}"#,
        ];
        let recording = format!("{HEADER}\n{}\n", lines.join("\n"));
        assert_eq!(
            sentences(&recording).expect("a recording"),
            "At 0.048 seconds, UI Automation focus on Document, Text editor, in process 4120.\n\
             At 0.051 seconds, UI Automation property Name changed to Untitled on Document, Text editor, in process 4120.\n\
             At 0.060 seconds, UI Automation structure changed, ChildAdded on Pane, in process 4120.\n\
             At 0.070 seconds, UI Automation event MenuOpened on Menu, File, in process 4120.\n\
             At 0.730 seconds, MSAA focus on window 0x1A, class Edit, object -4, child 0, event 32773, in process 812, outside the target.\n\
             At 1.204 seconds, UI Automation focus arrived 682 milliseconds behind.\n\
             At 1.300 seconds, an MSAA event arrived 150 milliseconds behind.\n\
             At 1.400 seconds, the registration of window 0x3A0F2 completed.\n"
        );
    }

    #[test]
    fn another_format_version_is_refused() {
        let recording = r#"{"format_version":2,"started_utc":"x","windows_build":"y","target":{}}"#;
        assert_eq!(
            sentences(recording),
            Err("this recording has format version 2; this program reads version 1".to_owned())
        );
    }
}
