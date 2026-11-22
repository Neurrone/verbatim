//! A dialog's own text, such as a message box's question, gathered from the
//! dialog's children as NVDA gathers it (`docs/nvda/object-model.md`, "A
//! dialog's own text").
//!
//! The rule is written once, over [`DialogParent`] and [`DialogObject`],
//! which each backend implements beside its own reads: MSAA in
//! `verbatim-ia2` (`dialog::DialogObject`), and UI Automation in
//! `verbatim-uia-rops` (`dialog::UiaDialog`, the dialog, of which nothing
//! is read but its children, and `dialog::UiaObject`, each child). The
//! outpost gathers the text for a dialog the focus has newly entered and
//! reports it as the dialog's description, as NVDA does, so Core speaks it
//! after the dialog's name, role, and states and before the focused
//! control.

use std::collections::BTreeSet;

use verbatim_model::{Role, State, StateSet};

/// The most objects one gathering fetches, every child counted as it is
/// fetched: a safety bound against a tree with no identity that loops,
/// far above any real dialog (the largest measured, 1,500 objects), where
/// NVDA gathers with no cap. A dialog that has more gives the text read up
/// to it (Dickson, 2026-11-21; `docs/design/security-model.md`, U2 and
/// U3).
pub const MAX_OBJECTS: usize = 65_536;

/// What identifies an object a gathering has visited, where it has an
/// identity: a UI Automation element's runtime id, or an MSAA object with
/// an address of its own, its window, object id and child id. An MSAA
/// object reached through its parent's children with no address has
/// none, and only the cap bounds a walk through such objects.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Visited {
    /// A UI Automation element's runtime id.
    Uia(Vec<i32>),
    /// An MSAA object's window, object id and child id.
    Msaa {
        /// Its window.
        window: isize,
        /// Its object id.
        object: i32,
        /// Its child id.
        child: i32,
    },
}

/// What a dialog's text is gathered under: the dialog, or a container in
/// it. Its one method may call into the application.
pub trait DialogParent {
    /// What its children are.
    type Child: DialogObject;
    /// Its first children, at most `room`, in order; none when they
    /// cannot be read, and none fetched at a `room` of 0.
    fn children(&self, room: usize) -> Vec<Self::Child>;
}

/// One object a dialog's text is gathered from: a descendant of the
/// dialog. Each method may call into the application.
pub trait DialogObject: DialogParent<Child = Self> + Sized {
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
    /// What identifies it, `None` for an object with no identity.
    fn visited(&self) -> Option<Visited>;
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

/// A dialog inside the dialog has the focus within it: the gathering
/// gives no text at all.
struct Focused;

/// How far a gathering has come: the objects fetched, the identities
/// visited, and whether it has ended, and why.
#[derive(Default)]
struct Walk {
    fetched: usize,
    visited: BTreeSet<Visited>,
    end: Option<End>,
}

/// Why a gathering ended before the tree did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum End {
    /// It fetched [`MAX_OBJECTS`]: the objects fetched still give their
    /// text, and nothing more is fetched.
    Cap,
    /// It met an object it visited before: the tree loops, and nothing
    /// after it is read.
    Repeat,
}

/// The text of `dialog`, its pieces joined by line breaks; `None` when it
/// has none, or when a dialog inside it holds the focus. A gathering that
/// reaches [`MAX_OBJECTS`], or an object it visited before, gives the text
/// read up to there.
#[must_use]
pub fn dialog_text<P: DialogParent>(dialog: &P) -> Option<String> {
    let mut walk = Walk::default();
    let pieces = gather(dialog, true, &mut walk).ok()?;
    if let Some(end) = walk.end {
        tracing::debug!(
            ?end,
            fetched = walk.fetched,
            "a dialog's text was gathered up to where its walk ended"
        );
    }
    (!pieces.is_empty()).then(|| pieces.join("\n"))
}

/// The pieces of text inside `parent`. `allow_focused` is false inside a
/// dialog nested in the one being read, where a focused child ends the
/// gathering.
fn gather<P: DialogParent>(
    parent: &P,
    allow_focused: bool,
    walk: &mut Walk,
) -> Result<Vec<String>, Focused> {
    if walk.end.is_some() {
        return Ok(Vec::new());
    }
    let room = MAX_OBJECTS.saturating_sub(walk.fetched);
    if room == 0 {
        walk.end = Some(End::Cap);
        return Ok(Vec::new());
    }
    let children = parent.children(room);
    walk.fetched = walk.fetched.saturating_add(children.len());
    let mut pieces = Vec::new();
    for (index, child) in children.iter().enumerate() {
        if let Some(visited) = child.visited()
            && !walk.visited.insert(visited)
        {
            walk.end = Some(End::Repeat);
        }
        if walk.end == Some(End::Repeat) {
            break;
        }
        let states = child.states();
        if child.invisible() || states.contains(State::Disabled) {
            continue;
        }
        let role = child.role();
        if is_container(role) {
            pieces.extend(gather(child, !is_dialog(role), walk)?);
            if walk.end == Some(End::Repeat) {
                break;
            }
            continue;
        }
        if !allow_focused && states.contains(State::Focused) {
            return Err(Focused);
        }
        if !gives_text(role, states) {
            continue;
        }
        // A grouping's description, right after it or after its graphic.
        let before = |back: usize| {
            index
                .checked_sub(back)
                .and_then(|at| children.get(at))
                .map(DialogObject::role)
        };
        if before(1) == Some(Role::Group)
            || (before(1) == Some(Role::Graphic) && before(2) == Some(Role::Group))
        {
            continue;
        }
        let name = child.name();
        // The label of the control after it.
        if let (Some(name), Some(next)) = (
            name.as_deref(),
            index.checked_add(1).and_then(|next| children.get(next)),
        ) && !is_never_labelled(next.role())
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
        id: Option<i32>,
        /// The rooms its children were asked for in.
        asked: std::rc::Rc<std::cell::RefCell<Vec<usize>>>,
    }

    impl DialogParent for Fake {
        type Child = Self;
        fn children(&self, room: usize) -> Vec<Self> {
            self.asked.borrow_mut().push(room);
            self.children.iter().take(room).cloned().collect()
        }
    }

    impl DialogObject for Fake {
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
        fn visited(&self) -> Option<Visited> {
            self.id.map(|id| Visited::Uia(vec![id]))
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

    /// A dialog of 600 objects, past the 512 at which Verbatim once said
    /// nothing, gives its whole text, as NVDA, which gathers with no cap.
    /// Source-derived, approved by Dickson on 2026-11-21
    /// (`nvda/source/NVDAObjects/behaviors.py` 93 to 188).
    #[test]
    fn a_dialog_of_600_objects_gives_its_whole_text() {
        let lines = dialog(vec![object(Role::StaticText, "Line"); 600]);
        assert_eq!(
            dialog_text(&lines),
            Some(vec!["Line"; 600].join(
                "
"
            ))
        );
    }

    /// A dialog of one object more than the cap gives the text of the
    /// first 65,536, the text read up to the cap (Dickson, 2026-11-21); no
    /// parent is asked for more children than the room left, and none is
    /// asked once the room is gone.
    #[test]
    fn a_dialog_past_the_cap_gives_the_text_read_up_to_it() {
        let mut children = vec![object(Role::StaticText, "Line"); MAX_OBJECTS - 3];
        let group = Fake {
            role: Some(Role::Group),
            children: vec![object(Role::StaticText, "Line"); 3],
            ..Fake::default()
        };
        let after = Fake {
            role: Some(Role::Group),
            children: vec![object(Role::StaticText, "Past the cap")],
            ..Fake::default()
        };
        children.push(group.clone());
        children.push(after.clone());
        let crowd = dialog(children);
        assert_eq!(
            dialog_text(&crowd),
            Some(vec!["Line"; MAX_OBJECTS - 2].join(
                "
"
            ))
        );
        assert_eq!(*crowd.asked.borrow(), [MAX_OBJECTS]);
        assert_eq!(*group.asked.borrow(), [1], "the room left");
        assert_eq!(*after.asked.borrow(), Vec::<usize>::new(), "no room left");
    }

    /// A tree that loops, an element met again below itself, ends the
    /// walk at the repeated element, with the text read before it.
    #[test]
    fn a_repeated_element_ends_the_walk() {
        let looping = Fake {
            role: Some(Role::Pane),
            id: Some(1),
            children: vec![
                object(Role::StaticText, "Before"),
                Fake {
                    role: Some(Role::Pane),
                    id: Some(1),
                    children: vec![object(Role::StaticText, "Again")],
                    ..Fake::default()
                },
                object(Role::StaticText, "After"),
            ],
            ..Fake::default()
        };
        let mut dialog = dialog(vec![looping]);
        dialog.id = Some(0);
        assert_eq!(dialog_text(&dialog).as_deref(), Some("Before"));
    }
}
