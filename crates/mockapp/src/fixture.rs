//! Fixture parsing: the JSON tree format scripted trees are authored in.
//!
//! A fixture is one JSON object per node: `id` (unique string), `role` (a
//! [`Role`] name in snake case, e.g. `check_box`), optional `name` and
//! `value` strings, `states` (an array of [`State`] names in snake case,
//! e.g. `read_only`), optional detail properties — `description` and
//! `keyboard_shortcut` strings, and one-based `position_in_set`,
//! `set_size`, and `level` integers (the M3
//! [`NodeDetails`](verbatim_model::NodeDetails) vocabulary; each backend
//! serves the subset its API can express), an optional `default_action`
//! string (MSAA's `accDefaultAction`, which `accDoDefaultAction` then
//! performs), an optional `controller_for`
//! (the `id` of the node this one controls, served as UIA's
//! `ControllerFor` relation, as a search box names its suggestion list) —
//! and `children` (an array of nested nodes). The root node conceptually corresponds to the host
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
    /// A `controller_for` named an `id` no node declares.
    UnknownControlled { id: String, controlled: String },
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
            Self::UnknownControlled { id, controlled } => {
                write!(f, "node {id:?} controls unknown node {controlled:?}")
            }
        }
    }
}

impl std::error::Error for FixtureError {}

/// The raw JSON shape, deserialized before role and state names are
/// validated against the normalized vocabulary.
#[derive(Deserialize)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag is an independent fixture option"
)]
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
    default_action: Option<String>,
    #[serde(default)]
    position_in_set: Option<u32>,
    #[serde(default)]
    set_size: Option<u32>,
    #[serde(default)]
    level: Option<u32>,
    #[serde(default)]
    controller_for: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    spelling_errors: Vec<(usize, usize)>,
    #[serde(default)]
    bold: Vec<(usize, usize)>,
    #[serde(default)]
    italic: Vec<(usize, usize)>,
    #[serde(default)]
    italic_fails: bool,
    #[serde(default)]
    find_text_fails: bool,
    #[serde(default)]
    cultures: Vec<(usize, usize, i32)>,
    /// Boxed, so a node nested sixty deep still parses on the main
    /// thread's stack.
    #[serde(default)]
    styles: Box<Styles>,
    #[serde(default)]
    backward_moves_positive: bool,
    #[serde(default)]
    native: Option<String>,
    #[serde(default)]
    window_class: Option<String>,
    #[serde(default)]
    class_name: Option<String>,
    #[serde(default)]
    columns: Vec<(String, i32)>,
    #[serde(default)]
    state_images: bool,
    #[serde(default)]
    edit_version_6: bool,
    #[serde(default)]
    children: Vec<RawNode>,
}

/// A text's other formatting, each a list of stretches, as UTF-16 offsets
/// from a start up to an end, with the value UIA's attribute has there.
/// An attribute whose list is absent is not supported: UIA's "not
/// supported" answers it everywhere, as Windows Terminal answers the
/// attributes it lacks. One whose list is present is supported, with its
/// default value outside the stretches listed. The format unit ends
/// wherever one of these stretches starts or ends.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Styles {
    /// The font size in points, 11 elsewhere; supported always.
    #[serde(default)]
    pub(crate) font_sizes: Vec<(usize, usize, f64)>,
    /// The underline style, a `TextDecorationLineStyle`, 0 elsewhere;
    /// supported always.
    #[serde(default)]
    pub(crate) underlines: Vec<(usize, usize, i32)>,
    /// The strikethrough style, 0 elsewhere.
    #[serde(default)]
    pub(crate) strikethroughs: Option<Vec<(usize, usize, i32)>>,
    /// The background color, a `COLORREF`, white (0xFFFFFF) elsewhere.
    #[serde(default)]
    pub(crate) backgrounds: Option<Vec<(usize, usize, i32)>>,
    /// The bullet style, 0 elsewhere.
    #[serde(default)]
    pub(crate) bullets: Option<Vec<(usize, usize, i32)>>,
    /// Links, each served as its own range; null elsewhere.
    #[serde(default)]
    pub(crate) links: Option<Vec<(usize, usize)>>,
}

/// One parsed, validated fixture node, still shaped as a tree (not yet an
/// arena); [`crate::tree::Tree::build`] flattens it.
#[derive(Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag is an independent fixture option"
)]
pub(crate) struct FixtureNode {
    pub(crate) id: String,
    pub(crate) role: Role,
    pub(crate) name: Option<String>,
    pub(crate) value: Option<String>,
    pub(crate) states: StateSet,
    pub(crate) description: Option<String>,
    pub(crate) keyboard_shortcut: Option<String>,
    pub(crate) default_action: Option<String>,
    pub(crate) position_in_set: Option<u32>,
    pub(crate) set_size: Option<u32>,
    pub(crate) level: Option<u32>,
    /// The `id` of the node this one controls (UIA `ControllerFor`).
    pub(crate) controller_for: Option<String>,
    /// The node's text, served through UIA's text pattern, or for the MSAA
    /// backend by a real Win32 edit control (milestone M4).
    pub(crate) text: Option<String>,
    /// Stretches of the text that are spelling errors, as UTF-16 offsets
    /// from a start up to an end, served as UIA's annotation types.
    pub(crate) spelling_errors: Vec<(usize, usize)>,
    /// Stretches of the text in bold, served as UIA's font weight.
    pub(crate) bold: Vec<(usize, usize)>,
    /// Stretches of the text in italics, served as UIA's `IsItalic`; the
    /// format unit does not end at them, as Windows Terminal's and the
    /// console host's do not, so a stretch of it reads as mixed.
    pub(crate) italic: Vec<(usize, usize)>,
    /// Whether reading the text's `IsItalic` attribute fails, as a provider
    /// that fails an attribute read does.
    pub(crate) italic_fails: bool,
    /// Whether `FindText` on the text fails, as Windows Terminal's has
    /// thrown.
    pub(crate) find_text_fails: bool,
    /// Stretches of the text in a language other than English, each a
    /// start and an end UTF-16 offset and a Windows locale id, served as
    /// UIA's `Culture` attribute.
    pub(crate) cultures: Vec<(usize, usize, i32)>,
    /// The text's other formatting ([`Styles`]), boxed as in the JSON.
    pub(crate) styles: Box<Styles>,
    /// Whether a backward `Move` or `MoveEndpointByUnit` answers with a
    /// positive count, as some providers do.
    pub(crate) backward_moves_positive: bool,
    /// The real control the node becomes for the MSAA backend instead of a
    /// scripted node: `tree_view` for a comctl32 tree view
    /// ([`crate::tree_view`]), `list_view` for a comctl32 list view in the
    /// report view ([`crate::list_view`]), `group_box` for a group box with its child
    /// buttons inside it, or `button` for a push button outside it
    /// ([`crate::buttons`]).
    pub(crate) native: Option<String>,
    /// The window class a real control is registered under, a superclass
    /// of the comctl32 one, as Windows Forms names its controls.
    pub(crate) window_class: Option<String>,
    /// The UIA class name the node reports (`TermControl` for a terminal
    /// as Windows Terminal's control is known).
    pub(crate) class_name: Option<String>,
    /// A real list view's columns, each its header and width in pixels.
    pub(crate) columns: Vec<(String, i32)>,
    /// Whether a real tree view gives its items state images, as
    /// applications that draw their own check boxes do.
    pub(crate) state_images: bool,
    /// Whether, on the MSAA backend, the edit control holding the node's
    /// text is Common Controls version 6's, as a Windows Forms text box is,
    /// rather than the classic one.
    pub(crate) edit_version_6: bool,
    pub(crate) children: Vec<FixtureNode>,
}

impl FixtureNode {
    /// Takes the nodes that become real controls out of the tree, depth
    /// first: they are created as child windows rather than served as
    /// scripted nodes.
    pub(crate) fn take_native(&mut self) -> Vec<FixtureNode> {
        let mut taken = Vec::new();
        let children = std::mem::take(&mut self.children);
        for mut child in children {
            if child.native.is_some() {
                taken.push(child);
            } else {
                taken.extend(child.take_native());
                self.children.push(child);
            }
        }
        taken
    }

    /// Whether the first node with text, depth first, the one the MSAA
    /// backend's edit control holds, asks for Common Controls version 6's
    /// edit control.
    pub(crate) fn edit_version_6(&self) -> bool {
        self.first_text().is_some_and(|node| node.edit_version_6)
    }

    /// The first node with text, depth first.
    fn first_text(&self) -> Option<&FixtureNode> {
        if self.text.is_some() {
            return Some(self);
        }
        self.children.iter().find_map(FixtureNode::first_text)
    }
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
    let root = convert(raw, &mut seen_ids)?;
    check_controlled(&root, &seen_ids)?;
    Ok(root)
}

/// Checks that every `controller_for` names a node the fixture declares.
fn check_controlled(
    node: &FixtureNode,
    ids: &std::collections::HashSet<String>,
) -> Result<(), FixtureError> {
    if let Some(controlled) = node.controller_for.as_ref().filter(|id| !ids.contains(*id)) {
        return Err(FixtureError::UnknownControlled {
            id: node.id.clone(),
            controlled: controlled.clone(),
        });
    }
    node.children
        .iter()
        .try_for_each(|child| check_controlled(child, ids))
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
        default_action: raw.default_action,
        position_in_set: raw.position_in_set,
        set_size: raw.set_size,
        level: raw.level,
        controller_for: raw.controller_for,
        text: raw.text,
        spelling_errors: raw.spelling_errors,
        bold: raw.bold,
        italic: raw.italic,
        italic_fails: raw.italic_fails,
        find_text_fails: raw.find_text_fails,
        cultures: raw.cultures,
        styles: raw.styles,
        backward_moves_positive: raw.backward_moves_positive,
        native: raw.native,
        window_class: raw.window_class,
        class_name: raw.class_name,
        columns: raw.columns,
        state_images: raw.state_images,
        edit_version_6: raw.edit_version_6,
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
        "progress_bar" => Role::ProgressBar,
        "tool_tip" => Role::ToolTip,
        "help_balloon" => Role::HelpBalloon,
        "unknown" => Role::Unknown,
        _ => return None,
    })
}

/// Maps a fixture's snake-case state name to a [`State`]. `None` for any
/// name outside the vocabulary a fixture author might use.
pub(crate) fn state_from_fixture_str(name: &str) -> Option<State> {
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
        "protected" => State::Protected,
        "required" => State::Required,
        "invalid_entry" => State::InvalidEntry,
        "checkable" => State::Checkable,
        "offscreen" => State::Offscreen,
        "busy" => State::Busy,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_role_name_maps_to_its_role() {
        for (name, expected) in [
            ("window", Role::Window),
            ("dialog", Role::Dialog),
            ("pane", Role::Pane),
            ("property_page", Role::PropertyPage),
            ("group", Role::Group),
            ("menu_bar", Role::MenuBar),
            ("menu", Role::Menu),
            ("menu_item", Role::MenuItem),
            ("button", Role::Button),
            ("toggle_button", Role::ToggleButton),
            ("check_box", Role::CheckBox),
            ("radio_button", Role::RadioButton),
            ("combo_box", Role::ComboBox),
            ("list", Role::List),
            ("list_item", Role::ListItem),
            ("slider", Role::Slider),
            ("spin_button", Role::SpinButton),
            ("tab_control", Role::TabControl),
            ("tab", Role::Tab),
            ("static_text", Role::StaticText),
            ("editable_text", Role::EditableText),
            ("link", Role::Link),
            ("tool_bar", Role::ToolBar),
            ("status_bar", Role::StatusBar),
            ("tree", Role::Tree),
            ("tree_item", Role::TreeItem),
            ("progress_bar", Role::ProgressBar),
            ("tool_tip", Role::ToolTip),
            ("help_balloon", Role::HelpBalloon),
            ("unknown", Role::Unknown),
        ] {
            assert_eq!(
                role_from_fixture_str(name),
                Some(expected),
                "role name {name:?}"
            );
        }
        assert_eq!(role_from_fixture_str("no_such_name"), None);
    }

    #[test]
    fn every_state_name_maps_to_its_state() {
        for (name, expected) in [
            ("focused", State::Focused),
            ("focusable", State::Focusable),
            ("selected", State::Selected),
            ("selectable", State::Selectable),
            ("checked", State::Checked),
            ("mixed", State::Mixed),
            ("disabled", State::Disabled),
            ("read_only", State::ReadOnly),
            ("expanded", State::Expanded),
            ("collapsed", State::Collapsed),
            ("pressed", State::Pressed),
            ("has_popup", State::HasPopup),
            ("protected", State::Protected),
            ("required", State::Required),
            ("invalid_entry", State::InvalidEntry),
            ("checkable", State::Checkable),
            ("offscreen", State::Offscreen),
            ("busy", State::Busy),
        ] {
            assert_eq!(
                state_from_fixture_str(name),
                Some(expected),
                "state name {name:?}"
            );
        }
        assert_eq!(state_from_fixture_str("no_such_name"), None);
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
