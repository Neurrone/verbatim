//! NVDA's equality of two MSAA objects (`IAccessible._isEqual`), by which a
//! focus walk gives a new sighting the node id of a kept node it is equal
//! to (`docs/design/focus-pipeline-rework.md`, "MSAA object equality").
//!
//! The kept nodes compared with are the [`Candidates`] the outpost gives a
//! walk: the nodes of the focus line Core holds and of the newest focus the
//! outpost reported that Core has not judged, each line innermost first,
//! NVDA's meeting order. A sighting the registry does not already place, by
//! COM identity or by its address, is compared with each in NVDA's order
//! with its role and location steps swapped: the child id; the event
//! window, event object id and event child id, each where both are known;
//! the window; the identity string, where both asked for it; the raw role;
//! the location, the kept object's read live; and the name, the kept
//! object's read live when the kept name differs. NVDA compares the
//! location before the role; since each step only rejects, the order
//! changes no verdict, and the role, already read, rejects first, saving
//! the location reads of a candidate of another role. The first
//! equal candidate gives its id, and nothing else: the node keeps its old
//! object, address and event parameters, as NVDA keeps its old objects
//! through the meeting point, and takes the sighting's name. After a reissue
//! from a line's entry only the entries outward of it are scanned, in every
//! line that has it, so within one walk a candidate gives its id at most
//! once. `IAccessible2`'s unique
//! ID, NVDA's second step, waits for milestone M7.

#![forbid(unsafe_code)]

use std::cell::RefCell;
use std::collections::HashMap;

use windows::Win32::UI::Accessibility::IAccessible;
use windows::core::AgileReference;

use verbatim_model::{NodeId, Rect, Role};

use crate::accessible::Accessible;
use crate::registry::{Facts, Interning, MsaaKey};

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
    /// Its location, when its read already made it (`None` for a failed
    /// read), else `None`, read when a candidate first needs it.
    #[expect(clippy::option_option, reason = "a location, once read, may be none")]
    pub(crate) location: Option<Option<Rect>>,
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
    registry: &dyn Interning,
    candidates: &Candidates,
    sighting: &Sighting<'_>,
    name: impl Fn(&Accessible, MsaaKey, Role) -> Option<String>,
) -> Option<NodeId> {
    let child = sighting.acc.child();
    let mine = sighting.facts;
    let mut location = sighting.location;
    let mut scan = candidates.scan.borrow_mut();
    let Scan { floors, live } = &mut *scan;
    for (line, ids) in candidates.lines.iter().enumerate() {
        for &id in ids.iter().skip(floors[line]) {
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
            // 6. The raw role, before the location, which NVDA compares
            // first: each step only rejects, and this one needs no read.
            if theirs.raw_role != mine.raw_role {
                continue;
            }
            // 7. The location: the sighting's, read once, against the kept
            // object's, read live once per walk. A kept object reached
            // through `accParent` has oleacc walk its parents for it.
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
            // 9. Equal: the node gives its id, and only the entries outward
            // of it, in every line that has it, are scanned from now on, so
            // it gives its id at most once.
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicU64;

    use windows::Win32::UI::WindowsAndMessaging::OBJID_CLIENT;

    use super::*;
    use crate::registry::{Held, NodeIdRegistry};

    /// The desktop window's client object, served in this process, which
    /// every test sights and keeps: its location reads the same each time.
    fn desktop() -> Accessible {
        crate::accessible::join_mta();
        Accessible::client_of_window(crate::window::desktop()).expect("the desktop's client")
    }

    /// The facts both sides have unless a test changes one.
    fn facts() -> Facts {
        Facts {
            event: (crate::window::desktop(), None, 0),
            identity: Some(Some(vec![1, 2, 3])),
            raw_role: Some(10),
            name: Some("Desktop".to_owned()),
        }
    }

    /// A registry keeping one node, the desktop's client at child id 0
    /// with `facts`, returned with its id.
    fn kept(facts: Facts) -> (NodeIdRegistry, NodeId) {
        let registry = NodeIdRegistry::new(Arc::new(AtomicU64::new(1)));
        let object = desktop();
        let held = Held {
            object: object.agile().expect("an agile reference"),
            child: 0,
            identity: object.canonical().expect("its identity"),
        };
        let key = (crate::window::desktop(), OBJID_CLIENT.0, 0);
        let id = registry.insert(key, Some(held), (Role::Pane, facts), false);
        (registry, id)
    }

    /// The node `sighting`, with `facts`, at child id `child` in window
    /// `window`, is given among `candidates`.
    fn compared(
        registry: &dyn Interning,
        candidates: &Candidates,
        (window, child): (isize, i32),
        facts: &Facts,
    ) -> Option<NodeId> {
        let acc = desktop().with_child(child);
        let location = Some(acc.with_child(0).location());
        let sighting = Sighting {
            acc: &acc,
            key: (window, OBJID_CLIENT.0, child),
            facts,
            location,
        };
        equal_candidate(registry, candidates, &sighting, |_, _, _| {
            panic!("the names are the same, so none is read live")
        })
    }

    /// Whether a sighting of the desktop's client, otherwise equal to the
    /// kept node, is given its id with `change` made to its facts, at
    /// `child` in `window`.
    fn given(change: impl FnOnce(&mut Facts), (window, child): (isize, i32)) -> bool {
        let (registry, id) = kept(facts());
        let mut mine = facts();
        change(&mut mine);
        let candidates = Candidates::new(vec![vec![id]]);
        compared(&registry, &candidates, (window, child), &mine) == Some(id)
    }

    #[test]
    fn an_equal_sighting_is_given_the_kept_nodes_id() {
        assert!(given(|_| {}, (crate::window::desktop(), 0)));
    }

    #[test]
    fn a_sighting_at_another_child_id_is_not_given_the_id() {
        assert!(!given(|_| {}, (crate::window::desktop(), 4)));
    }

    #[test]
    fn a_sighting_with_another_event_child_id_is_not_given_the_id() {
        assert!(!given(
            |mine| mine.event.2 = 4,
            (crate::window::desktop(), 0)
        ));
    }

    #[test]
    fn a_sighting_in_another_window_is_not_given_the_id() {
        assert!(!given(|_| {}, (crate::window::desktop() + 1, 0)));
    }

    #[test]
    fn a_sighting_with_another_identity_string_is_not_given_the_id() {
        assert!(!given(
            |mine| mine.identity = Some(Some(vec![9])),
            (crate::window::desktop(), 0)
        ));
    }

    #[test]
    fn a_sighting_with_another_raw_role_is_not_given_the_id() {
        assert!(!given(
            |mine| mine.raw_role = Some(20),
            (crate::window::desktop(), 0)
        ));
    }

    /// A candidate gives its id once in a walk: a second equal sighting
    /// gets none.
    #[test]
    fn a_candidate_gives_its_id_at_most_once() {
        let (registry, id) = kept(facts());
        let candidates = Candidates::new(vec![vec![id]]);
        let window = (crate::window::desktop(), 0);
        let given = [
            compared(&registry, &candidates, window, &facts()),
            compared(&registry, &candidates, window, &facts()),
        ];
        assert_eq!(given, [Some(id), None]);
    }
}
