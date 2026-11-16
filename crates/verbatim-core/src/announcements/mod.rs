//! Announcements as event-queue entries (`docs/design/readers-tell-core-asks-build1.md`,
//! "Announcements" and Table 12): UI Automation notifications, toasts, UI
//! Automation selections, MSAA alerts and shows, each judged at its turn by
//! its kind's async function, in its own module.
//!
//! Each kind's entry is one [`Announcement`], the tell with what its kind
//! parsed from it: the queue's rules read the kind for its place, its
//! merge, its window and whether the foreground stop reaches it.

pub(crate) mod alert;
pub(crate) mod notification;
pub(crate) mod selection;
pub(crate) mod show;
pub(crate) mod toast;

use serde::{Deserialize, Serialize};
use verbatim_model::event_kind::EventKind;
use verbatim_model::property::{PropertyValue, RuntimeId};
use verbatim_model::request::{MsaaIds, Probe};
use verbatim_model::tell::{ElementToken, Event, EventObject, Payload};
use verbatim_model::{NotificationKind, NotificationProcessing};

use crate::entries::{Entries, request_window, window_facts};
use crate::event_queue::{Cx, Function, MsaaClass, MsaaKey, Place, RequestWindow};
use crate::focus::acceptance::{self, Accepted};
use crate::focus::backend::{Judged, Judgement};
use crate::keys::entry::Fence;

/// An announcement's tell, parsed by its kind.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Announcement {
    /// The tell.
    pub(crate) tell: Event,
    /// What its kind parsed from it.
    pub(crate) kind: Kind,
    /// Its speech fence, set when it joins the queue.
    pub(crate) fence: Fence,
}

/// The kinds of announcement, each with what its rules read beyond the
/// tell.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Kind {
    /// A UI Automation notification: no request.
    Notification {
        /// The sender's runtime id.
        sender: RuntimeId,
        /// Its text.
        text: String,
        /// Its kind.
        kind: NotificationKind,
        /// How it asks to be processed.
        processing: NotificationProcessing,
        /// Its activity id.
        activity: String,
    },
    /// A UI Automation window opened, a toast candidate, the element kept
    /// by the listener.
    Toast(Kept),
    /// A UI Automation element selected, the element kept by the listener.
    UiaSelection(Kept),
    /// An MSAA alert.
    Alert {
        /// The ids its event named.
        ids: MsaaIds,
        /// The window its request names.
        window: RequestWindow,
        /// Whether it was moved once behind the pending entries.
        moved: bool,
    },
    /// An MSAA show.
    Show {
        /// The ids its event named.
        ids: MsaaIds,
        /// The window its request names.
        window: RequestWindow,
    },
}

/// A UI Automation element the listener keeps for a tell.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Kept {
    /// The token the listener keeps it behind.
    pub(crate) token: ElementToken,
    /// Its runtime id, from the event's cache.
    pub(crate) runtime_id: RuntimeId,
    /// The generation of the listener that keeps it.
    pub(crate) listener: u64,
}

impl Announcement {
    /// The announcement `tell`, told by the listener of generation
    /// `listener`, becomes; `None` for another kind, and for a tell whose
    /// object or payload does not fit its kind, a reader's fault.
    pub(crate) fn of_tell(listener: u64, tell: Event) -> Option<Self> {
        let kind = match (tell.kind, &tell.object, &tell.payload) {
            (
                EventKind::UiaNotification,
                EventObject::Uia { runtime_id, .. },
                Payload::Notification {
                    text,
                    kind,
                    processing,
                    activity,
                },
            ) => Kind::Notification {
                sender: runtime_id.clone(),
                text: text.clone(),
                kind: *kind,
                processing: *processing,
                activity: activity.clone(),
            },
            (
                kind @ (EventKind::UiaWindowOpened | EventKind::UiaElementSelected),
                EventObject::Uia { token, runtime_id },
                Payload::None,
            ) => {
                let kept = Kept {
                    token: *token,
                    runtime_id: runtime_id.clone(),
                    listener,
                };
                if kind == EventKind::UiaWindowOpened {
                    Kind::Toast(kept)
                } else {
                    Kind::UiaSelection(kept)
                }
            }
            (
                kind @ (EventKind::Alert | EventKind::Show),
                EventObject::Msaa {
                    window,
                    object,
                    child,
                },
                _,
            ) => {
                let ids = MsaaIds {
                    window: *window,
                    object: *object,
                    child: *child,
                };
                let window = request_window(&tell)?;
                if kind == EventKind::Alert {
                    Kind::Alert {
                        ids,
                        window,
                        moved: false,
                    }
                } else {
                    Kind::Show { ids, window }
                }
            }
            _ => return None,
        };
        Some(Self {
            tell,
            kind,
            fence: Fence::default(),
        })
    }

    /// The function its kind runs at its turn.
    pub(crate) fn run(cx: Cx<Entries>, announcement: Self) -> Function {
        match announcement.kind {
            Kind::Notification { .. } => Box::pin(async move {
                let mut cx = cx;
                notification::run(&mut cx, &announcement);
            }),
            Kind::Toast(_) => Box::pin(toast::run(cx, announcement)),
            Kind::UiaSelection(_) => Box::pin(selection::run(cx, announcement)),
            Kind::Alert { .. } => Box::pin(alert::run(cx, announcement)),
            Kind::Show { .. } => Box::pin(show::run(cx, announcement)),
        }
    }

    /// How it waits: an MSAA kind by its key, a UI Automation kind merging
    /// a waiting one of the same kind and object (Table 12, column F).
    pub(crate) fn place(&self) -> Place {
        let msaa = |ids: &MsaaIds, class: MsaaClass| Place::Msaa {
            key: MsaaKey {
                kind: self.tell.kind,
                window: ids.window,
                object: ids.object,
                child: ids.child,
                thread: self.tell.thread,
            },
            class,
            accepted: acceptance::accept(self.tell.kind, window_facts(&self.tell), false).is_ok(),
        };
        match &self.kind {
            Kind::Alert { ids, .. } => msaa(ids, MsaaClass::Other),
            Kind::Show { ids, .. } => msaa(ids, MsaaClass::Show),
            Kind::Notification { .. } | Kind::Toast(_) | Kind::UiaSelection(_) => Place::Other {
                merges: true,
                focus_kind: false,
                ends_run: false,
            },
        }
    }

    /// Whether `arriving` replaces it as it waits: a notification of the
    /// same sender, kind, processing and activity; a selection of the same
    /// element. A toast's repeat is dropped at its turn instead.
    pub(crate) fn merged_by(&self, arriving: &Self) -> bool {
        match (&self.kind, &arriving.kind) {
            (
                Kind::Notification {
                    sender,
                    kind,
                    processing,
                    activity,
                    ..
                },
                Kind::Notification {
                    sender: other_sender,
                    kind: other_kind,
                    processing: other_processing,
                    activity: other_activity,
                    ..
                },
            ) => {
                (sender, kind, processing, activity)
                    == (other_sender, other_kind, other_processing, other_activity)
            }
            (Kind::UiaSelection(waiting), Kind::UiaSelection(other)) => {
                (&waiting.runtime_id, self.tell.thread) == (&other.runtime_id, arriving.tell.thread)
            }
            _ => false,
        }
    }

    /// The window its requests name, `None` for a kind that asks nothing
    /// or a tell with no window.
    pub(crate) fn window(&self) -> Option<RequestWindow> {
        match &self.kind {
            Kind::Notification { .. } => None,
            Kind::Toast(_) | Kind::UiaSelection(_) => request_window(&self.tell),
            Kind::Alert { window, .. } | Kind::Show { window, .. } => Some(*window),
        }
    }

    /// Whether the foreground stop does not reach it: a toast and a show,
    /// accepted by their class, and an entry accepted by something other
    /// than the foreground relation (Table 12, column E).
    pub(crate) fn exempt_from_foreground(&self) -> bool {
        match &self.kind {
            Kind::Notification { .. } | Kind::Toast(_) | Kind::Show { .. } => true,
            Kind::UiaSelection(_) | Kind::Alert { .. } => matches!(
                acceptance::accept(self.tell.kind, window_facts(&self.tell), false),
                Ok(Accepted::NoWindow
                    | Accepted::Toast
                    | Accepted::DesktopWindow
                    | Accepted::Topmost
                    | Accepted::ShowClass
                    | Accepted::AnyWindow
                    | Accepted::BackgroundProgress)
            ),
        }
    }

    /// The probe its check made, which the backend cache records.
    pub(crate) fn probe(&self) -> Option<Probe> {
        match &self.kind {
            Kind::Toast(_) | Kind::UiaSelection(_) => self.tell.checked.as_ref()?.probe,
            Kind::Notification { .. } | Kind::Alert { .. } | Kind::Show { .. } => None,
        }
    }

    /// Whether the listener of generation `listener` keeps its element.
    pub(crate) fn kept_by(&self, listener: u64) -> bool {
        match &self.kind {
            Kind::Toast(kept) | Kind::UiaSelection(kept) => kept.listener == listener,
            Kind::Notification { .. } | Kind::Alert { .. } | Kind::Show { .. } => false,
        }
    }
}

/// Whether a UI Automation tell's element is native, as the check's probe
/// of its nearest window answered: a window with no provider of its own is
/// read through MSAA, so its UI Automation events are dropped, as NVDA's
/// `isNativeUIAElement` drops them. A tell whose check made no probe
/// stands.
fn native(tell: &Event) -> bool {
    tell.checked
        .as_ref()
        .and_then(|checked| checked.probe)
        .is_none_or(|probe| probe.provider)
}

/// Whether `tell` passes acceptance on its tell's facts.
fn accepted(tell: &Event) -> bool {
    acceptance::accept(tell.kind, window_facts(tell), false).is_ok()
}

/// Whether an MSAA announcement passes acceptance and the backend verdict
/// on its tell's facts.
fn msaa_passes(cx: &Cx<Entries>, tell: &Event, window: &RequestWindow) -> bool {
    if !accepted(tell) {
        return false;
    }
    cx.backend()
        .judge(Judged::Msaa, &window.judged(), tell.at_us, cx.now_us())
        != Judgement::Drop
}

/// The text of `value`, a property read, when it is text.
fn text(value: Option<&PropertyValue>) -> Option<&str> {
    match value {
        Some(PropertyValue::Text(text)) => Some(text),
        _ => None,
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
