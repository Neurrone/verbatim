//! A UI Automation dialog's objects, which `verbatim_reader_rules::dialog`
//! gathers a dialog's own text from (`docs/nvda/object-model.md`, "A
//! dialog's own text"): [`UiaDialog`], the dialog, of which nothing is read
//! but its children, and [`UiaObject`], each child, whose properties come
//! from one cache request per container ([`verbatim_uia::DialogText`]).

use verbatim_model::{Role, StateSet};
use verbatim_reader_rules::dialog::{DialogObject, DialogParent, Visited};
use verbatim_uia::map::CachedUiaParts;
use verbatim_uia::{DialogText, Live, Request, Uia};

/// A UI Automation dialog whose text is gathered: nothing of its own
/// element is read, only its children, each read as a
/// [`verbatim_uia::DialogText`] (Dickson, 2026-10-13, decision 5 of
/// `books/dev/src/explanation/design/uia-shapes.md`).
pub struct UiaDialog<'a> {
    uia: &'a Uia,
    request: &'a Request<DialogText>,
    element: Live,
    /// Its children, when they were read already, filled by `request`.
    read: Option<Vec<verbatim_uia::Cached<DialogText>>>,
}

impl<'a> UiaDialog<'a> {
    /// The dialog `element`, whose children, and theirs, are read with
    /// `request`.
    #[must_use]
    pub fn new(uia: &'a Uia, request: &'a Request<DialogText>, element: Live) -> Self {
        Self {
            uia,
            request,
            element,
            read: None,
        }
    }

    /// The dialog `element` whose children, `children`, were read already
    /// with `request`, which reads their own children.
    #[must_use]
    pub fn with_children(
        uia: &'a Uia,
        request: &'a Request<DialogText>,
        element: Live,
        children: Vec<verbatim_uia::Cached<DialogText>>,
    ) -> Self {
        Self {
            uia,
            request,
            element,
            read: Some(children),
        }
    }
}

impl<'a> DialogParent for UiaDialog<'a> {
    type Child = UiaObject<'a>;
    /// One call: the dialog's first children with their properties
    /// cached, or those read already; none at a `room` of 0.
    fn children(&self, room: usize) -> Vec<UiaObject<'a>> {
        match &self.read {
            Some(children) => children
                .iter()
                .take(room)
                .map(|child| object(self.uia, self.request, child))
                .collect(),
            None => children_of(self.uia, self.request, &self.element, room),
        }
    }
}

/// A UI Automation element a dialog's text is gathered from, with the
/// properties read when its parent's children were.
pub struct UiaObject<'a> {
    uia: &'a Uia,
    request: &'a Request<DialogText>,
    element: Live,
    parts: CachedUiaParts,
}

/// `parent`'s first children, at most `room`, each read with `request`,
/// in one call; none when they cannot be read, and no call at a `room` of
/// 0. UI Automation's call returns every child, so past the room the
/// children are fetched and let go of unread.
fn children_of<'a>(
    uia: &'a Uia,
    request: &'a Request<DialogText>,
    parent: &Live,
    room: usize,
) -> Vec<UiaObject<'a>> {
    if room == 0 {
        return Vec::new();
    }
    uia.children(parent, request)
        .unwrap_or_default()
        .iter()
        .take(room)
        .map(|child| object(uia, request, child))
        .collect()
}

/// The object a child read with `request` is.
fn object<'a>(
    uia: &'a Uia,
    request: &'a Request<DialogText>,
    child: &verbatim_uia::Cached<DialogText>,
) -> UiaObject<'a> {
    UiaObject {
        uia,
        request,
        parts: child.read().parts(child.runtime_id()),
        element: child.live().clone(),
    }
}

impl DialogParent for UiaObject<'_> {
    type Child = Self;
    /// One call: the element's first children with their properties
    /// cached.
    fn children(&self, room: usize) -> Vec<Self> {
        children_of(self.uia, self.request, &self.element, room)
    }
}

impl DialogObject for UiaObject<'_> {
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
    fn visited(&self) -> Option<Visited> {
        (!self.parts.runtime_id.is_empty()).then(|| Visited::Uia(self.parts.runtime_id.clone()))
    }
}

/// `text`, unless it is empty or blank.
fn visible(text: Option<String>) -> Option<String> {
    text.filter(|text| !text.trim().is_empty())
}
