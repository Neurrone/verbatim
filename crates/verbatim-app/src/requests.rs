//! Who asks Core's requests from outside the event queue, and where each
//! outcome goes (`books/dev/src/reference/crates/verbatim-app.md`,
//! `requests`): the control plane's tree dump and the notification area
//! list's two reads of the taskbar
//! (`docs/design/readers-tell-core-asks-build1.md`, "Requests in Core").
//! Core keeps each request's due time, hang check and outpost, and answers
//! it once, under the token the shell keeps its asker by
//! (`QueueEffect::Answered`); [`Outside`] holds each [`Asker`] by its token
//! until then, and [`deliver`] carries the outcome out, into a
//! control-plane reply or a dialog's responder.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

use crossbeam_channel::Sender;
use verbatim_core::batch::Sent;
use verbatim_model::TreeNode;
use verbatim_model::protocol::Responder;
use verbatim_model::request::Outcome;

use verbatim_core::outside::{self, Dump, Shell};

/// The answer a tree-dump request waits for: the walked tree and whether the
/// walk was truncated, or a reason it failed.
pub(crate) type DumpTreeResult = Result<(TreeNode, bool), String>;

/// Names one control-plane tree dump, minted by the caller that waits for
/// it, so the caller can withdraw it without holding anything that keeps
/// its reply channel open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DumpTicket(u64);

impl DumpTicket {
    /// A ticket no other dump in this process has.
    pub(crate) fn mint() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// Who in the shell asked, and therefore where the outcome goes.
pub(crate) enum Asker {
    /// A control-plane tree dump, answered on `reply`. The channel is
    /// bounded with room for the one answer, so sending never blocks; a
    /// requester that already gave up has dropped its receiver.
    DumpTree {
        /// The requester's name for the dump.
        ticket: DumpTicket,
        /// The batch asked, its answer parsed against it.
        sent: Sent<Dump>,
        /// Whether each node's location is read.
        locations: bool,
        /// Where the answer goes.
        reply: Sender<DumpTreeResult>,
    },
    /// The tray or taskbar list dialog's request for the shell's items,
    /// whose outcome goes to `deliver`: the items, or `None` for any
    /// failure.
    ShellItems {
        /// The batch asked, its answer parsed against it.
        sent: Sent<Shell>,
        /// Where the outcome goes.
        deliver: Responder<Vec<verbatim_gui::ShellItem>>,
    },
    /// The list dialog's request for where the item clicked is now, whose
    /// outcome goes to `deliver`: the buttons that may be it, or `None` for
    /// any failure.
    ShellItemPosition {
        /// The batch asked, its answer parsed against it.
        sent: Sent<Shell>,
        /// Where the outcome goes.
        deliver: Responder<verbatim_model::ShellCandidates>,
    },
}

/// The askers of Core's requests from outside the event queue, each by
/// the token Core answers it under, from its request until its one
/// answer.
#[derive(Default)]
pub(crate) struct Outside {
    /// The next token, never reused.
    next: u64,
    /// Each asker waiting, by its token.
    waiting: BTreeMap<u64, Asker>,
}

impl Outside {
    /// Keeps `asker` and returns the token its request is asked under.
    pub(crate) fn begin(&mut self, asker: Asker) -> u64 {
        self.next += 1;
        self.waiting.insert(self.next, asker);
        self.next
    }

    /// The asker of the request asked under `token`, taken: its one
    /// answer has come. `None` for a token no asker waits under.
    pub(crate) fn answered(&mut self, token: u64) -> Option<Asker> {
        self.waiting.remove(&token)
    }

    /// The token the tree dump named `ticket` is asked under, while its
    /// asker waits.
    pub(crate) fn dump(&self, ticket: DumpTicket) -> Option<u64> {
        self.waiting
            .iter()
            .find(|(_, asker)| matches!(asker, Asker::DumpTree { ticket: asked, .. } if *asked == ticket))
            .map(|(token, _)| *token)
    }
}

/// A short description of an outcome other than a result, for logs and
/// control-plane errors.
fn describe(outcome: &Outcome) -> String {
    match outcome {
        Outcome::Result(_) => "the request returned an answer that did not parse".to_owned(),
        Outcome::Refused(refusal) => format!("the reader refused the request: {refusal:?}"),
        Outcome::NotThere => "the object is not there".to_owned(),
        Outcome::NotResponding => "the application is not responding".to_owned(),
        Outcome::Ended => "its outpost ended, the read failed or Core stopped waiting".to_owned(),
    }
}

/// Hands one outcome to its asker: a dialog or the control plane, its
/// result parsed against the batch the asker holds. Nothing re-enters the
/// reducer.
pub(crate) fn deliver(asker: Asker, outcome: Outcome) {
    match asker {
        Asker::DumpTree {
            sent,
            locations,
            reply,
            ..
        } => {
            let dumped = match outcome {
                Outcome::Result(answer) => outside::dumped(&sent, answer, locations),
                other => Err(describe(&other)),
            };
            // The requester may have timed out and dropped its receiver.
            let _ = reply.try_send(dumped.map(|dumped| (dumped.root, dumped.truncated)));
        }
        Asker::ShellItems { sent, deliver } => {
            let items = match outcome {
                Outcome::Result(answer) => outside::items(&sent, answer)
                    .ok_or_else(|| "the shell walk could not read every item".to_owned()),
                other => Err(describe(&other)),
            };
            match items {
                Ok(items) => deliver.answer(items),
                Err(reason) => {
                    tracing::warn!(reason, "the shell's items were not listed");
                    // Dropped unanswered: the dialog is handed nothing.
                    drop(deliver);
                }
            }
        }
        Asker::ShellItemPosition { sent, deliver } => {
            let found = match outcome {
                Outcome::Result(answer) => outside::candidates(&sent, answer)
                    .ok_or_else(|| "the taskbar's window could not be walked".to_owned()),
                other => Err(describe(&other)),
            };
            match found {
                Ok(found) => deliver.answer(found),
                Err(reason) => {
                    tracing::warn!(reason, "the shell item's position was not read");
                    // Dropped unanswered: the dialog is handed nothing.
                    drop(deliver);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crossbeam_channel::bounded;
    use verbatim_model::ShellItemKind;
    use verbatim_model::batch::{Descendant, Descended, Done, Missing, StepOutcome};
    use verbatim_model::property::{PropertyAnswer, PropertyId, PropertyValue, PropertyValues};
    use verbatim_model::request::{NodeRead, NodeWindow, Reply};
    use verbatim_model::{NodeId, Pid, Role, WindowHandle};

    use super::*;

    /// Answers the request asked under `token` of `outside` with
    /// `outcome`, as the reducer thread does at `QueueEffect::Answered`.
    fn answer(outside: &mut Outside, token: u64, outcome: Outcome) {
        if let Some(asker) = outside.answered(token) {
            deliver(asker, outcome);
        }
    }

    fn dump(reply: Sender<DumpTreeResult>) -> Asker {
        Asker::DumpTree {
            ticket: DumpTicket::mint(),
            sent: outside::dump(WindowHandle(5), false),
            locations: false,
            reply,
        }
    }

    /// One walk's outcome: the buttons named, the first the root.
    fn walked(names: &[&str]) -> StepOutcome {
        let window = NodeWindow {
            handle: WindowHandle(5),
            top_level: WindowHandle(5),
            root_owner: WindowHandle(5),
            process: Pid(9),
            thread: 1,
            desktop: false,
        };
        StepOutcome::Done(Done::Descendants(Descended {
            nodes: names
                .iter()
                .enumerate()
                .map(|(place, name)| Descendant {
                    node: NodeRead {
                        node: Some(NodeId::new(1 + u64::try_from(place).expect("small"))),
                        window,
                        values: PropertyValues(vec![
                            PropertyAnswer {
                                id: PropertyId::Name,
                                value: Some(PropertyValue::Text((*name).to_owned())),
                            },
                            PropertyAnswer {
                                id: PropertyId::Role,
                                value: Some(PropertyValue::Role(Role::Button)),
                            },
                        ]),
                    },
                    parent: (place > 0).then_some(0),
                    root: 0,
                })
                .collect(),
            complete: true,
        }))
    }

    /// A shell items outcome reaches the dialog, and a request whose
    /// outpost ended gives it nothing.
    #[test]
    fn a_shell_items_answer_reaches_the_dialog_and_an_ended_outpost_gives_none() {
        let outcomes = std::sync::Arc::default();
        let mut outside = Outside::default();
        let answered = outside.begin(shell_items(&outcomes));
        let ended = outside.begin(shell_items(&outcomes));
        let reply = Reply::Steps(vec![
            walked(&["Volume"]),
            StepOutcome::NotThere(Missing::Object),
            StepOutcome::NotThere(Missing::Object),
        ]);
        answer(&mut outside, answered, Outcome::Result(reply));
        answer(&mut outside, ended, Outcome::Ended);
        assert_eq!(
            *outcomes.lock().expect("outcomes"),
            vec![
                Some(vec![verbatim_gui::ShellItem {
                    name: "Volume".to_owned(),
                    runtime_id: Vec::new(),
                }]),
                None
            ]
        );
    }

    /// A shell items request whose outcomes are recorded in order.
    fn shell_items(
        outcomes: &std::sync::Arc<std::sync::Mutex<Vec<Option<Vec<verbatim_gui::ShellItem>>>>>,
    ) -> Asker {
        let kept = std::sync::Arc::clone(outcomes);
        Asker::ShellItems {
            sent: outside::shell(ShellItemKind::SystemTray, None),
            deliver: Responder::new(move |items| kept.lock().expect("outcomes").push(items)),
        }
    }

    #[test]
    fn a_shell_item_position_answer_reaches_the_dialog_and_a_failure_gives_none() {
        let outcomes: std::sync::Arc<
            std::sync::Mutex<Vec<Option<verbatim_model::ShellCandidates>>>,
        > = std::sync::Arc::default();
        let mut outside = Outside::default();
        let chosen = verbatim_model::ShellItem {
            name: "Clock".to_owned(),
            runtime_id: Vec::new(),
        };
        let asker = |outcomes: &std::sync::Arc<std::sync::Mutex<Vec<_>>>| {
            let kept = std::sync::Arc::clone(outcomes);
            Asker::ShellItemPosition {
                sent: outside::shell(ShellItemKind::Taskbar, Some(&chosen)),
                deliver: Responder::new(move |position| {
                    kept.lock().expect("outcomes").push(position);
                }),
            }
        };
        let found = verbatim_model::ShellCandidates {
            candidates: vec![verbatim_model::ShellCandidate {
                item: chosen.clone(),
                offscreen: false,
                rect: None,
            }],
            complete: true,
        };
        let answered = outside.begin(asker(&outcomes));
        let failed = outside.begin(asker(&outcomes));
        let reply = Reply::Steps(vec![walked(&["Clock"])]);
        answer(&mut outside, answered, Outcome::Result(reply));
        answer(&mut outside, failed, Outcome::NotResponding);
        assert_eq!(*outcomes.lock().expect("outcomes"), vec![Some(found), None]);
        assert!(outside.answered(answered).is_none(), "answered once");
    }

    #[test]
    fn an_old_dump_answer_never_satisfies_a_newer_request() {
        let mut outside = Outside::default();
        let (old_tx, old_rx) = bounded(1);
        let old = outside.begin(dump(old_tx));
        // The first requester timed out and went away; a second asks.
        drop(old_rx);
        let (new_tx, new_rx) = bounded(1);
        let new = outside.begin(dump(new_tx));

        // The old request's late answer arrives first.
        answer(&mut outside, old, Outcome::Ended);
        assert!(new_rx.try_recv().is_err(), "the newer request is untouched");

        let reply = Reply::Steps(vec![walked(&["new"])]);
        answer(&mut outside, new, Outcome::Result(reply));
        let (root, _) = new_rx.try_recv().expect("answered").expect("a tree");
        assert_eq!(root.snapshot.name.as_deref(), Some("new"));
    }

    /// Giving up a tree dump finds the dump with that ticket and no other.
    #[test]
    fn a_dump_given_up_matches_only_its_ticket() {
        let mut outside = Outside::default();
        let (reply, _answer) = bounded(1);
        let ticket = DumpTicket::mint();
        let token = outside.begin(Asker::DumpTree {
            ticket,
            sent: outside::dump(WindowHandle(5), false),
            locations: false,
            reply,
        });
        assert_eq!(outside.dump(ticket), Some(token));
        assert_eq!(outside.dump(DumpTicket::mint()), None);
    }
}
