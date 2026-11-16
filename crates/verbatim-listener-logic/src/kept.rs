//! The UI Automation elements the listener keeps for its tells, one
//! application's (design `docs/design/readers-tell-core-asks-build1.md`,
//! Table 6): each kept behind the token Core names it by, from its tell
//! until Core asks about it or it is let go. Which element is the readers'
//! last focus is Core's alone to say: the listener keeps every element a
//! focus read read until a focus request names the one to keep.
//!
//! An element let go is handed back to the caller, which releases it as a
//! job on its application's reading thread, never on a callback thread,
//! since a release can call into the application. No message tells Core of
//! a release; a later ask of its token is answered "not there".
//!
//! An element awaiting or in its check is not here: the check thread holds
//! it until it is told and kept ([`KeptElements::keep`]).

use std::collections::BTreeMap;

use verbatim_model::property::RuntimeId;
use verbatim_model::tell::ElementToken;

/// How long an element told and not yet asked about is kept, in
/// microseconds: four times the longest due time.
pub const KEPT_FOR_US: u64 = 60_000_000;

/// How many elements told and not yet asked about an application keeps:
/// the newest, the per-application cap the bar limits use.
pub const KEPT_PER_APPLICATION: usize = 64;

/// One kept element's state, each carrying what is valid in it.
#[derive(Debug)]
enum Kept<E> {
    /// Told, not yet asked about: swept by age and count.
    Told {
        element: E,
        runtime_id: RuntimeId,
        /// When its event was received, in microseconds.
        at_us: u64,
    },
    /// Being read on the application's reading thread, which holds the
    /// element.
    InRead { runtime_id: RuntimeId },
    /// A focus read's element, read and not yet named by a focus request:
    /// Core may execute it.
    ReadNotNamed { element: E, runtime_id: RuntimeId },
    /// The readers' last focus, as the last focus request named it.
    Named { element: E, runtime_id: RuntimeId },
}

impl<E> Kept<E> {
    fn runtime_id(&self) -> &RuntimeId {
        match self {
            Self::Told { runtime_id, .. }
            | Self::InRead { runtime_id }
            | Self::ReadNotNamed { runtime_id, .. }
            | Self::Named { runtime_id, .. } => runtime_id,
        }
    }

    fn element(&self) -> Option<&E> {
        match self {
            Self::Told { element, .. }
            | Self::ReadNotNamed { element, .. }
            | Self::Named { element, .. } => Some(element),
            Self::InRead { .. } => None,
        }
    }

    fn into_element(self) -> Option<E> {
        match self {
            Self::Told { element, .. }
            | Self::ReadNotNamed { element, .. }
            | Self::Named { element, .. } => Some(element),
            Self::InRead { .. } => None,
        }
    }
}

/// What starting a read of a token finds.
#[derive(Debug, PartialEq, Eq)]
pub enum Start<E> {
    /// The element, now in read: the reading thread reads it.
    Read(E),
    /// Released, or never kept: answered "not there".
    NotThere,
    /// The token is in read already, or read and waiting for Core to name
    /// it: no request starts on it, so this is a logic error.
    NotAsked,
}

/// What a read's end does with its element.
#[derive(Debug, PartialEq, Eq)]
pub enum Ended<E> {
    /// Kept, read and not yet named.
    Kept,
    /// To release.
    Release(E),
}

/// One application's kept elements.
#[derive(Debug)]
pub struct KeptElements<E> {
    kept: BTreeMap<ElementToken, Kept<E>>,
}

impl<E> Default for KeptElements<E> {
    fn default() -> Self {
        Self {
            kept: BTreeMap::new(),
        }
    }
}

impl<E> KeptElements<E> {
    /// Keeps `element`, with `runtime_id`, behind `token` as its event is
    /// told, its event received at `at_us`.
    pub fn keep(&mut self, token: ElementToken, element: E, runtime_id: RuntimeId, at_us: u64) {
        self.kept.insert(
            token,
            Kept::Told {
                element,
                runtime_id,
                at_us,
            },
        );
    }

    /// Every kept element with `runtime_id` the check thread can read,
    /// with its token, oldest first: all but one in read.
    pub fn with_runtime_id<'a>(
        &'a self,
        runtime_id: &'a RuntimeId,
    ) -> impl Iterator<Item = (ElementToken, &'a E)> + 'a {
        self.kept.iter().filter_map(move |(token, kept)| {
            (kept.runtime_id() == runtime_id)
                .then(|| kept.element().map(|element| (*token, element)))
                .flatten()
        })
    }

    /// The reading thread starts the read of `token`, which Core asked
    /// about (column A).
    pub fn start_read(&mut self, token: ElementToken) -> Start<E> {
        let Some(kept) = self.kept.remove(&token) else {
            return Start::NotThere;
        };
        match kept {
            Kept::Told {
                element,
                runtime_id,
                ..
            } => {
                self.kept.insert(token, Kept::InRead { runtime_id });
                Start::Read(element)
            }
            other => {
                self.kept.insert(token, other);
                Start::NotAsked
            }
        }
    }

    /// The read of `token` ended, handing back `element` (column B): a
    /// focus read's element is kept, read and not yet named; any other,
    /// and one whose token was let go meanwhile, is released.
    pub fn read_ended(&mut self, token: ElementToken, element: E, focus: bool) -> Ended<E> {
        match self.kept.remove(&token) {
            Some(Kept::InRead { runtime_id }) if focus => {
                self.kept.insert(
                    token,
                    Kept::ReadNotNamed {
                        element,
                        runtime_id,
                    },
                );
                Ended::Kept
            }
            Some(Kept::InRead { .. }) | None => Ended::Release(element),
            Some(other) => {
                self.kept.insert(token, other);
                Ended::Release(element)
            }
        }
    }

    /// Core cancelled the running read of `token` for the foreground
    /// (column E): its thread is abandoned, and the element it holds is
    /// released when its call returns ([`Self::read_ended`]). A cancel for
    /// any other reason leaves the read alone.
    pub fn read_abandoned(&mut self, token: ElementToken) {
        if matches!(self.kept.get(&token), Some(Kept::InRead { .. })) {
            self.kept.remove(&token);
        }
    }

    /// A focus request arrived, naming the readers' last focus's token,
    /// `None` for none (column C): that element, read and not yet named or
    /// named before, is the readers' last focus; every other read element
    /// is released.
    pub fn focus_request(&mut self, named: Option<ElementToken>) -> Vec<E> {
        let read: Vec<ElementToken> = self
            .kept
            .iter()
            .filter(|(_, kept)| matches!(kept, Kept::ReadNotNamed { .. } | Kept::Named { .. }))
            .map(|(token, _)| *token)
            .collect();
        let mut released = Vec::new();
        for token in read {
            let Some(kept) = self.kept.remove(&token) else {
                continue;
            };
            match kept {
                Kept::ReadNotNamed {
                    element,
                    runtime_id,
                }
                | Kept::Named {
                    element,
                    runtime_id,
                } if Some(token) == named => {
                    self.kept.insert(
                        token,
                        Kept::Named {
                            element,
                            runtime_id,
                        },
                    );
                }
                other => released.extend(other.into_element()),
            }
        }
        released
    }

    /// The sweep at an event at `now_us` (column D): every element told
    /// and not yet asked about past [`KEPT_FOR_US`], or beyond the newest
    /// [`KEPT_PER_APPLICATION`], is released.
    pub fn sweep(&mut self, now_us: u64) -> Vec<E> {
        let told: Vec<(ElementToken, u64)> = self
            .kept
            .iter()
            .filter_map(|(token, kept)| match kept {
                Kept::Told { at_us, .. } => Some((*token, *at_us)),
                _ => None,
            })
            .collect();
        // Tokens are minted in order, so the newest are the greatest.
        let beyond = told.len().saturating_sub(KEPT_PER_APPLICATION);
        let swept: Vec<ElementToken> = told
            .iter()
            .enumerate()
            .filter(|(at, (_, kept_at))| {
                *at < beyond || now_us.saturating_sub(*kept_at) > KEPT_FOR_US
            })
            .map(|(_, (token, _))| *token)
            .collect();
        swept
            .into_iter()
            .filter_map(|token| self.kept.remove(&token))
            .filter_map(Kept::into_element)
            .collect()
    }

    /// The application ended (column D): every element it keeps is
    /// released, but one in read, which its read releases as it ends.
    pub fn ended(&mut self) -> Vec<E> {
        std::mem::take(&mut self.kept)
            .into_values()
            .filter_map(Kept::into_element)
            .collect()
    }

    /// Whether it keeps `token`, in whatever state.
    #[must_use]
    pub fn holds(&self, token: ElementToken) -> bool {
        self.kept.contains_key(&token)
    }

    /// Whether it keeps nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.kept.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rid(id: i32) -> RuntimeId {
        RuntimeId(vec![42, id])
    }

    /// Elements kept with tokens 1 to `count`, each its own runtime id,
    /// received a microsecond apart from 0.
    fn kept(count: u64) -> KeptElements<u64> {
        let mut kept = KeptElements::default();
        for token in 1..=count {
            kept.keep(
                ElementToken(token),
                token,
                rid(i32::try_from(token).unwrap()),
                token,
            );
        }
        kept
    }

    /// A told element is read once Core asks; a second start on it is no
    /// request's; a token never kept, or let go, is not there.
    #[test]
    fn a_told_element_is_read_once_and_an_unknown_token_is_not_there() {
        let mut kept = kept(1);
        assert_eq!(kept.start_read(ElementToken(1)), Start::Read(1));
        assert_eq!(kept.start_read(ElementToken(1)), Start::NotAsked);
        assert_eq!(kept.start_read(ElementToken(9)), Start::NotThere);
    }

    /// A focus read's element is kept read and not yet named; any other
    /// read's is released as its read ends.
    #[test]
    fn a_focus_reads_element_is_kept_and_another_reads_released() {
        let mut kept = kept(2);
        let _ = kept.start_read(ElementToken(1));
        let _ = kept.start_read(ElementToken(2));
        assert_eq!(kept.read_ended(ElementToken(1), 1, true), Ended::Kept);
        assert_eq!(
            kept.read_ended(ElementToken(2), 2, false),
            Ended::Release(2)
        );
        assert_eq!(kept.start_read(ElementToken(2)), Start::NotThere);
        // Read and not yet named: Core names it, it starts no read.
        assert_eq!(kept.start_read(ElementToken(1)), Start::NotAsked);
    }

    /// A focus request keeps the token it names as the readers' last
    /// focus, and releases every other read element; the named one stays
    /// while it is named again.
    #[test]
    fn a_focus_request_keeps_the_named_token_and_releases_the_other_reads() {
        let mut kept = kept(3);
        for token in 1..=2 {
            let _ = kept.start_read(ElementToken(token));
            let _ = kept.read_ended(ElementToken(token), token, true);
        }
        assert_eq!(kept.focus_request(Some(ElementToken(2))), vec![1]);
        assert_eq!(kept.focus_request(Some(ElementToken(2))), Vec::<u64>::new());
        assert_eq!(kept.focus_request(None), vec![2]);
        // The told element is not a read one: kept.
        assert_eq!(kept.start_read(ElementToken(3)), Start::Read(3));
    }

    /// The check thread reads every kept element with the event's runtime
    /// id but one in read.
    #[test]
    fn the_check_reads_every_kept_element_with_the_runtime_id_but_one_in_read() {
        let mut kept = KeptElements::default();
        for token in 1..=4 {
            kept.keep(ElementToken(token), token, rid(7), token);
        }
        let _ = kept.start_read(ElementToken(2));
        let _ = kept.start_read(ElementToken(3));
        let _ = kept.read_ended(ElementToken(3), 3, true);
        kept.keep(ElementToken(5), 5, rid(8), 5);
        let found: Vec<(ElementToken, u64)> = kept
            .with_runtime_id(&rid(7))
            .map(|(token, element)| (token, *element))
            .collect();
        assert_eq!(
            found,
            [
                (ElementToken(1), 1),
                (ElementToken(3), 3),
                (ElementToken(4), 4)
            ]
        );
    }

    /// The sweep releases told elements past a minute and beyond the
    /// newest 64, and none read, in read or named.
    #[test]
    fn the_sweep_releases_told_elements_by_age_and_count_alone() {
        let mut kept = kept(66);
        let _ = kept.start_read(ElementToken(1));
        let _ = kept.start_read(ElementToken(2));
        let _ = kept.read_ended(ElementToken(2), 2, true);
        // 64 told remain of 66 less the two read: within the count, and
        // none older than a minute at 60 s and 3 µs.
        assert_eq!(kept.sweep(KEPT_FOR_US + 3), Vec::<u64>::new());
        kept.keep(ElementToken(67), 67, rid(67), KEPT_FOR_US + 4);
        // 65 told: the oldest beyond the count, and the next past a minute.
        assert_eq!(kept.sweep(KEPT_FOR_US + 5), vec![3, 4]);
        assert_eq!(kept.start_read(ElementToken(1)), Start::NotAsked);
        assert_eq!(kept.start_read(ElementToken(5)), Start::Read(5));
    }

    /// A read abandoned for the foreground lets its token go, and the
    /// element its call hands back is released.
    #[test]
    fn a_read_abandoned_for_the_foreground_is_released_when_its_call_returns() {
        let mut kept = kept(1);
        let _ = kept.start_read(ElementToken(1));
        kept.read_abandoned(ElementToken(1));
        assert_eq!(kept.start_read(ElementToken(1)), Start::NotThere);
        assert_eq!(kept.read_ended(ElementToken(1), 1, true), Ended::Release(1));
    }

    /// The application's end releases every element it keeps but one in
    /// read, which its read releases.
    #[test]
    fn the_applications_end_releases_all_but_a_read_in_flight() {
        let mut kept = kept(3);
        let _ = kept.start_read(ElementToken(1));
        let _ = kept.start_read(ElementToken(2));
        let _ = kept.read_ended(ElementToken(2), 2, true);
        assert_eq!(kept.ended(), vec![2, 3]);
        assert!(kept.is_empty());
        assert_eq!(kept.read_ended(ElementToken(1), 1, true), Ended::Release(1));
    }
}
