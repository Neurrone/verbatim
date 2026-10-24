//! The focus rules, each defined once (decision D14, "Where the rules
//! run"): which events are accepted, which focus repeats the focus Core
//! holds, which property changes and selections are spoken, and which
//! dialogs are newly entered. The reducer applies them after an event is
//! read; the focus listener and the outposts apply the same definitions
//! where a judgment must come before an element is read, because a UI
//! Automation element cannot pass to another process. Those judgments use
//! the focus Core holds, which Core sends them whenever it changes
//! ([`HeldFocus`]), never a copy of the focus they last reported.
//!
//! The behaviour is specified in `docs/parity/focus-events.md` ("Event
//! acceptance", "Duplicate focus suppression"),
//! `docs/parity/changes-and-selection.md` and
//! `docs/parity/focus-context.md`.

use serde::{Deserialize, Serialize};
use verbatim_model::{Backend, NodeId, Role, StateSet, TraceId, WindowFacts, WindowHandle};

/// An event, by the kind its acceptance is judged as
/// (`docs/parity/focus-events.md`, "Event acceptance").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EventName {
    /// A focus or foreground change, or a UIA menu opening.
    GainFocus,
    /// A popup menu opening.
    MenuStart,
    /// A menu, or menu mode, ending.
    MenuEnd,
    /// The Alt+Tab switcher closing.
    SwitchEnd,
    /// An object shown.
    Show,
    /// An alert.
    Alert,
    /// A value change.
    ValueChange,
    /// A state change, or an item added to or removed from a selection.
    StateChange,
    /// A name change.
    NameChange,
    /// A description change.
    DescriptionChange,
    /// An item becoming the selection.
    Selection,
    /// A UIA text control's caret or selection moved.
    Caret,
    /// The system caret's location change, and its show, tested as a
    /// location change (`docs/parity/text.md`).
    LocationChange,
    /// A UIA text control's text changed.
    TextChange,
}

/// The events an application's support asked to have from any window of
/// its process, whatever the foreground, by event, process id, and window
/// class. Nothing asks yet, so it is empty; it is where application
/// support will ask.
#[derive(Debug, Default)]
pub struct RequestedEvents {
    requested: std::collections::HashSet<(EventName, u32, String)>,
}

impl RequestedEvents {
    /// Asks for `event` from every window of class `class` in process `pid`.
    pub fn request(&mut self, event: EventName, pid: u32, class: &str) {
        self.requested.insert((event, pid, class.to_owned()));
    }

    fn contains(&self, event: EventName, pid: u32, class: &str) -> bool {
        self.requested.contains(&(event, pid, class.to_owned()))
    }
}

/// The window queries the acceptance test makes. Every one is a local call
/// on Windows, so the test never waits on an application.
pub trait WindowQueries {
    /// The window's class name, empty for an invalid window.
    fn class_name(&self, window: isize) -> String;
    /// The id of the process that owns the window, 0 for none.
    fn process(&self, window: isize) -> u32;
    /// The window's parent, 0 for none.
    fn parent(&self, window: isize) -> isize;
    /// The window's root, the top of its parent chain, 0 for none.
    fn root(&self, window: isize) -> isize;
    /// The window's root owner, the top of its owner chain, 0 for none.
    fn root_owner(&self, window: isize) -> isize;
    /// Whether `child` is a child window of `parent`.
    fn is_child(&self, parent: isize, child: isize) -> bool;
    /// Whether the window has the topmost extended style.
    fn is_topmost(&self, window: isize) -> bool;
    /// The desktop window.
    fn desktop(&self) -> isize;
    /// The active window of the thread that has the keyboard input, 0 for
    /// none.
    fn active_window(&self) -> isize;
    /// The foreground window, 0 for none.
    fn foreground(&self) -> isize;
}

/// `child` is `parent` or one of its child windows.
fn is_descendant(windows: &impl WindowQueries, parent: isize, child: isize) -> bool {
    parent == child || windows.is_child(parent, child)
}

/// How a window stands to the system's foreground window, the facts the
/// acceptance test ends with (`docs/parity/focus-events.md`, "Event
/// acceptance"): read before an event is read, and again by an outpost
/// when it reports the event ([`WindowFacts`]), so the reducer judges a
/// focus by the same rule after the read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ForegroundRelation {
    /// The window is in the foreground window: it shares the foreground
    /// window's root owner, is the foreground window or inside it, or its
    /// root owner is.
    pub in_foreground: bool,
    /// The window or its root is topmost: menus, combo box drop-downs, the
    /// task switching list.
    pub topmost: bool,
    /// For a `Windows.UI.Core` window only, whether it is the input
    /// thread's active window or inside it; `None` for any other class.
    pub under_active_window: Option<bool>,
}

impl ForegroundRelation {
    /// The relation of `window`, of class `class`, as `windows` read it.
    pub fn of(window: isize, class: &str, windows: &impl WindowQueries) -> Self {
        let foreground = windows.foreground();
        let root_owner = windows.root_owner(window);
        Self {
            in_foreground: root_owner == windows.root_owner(foreground)
                || is_descendant(windows, foreground, window)
                || is_descendant(windows, foreground, root_owner),
            topmost: windows.is_topmost(window) || windows.is_topmost(windows.root(window)),
            under_active_window: class
                .starts_with("Windows.UI.Core")
                .then(|| is_descendant(windows, windows.active_window(), window)),
        }
    }

    /// The relation an event's window facts record.
    #[must_use]
    pub fn of_facts(facts: &WindowFacts) -> Self {
        Self {
            in_foreground: facts.in_foreground,
            topmost: facts.topmost,
            under_active_window: facts.under_active_window,
        }
    }

    /// Whether the window is related to the foreground window: in it,
    /// topmost, or a `Windows.UI.Core` window under the active window.
    #[must_use]
    pub fn related(&self) -> bool {
        self.in_foreground || self.topmost || self.under_active_window == Some(true)
    }
}

/// Whether an event named `event` from `window` is accepted
/// (`docs/parity/focus-events.md`, "Event acceptance"). `window` 0 means
/// the event has no window, which cannot be filtered.
pub fn accepts_event(
    event: EventName,
    window: isize,
    windows: &impl WindowQueries,
    requested: &RequestedEvents,
) -> bool {
    if window == 0 {
        return true;
    }
    let class = windows.class_name(window);
    if requested.contains(event, windows.process(window), &class) {
        return true;
    }
    // A background progress bar's value change is not accepted: Verbatim
    // has no setting for it yet, and NVDA's is off by default (Dickson,
    // 2026-10-10).
    match event {
        EventName::Show => {
            return matches!(
                class.as_str(),
                "Frame Notification Bar"
                    | "tooltips_class32"
                    | "mscandui21.candidate"
                    | "mscandui40.candidate"
                    | "MSCandUIWindow_Candidate"
                    | "TTrayAlert"
            );
        }
        // A toast.
        EventName::Alert
            if windows.class_name(windows.parent(window)) == "ToastChildWindowClass" =>
        {
            return true;
        }
        // Raised on the desktop window or on windows otherwise refused, and
        // taken as focus changes.
        EventName::MenuEnd | EventName::SwitchEnd => return true,
        _ => {}
    }
    // The mouse cursor's events come from the desktop window.
    if window == windows.desktop() {
        return true;
    }
    ForegroundRelation::of(window, &class, windows).related()
}

/// One node of the focus Core holds, as the rules compare it: the focus or
/// one of its ancestors.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldNode {
    /// The node.
    pub id: NodeId,
    /// The backend that read it.
    pub backend: Backend,
    /// Its role.
    pub role: Role,
    /// Its name, as last known.
    pub name: Option<String>,
    /// Its states, as last known.
    pub states: StateSet,
}

/// The focus Core holds, which Core sends the focus listener and the
/// outposts concerned whenever it changes, for the judgments they make
/// before an element is read (decision D14, "Where the rules run").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldFocus {
    /// The trace of the event Core took the focus from: the focus
    /// listener's own trace for a UIA focus it read.
    pub trace_id: TraceId,
    /// When that event was observed, in milliseconds and microseconds on
    /// the run's shared clock (0 when not known).
    pub observed: (u64, u64),
    /// The focus.
    pub focus: HeldNode,
    /// Its ancestors, outermost first.
    pub ancestors: Vec<HeldNode>,
    /// Its top-level window, when known.
    pub top_level: Option<WindowHandle>,
    /// False once the outpost that issued its node ids has ended.
    pub alive: bool,
}

impl HeldFocus {
    /// This focus as the outpost `outpost` knows its own nodes: their ids
    /// unstamped, as it issued them, and every node another outpost issued
    /// left out; `None` when the focus itself is another outpost's.
    #[must_use]
    pub fn for_outpost(&self, outpost: verbatim_model::OutpostId) -> Option<Self> {
        let own = |node: &HeldNode| {
            (node.id.outpost() == outpost).then(|| HeldNode {
                id: node.id.unstamped(),
                ..node.clone()
            })
        };
        Some(Self {
            focus: own(&self.focus)?,
            ancestors: self.ancestors.iter().filter_map(own).collect(),
            ..self.clone()
        })
    }

    /// The node `id`, the focus or an ancestor, when it is one.
    #[must_use]
    pub fn node(&self, id: NodeId) -> Option<&HeldNode> {
        std::iter::once(&self.focus)
            .chain(&self.ancestors)
            .find(|node| node.id == id)
    }
}

/// What a judge knows of one focus event against the focus Core holds
/// (`docs/parity/focus-events.md`, "Duplicate focus suppression").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RepeatEvidence {
    /// Core has judged every focus the judge sent it, so the focus it holds
    /// is the focus it will compare the event with. Core itself always has.
    pub settled: bool,
    /// The event names the element of the focus Core holds, by the judge's
    /// evidence of identity: the node id in Core, UI Automation's
    /// comparison in the focus listener, the event's address in an outpost.
    pub same_element: bool,
    /// That element still has the focus: the node still alive in Core, its
    /// keyboard focus read live in the focus listener, its address a whole
    /// object rather than a child id that can be reused in an outpost.
    pub still_focused: bool,
}

/// Whether a focus event repeats the focus Core holds, and is dropped
/// before anything else judges it.
#[must_use]
pub fn repeats_focus(evidence: RepeatEvidence) -> bool {
    evidence.settled && evidence.same_element && evidence.still_focused
}

/// Why a foreground change of window `hwnd`, which is in front, repeats the
/// focus Core holds, as NVDA's two foreground tests drop it
/// (`docs/parity/focus-events.md`, "Event acceptance"): the focus's own
/// window, `focus_window`, is `hwnd` or a child window of it
/// (`is_descendant`), or the focus came from an MSAA event with the
/// foreground change's own event address (`focus_event` against
/// `foreground_address`). `None` when it does not, or when Core holds no
/// focus the judge knows the window of.
pub fn foreground_repeats_focus(
    focus_window: Option<isize>,
    focus_event: Option<(isize, i32, i32)>,
    (hwnd, foreground_address): (isize, (isize, i32, i32)),
    is_descendant: impl Fn(isize, isize) -> bool,
) -> Option<&'static str> {
    let window = focus_window?;
    if is_descendant(hwnd, window) {
        Some("the focus is already in the window")
    } else if focus_event == Some(foreground_address) {
        Some("it repeats the focus's own event")
    } else {
        None
    }
}

/// A property change, by what it changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    /// The name.
    Name,
    /// The description.
    Description,
    /// The value.
    Value,
    /// The states.
    States,
}

/// What a spoken property change concerns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeTarget {
    /// The focus.
    Focus,
    /// One of its ancestors.
    Ancestor,
}

/// What a change of `kind` on `node` concerns, when it is spoken
/// (`docs/parity/changes-and-selection.md`): a name, description, or value
/// change only on the focus, `focus`, and a state change on the focus or on
/// one of its `ancestors` whose state changes are spoken
/// (`speaks_states`): an outpost knows of two whose are not, an ancestor
/// reached through a tree view item's logical parents and the foreground
/// window's own object. `None` for a change that is not spoken.
pub fn change_target(
    kind: ChangeKind,
    node: NodeId,
    focus: NodeId,
    ancestors: impl IntoIterator<Item = NodeId>,
    speaks_states: impl Fn(NodeId) -> bool,
) -> Option<ChangeTarget> {
    if node == focus {
        return Some(ChangeTarget::Focus);
    }
    (kind == ChangeKind::States
        && ancestors
            .into_iter()
            .any(|ancestor| ancestor == node && speaks_states(ancestor)))
    .then_some(ChangeTarget::Ancestor)
}

/// The nodes a change of `kind` can be spoken for, given the focus and its
/// ancestors: the focus, and for a state change the ancestors whose state
/// changes are spoken, by [`change_target`]. An outpost tells the event's
/// object from these by its identity before it reads any property of it.
pub fn change_candidates(
    kind: ChangeKind,
    focus: NodeId,
    ancestors: &[NodeId],
    speaks_states: impl Fn(NodeId) -> bool,
) -> Vec<NodeId> {
    ancestors
        .iter()
        .copied()
        .chain(std::iter::once(focus))
        .filter(|&node| {
            change_target(kind, node, focus, ancestors.iter().copied(), &speaks_states).is_some()
        })
        .collect()
}

/// What a selection of a node concerns, when it is spoken
/// (`docs/parity/changes-and-selection.md`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionTarget {
    /// The focus itself: a change of its states.
    FocusStates,
    /// One of the focus's ancestors: a change of its states.
    AncestorStates,
    /// An item inside the focus, a selection container, of the same
    /// application.
    Item,
}

/// Whether a focus of `role` is a selection container: its selection
/// events are spoken (NVDA's generic selection behaviour: arrowing through
/// a list whose focus stays on the container speaks each newly selected
/// item). A combo box is not one: its selection reaches it as a value
/// change already.
#[must_use]
pub fn is_selection_container(role: Role) -> bool {
    matches!(role, Role::List | Role::TabControl)
}

/// What a selection of `node` concerns, given the focus `focus` of
/// `focus_role` and its `ancestors`: the focus's or an ancestor's states
/// (an ancestor `selects_states` refuses, as an outpost refuses a tree view
/// item's logical parent, is not spoken at all), or an item when the
/// focus is a selection container of the same outpost; `None` when it is
/// not spoken.
pub fn selection_target(
    node: NodeId,
    (focus, focus_role): (NodeId, Role),
    ancestors: impl IntoIterator<Item = NodeId>,
    selects_states: impl Fn(NodeId) -> bool,
) -> Option<SelectionTarget> {
    if node == focus {
        return Some(SelectionTarget::FocusStates);
    }
    if ancestors.into_iter().any(|ancestor| ancestor == node) {
        return selects_states(node).then_some(SelectionTarget::AncestorStates);
    }
    (focus.outpost() == node.outpost() && is_selection_container(focus_role))
        .then_some(SelectionTarget::Item)
}

/// A node as [`already_entered`] compares it.
pub trait EnteredKey {
    /// Its id.
    fn key_id(&self) -> NodeId;
    /// The backend that read it.
    fn key_backend(&self) -> Backend;
    /// Its role.
    fn key_role(&self) -> Role;
    /// Its name.
    fn key_name(&self) -> Option<&str>;
}

impl EnteredKey for HeldNode {
    fn key_id(&self) -> NodeId {
        self.id
    }
    fn key_backend(&self) -> Backend {
        self.backend
    }
    fn key_role(&self) -> Role {
        self.role
    }
    fn key_name(&self) -> Option<&str> {
        self.name.as_deref()
    }
}

impl EnteredKey for verbatim_model::NodeSnapshot {
    fn key_id(&self) -> NodeId {
        self.id
    }
    fn key_backend(&self) -> Backend {
        self.backend
    }
    fn key_role(&self) -> Role {
        self.role
    }
    fn key_name(&self) -> Option<&str> {
        self.name.as_deref()
    }
}

/// Whether the focus had already entered `node`, a container of a new
/// focus, given the previous focus and its ancestors, `previous`
/// (`docs/parity/focus-context.md`, "Focus-ancestry context"): `node` is
/// one of them; or, when the new focus is in the same top-level window as
/// the previous one (`same_window`), one of them has `node`'s role and
/// name and is not the same element under another node, which only a UIA
/// node of the same outpost can be. A newly entered container is spoken,
/// and a newly entered dialog gathers its own text.
pub fn already_entered<'a, P: EnteredKey + 'a>(
    previous: impl IntoIterator<Item = &'a P>,
    same_window: bool,
    node: &impl EnteredKey,
) -> bool {
    previous.into_iter().any(|old| {
        let one_identity = old.key_id().outpost() == node.key_id().outpost()
            && old.key_backend() == Backend::Uia
            && node.key_backend() == Backend::Uia;
        old.key_id() == node.key_id()
            || (same_window
                && !one_identity
                && old.key_role() == node.key_role()
                && old.key_name() == node.key_name())
    })
}

/// Whether a container of `role` is a dialog, whose own text is gathered
/// when the focus newly enters it and it has no description of its own.
#[must_use]
pub fn is_dialog(role: Role) -> bool {
    matches!(role, Role::Dialog | Role::Alert | Role::PropertyPage)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use verbatim_model::OutpostId;

    /// A made-up window: its class, process, parent, root, root owner, and
    /// whether it is topmost.
    #[derive(Clone, Default)]
    struct Window {
        class: &'static str,
        process: u32,
        parent: isize,
        root: isize,
        root_owner: isize,
        topmost: bool,
    }

    /// Made-up windows, the foreground and active windows among them.
    #[derive(Default)]
    struct Desk {
        windows: HashMap<isize, Window>,
        foreground: isize,
        active: isize,
    }

    const DESKTOP: isize = 1;
    const FOREGROUND: isize = 10;
    /// A child window of the foreground window.
    const FOREGROUND_CHILD: isize = 11;
    /// A top-level window unrelated to the foreground window.
    const BACKGROUND: isize = 20;
    const BACKGROUND_CHILD: isize = 21;

    impl Desk {
        /// The foreground window and a child, and a background window and
        /// a child, in processes 100 and 200.
        fn new() -> Self {
            let mut desk = Self {
                foreground: FOREGROUND,
                ..Self::default()
            };
            desk.top_level(FOREGROUND, "Frame", 100);
            desk.child(FOREGROUND_CHILD, FOREGROUND, "Edit");
            desk.top_level(BACKGROUND, "Frame", 200);
            desk.child(BACKGROUND_CHILD, BACKGROUND, "Edit");
            desk
        }

        fn top_level(&mut self, handle: isize, class: &'static str, process: u32) {
            self.windows.insert(
                handle,
                Window {
                    class,
                    process,
                    parent: DESKTOP,
                    root: handle,
                    root_owner: handle,
                    topmost: false,
                },
            );
        }

        fn child(&mut self, handle: isize, parent: isize, class: &'static str) {
            let of = self.windows[&parent].clone();
            self.windows.insert(
                handle,
                Window {
                    class,
                    parent,
                    topmost: false,
                    ..of
                },
            );
        }

        fn window(&mut self, handle: isize) -> &mut Window {
            self.windows.get_mut(&handle).expect("a made-up window")
        }

        fn accepts(&self, event: EventName, window: isize) -> bool {
            accepts_event(event, window, self, &RequestedEvents::default())
        }
    }

    impl WindowQueries for Desk {
        fn class_name(&self, window: isize) -> String {
            self.windows
                .get(&window)
                .map_or_else(String::new, |window| window.class.to_owned())
        }

        fn process(&self, window: isize) -> u32 {
            self.windows.get(&window).map_or(0, |window| window.process)
        }

        fn parent(&self, window: isize) -> isize {
            self.windows.get(&window).map_or(0, |window| window.parent)
        }

        fn root(&self, window: isize) -> isize {
            self.windows.get(&window).map_or(0, |window| window.root)
        }

        fn root_owner(&self, window: isize) -> isize {
            self.windows
                .get(&window)
                .map_or(0, |window| window.root_owner)
        }

        fn is_child(&self, parent: isize, child: isize) -> bool {
            let mut at = child;
            while let Some(window) = self.windows.get(&at) {
                if window.parent == parent && parent != DESKTOP {
                    return true;
                }
                if window.root == at {
                    return false;
                }
                at = window.parent;
            }
            false
        }

        fn is_topmost(&self, window: isize) -> bool {
            self.windows
                .get(&window)
                .is_some_and(|window| window.topmost)
        }

        fn desktop(&self) -> isize {
            DESKTOP
        }

        fn active_window(&self) -> isize {
            self.active
        }

        fn foreground(&self) -> isize {
            self.foreground
        }
    }

    #[test]
    fn an_event_with_no_window_is_accepted() {
        assert!(Desk::new().accepts(EventName::StateChange, 0));
    }

    #[test]
    fn a_requested_event_is_accepted_from_a_background_window() {
        let desk = Desk::new();
        let mut requested = RequestedEvents::default();
        requested.request(EventName::Selection, 200, "Edit");
        assert!(accepts_event(
            EventName::Selection,
            BACKGROUND_CHILD,
            &desk,
            &requested
        ));
        assert!(
            !accepts_event(EventName::NameChange, BACKGROUND_CHILD, &desk, &requested),
            "only the event asked for"
        );
    }

    #[test]
    fn a_background_value_change_is_dropped() {
        assert!(!Desk::new().accepts(EventName::ValueChange, BACKGROUND_CHILD));
    }

    #[test]
    fn a_show_is_accepted_only_from_the_listed_classes() {
        let mut desk = Desk::new();
        desk.top_level(30, "tooltips_class32", 200);
        desk.top_level(31, "TTrayAlert", 200);
        assert!(desk.accepts(EventName::Show, 30));
        assert!(desk.accepts(EventName::Show, 31));
        assert!(
            !desk.accepts(EventName::Show, FOREGROUND_CHILD),
            "not even from the foreground window"
        );
    }

    #[test]
    fn a_toasts_alert_is_accepted_from_the_background() {
        let mut desk = Desk::new();
        desk.top_level(30, "ToastChildWindowClass", 200);
        desk.child(31, 30, "DirectUIHWND");
        assert!(desk.accepts(EventName::Alert, 31));
        assert!(!desk.accepts(EventName::Alert, BACKGROUND_CHILD));
    }

    #[test]
    fn a_menu_or_switch_end_is_accepted_from_anywhere() {
        let desk = Desk::new();
        assert!(desk.accepts(EventName::MenuEnd, BACKGROUND_CHILD));
        assert!(desk.accepts(EventName::SwitchEnd, BACKGROUND_CHILD));
    }

    #[test]
    fn an_event_of_the_desktop_window_is_accepted() {
        assert!(Desk::new().accepts(EventName::NameChange, DESKTOP));
    }

    #[test]
    fn a_ui_core_window_under_the_active_window_is_accepted() {
        let mut desk = Desk::new();
        desk.top_level(30, "ApplicationFrameWindow", 300);
        desk.child(31, 30, "Windows.UI.Core.CoreWindow");
        assert!(!desk.accepts(EventName::GainFocus, 31));
        desk.active = 30;
        assert!(desk.accepts(EventName::GainFocus, 31));
    }

    #[test]
    fn a_window_sharing_the_foregrounds_root_owner_is_accepted() {
        let mut desk = Desk::new();
        desk.window(FOREGROUND).root_owner = 5;
        desk.top_level(30, "Popup", 100);
        desk.window(30).root_owner = 5;
        assert!(desk.accepts(EventName::StateChange, 30));
    }

    #[test]
    fn a_window_inside_the_foreground_window_is_accepted() {
        let desk = Desk::new();
        assert!(desk.accepts(EventName::StateChange, FOREGROUND));
        assert!(desk.accepts(EventName::StateChange, FOREGROUND_CHILD));
    }

    #[test]
    fn a_window_whose_root_owner_is_inside_the_foreground_window_is_accepted() {
        let mut desk = Desk::new();
        desk.top_level(30, "Popup", 100);
        desk.window(30).root_owner = FOREGROUND_CHILD;
        assert!(desk.accepts(EventName::NameChange, 30));
    }

    #[test]
    fn a_topmost_window_is_accepted() {
        let mut desk = Desk::new();
        desk.window(BACKGROUND_CHILD).topmost = true;
        assert!(desk.accepts(EventName::StateChange, BACKGROUND_CHILD));
    }

    #[test]
    fn a_window_whose_root_is_topmost_is_accepted() {
        let mut desk = Desk::new();
        desk.window(BACKGROUND).topmost = true;
        assert!(desk.accepts(EventName::StateChange, BACKGROUND_CHILD));
    }

    #[test]
    fn any_other_window_is_dropped() {
        let desk = Desk::new();
        assert!(!desk.accepts(EventName::GainFocus, BACKGROUND));
        assert!(!desk.accepts(EventName::StateChange, BACKGROUND_CHILD));
    }

    #[test]
    fn the_relation_an_outpost_reports_is_the_one_the_test_judges_by() {
        let mut desk = Desk::new();
        desk.top_level(30, "Windows.UI.Core.CoreWindow", 300);
        desk.active = 30;
        assert_eq!(
            ForegroundRelation::of(30, "Windows.UI.Core.CoreWindow", &desk),
            ForegroundRelation {
                in_foreground: false,
                topmost: false,
                under_active_window: Some(true),
            }
        );
        assert_eq!(
            ForegroundRelation::of(FOREGROUND_CHILD, "Edit", &desk),
            ForegroundRelation {
                in_foreground: true,
                topmost: false,
                under_active_window: None,
            }
        );
        assert!(
            !ForegroundRelation::of(BACKGROUND_CHILD, "Edit", &desk).related(),
            "a background window is unrelated"
        );
    }

    #[test]
    fn a_repeat_needs_every_piece_of_evidence() {
        let all = RepeatEvidence {
            settled: true,
            same_element: true,
            still_focused: true,
        };
        assert!(repeats_focus(all));
        assert!(!repeats_focus(RepeatEvidence {
            settled: false,
            ..all
        }));
        assert!(!repeats_focus(RepeatEvidence {
            same_element: false,
            ..all
        }));
        assert!(!repeats_focus(RepeatEvidence {
            still_focused: false,
            ..all
        }));
    }

    #[test]
    fn a_foreground_change_of_the_focus_s_window_or_its_parent_repeats_it() {
        let is_descendant =
            |parent: isize, child: isize| (parent, child) == (5, 6) || parent == child;
        assert_eq!(
            foreground_repeats_focus(Some(6), None, (5, (5, -4, 0)), is_descendant),
            Some("the focus is already in the window")
        );
        assert_eq!(
            foreground_repeats_focus(Some(7), Some((5, -4, 0)), (5, (5, -4, 0)), is_descendant),
            Some("it repeats the focus's own event")
        );
        assert_eq!(
            foreground_repeats_focus(Some(7), None, (5, (5, -4, 0)), is_descendant),
            None
        );
        assert_eq!(
            foreground_repeats_focus(None, Some((5, -4, 0)), (5, (5, -4, 0)), is_descendant),
            None,
            "no focus known, no repeat"
        );
    }

    #[test]
    fn a_name_change_is_spoken_on_the_focus_alone_and_a_state_change_on_ancestors_too() {
        let (focus, parent, other) = (NodeId::new(1), NodeId::new(2), NodeId::new(3));
        let speaks = |_| true;
        assert_eq!(
            change_target(ChangeKind::Name, focus, focus, [parent], speaks),
            Some(ChangeTarget::Focus)
        );
        assert_eq!(
            change_target(ChangeKind::Name, parent, focus, [parent], speaks),
            None
        );
        assert_eq!(
            change_target(ChangeKind::States, parent, focus, [parent], speaks),
            Some(ChangeTarget::Ancestor)
        );
        assert_eq!(
            change_target(ChangeKind::States, parent, focus, [parent], |_| false),
            None,
            "an ancestor whose state changes are not spoken"
        );
        assert_eq!(
            change_target(ChangeKind::States, other, focus, [parent], speaks),
            None
        );
        assert_eq!(
            change_candidates(ChangeKind::States, focus, &[parent], speaks),
            vec![parent, focus]
        );
        assert_eq!(
            change_candidates(ChangeKind::Value, focus, &[parent], speaks),
            vec![focus]
        );
    }

    #[test]
    fn a_selection_is_spoken_on_the_focus_its_ancestors_or_inside_a_focused_list() {
        let focus = NodeId::new(1).with_outpost(OutpostId(2));
        let parent = NodeId::new(2).with_outpost(OutpostId(2));
        let item = NodeId::new(3).with_outpost(OutpostId(2));
        let elsewhere = NodeId::new(3).with_outpost(OutpostId(4));
        let all = |_| true;
        assert_eq!(
            selection_target(focus, (focus, Role::Button), [parent], all),
            Some(SelectionTarget::FocusStates)
        );
        assert_eq!(
            selection_target(parent, (focus, Role::Button), [parent], all),
            Some(SelectionTarget::AncestorStates)
        );
        assert_eq!(
            selection_target(parent, (focus, Role::List), [parent], |_| false),
            None,
            "an ancestor refused is not spoken, even in a list"
        );
        assert_eq!(
            selection_target(item, (focus, Role::Button), [parent], all),
            None
        );
        assert_eq!(
            selection_target(item, (focus, Role::List), [parent], all),
            Some(SelectionTarget::Item)
        );
        assert_eq!(
            selection_target(elsewhere, (focus, Role::List), [parent], all),
            None,
            "another application's item"
        );
    }

    fn held(id: u64, backend: Backend, role: Role, name: &str) -> HeldNode {
        HeldNode {
            id: NodeId::new(id).with_outpost(OutpostId(1)),
            backend,
            role,
            name: Some(name.to_owned()),
            states: StateSet::default(),
        }
    }

    #[test]
    fn a_container_is_entered_unless_the_previous_focus_was_in_it() {
        let dialog = held(1, Backend::Msaa, Role::Dialog, "Remove");
        let previous = [dialog.clone()];
        assert!(already_entered(&previous, false, &dialog));
        let same_name = held(2, Backend::Msaa, Role::Dialog, "Remove");
        assert!(
            already_entered(&previous, true, &same_name),
            "in the same window"
        );
        assert!(
            !already_entered(&previous, false, &same_name),
            "in another window"
        );
        let uia_previous = [held(1, Backend::Uia, Role::Dialog, "Remove")];
        let uia_same_name = held(2, Backend::Uia, Role::Dialog, "Remove");
        assert!(
            !already_entered(&uia_previous, true, &uia_same_name),
            "one outpost's UIA node is one element"
        );
    }

    #[test]
    fn a_focus_for_an_outpost_keeps_its_own_nodes_unstamped() {
        let ours = |id| NodeId::new(id).with_outpost(OutpostId(3));
        let node = |id: NodeId| HeldNode {
            id,
            backend: Backend::Msaa,
            role: Role::Pane,
            name: None,
            states: StateSet::default(),
        };
        let focus = HeldFocus {
            trace_id: TraceId::mint(),
            observed: (1, 0),
            focus: node(ours(5)),
            ancestors: vec![
                node(NodeId::new(9).with_outpost(OutpostId(4))),
                node(ours(4)),
            ],
            top_level: None,
            alive: true,
        };
        let mine = focus.for_outpost(OutpostId(3)).expect("the focus is ours");
        assert_eq!(mine.focus.id, NodeId::new(5));
        assert_eq!(
            mine.ancestors
                .iter()
                .map(|node| node.id)
                .collect::<Vec<_>>(),
            vec![NodeId::new(4)]
        );
        assert_eq!(focus.for_outpost(OutpostId(4)), None);
    }
}
