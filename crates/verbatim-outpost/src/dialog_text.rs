//! A dialog's own text, such as a message box's question, gathered from the
//! dialog's children as NVDA gathers it (`docs/nvda/object-model.md`, "A
//! dialog's own text"). Public, so mockapp's tests gather it as the worker
//! does.
//!
//! The rules are written once, over [`DialogObject`], which each backend
//! implements: MSAA through [`verbatim_ia2::dialog::DialogObject`], and UI
//! Automation through [`UiaObject`], whose properties come from one cache
//! request per container. The worker gathers the text for a dialog the
//! focus has newly entered and reports it as the dialog's description, as
//! NVDA does, so Core speaks it after the dialog's name, role, and states
//! and before the focused control.

use windows::Win32::UI::Accessibility::{IUIAutomationElement, UIA_PROPERTY_ID};

use verbatim_model::{Role, State, StateSet};
use verbatim_uia::Uia;
use verbatim_uia::map::{CachedUiaParts, snapshot_parts_from_cached_element};

/// The most objects one gathering looks at, so a dialog with an enormous
/// tree costs a bounded number of calls; a dialog that has more says
/// nothing rather than part of its text.
pub const MAX_OBJECTS: usize = 512;

/// One object a dialog's text is gathered from: the dialog or a
/// descendant. Each method may call into the application.
pub trait DialogObject: Sized {
    /// Its children, in order; none when they cannot be read.
    fn children(&self) -> Vec<Self>;
    /// Its role.
    fn role(&self) -> Role;
    /// Its states.
    fn states(&self) -> StateSet;
    /// Whether it is invisible.
    fn invisible(&self) -> bool;
    /// Its name, `None` when empty or blank.
    fn name(&self) -> Option<String>;
    /// Its value, `None` when empty or blank; an edit field's is its text.
    fn value(&self) -> Option<String>;
    /// Its description, `None` when empty.
    fn description(&self) -> Option<String>;
}

impl DialogObject for verbatim_ia2::dialog::DialogObject {
    fn children(&self) -> Vec<Self> {
        Self::children(self)
    }
    fn role(&self) -> Role {
        Self::role(self)
    }
    fn states(&self) -> StateSet {
        Self::states(self)
    }
    fn invisible(&self) -> bool {
        Self::invisible(self)
    }
    fn name(&self) -> Option<String> {
        Self::name(self)
    }
    fn value(&self) -> Option<String> {
        Self::value(self)
    }
    fn description(&self) -> Option<String> {
        Self::description(self)
    }
}

/// Whether a node of `role` is a dialog, whose description is its own text
/// when it has none of its own: a dialog, an alert, or a property page.
#[must_use]
pub fn is_dialog(role: Role) -> bool {
    matches!(role, Role::Dialog | Role::Alert | Role::PropertyPage)
}

/// Whether a child of `role` is a container the gathering goes into.
fn is_container(role: Role) -> bool {
    matches!(
        role,
        Role::Pane | Role::PropertyPage | Role::Window | Role::Group | Role::Unknown
    )
}

/// Whether a child of `role` and `states` gives text: a static text, a
/// link, or a read-only edit field that is not multi-line.
fn gives_text(role: Role, states: StateSet) -> bool {
    match role {
        Role::StaticText | Role::Link => true,
        Role::EditableText => {
            states.contains(State::ReadOnly) && !states.contains(State::Multiline)
        }
        _ => false,
    }
}

/// Whether a sibling of `role` is never labelled by the text before it.
fn is_never_labelled(role: Role) -> bool {
    matches!(
        role,
        Role::Graphic
            | Role::StaticText
            | Role::Separator
            | Role::Window
            | Role::Pane
            | Role::Button
    )
}

/// Why a gathering gave no text at all.
enum Stop {
    /// A dialog inside the dialog has the focus within it.
    Focused,
    /// It looked at [`MAX_OBJECTS`] objects.
    TooMany,
}

/// The text of `dialog`, its pieces joined by line breaks; `None` when it
/// has none, when a dialog inside it holds the focus, or when it is too big
/// to gather.
#[must_use]
pub fn dialog_text<O: DialogObject>(dialog: &O) -> Option<String> {
    let mut looked_at = 0;
    match gather(dialog, true, &mut looked_at) {
        Ok(pieces) if !pieces.is_empty() => Some(pieces.join("\n")),
        // No text, or a dialog inside holding the focus.
        Ok(_) | Err(Stop::Focused) => None,
        Err(Stop::TooMany) => {
            tracing::debug!("a dialog's text was not gathered: it has too many objects");
            None
        }
    }
}

/// The pieces of text inside `parent`. `allow_focused` is false inside a
/// dialog nested in the one being read, where a focused child ends the
/// gathering.
fn gather<O: DialogObject>(
    parent: &O,
    allow_focused: bool,
    looked_at: &mut usize,
) -> Result<Vec<String>, Stop> {
    let children = parent.children();
    *looked_at += children.len();
    if *looked_at > MAX_OBJECTS {
        return Err(Stop::TooMany);
    }
    let mut pieces = Vec::new();
    for (index, child) in children.iter().enumerate() {
        let states = child.states();
        if child.invisible() || states.contains(State::Disabled) {
            continue;
        }
        let role = child.role();
        if is_container(role) {
            pieces.extend(gather(child, !is_dialog(role), looked_at)?);
            continue;
        }
        if !allow_focused && states.contains(State::Focused) {
            return Err(Stop::Focused);
        }
        if !gives_text(role, states) {
            continue;
        }
        // A grouping's description, right after it or after its graphic.
        let before = |back: usize| index.checked_sub(back).map(|at| children[at].role());
        if before(1) == Some(Role::Group)
            || (before(1) == Some(Role::Graphic) && before(2) == Some(Role::Group))
        {
            continue;
        }
        let name = child.name();
        // The label of the control after it.
        if let (Some(name), Some(next)) = (name.as_deref(), children.get(index + 1))
            && !is_never_labelled(next.role())
            && next.name().as_deref() == Some(name)
        {
            continue;
        }
        pieces.extend(text_of(child, role, name));
    }
    Ok(pieces)
}

/// What one child that gives text gives: an edit field its name and its
/// text, anything else, or an edit field with no text, its name, value,
/// and description joined by spaces.
fn text_of<O: DialogObject>(child: &O, role: Role, name: Option<String>) -> Option<String> {
    let value = child.value();
    if role == Role::EditableText
        && let Some(text) = &value
    {
        return Some(match name {
            Some(name) => format!("{name}\n{text}"),
            None => text.clone(),
        });
    }
    let parts: Vec<String> = [name, value, child.description()]
        .into_iter()
        .flatten()
        .filter(|part| !part.trim().is_empty())
        .collect();
    (!parts.is_empty()).then(|| parts.join(" "))
}

/// A UI Automation element a dialog's text is gathered from, with the
/// properties read when its parent's children were.
pub struct UiaObject<'a> {
    uia: &'a Uia,
    properties: &'a [UIA_PROPERTY_ID],
    element: IUIAutomationElement,
    parts: CachedUiaParts,
}

impl<'a> UiaObject<'a> {
    /// The dialog `element`, whose children are read with `properties`
    /// cached, which must be enough for [`snapshot_parts_from_cached_element`]
    /// (`verbatim_uia::cached_properties`). `element`'s own cached
    /// properties give its role and states.
    #[must_use]
    pub fn new(
        uia: &'a Uia,
        properties: &'a [UIA_PROPERTY_ID],
        element: IUIAutomationElement,
    ) -> Self {
        let parts = snapshot_parts_from_cached_element(&element);
        Self {
            uia,
            properties,
            element,
            parts,
        }
    }
}

impl DialogObject for UiaObject<'_> {
    /// One call: the element's children with their properties cached.
    fn children(&self) -> Vec<Self> {
        self.uia
            .children_with(&self.element, self.properties)
            .unwrap_or_default()
            .into_iter()
            .map(|element| Self::new(self.uia, self.properties, element))
            .collect()
    }
    fn role(&self) -> Role {
        self.parts.role
    }
    fn states(&self) -> StateSet {
        self.parts.states
    }
    /// UI Automation has no invisible state; an element off the screen is
    /// still read, as NVDA reads it.
    fn invisible(&self) -> bool {
        false
    }
    fn name(&self) -> Option<String> {
        visible(self.parts.name.clone())
    }
    fn value(&self) -> Option<String> {
        visible(self.parts.value.clone())
    }
    fn description(&self) -> Option<String> {
        self.parts
            .details
            .description
            .clone()
            .filter(|description| !description.is_empty())
    }
}

/// `text`, unless it is empty or blank.
fn visible(text: Option<String>) -> Option<String> {
    text.filter(|text| !text.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scripted object: its role, states, name, value, and children.
    #[derive(Clone, Default)]
    struct Fake {
        role: Option<Role>,
        states: StateSet,
        invisible: bool,
        name: Option<&'static str>,
        value: Option<&'static str>,
        children: Vec<Fake>,
    }

    impl DialogObject for Fake {
        fn children(&self) -> Vec<Self> {
            self.children.clone()
        }
        fn role(&self) -> Role {
            self.role.unwrap_or(Role::Unknown)
        }
        fn states(&self) -> StateSet {
            self.states
        }
        fn invisible(&self) -> bool {
            self.invisible
        }
        fn name(&self) -> Option<String> {
            self.name.map(str::to_owned)
        }
        fn value(&self) -> Option<String> {
            self.value.map(str::to_owned)
        }
        fn description(&self) -> Option<String> {
            None
        }
    }

    fn object(role: Role, name: &'static str) -> Fake {
        Fake {
            role: Some(role),
            name: Some(name),
            ..Fake::default()
        }
    }

    fn with(mut fake: Fake, state: State) -> Fake {
        fake.states.insert(state);
        fake
    }

    fn dialog(children: Vec<Fake>) -> Fake {
        Fake {
            role: Some(Role::Dialog),
            children,
            ..Fake::default()
        }
    }

    #[test]
    fn a_message_box_reads_its_question_and_not_its_buttons() {
        let message_box = dialog(vec![
            object(Role::Graphic, "Warning"),
            object(Role::StaticText, "Remove the theme Mine?"),
            with(object(Role::Button, "Yes"), State::Focused),
            object(Role::Button, "No"),
        ]);
        assert_eq!(
            dialog_text(&message_box).as_deref(),
            Some("Remove the theme Mine?")
        );
    }

    #[test]
    fn texts_in_panes_are_read_in_order_and_hidden_ones_skipped() {
        let mut hidden = object(Role::StaticText, "Hidden");
        hidden.invisible = true;
        let message_box = dialog(vec![
            Fake {
                role: Some(Role::Pane),
                children: vec![object(Role::StaticText, "First"), hidden],
                ..Fake::default()
            },
            with(object(Role::StaticText, "Greyed"), State::Disabled),
            object(Role::Link, "Second"),
        ]);
        assert_eq!(dialog_text(&message_box).as_deref(), Some("First\nSecond"));
    }

    #[test]
    fn labels_and_group_descriptions_are_not_read() {
        let settings = dialog(vec![
            object(Role::StaticText, "Rate"),
            object(Role::Slider, "Rate"),
            object(Role::Group, "Voice"),
            object(Role::StaticText, "About the voice"),
            object(Role::StaticText, "Kept"),
            object(Role::Button, "Kept"),
        ]);
        assert_eq!(dialog_text(&settings).as_deref(), Some("Kept"));
    }

    #[test]
    fn a_read_only_field_gives_its_name_and_text_and_an_editable_one_nothing() {
        let mut path = with(object(Role::EditableText, "Path"), State::ReadOnly);
        path.value = Some("C:\\Themes");
        let mut notes = with(object(Role::EditableText, "Notes"), State::ReadOnly);
        notes.states.insert(State::Multiline);
        notes.value = Some("long");
        let mut name = object(Role::EditableText, "Name");
        name.value = Some("typed");
        let found = dialog(vec![path, notes, name]);
        assert_eq!(dialog_text(&found).as_deref(), Some("Path\nC:\\Themes"));
    }

    #[test]
    fn a_property_page_holding_the_focus_silences_the_dialog() {
        let page = Fake {
            role: Some(Role::PropertyPage),
            children: vec![
                object(Role::StaticText, "Page text"),
                with(object(Role::CheckBox, "Option"), State::Focused),
            ],
            ..Fake::default()
        };
        let properties = dialog(vec![object(Role::StaticText, "Dialog text"), page]);
        assert_eq!(dialog_text(&properties), None);
        // The page itself reads its own text.
        let page = &properties.children[1];
        assert_eq!(dialog_text(page).as_deref(), Some("Page text"));
    }

    #[test]
    fn a_dialog_with_too_many_objects_says_nothing() {
        let crowd = dialog(vec![object(Role::StaticText, "Line"); MAX_OBJECTS + 1]);
        assert_eq!(dialog_text(&crowd), None);
    }
}
