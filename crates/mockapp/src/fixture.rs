//! Fixture parsing: the JSON tree format scripted trees are authored in.
//!
//! A fixture is one JSON object per node: `id` (unique string), `role` (a
//! [`Role`] name in snake case, e.g. `check_box`), optional `name` and
//! `value` strings, `states` (an array of [`State`] names in snake case,
//! e.g. `read_only`), optional detail properties — `description` and
//! `keyboard_shortcut` strings, and one-based `position_in_set`,
//! `set_size`, and `level` integers (the M3
//! [`NodeDetails`](verbatim_model::NodeDetails) vocabulary; each backend
//! serves the subset its API can express) — and `children` (an array of
//! nested nodes). The root node conceptually corresponds to the host
//! window itself.

use std::fmt;
use std::path::Path;

use serde::Deserialize;
use verbatim_model::{Role, State, StateSet};

/// Everything that can go wrong loading a fixture: the file, its JSON, or an
/// unrecognized role or state name.
#[derive(Debug)]
pub(crate) enum FixtureError {
    /// The fixture file could not be read.
    Io(std::io::Error),
    /// The fixture's contents were not valid JSON in the expected shape.
    Json(serde_json::Error),
    /// A `role` field did not match any known snake-case [`Role`] name.
    UnknownRole { id: String, role: String },
    /// A `states` entry did not match any known snake-case [`State`] name.
    UnknownState { id: String, state: String },
    /// Two nodes in the fixture declared the same `id`.
    DuplicateId(String),
}

impl fmt::Display for FixtureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "could not read fixture file: {error}"),
            Self::Json(error) => write!(f, "could not parse fixture JSON: {error}"),
            Self::UnknownRole { id, role } => {
                write!(f, "node {id:?} has unknown role {role:?}")
            }
            Self::UnknownState { id, state } => {
                write!(f, "node {id:?} has unknown state {state:?}")
            }
            Self::DuplicateId(id) => write!(f, "duplicate node id {id:?}"),
        }
    }
}

impl std::error::Error for FixtureError {}

/// The raw JSON shape, deserialized before role and state names are
/// validated against the normalized vocabulary.
#[derive(Deserialize)]
struct RawNode {
    id: String,
    role: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    states: Vec<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    keyboard_shortcut: Option<String>,
    #[serde(default)]
    position_in_set: Option<u32>,
    #[serde(default)]
    set_size: Option<u32>,
    #[serde(default)]
    level: Option<u32>,
    #[serde(default)]
    children: Vec<RawNode>,
}

/// One parsed, validated fixture node, still shaped as a tree (not yet an
/// arena); [`crate::tree::Tree::build`] flattens it.
#[derive(Debug)]
pub(crate) struct FixtureNode {
    pub(crate) id: String,
    pub(crate) role: Role,
    pub(crate) name: Option<String>,
    pub(crate) value: Option<String>,
    pub(crate) states: StateSet,
    pub(crate) description: Option<String>,
    pub(crate) keyboard_shortcut: Option<String>,
    pub(crate) position_in_set: Option<u32>,
    pub(crate) set_size: Option<u32>,
    pub(crate) level: Option<u32>,
    pub(crate) children: Vec<FixtureNode>,
}

/// Loads and validates a fixture file.
///
/// # Errors
///
/// Returns [`FixtureError`] if the file cannot be read, is not valid JSON in
/// the fixture shape, or names an unknown role, unknown state, or a
/// duplicate node id.
pub(crate) fn load(path: &Path) -> Result<FixtureNode, FixtureError> {
    let text = std::fs::read_to_string(path).map_err(FixtureError::Io)?;
    let raw: RawNode = serde_json::from_str(&text).map_err(FixtureError::Json)?;
    let mut seen_ids = std::collections::HashSet::new();
    convert(raw, &mut seen_ids)
}

fn convert(
    raw: RawNode,
    seen_ids: &mut std::collections::HashSet<String>,
) -> Result<FixtureNode, FixtureError> {
    if !seen_ids.insert(raw.id.clone()) {
        return Err(FixtureError::DuplicateId(raw.id));
    }
    let role = role_from_fixture_str(&raw.role).ok_or_else(|| FixtureError::UnknownRole {
        id: raw.id.clone(),
        role: raw.role.clone(),
    })?;
    let mut states = StateSet::new();
    for state_name in &raw.states {
        let state =
            state_from_fixture_str(state_name).ok_or_else(|| FixtureError::UnknownState {
                id: raw.id.clone(),
                state: state_name.clone(),
            })?;
        states.insert(state);
    }
    let mut children = Vec::with_capacity(raw.children.len());
    for child in raw.children {
        children.push(convert(child, seen_ids)?);
    }
    Ok(FixtureNode {
        id: raw.id,
        role,
        name: raw.name,
        value: raw.value,
        states,
        description: raw.description,
        keyboard_shortcut: raw.keyboard_shortcut,
        position_in_set: raw.position_in_set,
        set_size: raw.set_size,
        level: raw.level,
        children,
    })
}

/// Maps a fixture's snake-case role name to a [`Role`]. `None` for any name
/// outside the vocabulary a fixture author might use.
fn role_from_fixture_str(name: &str) -> Option<Role> {
    Some(match name {
        "window" => Role::Window,
        "dialog" => Role::Dialog,
        "pane" => Role::Pane,
        "property_page" => Role::PropertyPage,
        "group" => Role::Group,
        "menu_bar" => Role::MenuBar,
        "menu" => Role::Menu,
        "menu_item" => Role::MenuItem,
        "button" => Role::Button,
        "toggle_button" => Role::ToggleButton,
        "check_box" => Role::CheckBox,
        "radio_button" => Role::RadioButton,
        "combo_box" => Role::ComboBox,
        "list" => Role::List,
        "list_item" => Role::ListItem,
        "slider" => Role::Slider,
        "spin_button" => Role::SpinButton,
        "tab_control" => Role::TabControl,
        "tab" => Role::Tab,
        "static_text" => Role::StaticText,
        "editable_text" => Role::EditableText,
        "link" => Role::Link,
        "tool_bar" => Role::ToolBar,
        "status_bar" => Role::StatusBar,
        "tree" => Role::Tree,
        "tree_item" => Role::TreeItem,
        "unknown" => Role::Unknown,
        _ => return None,
    })
}

/// Maps a fixture's snake-case state name to a [`State`]. `None` for any
/// name outside the vocabulary a fixture author might use.
fn state_from_fixture_str(name: &str) -> Option<State> {
    Some(match name {
        "focused" => State::Focused,
        "focusable" => State::Focusable,
        "selected" => State::Selected,
        "selectable" => State::Selectable,
        "checked" => State::Checked,
        "mixed" => State::Mixed,
        "disabled" => State::Disabled,
        "read_only" => State::ReadOnly,
        "expanded" => State::Expanded,
        "collapsed" => State::Collapsed,
        "pressed" => State::Pressed,
        "has_popup" => State::HasPopup,
        "default_control" => State::DefaultControl,
        "offscreen" => State::Offscreen,
        "busy" => State::Busy,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_role_name_round_trips() {
        for name in [
            "window",
            "dialog",
            "pane",
            "property_page",
            "group",
            "menu_bar",
            "menu",
            "menu_item",
            "button",
            "toggle_button",
            "check_box",
            "radio_button",
            "combo_box",
            "list",
            "list_item",
            "slider",
            "spin_button",
            "tab_control",
            "tab",
            "static_text",
            "editable_text",
            "link",
            "tool_bar",
            "status_bar",
            "tree",
            "tree_item",
            "unknown",
        ] {
            assert!(
                role_from_fixture_str(name).is_some(),
                "role name {name:?} did not map to a Role"
            );
        }
    }

    #[test]
    fn every_state_name_round_trips() {
        for name in [
            "focused",
            "focusable",
            "selected",
            "selectable",
            "checked",
            "mixed",
            "disabled",
            "read_only",
            "expanded",
            "collapsed",
            "pressed",
            "has_popup",
            "default_control",
            "offscreen",
            "busy",
        ] {
            assert!(
                state_from_fixture_str(name).is_some(),
                "state name {name:?} did not map to a State"
            );
        }
    }

    #[test]
    fn unknown_role_and_state_are_none() {
        assert!(role_from_fixture_str("not_a_role").is_none());
        assert!(state_from_fixture_str("not_a_state").is_none());
    }

    #[test]
    fn duplicate_ids_are_rejected() {
        let raw: RawNode = serde_json::from_str(
            r#"{"id":"root","role":"window","children":[
                {"id":"root","role":"button"}
            ]}"#,
        )
        .unwrap();
        let mut seen = std::collections::HashSet::new();
        let error = convert(raw, &mut seen).unwrap_err();
        assert!(matches!(error, FixtureError::DuplicateId(id) if id == "root"));
    }
}
