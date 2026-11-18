//! The UI Automation elements the listener keeps for its tells, one
//! application's (design of the listener group, "Told elements released
//! by Core's held view", its state table of a kept element): each kept
//! behind the token Core names it by, from its tell until Core's held view
//! leaves it out, as an outpost keeps its nodes. Which elements Core holds
//! is Core's alone to say: the listener compares each view with what it
//! keeps and judges nothing.
//!
//! A view lets go of every element it leaves out whose tell Core has
//! handled: one whose tell is not yet written, or was written after the
//! position the view acknowledges, is too early to judge
//! ([`too_early`], from the writer's [`Reported`] record). A read's end
//! lets go of a focus read's element when the last view already rules it
//! out, and of any other read's element at once.
//!
//! An element let go is handed back to the caller, which releases it on a
//! thread allowed to block, never on a callback thread or under a lock,
//! since a release can call into the application. No message tells Core of
//! a release; a later ask of its token is answered "not there".
//!
//! An element awaiting or in its check is not here: the check thread holds
//! it until it is told and kept ([`KeptElements::keep`]).

use std::collections::{BTreeMap, BTreeSet};

use verbatim_model::property::RuntimeId;
use verbatim_model::tell::ElementToken;

/// One kept element's state, each carrying what is valid in it.
#[derive(Debug)]
enum Kept<E> {
    /// Told, not yet asked about. `focus_kind` is whether its event was a
    /// focus or a menu opening, the only kinds that can become the
    /// readers' last focus, whose keyboard focus a later check reads.
    Told {
        element: E,
        runtime_id: RuntimeId,
        focus_kind: bool,
    },
    /// Being read on the application's reading thread for request
    /// `request`, which holds its own reference to the element; this one
    /// is the check thread's, so a later event with its runtime id reads
    /// its keyboard focus as NVDA reads its last queued focus's at
    /// delivery, when `focus` says the read is a focus read.
    InRead {
        element: E,
        runtime_id: RuntimeId,
        focus: bool,
        request: u64,
    },
    /// A focus read's element, read: Core may execute it, and it is kept
    /// until a view leaves it out.
    Read { element: E, runtime_id: RuntimeId },
}

impl<E> Kept<E> {
    /// Whether a check reads the keyboard focus of this element when a
    /// later event has `runtime_id`.
    fn checked_by(&self, runtime_id: &RuntimeId) -> bool {
        match self {
            Self::Told {
                runtime_id: own,
                focus_kind,
                ..
            } => *focus_kind && own == runtime_id,
            Self::InRead {
                runtime_id: own,
                focus,
                ..
            } => *focus && own == runtime_id,
            Self::Read {
                runtime_id: own, ..
            } => own == runtime_id,
        }
    }

    fn element(&self) -> &E {
        match self {
            Self::Told { element, .. }
            | Self::InRead { element, .. }
            | Self::Read { element, .. } => element,
        }
    }

    fn into_element(self) -> E {
        match self {
            Self::Told { element, .. }
            | Self::InRead { element, .. }
            | Self::Read { element, .. } => element,
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
    /// The token is in read already, or read: Core asks of a token once,
    /// so this is a logic error.
    NotAsked,
}

/// What a read's end does with its element.
#[derive(Debug, PartialEq, Eq)]
pub enum Ended<E> {
    /// Kept, read.
    Kept,
    /// To release.
    Release(E),
}

/// Where the writer placed a told element's tell: the position of the last
/// message carrying tokens written that carried it, and how many messages
/// carrying it still wait to be written. The outpost's record of its
/// reported nodes, in the same shape.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// The position of the last message written that carried it.
    pub position: u64,
    /// How many messages carrying it wait to be written.
    pub waiting: u32,
}

/// The writer's numbering of the messages that carry tokens
/// (`ListenerToCore::carries_tokens`), in the order they are written, as
/// Core counts them as it handles them, and the [`Report`] of each token
/// they carried. Kept in the writer's backlog, under its lock.
#[derive(Debug, Default)]
pub struct Reported {
    /// How many messages carrying tokens have been written: the position
    /// of the last one.
    position: u64,
    reports: BTreeMap<ElementToken, Report>,
}

impl Reported {
    /// A message carrying `token` was queued to be written.
    pub fn queued(&mut self, token: ElementToken) {
        let report = self.reports.entry(token).or_default();
        report.waiting = report.waiting.saturating_add(1);
    }

    /// A message carrying `token` is being written now: it is numbered,
    /// and the token's report records it, unless the token was released
    /// meanwhile ([`Self::forget`]).
    pub fn written(&mut self, token: ElementToken) {
        self.position = self.position.saturating_add(1);
        if let Some(report) = self.reports.get_mut(&token) {
            report.waiting = report.waiting.saturating_sub(1);
            report.position = self.position;
        }
    }

    /// A message carrying `token` that waited was replaced unwritten.
    pub fn withdrawn(&mut self, token: ElementToken) {
        if let Some(report) = self.reports.get_mut(&token) {
            report.waiting = report.waiting.saturating_sub(1);
        }
    }

    /// The report of `token`, `None` when no message carried it.
    #[must_use]
    pub fn report(&self, token: ElementToken) -> Option<Report> {
        self.reports.get(&token).copied()
    }

    /// Forgets `token`, which the listener released.
    pub fn forget(&mut self, token: ElementToken) {
        self.reports.remove(&token);
    }

    /// The position of the last message carrying tokens written.
    #[must_use]
    pub fn position(&self) -> u64 {
        self.position
    }
}

/// Whether a token whose tell's report is `report` is too early for a view
/// acknowledging `acknowledged` to judge: no message carried it yet, one
/// still waits to be written, or the last was written after the position
/// Core acknowledges.
#[must_use]
pub fn too_early(report: Option<Report>, acknowledged: u64) -> bool {
    report.is_none_or(|report| report.waiting > 0 || report.position > acknowledged)
}

/// Core's held view, as the listener keeps the last one: the tokens Core
/// holds, and the position of the last message carrying tokens Core has
/// handled.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct View {
    /// The tokens Core holds.
    pub tokens: BTreeSet<ElementToken>,
    /// The position acknowledged.
    pub acknowledged: u64,
}

impl View {
    /// Whether this view rules `token` out: it leaves the token out, and
    /// the token's tell, reported as `report`, is not too early for it.
    #[must_use]
    pub fn rules_out(&self, token: ElementToken, report: Option<Report>) -> bool {
        !self.tokens.contains(&token) && !too_early(report, self.acknowledged)
    }
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
    /// told; `focus_kind` is whether the event was a focus or a menu
    /// opening.
    pub fn keep(
        &mut self,
        token: ElementToken,
        element: E,
        runtime_id: RuntimeId,
        focus_kind: bool,
    ) {
        self.kept.insert(
            token,
            Kept::Told {
                element,
                runtime_id,
                focus_kind,
            },
        );
    }

    /// Every kept element a check of an event with `runtime_id` reads the
    /// keyboard focus of, with its token, oldest first (column G): one
    /// told by a focus or a menu opening, one in a focus read, and one
    /// read, since only these can be the readers' last focus.
    pub fn with_runtime_id<'a>(
        &'a self,
        runtime_id: &'a RuntimeId,
    ) -> impl Iterator<Item = (ElementToken, &'a E)> + 'a {
        self.kept
            .iter()
            .filter(move |(_, kept)| kept.checked_by(runtime_id))
            .map(|(token, kept)| (*token, kept.element()))
    }

    /// The reading thread starts the read of `token` for request
    /// `request`, which Core asked about (column A), a focus read when
    /// `focus`: it is handed a reference of its own, and the kept one stays
    /// for the check thread.
    pub fn start_read(&mut self, token: ElementToken, request: u64, focus: bool) -> Start<E>
    where
        E: Clone,
    {
        let Some(kept) = self.kept.remove(&token) else {
            return Start::NotThere;
        };
        match kept {
            Kept::Told {
                element,
                runtime_id,
                ..
            } => {
                let read = element.clone();
                self.kept.insert(
                    token,
                    Kept::InRead {
                        element,
                        runtime_id,
                        focus,
                        request,
                    },
                );
                Start::Read(read)
            }
            other => {
                self.kept.insert(token, other);
                Start::NotAsked
            }
        }
    }

    /// The read of `token` ended, handing back `element` (column B): a
    /// focus read's element is kept, read, unless `ruled_out`, the last
    /// view already leaving it out with its tell handled; any other read's,
    /// and one whose token was let go meanwhile, is released.
    pub fn read_ended(&mut self, token: ElementToken, element: E, ruled_out: bool) -> Ended<E> {
        match self.kept.remove(&token) {
            Some(Kept::InRead {
                runtime_id,
                focus: true,
                ..
            }) if !ruled_out => {
                self.kept.insert(
                    token,
                    Kept::Read {
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

    /// Core cancelled the running request `request` for the foreground
    /// (column E): its thread is abandoned, and the element it holds is
    /// released when its call returns ([`Self::read_ended`]); the kept
    /// reference is handed back to release, with its token. `None` when no
    /// element is in read for that request.
    pub fn read_abandoned(&mut self, request: u64) -> Option<(ElementToken, E)> {
        let token = self.kept.iter().find_map(|(token, kept)| {
            matches!(kept, Kept::InRead { request: own, .. } if *own == request).then_some(*token)
        })?;
        self.kept
            .remove(&token)
            .map(|kept| (token, kept.into_element()))
    }

    /// Core's held view `view` arrived (columns C and D): every element
    /// told or read that it rules out, by each token's report from
    /// `report`, is handed back with its token to release; one in read is
    /// left for its read's end to judge.
    pub fn view(
        &mut self,
        view: &View,
        report: impl Fn(ElementToken) -> Option<Report>,
    ) -> Vec<(ElementToken, E)> {
        let ruled_out: Vec<ElementToken> = self
            .kept
            .iter()
            .filter(|(token, kept)| {
                !matches!(kept, Kept::InRead { .. }) && view.rules_out(**token, report(**token))
            })
            .map(|(token, _)| *token)
            .collect();
        ruled_out
            .into_iter()
            .filter_map(|token| {
                self.kept
                    .remove(&token)
                    .map(|kept| (token, kept.into_element()))
            })
            .collect()
    }

    /// The application ended (column F): every element it keeps is
    /// released with its token, the kept reference of one in read
    /// included; the read releases its own as it ends.
    pub fn ended(&mut self) -> Vec<(ElementToken, E)> {
        std::mem::take(&mut self.kept)
            .into_iter()
            .map(|(token, kept)| (token, kept.into_element()))
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
    /// told by focuses.
    fn kept(count: u64) -> KeptElements<u64> {
        let mut kept = KeptElements::default();
        for token in 1..=count {
            kept.keep(
                ElementToken(token),
                token,
                rid(i32::try_from(token).unwrap()),
                true,
            );
        }
        kept
    }

    /// Every token written and handled at its own number, nothing waiting.
    fn written(token: ElementToken) -> Report {
        Report {
            position: token.0,
            waiting: 0,
        }
    }

    fn view(tokens: &[u64], acknowledged: u64) -> View {
        View {
            tokens: tokens.iter().copied().map(ElementToken).collect(),
            acknowledged,
        }
    }

    /// Column A: a told element is read once Core asks; a second start on
    /// it is no request's; a token never kept, or let go, is not there.
    #[test]
    fn a_told_element_is_read_once_and_an_unknown_token_is_not_there() {
        let mut kept = kept(1);
        assert_eq!(kept.start_read(ElementToken(1), 1, true), Start::Read(1));
        assert_eq!(kept.start_read(ElementToken(1), 2, true), Start::NotAsked);
        assert_eq!(kept.start_read(ElementToken(9), 3, true), Start::NotThere);
        let _ = kept.read_ended(ElementToken(1), 1, false);
        assert_eq!(kept.start_read(ElementToken(1), 4, true), Start::NotAsked);
    }

    /// Column B: a focus read's element is kept, read, unless the last
    /// view rules it out; any other read's is released as its read ends,
    /// and so is one whose token was let go meanwhile.
    #[test]
    fn a_reads_end_keeps_a_focus_reads_element_unless_ruled_out() {
        let mut kept = kept(4);
        let _ = kept.start_read(ElementToken(1), 1, true);
        let _ = kept.start_read(ElementToken(2), 2, false);
        let _ = kept.start_read(ElementToken(3), 3, true);
        assert_eq!(kept.read_ended(ElementToken(1), 1, false), Ended::Kept);
        assert_eq!(
            kept.read_ended(ElementToken(2), 2, false),
            Ended::Release(2)
        );
        assert_eq!(kept.read_ended(ElementToken(3), 3, true), Ended::Release(3));
        assert_eq!(
            kept.read_ended(ElementToken(9), 9, false),
            Ended::Release(9)
        );
        assert!(kept.holds(ElementToken(1)));
        assert!(!kept.holds(ElementToken(2)));
        assert!(!kept.holds(ElementToken(3)));
        // A read element ending again, a logic error, keeps it.
        assert_eq!(
            kept.read_ended(ElementToken(1), 10, false),
            Ended::Release(10)
        );
        assert!(kept.holds(ElementToken(1)));
    }

    /// Columns C and D: a view releases every told or read element it
    /// leaves out whose tell was handled, and keeps one it holds, one too
    /// early, and one in read, whose read's end decides.
    #[test]
    fn a_view_releases_what_it_leaves_out_once_its_tell_is_handled() {
        let mut kept = kept(5);
        let _ = kept.start_read(ElementToken(2), 1, true);
        let _ = kept.read_ended(ElementToken(2), 2, false);
        let _ = kept.start_read(ElementToken(3), 2, true);
        // 1 told, 2 read, 3 in read, 4 held, 5 written after the view.
        let released = kept.view(&view(&[4], 4), |token| Some(written(token)));
        assert_eq!(released, vec![(ElementToken(1), 1), (ElementToken(2), 2)]);
        assert!(kept.holds(ElementToken(3)));
        assert!(kept.holds(ElementToken(4)));
        assert!(kept.holds(ElementToken(5)));
        assert_eq!(
            kept.view(&view(&[], 5), |token| Some(written(token))),
            vec![(ElementToken(4), 4), (ElementToken(5), 5)]
        );
    }

    /// Too early: a tell with no report, one still waiting to be written,
    /// and one written after the view's position.
    #[test]
    fn a_tell_not_yet_handled_is_too_early_to_judge() {
        assert!(too_early(None, 9));
        assert!(too_early(
            Some(Report {
                position: 3,
                waiting: 1
            }),
            9
        ));
        assert!(too_early(
            Some(Report {
                position: 10,
                waiting: 0
            }),
            9
        ));
        assert!(!too_early(
            Some(Report {
                position: 9,
                waiting: 0
            }),
            9
        ));
    }

    /// The writer's record: kept, then a view whose acknowledged position
    /// precedes the write, then the write: the token stays until a view
    /// acknowledges its position.
    #[test]
    fn a_view_before_the_tells_write_keeps_its_element() {
        let mut reported = Reported::default();
        let mut kept = KeptElements::default();
        reported.queued(ElementToken(1));
        reported.written(ElementToken(1));
        kept.keep(ElementToken(2), 2_u64, rid(2), true);
        reported.queued(ElementToken(2));
        assert_eq!(
            kept.view(&view(&[], 1), |token| reported.report(token)),
            Vec::new()
        );
        reported.written(ElementToken(2));
        assert_eq!(
            reported.report(ElementToken(2)),
            Some(Report {
                position: 2,
                waiting: 0
            })
        );
        assert_eq!(
            kept.view(&view(&[], 1), |token| reported.report(token)),
            Vec::new()
        );
        assert_eq!(
            kept.view(&view(&[], 2), |token| reported.report(token)),
            vec![(ElementToken(2), 2)]
        );
        reported.forget(ElementToken(2));
        assert_eq!(reported.report(ElementToken(2)), None);
        assert_eq!(reported.position(), 2);
    }

    /// Column E: a read abandoned for the foreground lets its token go,
    /// and the element its call hands back is released.
    #[test]
    fn a_read_abandoned_for_the_foreground_is_released_when_its_call_returns() {
        let mut kept = kept(1);
        let _ = kept.start_read(ElementToken(1), 7, true);
        assert_eq!(kept.read_abandoned(8), None);
        assert_eq!(kept.read_abandoned(7), Some((ElementToken(1), 1)));
        assert_eq!(kept.start_read(ElementToken(1), 9, true), Start::NotThere);
        assert_eq!(
            kept.read_ended(ElementToken(1), 1, false),
            Ended::Release(1)
        );
    }

    /// Column F: the application's end releases every element it keeps, a
    /// read in flight's kept reference included, and the read releases
    /// its own.
    #[test]
    fn the_applications_end_releases_every_kept_reference() {
        let mut kept = kept(3);
        let _ = kept.start_read(ElementToken(1), 1, true);
        let _ = kept.start_read(ElementToken(2), 2, true);
        let _ = kept.read_ended(ElementToken(2), 2, false);
        assert_eq!(
            kept.ended(),
            vec![
                (ElementToken(1), 1),
                (ElementToken(2), 2),
                (ElementToken(3), 3)
            ]
        );
        assert!(kept.is_empty());
        assert_eq!(
            kept.read_ended(ElementToken(1), 1, false),
            Ended::Release(1)
        );
    }

    /// Column G: a check reads the keyboard focus of a told focus or menu
    /// opening, an element in a focus read and a read element with the
    /// runtime id, and not a told element of another kind, nor one in
    /// another read.
    #[test]
    fn the_check_reads_only_what_can_be_the_readers_last_focus() {
        let mut kept = KeptElements::default();
        kept.keep(ElementToken(1), 1, rid(7), true);
        kept.keep(ElementToken(2), 2, rid(7), false);
        kept.keep(ElementToken(3), 3, rid(7), true);
        kept.keep(ElementToken(4), 4, rid(7), false);
        kept.keep(ElementToken(5), 5, rid(7), true);
        kept.keep(ElementToken(6), 6, rid(8), true);
        let _ = kept.start_read(ElementToken(3), 1, true);
        let _ = kept.start_read(ElementToken(4), 2, false);
        let _ = kept.start_read(ElementToken(5), 3, true);
        let _ = kept.read_ended(ElementToken(5), 5, false);
        let found: Vec<(ElementToken, u64)> = kept
            .with_runtime_id(&rid(7))
            .map(|(token, element)| (token, *element))
            .collect();
        assert_eq!(
            found,
            [
                (ElementToken(1), 1),
                (ElementToken(3), 3),
                (ElementToken(5), 5)
            ]
        );
    }
}
