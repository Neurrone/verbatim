//! `Follow`, Core's request at each commit (design, "Changes on objects"
//! and Table 10): it names the committed focus's node to the outpost that
//! holds it, with its event group and any extra object, so that the
//! outpost keeps that node's registration and removes the rest; and it
//! names nothing to an outpost whose node was followed when the commit's
//! source leaves it, as NVDA's lose-focus removes the old focus's group
//! whatever application the new focus is in. The old focus stays followed
//! until the commit, and a completion whose focus is not executed has its
//! registration removed by a `Follow` naming what Core names there.
//!
//! Core keeps only the node it last named per outpost. A `Follow`'s
//! ending changes nothing in Core: its answer serves the test harness's
//! idle, and a newer `Follow` replaces it. When the named node's window is
//! destroyed, Core sends that outpost a `Follow` naming nothing as its own
//! request (column E).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use verbatim_model::request::{FollowGroup, Followed};
use verbatim_model::{NodeSnapshot, OutpostId, Pid, Role, WindowHandle};

use super::versions::{Kept, ReadBy};
use crate::batch::{Builder, On, Sent};
use crate::entries::Entries;
use crate::event_queue::pairs;
use crate::event_queue::{CoreRequest, Cx, OTHER_DUE_US, Reader, RequestWindow};

/// The class of a console host's window, whose terminal is followed
/// without its text and caret events.
pub(crate) const CONSOLE_WINDOW_CLASS: &str = "ConsoleWindowClass";

/// The node Core last named in `Follow` to one outpost (Table 10's rows).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Named {
    /// The outpost generation that holds it.
    pub(crate) generation: OutpostId,
    /// What was named.
    pub(crate) followed: Followed,
    /// The window the `Follow` named, for the hang check of the next one.
    pub(crate) window: RequestWindow,
    /// The node's own nearest window, whose destruction drops its
    /// registration.
    pub(crate) node_window: WindowHandle,
}

/// The node Core last named per outpost, by the application's process:
/// an outpost absent here is named nothing.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Follows {
    #[serde(with = "pairs")]
    named: BTreeMap<Pid, Named>,
}

/// One `Follow` to send to an application's outpost.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FollowRequest {
    /// The application.
    pub(crate) process: Pid,
    /// The window it names for the hang check.
    pub(crate) window: RequestWindow,
    /// The node followed; `None` removes every registration.
    pub(crate) follow: Option<Followed>,
}

impl FollowRequest {
    /// The `Follow` as Core's own request, sent in the inbox's order and
    /// awaited by nothing, under the other requests' due time.
    pub(crate) fn own(self) -> CoreRequest {
        CoreRequest {
            token: None,
            reader: Reader::Outpost(self.process),
            window: self.window,
            request: batch(self.follow.as_ref()).question(),
            due_after_us: OTHER_DUE_US,
        }
    }
}

/// The brand of a `Follow`'s batch, named for its one site, [`batch`]:
/// its batches are sent outside the queue or held on Core's want list
/// (the request list's design, section 4).
pub(crate) enum FollowBatch {}

/// The one-step batch that follows `node` with its event group and extra
/// object, or nothing.
pub(crate) fn batch(node: Option<&Followed>) -> Sent<FollowBatch> {
    let mut batch = Builder::<FollowBatch>::named();
    let _ = batch
        .follow(node.map(|followed| (On::from(followed.node), followed.group, followed.extra)));
    batch.finish()
}

/// The UI Automation class of Word's document, the one focus outside a
/// terminal that takes text-change events (`docs/parity/backends.md`,
/// "Which focus takes text-change events").
const WORD_DOCUMENT_CLASS: &str = "_WwG";

/// The event group of the focus node `node`: a terminal's, with whether it
/// is a console host's; the extended group, which adds text changes, for
/// Word's document alone; the base group, whose caret event serves every
/// other text field, otherwise (`docs/parity/backends.md`, "Which focus
/// takes text-change events").
fn group(node: &NodeSnapshot, console: bool) -> FollowGroup {
    if node.role == Role::Terminal {
        FollowGroup::Terminal { console }
    } else if node.details.class_name.as_deref() == Some(WORD_DOCUMENT_CLASS) {
        FollowGroup::Extended
    } else {
        FollowGroup::Base
    }
}

impl Follows {
    /// What Core names to `process`'s outpost, if anything.
    #[cfg(test)]
    pub(crate) fn named(&self, process: Pid) -> Option<&Named> {
        self.named.get(&process)
    }

    /// A focus is committed (columns A and B). `committed` is what Core
    /// keeps of it when it is a reader's focus, with its node `node` and
    /// its innermost ancestor `parent`, and whether it is a console host's
    /// terminal; `None` for a focus Core committed itself. A UI Automation
    /// node is named to its outpost; every other outpost whose node was
    /// followed is named nothing, as is that outpost when the focus has no
    /// UI Automation element.
    pub(crate) fn committed(
        &mut self,
        committed: Option<(&Kept, &NodeSnapshot, Option<&NodeSnapshot>)>,
        console: bool,
    ) -> Vec<FollowRequest> {
        let named = committed.and_then(|(kept, node, parent)| {
            // Only an outpost's UI Automation node is followed; a plain
            // window, which no reader holds, has nothing to follow.
            let generation = node.id.outpost()?;
            matches!(kept.read_by, ReadBy::Uia { .. }).then(|| {
                let extra = parent
                    .filter(|parent| crate::focus_rules::is_sensitive_slider(parent))
                    .map(|parent| parent.id);
                (
                    kept.asked.process,
                    Named {
                        generation,
                        followed: Followed {
                            node: node.id,
                            group: group(node, console),
                            extra,
                        },
                        window: kept.asked,
                        node_window: kept.place.window,
                    },
                )
            })
        });
        let mut requests: Vec<FollowRequest> = std::mem::take(&mut self.named)
            .into_iter()
            .filter(|(process, _)| named.as_ref().is_none_or(|(to, _)| to != process))
            .map(|(process, left)| FollowRequest {
                process,
                window: left.window,
                follow: None,
            })
            .collect();
        if let Some((process, named)) = named {
            requests.push(FollowRequest {
                process,
                window: named.window,
                follow: Some(named.followed.clone()),
            });
            self.named.insert(process, named);
        }
        requests
    }

    /// A completion `process`'s outpost answered, read on `window`, ended
    /// without its focus executed (column C): a `Follow` naming what Core
    /// names there, which removes the completion's registration.
    pub(crate) fn unexecuted(&self, process: Pid, window: RequestWindow) -> FollowRequest {
        match self.named.get(&process) {
            Some(named) => FollowRequest {
                process,
                window: named.window,
                follow: Some(named.followed.clone()),
            },
            None => FollowRequest {
                process,
                window,
                follow: None,
            },
        }
    }

    /// `window` was destroyed (column E, 2026-11-19): a node named in it
    /// is named no more, and its outpost is sent a `Follow` naming nothing,
    /// which removes the registration. Core is the one judge of what is
    /// followed: the outpost judges nothing at a destroy. The generation
    /// named is always its process's ready one, since only a ready
    /// generation's answer is committed and its end names nothing
    /// ([`outpost_ended`](Self::outpost_ended)), so the request never
    /// starts an outpost.
    pub(crate) fn window_destroyed(&mut self, window: WindowHandle) -> Vec<FollowRequest> {
        let mut requests = Vec::new();
        self.named.retain(|&process, named| {
            if named.node_window != window {
                return true;
            }
            requests.push(FollowRequest {
                process,
                window: named.window,
                follow: None,
            });
            false
        });
        requests
    }

    /// The outpost generation `generation` of `process` ended (column F):
    /// it is named nothing; the next generation is named at the next
    /// commit.
    pub(crate) fn outpost_ended(&mut self, process: Pid, generation: OutpostId) {
        if self
            .named
            .get(&process)
            .is_some_and(|named| named.generation == generation)
        {
            self.named.remove(&process);
        }
    }
}

/// Sends `requests`, in order with the queue's requests, under the other
/// requests' due time.
pub(crate) fn send(cx: &mut Cx<Entries>, requests: Vec<FollowRequest>) {
    let due_us = cx.due_in(OTHER_DUE_US);
    for request in requests {
        let asked =
            batch(request.follow.as_ref()).of(Reader::Outpost(request.process), request.window);
        cx.send(&asked, due_us);
    }
}

#[cfg(test)]
#[path = "follow_tests.rs"]
mod tests;
