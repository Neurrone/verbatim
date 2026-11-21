//! The batches the shell asks from outside Core's event queue, and the
//! answers it builds from them (the request list's design, section 6,
//! `DumpTree`, `ShellItems` and `ShellItemPosition`): the control plane's
//! tree dump, a walk down the main window's tree, and the notification
//! area list's two reads, walks of the taskbar's windows in File
//! Explorer's outpost, so Core makes no UI Automation connection into the
//! shell's windows
//! (`books/dev/src/explanation/design/listener-without-connections.md`,
//! decision 9).
//!
//! The shell asks them and builds its answers from them; each batch is
//! built by one function here, under a brand named for it, and its answer
//! parsed against the batch the asker holds. A list is
//! complete or nothing: a walk the node cap or a failed read cut, or a
//! taskbar window not there, fails the list; an overflow window not shown
//! is not there and adds nothing.

use crate::batch::{Answers, Builder, Got, On, Ref, Sent, kind};
use verbatim_model::batch::{Descended, View, Walk, WindowOf};
use verbatim_model::matcher::Matcher;
use verbatim_model::property::{PropertyId, PropertyValue, RuntimeId};
use verbatim_model::protocol::DumpedTree;
use verbatim_model::request::{NodeRead, Reply};
use verbatim_model::{
    Backend, Role, ShellCandidate, ShellCandidates, ShellItem, ShellItemKind, State, TreeNode,
    WindowHandle,
};

/// How deep the tree dump walks below the main window.
const DUMP_DEPTH: u32 = 64;

/// The most nodes the tree dump visits and answers.
const DUMP_NODES: u32 = 4096;

/// What the tree dump reads of each node: what a node's snapshot holds,
/// its runtime id, which marks a UI Automation element, and, when
/// locations are read, its rectangle.
const DUMP_SET: [PropertyId; 14] = [
    PropertyId::Name,
    PropertyId::Role,
    PropertyId::Value,
    PropertyId::States,
    PropertyId::Description,
    PropertyId::KeyboardShortcut,
    PropertyId::PositionInSet,
    PropertyId::SetSize,
    PropertyId::Level,
    PropertyId::ClassName,
    PropertyId::NativeWindow,
    PropertyId::Cell,
    PropertyId::TableCounts,
    PropertyId::RuntimeId,
];

/// The brand of the tree dump's batch, built by [`dump`].
pub enum Dump {}

/// The tree dump's one step.
const DUMPED: Ref<Dump, kind::Descendants> = Ref::named(0);

/// The tree dump of the application whose main window is `main`, each
/// node's location read only with `locations`: its main window's own
/// object and every node below it, through UI Automation whatever serves
/// the window (Claude, 2026-11-22: Core names a window's backend, and the
/// shell holds no verdict, so an MSAA application's tree is UI
/// Automation's view of it).
#[must_use]
pub fn dump(main: WindowHandle, locations: bool) -> Sent<Dump> {
    let mut set = DUMP_SET.to_vec();
    if locations {
        set.push(PropertyId::Rect);
    }
    let mut batch = Builder::<Dump>::named();
    batch.outside();
    let _ = batch.descendants(
        On::window(WindowOf::Handle(main), Backend::Uia, false),
        Walk {
            depth: DUMP_DEPTH,
            view: View::Raw,
            matcher: None,
            prune: None,
            include_subject: true,
            limit: DUMP_NODES,
            nodes: DUMP_NODES,
            set,
        },
    );
    batch.finish()
}

/// The tree `reply`, the answer to `sent`, the dump of `locations`
/// asked, describes: its first node the root, each other under the node
/// at its parent's place; truncated when the walk was cut.
///
/// # Errors
///
/// A reason the dump has no tree: an answer that does not parse, or a
/// walk that did not run.
pub fn dumped(sent: &Sent<Dump>, reply: Reply, locations: bool) -> Result<DumpedTree, String> {
    let answers = Answers::parse(sent, reply).ok_or("the answer did not parse")?;
    let walked = match answers.get(DUMPED) {
        Got::Done(walked) => walked,
        other => return Err(format!("the walk did not run: {other:?}")),
    };
    let snapshots: Vec<_> = walked
        .nodes
        .iter()
        .map(|descendant| {
            let backend = if runtime_id(&descendant.node).is_some() {
                Backend::Uia
            } else {
                Backend::Msaa
            };
            descendant.node.snapshot(backend)
        })
        .collect::<Option<_>>()
        .ok_or("a node was answered with no node id")?;
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); walked.nodes.len()];
    for (place, descendant) in walked.nodes.iter().enumerate().skip(1) {
        let parent = descendant
            .parent
            .and_then(|parent| usize::try_from(parent).ok())
            .filter(|&parent| parent < place)
            .ok_or("a node was answered with no parent before it")?;
        if let Some(siblings) = children.get_mut(parent) {
            siblings.push(place);
        }
    }
    let mut snapshots: Vec<_> = snapshots.into_iter().map(Some).collect();
    let root = tree(0, &children, &mut snapshots).ok_or("the walk answered no root")?;
    Ok(DumpedTree {
        root,
        truncated: !walked.complete,
        locations,
    })
}

/// The tree under the node at `place`, its children by place in
/// `children`, each node's snapshot taken from `snapshots`.
fn tree(
    place: usize,
    children: &[Vec<usize>],
    snapshots: &mut [Option<verbatim_model::NodeSnapshot>],
) -> Option<TreeNode> {
    let snapshot = snapshots.get_mut(place)?.take()?;
    Some(TreeNode {
        snapshot,
        children: children
            .get(place)?
            .iter()
            .filter_map(|&child| tree(child, children, snapshots))
            .collect(),
    })
}

/// The depth the shell's walks go to below their windows: the shell trees
/// of interest are shallow, so anything deeper is runaway recursion.
const SHELL_DEPTH: u32 = 16;

/// The most nodes one shell walk visits and answers.
const SHELL_NODES: u32 = 1024;

/// The taskbar's window class.
const TASKBAR: &str = "Shell_TrayWnd";

/// The notification area's window class, a window inside the taskbar's.
const NOTIFICATION_AREA: &str = "TrayNotifyWnd";

/// The tray overflow flyout's window classes, on Windows 11 and on
/// Windows 10, each read only while it is shown, since a hidden one's
/// items report stale rectangles.
const OVERFLOW: [&str; 2] = [
    "TopLevelWindowForOverflowXamlIsland",
    "NotifyIconOverflowWindow",
];

/// The brand of the shell's batches, built by [`shell`].
pub enum Shell {}

/// The shell's walks of `kind`, at most three, in the shell's order.
const SHELL_WALKS: [Ref<Shell, kind::Descendants>; 3] =
    [Ref::named(0), Ref::named(1), Ref::named(2)];

/// A shell button the lists show: a button, toggle button included, with
/// a name, that is not off the screen. A button's own subtree is walked
/// too, since a walk goes on below a node it answers, where none is.
fn listed() -> Matcher {
    Matcher::All(vec![
        Matcher::IsAnyOf(
            PropertyId::Role,
            vec![
                PropertyValue::Role(Role::Button),
                PropertyValue::Role(Role::ToggleButton),
            ],
        ),
        Matcher::Not(Box::new(Matcher::Absent(PropertyId::Name))),
        Matcher::Not(Box::new(Matcher::HasState(State::Offscreen))),
    ])
}

/// The batch that walks the shell's surface `kind` in the taskbar's
/// process: for the tray, the notification area and each overflow window
/// shown; for the taskbar, the taskbar with the notification area's
/// subtree left out. Each listed button found is answered with its name
/// and runtime id; with `chosen`, only the buttons whose runtime id or
/// name is its, each with its states and rectangle, for a click on it.
/// The taskbar's windows are read through UI Automation whatever serves
/// them.
#[must_use]
pub fn shell(kind: ShellItemKind, chosen: Option<&ShellItem>) -> Sent<Shell> {
    let matcher = match chosen {
        None => listed(),
        Some(chosen) => {
            let mut same = vec![Matcher::Is(
                PropertyId::Name,
                PropertyValue::Text(chosen.name.clone()),
            )];
            if !chosen.runtime_id.is_empty() {
                same.insert(
                    0,
                    Matcher::Is(
                        PropertyId::RuntimeId,
                        PropertyValue::RuntimeId(RuntimeId(chosen.runtime_id.clone())),
                    ),
                );
            }
            Matcher::All(vec![listed(), Matcher::Any(same)])
        }
    };
    let mut set = vec![PropertyId::Name, PropertyId::RuntimeId];
    if chosen.is_some() {
        set.extend([PropertyId::States, PropertyId::Rect]);
    }
    let walk = |prune| Walk {
        depth: SHELL_DEPTH,
        view: View::Control,
        matcher: Some(matcher.clone()),
        prune,
        include_subject: true,
        limit: SHELL_NODES,
        nodes: SHELL_NODES,
        set: set.clone(),
    };
    let window = |classes: &[&str], visible| {
        On::window(
            WindowOf::ClassPath(classes.iter().map(|&class| class.to_owned()).collect()),
            Backend::Uia,
            visible,
        )
    };
    let mut batch = Builder::<Shell>::named();
    batch.outside();
    match kind {
        ShellItemKind::SystemTray => {
            let _ = batch.descendants(window(&[TASKBAR, NOTIFICATION_AREA], false), walk(None));
            for overflow in OVERFLOW {
                let _ = batch.descendants(window(&[overflow], true), walk(None));
            }
        }
        ShellItemKind::Taskbar => {
            let notification_area = Matcher::Is(
                PropertyId::ClassName,
                PropertyValue::Text(NOTIFICATION_AREA.to_owned()),
            );
            let _ = batch.descendants(window(&[TASKBAR], false), walk(Some(notification_area)));
        }
    }
    batch.finish()
}

/// The walks `reply`, the answer to `sent`, holds, in order, and whether
/// every walk that ran was complete; `None` when the answer does not
/// parse or the first window, the taskbar's or its notification area's,
/// could not be walked.
fn walks(sent: &Sent<Shell>, reply: Reply) -> Option<(Vec<NodeRead>, bool)> {
    let answers = Answers::parse(sent, reply)?;
    let mut nodes = Vec::new();
    let mut complete = true;
    let asked = sent.batch().steps.len();
    for (place, walk) in SHELL_WALKS.into_iter().take(asked).enumerate() {
        match answers.get(walk) {
            Got::Done(Descended {
                nodes: found,
                complete: whole,
            }) => {
                nodes.extend(found.iter().map(|descendant| descendant.node.clone()));
                complete &= *whole;
            }
            // An overflow window not shown.
            Got::NotThere(_) if place > 0 => {}
            _ => return None,
        }
    }
    Some((nodes, complete))
}

/// The listed button `node` is, with a name that is not blank.
fn item(node: &NodeRead) -> Option<ShellItem> {
    let name = match node
        .values
        .answer(PropertyId::Name)
        .and_then(|answer| answer.value.as_ref())
    {
        Some(PropertyValue::Text(name)) if !name.trim().is_empty() => name.clone(),
        _ => return None,
    };
    Some(ShellItem {
        name,
        runtime_id: runtime_id(node).map(|id| id.0.clone()).unwrap_or_default(),
    })
}

/// `node`'s runtime id, when it was read.
fn runtime_id(node: &NodeRead) -> Option<&RuntimeId> {
    match node
        .values
        .answer(PropertyId::RuntimeId)
        .and_then(|answer| answer.value.as_ref())
    {
        Some(PropertyValue::RuntimeId(id)) => Some(id),
        _ => None,
    }
}

/// The items `reply`, the answer to the list's batch `sent`, holds: the
/// whole list, or `None` when a walk left a button unread, so a list
/// missing part of the shell's buttons is never shown as the list.
#[must_use]
pub fn items(sent: &Sent<Shell>, reply: Reply) -> Option<Vec<ShellItem>> {
    let (nodes, complete) = walks(sent, reply)?;
    complete.then(|| nodes.iter().filter_map(item).collect())
}

/// The buttons `reply`, the answer to the position's batch `sent`, holds
/// that may be the item clicked, each with where it is now, and whether
/// every walk was complete; `None` when the taskbar's window could not be
/// walked.
#[must_use]
pub fn candidates(sent: &Sent<Shell>, reply: Reply) -> Option<ShellCandidates> {
    let (nodes, complete) = walks(sent, reply)?;
    let candidates = nodes
        .iter()
        .filter_map(|node| {
            let value = |id| {
                node.values
                    .answer(id)
                    .and_then(|answer| answer.value.as_ref())
            };
            Some(ShellCandidate {
                item: item(node)?,
                offscreen: matches!(
                    value(PropertyId::States),
                    Some(PropertyValue::States(states)) if states.contains(State::Offscreen)
                ),
                rect: match value(PropertyId::Rect) {
                    Some(PropertyValue::Rect(rect)) => Some(*rect),
                    _ => None,
                },
            })
        })
        .collect();
    Some(ShellCandidates {
        candidates,
        complete,
    })
}

#[cfg(test)]
#[path = "outside_tests.rs"]
mod tests;
