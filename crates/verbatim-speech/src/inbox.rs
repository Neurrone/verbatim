//! The speech queue thread's inbox (`books/dev/src/explanation/design/ownership.md`): every event the queue thread takes, from the
//! manager, [`SpeechControl`](crate::SpeechControl), the settings host,
//! the synth thread and the mixer's playback listener, is posted to one
//! inbox, first in, first out across posters, so a key's cancel and every
//! utterance keep the one order the key fence depends on.
//!
//! The inbox is two traits, so that the application can give the queue a
//! completion port, whose post takes no lock and never waits: the keyboard
//! hook's thread cancels speech through it on every key. This crate's own
//! tests use [`channel_inbox`], correct where no hook thread posts.

use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender, unbounded};

use crate::manager::QueueEvent;

/// One event for the speech queue thread, opaque outside this crate.
pub struct QueueItem(pub(crate) QueueEvent);

impl QueueItem {
    /// The key a key press's cancel names
    /// ([`SpeechControl::cancel_through`](crate::SpeechControl::cancel_through)),
    /// `None` for any other event: for a test that reads what was posted.
    #[must_use]
    pub fn cancels_through(&self) -> Option<verbatim_model::KeySeq> {
        match self.0 {
            QueueEvent::Cancel(key) => key,
            _ => None,
        }
    }
}

/// Where the speech queue thread's events are posted, from any thread.
pub trait SpeechInbox: Send + Sync {
    /// Posts `item` behind everything posted before it, from any thread.
    /// Never waits.
    ///
    /// # Errors
    ///
    /// Gives `item` back when the queue can no longer take it: the queue
    /// thread has gone, or the inbox refused it.
    fn post(&self, item: QueueItem) -> Result<(), QueueItem>;

    /// Whether the queue thread still takes what is posted: after a
    /// [`post`](Self::post) that gave its item back, true means the inbox
    /// refused it for now, and a later post may succeed; false means the
    /// queue thread's end has gone.
    fn is_open(&self) -> bool;
}

/// The speech queue thread's end of its inbox.
pub trait SpeechInboxTaker: Send {
    /// Waits for the next item, in the order posted; `None` once nothing
    /// more can come.
    fn take(&mut self) -> Option<QueueItem>;
}

/// An inbox on an unbounded channel. Its post takes the channel's lock to
/// wake the waiting queue thread, so it is for a speech pipeline no
/// keyboard hook posts to, such as a test's.
#[must_use]
pub fn channel_inbox() -> (Arc<dyn SpeechInbox>, Box<dyn SpeechInboxTaker>) {
    let (sender, receiver) = unbounded();
    (
        Arc::new(ChannelInbox(sender)),
        Box::new(ChannelTaker(receiver)),
    )
}

/// [`channel_inbox`]'s posting end.
struct ChannelInbox(Sender<QueueItem>);

impl SpeechInbox for ChannelInbox {
    fn post(&self, item: QueueItem) -> Result<(), QueueItem> {
        self.0.send(item).map_err(|error| error.0)
    }

    /// Always true: a channel inbox cannot see its taker go.
    fn is_open(&self) -> bool {
        true
    }
}

/// [`channel_inbox`]'s taking end.
struct ChannelTaker(Receiver<QueueItem>);

impl SpeechInboxTaker for ChannelTaker {
    fn take(&mut self) -> Option<QueueItem> {
        self.0.recv().ok()
    }
}

/// Posts `event` to `inbox`, giving it back when it was refused.
pub(crate) fn post(inbox: &dyn SpeechInbox, event: QueueEvent) -> Result<(), QueueEvent> {
    inbox.post(QueueItem(event)).map_err(|item| item.0)
}
