//! `FocusRead`'s arms on UI Automation subjects (the one-path design,
//! "FocusRead"): a fixed composition of the reads a focus takes, the
//! focus check, the element's fill, a menu item's legacy state, a cell's
//! table, the selected item a looping selector or a combo box without
//! `Value` reads as its value, the ancestors up to the first Core knows
//! with the nearest window's walk-on, and the text of the dialogs the
//! focus enters, made here; what is the outpost's, the subject's
//! resolution, each judgement between reads, the chaining above where a
//! walk ends across backends, the other backend's reads, and the answer
//! made from what was read, it resolves ([`FocusResolve`]).
//!
//! Two subjects: the application's focus ([`focus_now`]), read in the
//! window with the keyboard focus with its ancestors walked by the focus
//! ancestry program ([`crate::focus_ancestry`]) or the `Ancestors` arm's
//! walk; and an element the focus listener read ([`route`]), found by its
//! route, its states read once its registration is in with the children
//! of the dialogs it enters in the same round trip
//! ([`crate::focus_states`]), its ancestors built from the listener's read
//! with no call where the listener's walk met one Core knows.

use std::cell::RefCell;

use verbatim_model::request::FocusRead;
use verbatim_model::{NodeId, NodeSnapshot, State, TableId};
use verbatim_uia::map::with_legacy_checked_state;
use verbatim_uia::{
    Ancestor, AncestorParts, AncestorStops, AncestorWalk, Cached, DialogText, FailedHop, Fill,
    FocusInput, Interning, Live, Node, Request, Uia,
};

use crate::dialog::UiaDialog;
use crate::error::Error;
use crate::focus::{FocusAncestry, FocusQuery, Path, focus_ancestry};
use crate::focus_states::{StatesQuery, focus_states};

/// The most ancestors a focus's walk reads.
const MAX_ANCESTOR_HOPS: u32 = 64;

/// The focus an arm read, for the outpost to make its answer from
/// ([`FocusResolve::answer`]).
pub struct UiaFocus {
    /// The focus.
    pub node: NodeSnapshot,
    /// Its ancestors, nearest first, up to the first Core knows.
    pub ancestors: Vec<NodeSnapshot>,
    /// The place among the ancestors Core knows of the one the walk met.
    pub met: Option<u32>,
    /// The window it was read in.
    pub nearest: isize,
    /// The foreground window's own object, read when the focus met none of
    /// the ancestors Core knows.
    pub foreground: Option<NodeSnapshot>,
    /// Whether its value is its first selected item's.
    pub value_from_selection: bool,
    /// Whether its runtime id named another element before.
    pub runtime_id_reused: bool,
}

/// What [`focus_now`] found.
pub enum FocusNowFound {
    /// The focus's answer.
    Read(Box<FocusRead>),
    /// No focus could be read.
    NotThere,
}

/// The element a listener's route names, found and minted by the outpost
/// ([`FocusResolve::route`]).
pub struct RouteFocus {
    /// The outpost's own copy of the element, `None` when none was found.
    pub found: Option<Cached<Node>>,
    /// What the listener read of it, as its node.
    pub snapshot: NodeSnapshot,
    /// The route's window.
    pub nearest: isize,
    /// Whether its runtime id named another element before.
    pub reused: bool,
    /// The place among the ancestors Core knows of the one the listener's
    /// walk met.
    pub met: Option<u32>,
    /// The route's ancestors as the listener read them, nearest first.
    pub parents: Vec<AncestorParts>,
    /// Whether the listener read its value as its first selected item's.
    pub value_from_selection: bool,
}

/// Why [`route`] answered no focus.
pub enum RouteMiss {
    /// The element is not there.
    NotThere,
    /// Core named an ancestor the route's end does not, a defect.
    Unsupported,
}

/// What the outpost resolves and judges for a `FocusRead` on a UI
/// Automation subject. Each method may call into the application only
/// where it says so: through the dispatcher's chaining, through the other
/// backend, or to find an element by its route.
pub trait FocusResolve {
    /// The registry the outpost's UI Automation nodes are minted in.
    fn registry(&self) -> &dyn Interning;

    /// The request a node is filled by.
    fn node_request(&self, uia: &Uia) -> Option<Request<Node>>;

    /// The request an ancestor is filled by.
    fn ancestor_request(&self, uia: &Uia) -> Option<Request<Ancestor>>;

    /// The request the focus's own selected item is filled by.
    fn focus_request(&self, uia: &Uia) -> Option<Request<Ancestor>>;

    /// The application's focused element in window `hwnd`, read with the
    /// node's fill and its process, taken only when that process is this
    /// outpost's application: the subject's resolution.
    fn focused_element(
        &self,
        uia: &Uia,
        hwnd: isize,
        nodes: &Request<Node>,
    ) -> Option<Cached<Node>>;

    /// Gives the focus's node what its window makes of it, and records
    /// its nearest window: local.
    fn place(&self, node: &mut NodeSnapshot, hwnd: isize);

    /// Whether a program may run in `window`.
    fn tries_remote(&self, window: isize) -> bool;

    /// Logs a program that fell back to calls, and marks the window read
    /// classically when its element could not be imported.
    fn fell_back(&self, program: &'static str, window: isize, error: &Error);

    /// The UI Automation nodes among the ancestors Core knows, by their
    /// node ids and runtime ids, in the same order.
    fn known_uia_nodes(&self) -> (Vec<NodeId>, Vec<Vec<i32>>);

    /// Whether window `hwnd` cannot be read: a ghost's or a hung
    /// application's. Local.
    fn unreadable(&self, hwnd: isize) -> bool;

    /// Whether a window above `window`, below the process boundary,
    /// cannot be read. Local.
    fn plain_window_above(&self, window: isize) -> bool;

    /// Whether this outpost's application serves window `hwnd`. Local.
    fn serves(&self, hwnd: isize) -> bool;

    /// Whether a UI Automation walk stops at the root of window `hwnd`,
    /// the focus's nearest window being `nearest`.
    fn leaves(&self, nearest: isize, hwnd: isize) -> bool;

    /// Whether `node` is among the ancestors Core knows.
    fn knows(&self, node: NodeId) -> bool;

    /// The chain above an element whose walk failed, from the parent of
    /// its nearest window: the dispatcher's chaining. `None` when the
    /// focus was lost on the way.
    fn onward_after_failure(&self, own: isize, nearest: isize) -> Option<Vec<NodeSnapshot>>;

    /// `chain`, outermost first, completed above where the walk ended
    /// (`walked`, past `crossed`, its outermost element in `outermost`):
    /// the dispatcher's chaining. `None` when the focus was lost.
    fn complete(
        &self,
        chain: Vec<NodeSnapshot>,
        above: (Option<isize>, AncestorWalk, isize),
    ) -> Option<Vec<NodeSnapshot>>;

    /// The chain above `element`, whose own window is `own` and nearest
    /// `nearest`, walked by the `Ancestors` arm and completed above: the
    /// dispatcher's chaining, `parent_automation_id` given the walk's
    /// first parent's. `None` when the focus was lost.
    fn chain(
        &self,
        element: &Live,
        windows: (isize, isize),
        parent_automation_id: &mut Option<String>,
        visited: verbatim_uia::Visited<'_>,
    ) -> Option<Vec<NodeSnapshot>>;

    /// `chain`, outermost first, cut at the first ancestor Core knows:
    /// nearest first, with that ancestor's place.
    fn cut(&self, chain: Vec<NodeSnapshot>) -> (Vec<NodeSnapshot>, Option<u32>);

    /// Keeps the routes to the nodes a read of the application's focus
    /// minted, by `walked`, the raw view's chain it stepped through.
    fn keep_routes(
        &self,
        node: &NodeSnapshot,
        ancestors: &[NodeSnapshot],
        walked: &[(Vec<i32>, isize)],
    );

    /// The foreground window's own object, taken from `ancestors` when one
    /// is it, and read through its window's backend otherwise.
    fn foreground(&self, ancestors: &[NodeSnapshot]) -> Option<NodeSnapshot>;

    /// Whether `node` is a dialog whose own text is gathered as its
    /// description.
    fn wants_dialog_text(&self, node: &NodeSnapshot) -> bool;

    /// The element of the UI Automation dialog `node`: the one the outpost
    /// keeps, or one found by the route of the focus being read.
    fn dialog_element(&self, uia: &Uia, node: &NodeSnapshot) -> Option<Live>;

    /// The element the outpost keeps for `node`, with no call.
    fn held_element(&self, node: NodeId) -> Option<Live>;

    /// The own text of a dialog read through MSAA, the other backend.
    fn other_dialog_text(&self, node: &NodeSnapshot) -> Option<String>;

    /// The answer made from what the arm read: each node's properties as
    /// Core asked, and the focus's caret.
    fn answer(&self, focus: UiaFocus) -> FocusRead;

    /// The element the listener's route names, found by the route and
    /// minted, its registration added: the subject's resolution.
    ///
    /// # Errors
    ///
    /// [`RouteMiss`] when the route names no element, or Core named an
    /// ancestor its end does not.
    fn route(&self) -> Result<RouteFocus, RouteMiss>;

    /// The chain above the route's own ancestors, where the listener's walk
    /// met none Core knows: the dispatcher's chaining, outermost first.
    /// `None` when the focus was lost.
    fn route_chain(&self) -> Option<Vec<NodeSnapshot>>;

    /// Keeps what a route's read minted: its node's route, and its
    /// ancestors'.
    fn keep_minted(&self, node: NodeId, ancestors: &[NodeSnapshot]);
}

/// The application's focus in window `hwnd`, the window with the keyboard
/// focus, taken already: its element, a menu item's legacy state, its
/// ancestors walked to the first Core knows by the focus ancestry program
/// (its focus check, its selected item, its table and the nearest
/// window's walk-on with them) or, where no program runs or its element
/// no longer has the keyboard focus, by the `Ancestors` arm's walk and the
/// selected item's own read; then, as before the arm, the routes it
/// minted kept, the foreground object, and the text of the dialogs it
/// enters.
pub fn focus_now(uia: &Uia, hwnd: isize, resolver: &dyn FocusResolve) -> FocusNowFound {
    let Some(nodes) = resolver.node_request(uia) else {
        return FocusNowFound::NotThere;
    };
    let Some(ancestor_request) = resolver.ancestor_request(uia) else {
        return FocusNowFound::NotThere;
    };
    let Some(element) = resolver.focused_element(uia, hwnd, &nodes) else {
        return FocusNowFound::NotThere;
    };
    let mut node = with_legacy_checked_state(
        element.live().element(),
        element.snapshot(resolver.registry()),
    );
    // The element's own UIA class, from its cache, for Core's rules for an
    // application's elements (decision D6): no call.
    let fields = element.read();
    node.details.class_name.clone_from(&fields.role.class_name);
    // The raw view's chain the walk up steps through, for the routes to
    // the nodes this read mints.
    let walked = RefCell::new(vec![(element.runtime_id(), fields.role.hwnd)]);
    let visited = |runtime_id: &[i32], hwnd: isize| {
        walked.borrow_mut().push((runtime_id.to_vec(), hwnd));
    };
    resolver.place(&mut node, hwnd);
    let mut parent_automation_id = None;
    let mut table = None;
    let remote = program_enrichment(
        (uia, &ancestor_request),
        &element,
        (&mut parent_automation_id, &mut table, &visited),
        hwnd,
        resolver,
    );
    let enrichment = if let Programmed::Read(enrichment) = remote {
        enrichment.map(|enrichment| *enrichment)
    } else {
        table = element.table();
        let ancestors = resolver.chain(
            element.live(),
            (element.read().role.hwnd, hwnd),
            &mut parent_automation_id,
            &visited,
        );
        ancestors.map(|ancestors| (ancestors, selected(uia, &element, &nodes, resolver)))
    };
    let Some((ancestors, selected_child)) = enrichment else {
        return FocusNowFound::NotThere;
    };
    node.details.parent_automation_id = parent_automation_id;
    node.details.set_table(table);
    // A combo box without the `Value` pattern, or a looping selector: its
    // first selected item is its value.
    let value_from_selection = fields.value_from_selection();
    if value_from_selection {
        node.value = selected_child.and_then(|child| child.name);
    }
    let (mut ancestors, met) = resolver.cut(ancestors);
    resolver.keep_routes(&node, &ancestors, &walked.into_inner());
    let foreground = met
        .is_none()
        .then(|| resolver.foreground(&ancestors))
        .flatten();
    describe_dialogs(
        uia,
        resolver,
        ancestors.iter_mut().chain(std::iter::once(&mut node)),
        Vec::new(),
    );
    FocusNowFound::Read(Box::new(resolver.answer(UiaFocus {
        node,
        ancestors,
        met,
        nearest: hwnd,
        foreground,
        value_from_selection,
        runtime_id_reused: false,
    })))
}

/// `Test` of the application's focus in window `hwnd`, taken already: the
/// focus read as [`focus_now`] reads it, and `matcher` evaluated lazily
/// over that read, `FocusedWithin` answering none, since UI Automation's
/// focus is not tested for an MSAA focused state. Answers what was found,
/// which a `FocusRead` of the focus later in the batch takes instead of
/// reading again, and the test's result, `None` when no focus could be
/// read.
pub fn test_focus(
    uia: &Uia,
    hwnd: isize,
    matcher: &verbatim_model::matcher::Matcher,
    resolver: &dyn FocusResolve,
) -> (
    FocusNowFound,
    Option<(bool, Vec<verbatim_model::property::PropertyAnswer>)>,
) {
    let found = focus_now(uia, hwnd, resolver);
    let FocusNowFound::Read(focus) = &found else {
        return (found, None);
    };
    let mut read = Vec::new();
    let Ok(holds) = matcher.evaluate::<std::convert::Infallible>(&mut |id| {
        let value = match id {
            verbatim_model::property::PropertyId::FocusedWithin => None,
            id => focus
                .focus
                .read
                .values
                .answer(id)
                .and_then(|answer| answer.value.clone()),
        };
        read.push(verbatim_model::property::PropertyAnswer {
            id,
            value: value.clone(),
        });
        Ok(value)
    });
    (found, Some((holds, read)))
}

/// The selected item `element`, a combo box without the `Value` pattern
/// or a looping selector, reads as its value; none for another element,
/// and none read when the application does not answer in time.
fn selected(
    uia: &Uia,
    element: &Cached<Node>,
    nodes: &Request<Node>,
    resolver: &dyn FocusResolve,
) -> Option<NodeSnapshot> {
    if !element.read().value_from_selection() {
        return None;
    }
    match uia.selected_node(element.live(), nodes, resolver.registry()) {
        Ok(selected) => selected,
        Err(error) => {
            tracing::debug!(%error, "the selected item could not be read");
            None
        }
    }
}

/// What the focus ancestry program found: the ancestors, outermost first,
/// `None` when the focus was lost on the chain's way, and the selected
/// item.
type Enrichment = (Vec<NodeSnapshot>, Option<NodeSnapshot>);

/// What the focus ancestry program answered.
enum Programmed {
    /// No program may run: the walk reads the ancestors.
    NotRun,
    /// The element no longer has the keyboard focus, read live: the walk
    /// reads the ancestors.
    NotFocused,
    /// The ancestors and selected item, `None` when the focus was lost on
    /// the chain's way.
    Read(Option<Box<Enrichment>>),
}

/// The ancestors and selected item of `element`, the focus, read by the
/// focus ancestry program in one round trip, with the same stops and the
/// same completion above as the walk: [`Programmed::NotRun`], having made
/// no call, when no program may run in `window`, and
/// [`Programmed::NotFocused`] when the element no longer has the keyboard
/// focus, read live; either is read by the walk instead. A focus with a ghost's or a hung application's window above
/// its own is read by the program's classic twin, which stops at its step
/// into that window. A walk that fails goes on from the parent window of
/// the focus's nearest window; one that ends at a failed or missing
/// parent below the top from the parent window of its outermost element's
/// nearest window, and one cut at its depth by plain windows from that
/// nearest window. `parent_automation_id` and `table` are given the
/// element's raw-view parent's automation id and the table of a cell.
#[expect(
    clippy::too_many_lines,
    reason = "the program, its fallback, and the chain's completion, in the order they run"
)]
fn program_enrichment(
    (uia, request): (&Uia, &Request<Ancestor>),
    element: &Cached<Node>,
    (parent_automation_id, table, visited): (
        &mut Option<String>,
        &mut Option<TableId>,
        verbatim_uia::Visited<'_>,
    ),
    window: isize,
    resolver: &dyn FocusResolve,
) -> Programmed {
    if !resolver.tries_remote(window) {
        return Programmed::NotRun;
    }
    let (known_ids, known) = resolver.known_uia_nodes();
    let input = element.narrow::<FocusInput>();
    let fill = Fill::new(request);
    let Some(focus_request) = resolver.focus_request(uia) else {
        return Programmed::NotRun;
    };
    let element_fill = Fill::new(&focus_request);
    let plain = |hwnd: isize| resolver.unreadable(hwnd);
    let query = FocusQuery {
        element: &input,
        known: &known,
        depth_limit: MAX_ANCESTOR_HOPS,
        fill: &fill,
        element_fill: &element_fill,
        require_focus: true,
        refresh: false,
        nearest_window: None,
        before_call: None,
        value_available: Some(element.read().states.value_available),
        plain: Some(&plain),
        is_cell: Some(element.read().is_cell()),
        progress: None,
    };
    let remote = !resolver.plain_window_above(window);
    let (mut ancestry, path) = match focus_ancestry(uia, &query, remote) {
        Ok((FocusAncestry::Focused(ancestry), path)) => (ancestry, path),
        Ok((FocusAncestry::NotFocused, _)) => return Programmed::NotFocused,
        Err(error) => {
            tracing::debug!(%error, "the focus ancestry could not be read");
            let ancestors = resolver.onward_after_failure(element.read().role.hwnd, window);
            return Programmed::Read(ancestors.map(|ancestors| Box::new((ancestors, None))));
        }
    };
    if let Some(parent) = ancestry.ancestors.first() {
        *parent_automation_id = parent.read().automation_id;
    }
    *table = ancestry.element_table;
    if let Path::Fallback(error) = &path {
        resolver.fell_back("focus ancestry", window, error);
    }
    // A window's own element read inside the provider can lack what the
    // client side's window proxy gives it, its handle and title among
    // them: such an ancestor of this application's is read again through
    // the proxy, one call.
    for ancestor in &mut ancestry.ancestors {
        let Some(hwnd) =
            window_read_without_its_handle(ancestor.read().node.role.hwnd, &ancestor.runtime_id())
                .filter(|&hwnd| resolver.serves(hwnd))
        else {
            continue;
        };
        match uia.window_element(hwnd, request) {
            Ok(read) => *ancestor = read,
            Err(error) => {
                tracing::debug!(hwnd, %error, "a window ancestor could not be read again through its proxy");
            }
        }
    }
    let registry = resolver.registry();
    let selected = ancestry
        .selected_child
        .as_ref()
        .map(|child| child.snapshot(registry));
    let nearest = ancestry.window.unwrap_or(window);
    let read_by_other_api = |hwnd: isize| resolver.leaves(nearest, hwnd);
    let is_known = |id: NodeId| resolver.knows(id);
    let (chain, crossed, mut walked) = Uia::ancestors_from(
        &ancestry.ancestors,
        registry,
        &AncestorStops {
            read_by_other_api: &read_by_other_api,
            known: &is_known,
            out_of_time: None,
            visited: Some(visited),
        },
    );
    // The classic twin stopped at a step into a window read as a plain
    // window: the chain goes on from it.
    let crossed = crossed.or(ancestry.plain_window);
    // The program stops at any known ancestor, while only a reported one
    // ends the chain: a known ancestor that is not reported is still where
    // the previous chain takes over.
    if let (AncestorWalk::Complete, None, Some(index)) = (walked, crossed, ancestry.met_known)
        && let Some(&met) = known_ids.get(index)
    {
        walked = AncestorWalk::MetKnown(met);
    }
    let focus_window = element.read().role.hwnd;
    let outermost = ancestry
        .ancestors
        .last()
        .map_or(focus_window, |ancestor| ancestor.read().node.role.hwnd);
    if walked == AncestorWalk::Complete
        && crossed.is_none()
        && (ancestry.failed_hop.is_some() || ancestry.depth_limited)
    {
        let hwnds = || {
            std::iter::once(focus_window).chain(
                ancestry
                    .ancestors
                    .iter()
                    .map(|ancestor| ancestor.read().node.role.hwnd),
            )
        };
        if let Some(hop) = ancestry.failed_hop {
            walked = failed(hop, nearest, hwnds());
        } else if ancestry.depth_limited {
            walked = AncestorWalk::Cut(verbatim_win32::provider::plain_windows_start(
                nearest,
                hwnds(),
            ));
        }
    }
    let ancestors = resolver.complete(chain, (crossed, walked, outermost));
    tracing::debug!(
        ?path,
        ancestors = ancestry.ancestors.len(),
        "UIA focus ancestry read"
    );
    Programmed::Read(ancestors.map(|ancestors| Box::new((ancestors, selected))))
}

/// A walk that failed at `hop`, going on from the parent window of its
/// outermost element's nearest window.
fn failed(hop: FailedHop, nearest: isize, hwnds: impl Iterator<Item = isize>) -> AncestorWalk {
    AncestorWalk::Failed(
        hop,
        verbatim_win32::provider::parent_window(
            verbatim_win32::provider::outermost_nearest_window(nearest, hwnds),
        ),
    )
}

/// The window whose own element an ancestor is, by its runtime id, `[42,
/// handle]` as UI Automation gives a window's element, when the ancestor
/// was read without that handle (`hwnd`, 0).
fn window_read_without_its_handle(hwnd: isize, runtime_id: &[i32]) -> Option<isize> {
    match runtime_id {
        &[42, handle] if hwnd == 0 && handle != 0 => isize::try_from(handle).ok(),
        _ => None,
    }
}

/// The focus a listener's route names, read as a focus: found and minted
/// by the outpost, its registration added, its ancestors built from the
/// listener's read with no call where its walk met one Core knows and
/// chained above the route otherwise; then, as before the arm, what it
/// minted kept, the foreground object, its states read again once the
/// registration is in with the children of the dialogs it enters, and
/// the text of those dialogs.
///
/// # Errors
///
/// [`RouteMiss`] when the route names no element, or Core named an
/// ancestor its end does not.
pub fn route(uia: &Uia, resolver: &dyn FocusResolve) -> Result<Box<FocusRead>, RouteMiss> {
    let RouteFocus {
        found,
        mut snapshot,
        nearest,
        reused,
        met,
        parents,
        value_from_selection,
    } = resolver.route()?;
    let (mut ancestors, met) = match met {
        Some(met) => {
            let read_by_other_api = |hwnd: isize| resolver.leaves(nearest, hwnd);
            let known = |id: NodeId| resolver.knows(id);
            let (chain, _, _) = Uia::ancestor_chain_from_parts(
                &parents,
                resolver.registry(),
                &AncestorStops {
                    read_by_other_api: &read_by_other_api,
                    known: &known,
                    out_of_time: None,
                    visited: None,
                },
            );
            (chain.into_iter().rev().collect(), Some(met))
        }
        None => resolver.cut(resolver.route_chain().ok_or(RouteMiss::NotThere)?),
    };
    resolver.keep_minted(snapshot.id, &ancestors);
    let foreground = met
        .is_none()
        .then(|| resolver.foreground(&ancestors))
        .flatten();
    let gathered = found.as_ref().map_or_else(Vec::new, |element| {
        states_and_dialogs(
            uia,
            resolver,
            (element, nearest),
            (&mut snapshot, &ancestors),
        )
    });
    describe_dialogs(
        uia,
        resolver,
        ancestors.iter_mut().chain(std::iter::once(&mut snapshot)),
        gathered,
    );
    Ok(Box::new(resolver.answer(UiaFocus {
        node: snapshot,
        ancestors,
        met,
        nearest,
        foreground,
        value_from_selection,
        runtime_id_reused: reused,
    })))
}

/// The focus's states, read once its registration is in, and the text of
/// the dialogs among `ancestors` and the focus whose elements the outpost
/// keeps, their children read in the same round trip ([`focus_states`]),
/// by dialog: the states replace `snapshot`'s, its focused state kept.
/// `window` is the route's.
fn states_and_dialogs(
    uia: &Uia,
    resolver: &dyn FocusResolve,
    (element, window): (&Cached<Node>, isize),
    (snapshot, ancestors): (&mut NodeSnapshot, &[NodeSnapshot]),
) -> Vec<(NodeId, Option<String>)> {
    use verbatim_reader_rules::dialog::dialog_text;
    let dialogs: Vec<(NodeId, Live)> = ancestors
        .iter()
        .chain(std::iter::once(&*snapshot))
        .filter(|node| {
            node.backend == verbatim_model::Backend::Uia && resolver.wants_dialog_text(node)
        })
        .filter_map(|node| Some((node.id, resolver.held_element(node.id)?)))
        .collect();
    let Some(nodes) = resolver.node_request(uia) else {
        return Vec::new();
    };
    let Ok(children) = uia.request_fixed::<DialogText>() else {
        return Vec::new();
    };
    let lives: Vec<Live> = dialogs.iter().map(|(_, live)| live.clone()).collect();
    let fill = Fill::new(&nodes);
    let children_fill = Fill::new(&children);
    let query = StatesQuery {
        focus: element.live(),
        fill: &fill,
        dialogs: &lives,
        children: &children_fill,
    };
    let (states, path) = match focus_states(uia, &query, resolver.tries_remote(window)) {
        Ok(answer) => answer,
        Err(error) => {
            tracing::debug!(%error, "a focus's states could not be read");
            return Vec::new();
        }
    };
    if let Path::Fallback(error) = &path {
        resolver.fell_back("focus states", window, error);
    }
    if let Some(fresh) = states.focus {
        let fresh = with_legacy_checked_state(
            fresh.live().element(),
            fresh.snapshot_in_table(resolver.registry()),
        );
        let focused = snapshot.states.contains(State::Focused);
        snapshot.states = fresh.states;
        if focused {
            snapshot.states = snapshot.states.with(State::Focused);
        }
    }
    dialogs
        .into_iter()
        .zip(states.children)
        .map(|((id, live), read)| {
            let text = match read {
                Some(read) => dialog_text(&UiaDialog::with_children(uia, &children, live, read)),
                None => dialog_text(&UiaDialog::new(uia, &children, live)),
            };
            (id, text)
        })
        .collect()
}

/// Gives every dialog among `nodes`, a focus's chain, that wants its own
/// text that text as its description (`docs/nvda/object-model.md`, "A
/// dialog's own text"), a dialog `read` names, or met twice, given the
/// text gathered already: a UI Automation dialog's gathered here from its
/// children, by `verbatim_reader_rules::dialog`'s rule, an MSAA dialog's
/// through the outpost.
fn describe_dialogs<'a>(
    uia: &Uia,
    resolver: &dyn FocusResolve,
    nodes: impl IntoIterator<Item = &'a mut NodeSnapshot>,
    mut read: Vec<(NodeId, Option<String>)>,
) {
    for node in nodes {
        if !resolver.wants_dialog_text(node) {
            continue;
        }
        let text = if let Some((_, text)) = read.iter().find(|(id, _)| *id == node.id) {
            text.clone()
        } else {
            let started = std::time::Instant::now();
            let text = match node.backend {
                verbatim_model::Backend::Msaa => resolver.other_dialog_text(node),
                verbatim_model::Backend::Uia => uia_dialog_text(uia, resolver, node),
            };
            tracing::debug!(
                found = text.is_some(),
                elapsed_us = started.elapsed().as_micros(),
                "a dialog's text gathered"
            );
            read.push((node.id, text.clone()));
            text
        };
        node.details.description = text;
    }
}

/// The own text of the UI Automation dialog `node`, gathered from its
/// children: nothing of its own element is read.
fn uia_dialog_text(uia: &Uia, resolver: &dyn FocusResolve, node: &NodeSnapshot) -> Option<String> {
    let dialog = resolver.dialog_element(uia, node)?;
    // Each child's description whatever the theme, as NVDA's dialog text
    // includes it.
    let children = uia.request_fixed::<DialogText>().ok()?;
    verbatim_reader_rules::dialog::dialog_text(&UiaDialog::new(uia, &children, dialog))
}

#[cfg(test)]
mod tests {
    use super::window_read_without_its_handle;

    /// Only a window's own element read without its handle, as the console
    /// host's window is read inside its provider, is read again through
    /// its proxy: its handle comes from its runtime id.
    #[test]
    fn a_window_element_read_without_its_handle_is_named_by_its_runtime_id() {
        assert_eq!(
            window_read_without_its_handle(0, &[42, 76_482_230]),
            Some(76_482_230)
        );
        assert_eq!(
            window_read_without_its_handle(76_482_230, &[42, 76_482_230]),
            None
        );
        assert_eq!(
            window_read_without_its_handle(0, &[42, 76_482_230, 4, -1]),
            None
        );
        assert_eq!(window_read_without_its_handle(0, &[7, 76_482_230]), None);
    }
}
