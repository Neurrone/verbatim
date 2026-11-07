//! The generated test of Core's check before speaking and its live
//! re-reads, with the focus readers' title check, against a reference model
//! of NVDA's focus ancestry (`books/dev/src/reference/crates/verbatim-core.md`,
//! "The ancestry cache").
//!
//! A small world: two applications, the first with two top-level windows,
//! the second with one, each window holding an outer container, an inner
//! one inside it, and two controls inside that.
//! The first application has two focus readers whose views of an element
//! differ (the listener's UIA object and the outpost's MSAA object, by
//! backend and node), as a UIA window with an MSAA menu of the same process
//! has; the second has the listener alone. Each reader keeps an ancestry
//! cache, its last read's ancestors as it reported them, with the window
//! titles it saved, and stops a later walk at the first entry its cache
//! holds, reusing the entries above, unless a saved title changed. Names and
//! titles change at any step, the user in the window or not. A case steps
//! through renames, focuses from either reader in any order, a foreground
//! change before a focus in another top-level window, and the pending live
//! re-read answered, answered gone, replaced by a newer one, or ended by a
//! newer focus or the listener's replacement.
//!
//! The reference model is NVDA's focus ancestry: the live chain of the
//! focus NVDA last took, with no cache, emptied at a foreground change, as
//! NVDA's line then holds only the new window. The invariants:
//!
//! - no entry spoken, as entered or as the title, differs from the live
//!   tree's at the read;
//! - a report is set aside only where NVDA's ancestry would meet the new
//!   chain above the reader's cache join;
//! - no set-aside persists after its live re-read: a pending live re-read's
//!   answer asks for nothing, and is applied, its focus becoming Core's,
//!   while its window is still in front; once the user has gone to another
//!   window, it is refused by the foreground test as any report of the
//!   window left is.

use std::collections::BTreeMap;

use proptest::strategy::Strategy;
use proptest::test_runner::{Config, TestError, TestRunner};
use verbatim_model::{
    Backend, Effect, FirstCachedAncestor, FocusReaderKind, ForegroundWindow, Input, NodeDetails,
    NodeId, NodeSnapshot, NormalizedEvent, OutpostId, Pid, Role, SegmentContent, StateSet, TraceId,
    WindowFacts, WindowHandle,
};

use crate::SrState;

/// The world's windows: (application, handle).
const WINDOWS: [(u64, u64); 3] = [(1, 0x11), (1, 0x12), (2, 0x21)];

/// An element of the world: a window, its outer or inner container, or
/// one of its two controls, by window index and part (0 window, 1 outer
/// container, 2 inner container, 3 and 4 controls).
type Element = (usize, u8);

/// One step of a case.
#[derive(Clone, Copy, Debug)]
enum Action {
    /// The name of a window (`part` 0) or of one of its containers
    /// changes.
    Rename { window: u8, part: u8 },
    /// A focus on a control of a window, read by the application's
    /// listener (`outpost` false) or outpost.
    Focus {
        window: u8,
        control: u8,
        outpost: bool,
    },
    /// The pending live re-read is answered (0), answered gone (1), or the
    /// listener is replaced (2).
    Settle { how: u8 },
}

fn action() -> impl Strategy<Value = Action> {
    use proptest::prelude::*;
    prop_oneof![
        (0u8..3, 0u8..3).prop_map(|(window, part)| Action::Rename { window, part }),
        (0u8..3, 3u8..5, any::<bool>()).prop_map(|(window, control, outpost)| Action::Focus {
            window,
            control,
            outpost
        }),
        (0u8..3).prop_map(|how| Action::Settle { how }),
    ]
}

/// A focus reader's ancestry cache: its last read's ancestors, outermost
/// first, as it reported them, and the window titles it saved.
#[derive(Clone, Debug, Default)]
struct Cache {
    ancestors: Vec<(Element, String)>,
    titles: Vec<(u64, String)>,
}

/// A live re-read Core asked for, as the world keeps it.
#[derive(Clone, Copy, Debug)]
struct Asked {
    request: u64,
    reader: FocusReaderKind,
    control: Element,
    at_us: u64,
}

/// One case's world, its focus readers, Core, and NVDA's reference
/// ancestry.
struct World {
    names: BTreeMap<Element, String>,
    renames: u32,
    caches: BTreeMap<(u64, FocusReaderKind), Cache>,
    state: SrState,
    /// NVDA's focus ancestry: its focus's live chain, by reader and
    /// element.
    nvda: Vec<(FocusReaderKind, Element)>,
    /// The pending live re-read of each focus reader, by application and
    /// kind.
    pending: BTreeMap<(u64, FocusReaderKind), Asked>,
    front: Option<usize>,
    now: u64,
    log: Vec<String>,
}

fn kind_of(outpost: bool) -> FocusReaderKind {
    if outpost {
        FocusReaderKind::Outpost
    } else {
        FocusReaderKind::Listener
    }
}

impl World {
    fn new() -> Self {
        let mut names = BTreeMap::new();
        for window in 0..WINDOWS.len() {
            names.insert((window, 0), format!("Window {window}"));
            names.insert((window, 1), format!("Outer {window}"));
            names.insert((window, 2), format!("Inner {window}"));
            names.insert((window, 3), format!("A{window}"));
            names.insert((window, 4), format!("B{window}"));
        }
        Self {
            names,
            renames: 0,
            caches: BTreeMap::new(),
            state: SrState::new(),
            nvda: Vec::new(),
            pending: BTreeMap::new(),
            front: None,
            now: 100,
            log: Vec::new(),
        }
    }

    fn name(&self, element: Element) -> String {
        self.names[&element].clone()
    }

    /// The node a reader of `kind` gives `element`: its own object, so the
    /// two readers' views of one element are two objects.
    fn node_id(kind: FocusReaderKind, (window, part): Element) -> NodeId {
        let offset = match kind {
            FocusReaderKind::Listener => 0,
            FocusReaderKind::Outpost => 500,
        };
        NodeId::new(offset + 10 * window as u64 + u64::from(part) + 1)
            .with_outpost(OutpostId(WINDOWS[window].0))
    }

    fn snapshot(kind: FocusReaderKind, element: Element, name: String) -> NodeSnapshot {
        let (window, part) = element;
        NodeSnapshot {
            id: Self::node_id(kind, element),
            backend: match kind {
                FocusReaderKind::Listener => Backend::Uia,
                FocusReaderKind::Outpost => Backend::Msaa,
            },
            role: match part {
                0 => Role::Window,
                1 | 2 => Role::Group,
                _ => Role::Button,
            },
            name: Some(name),
            value: None,
            states: StateSet::new(),
            details: NodeDetails {
                window: (part == 0).then_some(WindowHandle(WINDOWS[window].1)),
                ..NodeDetails::default()
            },
        }
    }

    /// The element whose node `id` is, by either reader.
    fn element_of(id: NodeId) -> Option<Element> {
        let number = id.number().checked_sub(1)?;
        let number = if number >= 500 { number - 500 } else { number };
        let window = usize::try_from(number / 10).ok()?;
        let part = u8::try_from(number % 10).ok()?;
        (window < WINDOWS.len() && part < 5).then_some((window, part))
    }

    /// The reader's read of `control`: its ancestors, outermost first,
    /// each with the name it reports and whether it came from the cache;
    /// with `live`, no cache is used. The cache is refreshed by the read.
    fn read(
        &mut self,
        kind: FocusReaderKind,
        control: Element,
        live: bool,
    ) -> Vec<(Element, String, bool)> {
        let window = control.0;
        let application = WINDOWS[window].0;
        let cache = self.caches.entry((application, kind)).or_default().clone();
        let titles_hold = cache.titles.iter().all(|(handle, title)| {
            let index = WINDOWS.iter().position(|(_, h)| h == handle).unwrap();
            self.names[&(index, 0)] == *title
        });
        let usable = !live && titles_hold;
        // Walked from the control outward: the inner container, the outer
        // one, then the window.
        let mut chain = Vec::new();
        let mut met = None;
        for element in [(window, 2u8), (window, 1u8), (window, 0u8)] {
            chain.push((element, self.name(element), false));
            if usable
                && let Some(at) = cache
                    .ancestors
                    .iter()
                    .position(|(cached, _)| *cached == element)
            {
                met = Some(at);
                break;
            }
        }
        chain.reverse();
        if let Some(at) = met {
            let above: Vec<_> = cache.ancestors[..at]
                .iter()
                .map(|(element, name)| (*element, name.clone(), true))
                .collect();
            chain = above.into_iter().chain(chain).collect();
        }
        let titles = chain
            .iter()
            .filter(|((_, part), _, _)| *part == 0)
            .map(|((window, _), name, cached)| {
                // A window entry taken from the cache keeps the title saved
                // with it; one read live saves its text now.
                let title = if *cached {
                    cache
                        .titles
                        .iter()
                        .find(|(handle, _)| *handle == WINDOWS[*window].1)
                        .map_or_else(|| name.clone(), |(_, title)| title.clone())
                } else {
                    self.name((*window, 0))
                };
                (WINDOWS[*window].1, title)
            })
            .collect();
        self.caches.insert(
            (application, kind),
            Cache {
                ancestors: chain
                    .iter()
                    .map(|(element, name, _)| (*element, name.clone()))
                    .collect(),
                titles,
            },
        );
        chain
    }

    fn facts(window: usize) -> WindowFacts {
        let handle = WindowHandle(WINDOWS[window].1);
        WindowFacts {
            top_level: handle,
            root_owner: handle,
            topmost: false,
            under_active_window: None,
            in_foreground: true,
            foreground_window: handle,
            class: String::new(),
            application: String::new(),
            alt_held: false,
            foreground_is_shell: false,
        }
    }

    fn report(
        kind: FocusReaderKind,
        control: (Element, String),
        chain: &[(Element, String, bool)],
        (at_us, answers): (u64, Option<u64>),
    ) -> Input {
        let window = control.0.0;
        let first_cached_ancestor = chain
            .iter()
            .rposition(|(_, _, cached)| *cached)
            .map(|index| FirstCachedAncestor {
                index: u32::try_from(index).unwrap(),
                reader: kind,
            });
        Input::Event {
            trace_id: TraceId::from_raw(at_us),
            observed_at_us: at_us,
            source: Pid(u32::try_from(WINDOWS[window].0).unwrap()),
            backend: Backend::Uia,
            window: Some(Self::facts(window)),
            event: NormalizedEvent::FocusChanged {
                node: Self::snapshot(kind, control.0, control.1),
                whole_window: false,
                ancestors: chain
                    .iter()
                    .map(|(element, name, _)| Self::snapshot(kind, *element, name.clone()))
                    .collect(),
                selected_child: None,
                text: None,
                earlier: None,
                group: None,
                first_cached_ancestor,
                answers,
            },
            purpose: None,
        }
    }

    fn step(&mut self, action: Action) {
        self.now += 10;
        match action {
            Action::Rename { window, part } => {
                let element = (usize::from(window), part);
                self.renames += 1;
                let renamed = format!("{} {}", self.name(element), self.renames);
                self.names.insert(element, renamed);
            }
            Action::Focus {
                window,
                control,
                outpost,
            } => {
                let window = usize::from(window);
                // Only the first application has an outpost reader.
                let kind = kind_of(outpost && WINDOWS[window].0 == 1);
                if self.front != Some(window) {
                    self.front = Some(window);
                    self.nvda.clear();
                    let handle = WindowHandle(WINDOWS[window].1);
                    let _ = crate::reduce(
                        &mut self.state,
                        &Input::Foreground(ForegroundWindow {
                            window: handle,
                            root_owner: handle,
                            process: Pid(u32::try_from(WINDOWS[window].0).unwrap()),
                            observed_at_us: self.now,
                            own: false,
                            thread: 0,
                            console: false,
                            integrity: None,
                            own_integrity: None,
                        }),
                    );
                    self.now += 10;
                }
                let element = (window, control);
                let chain = self.read(kind, element, false);
                let input = Self::report(
                    kind,
                    (element, self.name(element)),
                    &chain,
                    (self.now, None),
                );
                self.reduce_report(kind, element, &chain, &input, None);
            }
            Action::Settle { how } => match how {
                0 => {
                    if let Some(&asked) = self.pending.values().next() {
                        let chain = self.read(asked.reader, asked.control, true);
                        let input = Self::report(
                            asked.reader,
                            (asked.control, self.name(asked.control)),
                            &chain,
                            (asked.at_us, Some(asked.request)),
                        );
                        self.reduce_report(
                            asked.reader,
                            asked.control,
                            &chain,
                            &input,
                            Some(asked),
                        );
                    }
                }
                1 => {
                    if let Some(key) = self.pending.keys().next().copied() {
                        let asked = self.pending.remove(&key).unwrap();
                        let effects = crate::reduce(
                            &mut self.state,
                            &Input::LiveRereadGone {
                                outpost: OutpostId(WINDOWS[asked.control.0].0),
                                request: asked.request,
                            },
                        );
                        assert_eq!(effects, [], "{:?}", self.log);
                    }
                }
                _ => {
                    let effects = crate::reduce(&mut self.state, &Input::ListenerReplaced);
                    assert_eq!(effects, [], "{:?}", self.log);
                    self.pending
                        .retain(|(_, reader), _| *reader != FocusReaderKind::Listener);
                }
            },
        }
        self.log.push(format!("{action:?}"));
        assert_eq!(
            self.state.live_rereads.pending().len(),
            self.pending.len(),
            "Core's pending live re-reads and the world's: {:?}",
            self.log
        );
    }

    /// Reduces a report of `control` by reader `kind`, whose chain is
    /// `chain`, answering `answering` when given, and checks the
    /// invariants against the live tree and NVDA's ancestry.
    fn reduce_report(
        &mut self,
        kind: FocusReaderKind,
        control: Element,
        chain: &[(Element, String, bool)],
        input: &Input,
        answering: Option<Asked>,
    ) {
        let effects = crate::reduce(&mut self.state, input);
        let log = &self.log;
        // No entry spoken differs from the live tree's.
        for effect in &effects {
            if let Effect::Speak(utterance, _) = effect
                && let Some(validity) = utterance.validity
                && !validity.had_focus
                && let Some(element) = Self::element_of(validity.node)
            {
                let live = SegmentContent::Label(self.name(element));
                assert!(
                    utterance
                        .segments
                        .iter()
                        .any(|segment| segment.content == live),
                    "{element:?} spoken as {:?}, live {live:?}: {log:?}",
                    utterance.segments
                );
            }
        }
        let new_chain: Vec<(FocusReaderKind, Element)> = chain
            .iter()
            .map(|(element, _, _)| (kind, *element))
            .chain(std::iter::once((kind, control)))
            .collect();
        let reread = effects.iter().find_map(|effect| match effect {
            Effect::LiveReread(reread) => Some(reread.clone()),
            _ => None,
        });
        if let Some(asked) = answering {
            // A pending live re-read's answer asks nothing, and is applied
            // while its window is still the one in front; once the user
            // has gone to another window, it is refused as any report of
            // the window left is, and says nothing.
            assert_eq!(reread, None, "an answer asked for another: {log:?}");
            if self.front == Some(control.0) {
                assert_eq!(
                    self.state.focused().map(|(_, node)| node.id),
                    Some(Self::node_id(kind, control)),
                    "the answer to {asked:?} was not applied: {log:?}"
                );
            } else {
                assert_eq!(effects, [], "the answer to {asked:?} was spoken: {log:?}");
            }
            self.pending.remove(&(WINDOWS[control.0].0, kind));
        }
        if let Some(reread) = reread {
            // Set aside only where NVDA's ancestry meets the new chain
            // above the reader's cache join.
            let join = chain.iter().rposition(|(_, _, cached)| *cached).unwrap() + 1;
            let met = (0..new_chain.len())
                .rev()
                .find(|&at| self.nvda.contains(&new_chain[at]));
            assert!(
                met.is_none_or(|at| at < join),
                "set aside though NVDA meets at {met:?}, the cache join {join}: {log:?}"
            );
            self.pending.insert(
                (WINDOWS[control.0].0, kind),
                Asked {
                    request: reread.request,
                    reader: kind,
                    control,
                    at_us: match input {
                        Input::Event { observed_at_us, .. } => *observed_at_us,
                        _ => 0,
                    },
                },
            );
            return;
        }
        if effects
            .iter()
            .any(|effect| matches!(effect, Effect::DropExpiredSpeech(_)))
        {
            // Taken, not dropped as a duplicate: NVDA's ancestry is the
            // live chain, and every pending live re-read ended.
            self.nvda = new_chain;
            self.pending.clear();
        }
    }
}

/// Runs `actions` from a new world, returning its log.
fn case(actions: &[Action]) -> Vec<String> {
    let mut world = World::new();
    for &action in actions {
        world.step(action);
    }
    world.log
}

fn run(config: Config) {
    let mut runner = TestRunner::new(config);
    let result = runner.run(&proptest::collection::vec(action(), 1..40), |actions| {
        case(&actions);
        Ok(())
    });
    if let Err(TestError::Fail(reason, actions)) = result {
        panic!("{reason}: {actions:?}");
    }
}

/// The gate's seed and case count.
const SEED: u64 = 20_261_108;
const CASES: u32 = 400;

#[test]
fn generated_cases_keep_the_ancestry_caches_invariants() {
    run(crate::queries::explore_config(SEED, CASES));
}
