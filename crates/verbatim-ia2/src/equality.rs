//! NVDA's equality of two MSAA objects (`IAccessible._isEqual`), by which a
//! focus walk gives a new sighting the node id of a kept node it is equal
//! to (`docs/design/focus-pipeline-rework.md`, "MSAA object equality").
//!
//! The kept nodes compared with are the [`Candidates`] the outpost gives a
//! walk: the nodes of the focus line Core holds and of the newest focus the
//! outpost reported that Core has not judged, each line innermost first,
//! NVDA's meeting order. A sighting the registry does not already place, by
//! COM identity or by its address, is compared with each in NVDA's order:
//! the child id; the event window, event object id and event child id, each
//! where both are known; the window; the identity string, where both asked
//! for it; the raw role; the location, the kept object's read live; and the
//! name, the kept object's read live when the kept name differs. The first
//! equal candidate gives its id, and nothing else: the node keeps its old
//! object, address and event parameters, as NVDA keeps its old objects
//! through the meeting point, and takes the sighting's name. Within one walk
//! a candidate gives its id at most once, and after a reissue from a line's
//! entry only the entries outward of it are scanned. `IAccessible2`'s unique
//! ID, NVDA's second step, waits for milestone M7.

#![forbid(unsafe_code)]

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use windows::Win32::UI::Accessibility::IAccessible;
use windows::core::AgileReference;

use verbatim_model::{NodeId, Rect, Role};

use crate::accessible::Accessible;
use crate::registry::{Facts, MsaaKey, NodeIdRegistry};

/// The kept nodes a focus walk compares each new MSAA sighting with, and
/// what the walk has learned of them: lines of node ids, each innermost
/// first, the newest focus's line after Core's.
#[derive(Debug, Default)]
pub struct Candidates {
    lines: Vec<Vec<NodeId>>,
    scan: RefCell<Scan>,
}

/// What a walk has learned while comparing ([`Candidates`]).
#[derive(Debug, Default)]
struct Scan {
    /// Per line, the first entry still scanned.
    floors: Vec<usize>,
    /// The candidates that have given their id in this walk.
    used: HashSet<NodeId>,
    /// Each candidate's live reads, made at most once per walk.
    live: HashMap<NodeId, Live>,
}

/// A candidate's live reads ([`Scan::live`]).
#[derive(Clone, Debug, Default)]
struct Live {
    location: Option<Located>,
    /// Its name once read, `None` before.
    #[expect(clippy::option_option, reason = "a name, once read, may be none")]
    name: Option<Option<String>>,
}

/// Where a kept object is, read live.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Located {
    /// Its location, `None` when the read failed.
    At(Option<Rect>),
    /// The kept object no longer resolves, or answers as gone.
    Gone,
}

impl Candidates {
    /// The candidates of `lines`, node ids each innermost first.
    #[must_use]
    pub fn new(lines: Vec<Vec<NodeId>>) -> Self {
        let floors = vec![0; lines.len()];
        Self {
            lines,
            scan: RefCell::new(Scan {
                floors,
                ..Scan::default()
            }),
        }
    }
}

/// A new sighting of an MSAA object as it is compared ([`equal_candidate`]).
pub(crate) struct Sighting<'a> {
    pub(crate) acc: &'a Accessible,
    pub(crate) key: MsaaKey,
    pub(crate) facts: &'a Facts,
}

/// What NVDA's `IAccessible.__init__` fixes for an object read at `key`:
/// the event's address for one acquired at it, else its window, no object
/// id, and its child id `child`.
pub(crate) fn event_of(key: MsaaKey, at_address: bool, child: i32) -> (isize, Option<i32>, i32) {
    if at_address {
        (key.0, Some(key.1), key.2)
    } else {
        (key.0, None, child)
    }
}

/// The kept node among `candidates` that `sighting` is equal to, by NVDA's
/// order, if one is; that node then takes the sighting's name, and is not
/// compared again in this walk. `name` normalises a kept object's live
/// name, of its role and at its address, as a sighting's is.
pub(crate) fn equal_candidate(
    registry: &NodeIdRegistry,
    candidates: &Candidates,
    sighting: &Sighting<'_>,
    name: impl Fn(&Accessible, MsaaKey, Role) -> Option<String>,
) -> Option<NodeId> {
    let child = sighting.acc.child();
    let mine = sighting.facts;
    let mut location: Option<Option<Rect>> = None;
    let mut scan = candidates.scan.borrow_mut();
    let Scan { floors, used, live } = &mut *scan;
    for (line, ids) in candidates.lines.iter().enumerate() {
        for &id in ids.iter().skip(floors[line]) {
            if used.contains(&id) {
                continue;
            }
            let Some(kept) = registry.facts_of(id) else {
                continue;
            };
            let Some((object, kept_child)) = &kept.object else {
                continue;
            };
            let theirs = &kept.facts;
            // 1. The child id.
            if *kept_child != child {
                continue;
            }
            // 3. The event window, object id and child id, each where both
            // are known.
            let (window, object_id, child_id) = theirs.event;
            if window != mine.event.0
                || child_id != mine.event.2
                || matches!((object_id, mine.event.1), (Some(one), Some(other)) if one != other)
            {
                continue;
            }
            // 4. The window.
            if kept.key.0 != sighting.key.0 {
                continue;
            }
            // 5. The identity string, where both asked, absence included.
            if let (Some(one), Some(other)) = (&theirs.identity, &mine.identity)
                && one != other
            {
                continue;
            }
            // 6. The raw role.
            if theirs.raw_role != mine.raw_role {
                continue;
            }
            // 7. The location: the sighting's, read once, against the kept
            // object's, read live once per walk.
            let at = *location.get_or_insert_with(|| sighting.acc.location());
            let reads = live.entry(id).or_default();
            let located = *reads
                .location
                .get_or_insert_with(|| live_location(object, *kept_child));
            if located != Located::At(at) {
                continue;
            }
            // 8. The name, the kept object's read live when the kept one
            // differs.
            if theirs.name != mine.name {
                let live_name = reads.name.get_or_insert_with(|| {
                    object.resolve().ok().and_then(|object| {
                        name(
                            &Accessible::new(object, *kept_child),
                            kept.key,
                            kept.role.unwrap_or(Role::Unknown),
                        )
                    })
                });
                if *live_name != mine.name {
                    continue;
                }
            }
            // 9. Equal: the node gives its id, once, and only the entries
            // outward of it are scanned from now on.
            used.insert(id);
            for (line, ids) in candidates.lines.iter().enumerate() {
                if let Some(at) = ids.iter().position(|&other| other == id) {
                    floors[line] = floors[line].max(at + 1);
                }
            }
            registry.reissue(id, mine.name.clone());
            return Some(id);
        }
    }
    None
}

/// Where the kept object `object`, at child id `child`, is now.
fn live_location(object: &AgileReference<IAccessible>, child: i32) -> Located {
    let Ok(object) = object.resolve() else {
        return Located::Gone;
    };
    match Accessible::new(object, child).location_read() {
        Ok(rect) => Located::At(Some(rect)),
        Err(error) if crate::acquire::disconnected(&error) => Located::Gone,
        Err(_) => Located::At(None),
    }
}
