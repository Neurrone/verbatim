//! The vocabulary of the listener's reading threads and its progress
//! path: how many threads may be abandoned, and a value change's object,
//! its read and what was found of it.

use std::time::Duration;

/// How many of an application's reading threads may be abandoned to
/// requests and still in their calls before it gets no new thread: its
/// jobs then wait for the first of those threads whose call returns, Core's
/// due time and Cancel bounding a request's wait.
pub const ABANDONED_LIMIT: usize = 2;

/// One object a value change was raised for: a window, and the object and
/// child ids that name it in that window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BarObject {
    /// The window, which may be a bar's own or its container's.
    pub window: isize,
    /// The MSAA object id.
    pub object: i32,
    /// The MSAA child id.
    pub child: i32,
}

/// Whether an object a value change came from is a progress bar, as the
/// reading thread found by its role (`docs/design/progress-bars.md`,
/// "Rules"): what an application's record remembers for the object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarVerdict {
    /// It is a progress bar.
    Bar,
    /// It is not: its later value changes are dropped in the hook, with no
    /// call into the application.
    NotABar,
}

/// The most objects an application's record remembers a [`BarVerdict`] for:
/// the oldest is forgotten first, so an application that changes the values
/// of more objects than this reads a role again as each comes back, and no
/// application's record grows with what it does.
pub const BAR_VERDICTS_KEPT: usize = 256;

/// How a classic value change is to be read, once the hook's filter has
/// let it through ([`classic_value_change`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClassicBarRead {
    /// A bar of a class NVDA lists: its percentage by window messages.
    Listed,
    /// An object the record knows to be a bar: its percentage by its value.
    KnownBar,
    /// An object the record has no answer for: its role is read, and a bar
    /// has its percentage read too.
    Unknown,
    /// A window with no answer: it is judged first, by its class and its
    /// probe, and with no provider NVDA uses this object is read as
    /// [`Self::Unknown`].
    Probe,
}

/// What the listener's hook does with one `EVENT_OBJECT_VALUECHANGE`, with no
/// call into the application (`docs/design/progress-bars.md`, "Rules"): a
/// window of a class NVDA lists as a progress bar, `listed`, read when it
/// is visible; otherwise what `verdict`, the record's memory of the object,
/// says. `None` drops the event.
#[must_use]
pub fn classic_value_change(
    listed: bool,
    visible: bool,
    verdict: Option<BarVerdict>,
) -> Option<ClassicBarRead> {
    if listed {
        return visible.then_some(ClassicBarRead::Listed);
    }
    match verdict {
        Some(BarVerdict::NotABar) => None,
        Some(BarVerdict::Bar) => Some(ClassicBarRead::KnownBar),
        None => Some(ClassicBarRead::Unknown),
    }
}

/// How long a read of a classic value change is given before its thread
/// is abandoned: shorter than a focus's deadline, so that a slow
/// application's reading thread is held by its value changes for no more
/// than this ahead of a focus.
pub const BAR_READ_DEADLINE: Duration = Duration::from_secs(1);

/// How many of an application's reads of classic value changes may be
/// abandoned and still in their calls. At this many, its further value
/// changes are dropped with no thread started until a call returns, so a
/// slow application holds a bounded number of threads. Counted apart from
/// [`ABANDONED_LIMIT`], so they never starve a focus.
pub const BAR_READS_ABANDONED_LIMIT: usize = 2;

/// The most classic value changes an application's queue holds, from all
/// its raising threads together, the oldest dropped first: the limit per
/// thread does not bound a pool of raising threads.
pub const BARS_PER_APPLICATION: usize = 64;

/// The thread that raised a classic value change, and whether it is of
/// the object with the MSAA focus, whose events the limit always keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BarRaiser {
    /// The raising thread.
    pub raised_by: u32,
    /// Whether the object has the MSAA focus.
    pub focused: bool,
}

/// What a value change's window was found to be
/// (`ClassicBarRead::Probe`), as a window's outcome maps in the outposts'
/// arbitrator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowAnswer {
    /// A provider NVDA uses, for the window's lifetime.
    Uia,
    /// MSAA for its lifetime: a provider NVDA does not use, or a class on
    /// NVDA's MSAA list.
    Excluded,
    /// No provider, for now.
    NotUia,
    /// The probe was not answered in time: no provider, for now.
    TimedOut,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A listed class is read when visible and dropped when not.
    #[test]
    fn a_listed_class_is_read_only_while_visible() {
        assert_eq!(
            classic_value_change(true, true, None),
            Some(ClassicBarRead::Listed)
        );
        assert_eq!(classic_value_change(true, false, None), None);
    }

    /// Any other object: dropped when known not to be a bar, read when
    /// known to be one, read once when unknown.
    #[test]
    fn another_class_goes_by_the_records_verdict() {
        assert_eq!(
            classic_value_change(false, true, Some(BarVerdict::NotABar)),
            None
        );
        assert_eq!(
            classic_value_change(false, true, Some(BarVerdict::Bar)),
            Some(ClassicBarRead::KnownBar)
        );
        assert_eq!(
            classic_value_change(false, true, None),
            Some(ClassicBarRead::Unknown)
        );
    }
}
