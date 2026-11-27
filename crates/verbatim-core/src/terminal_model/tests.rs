//! Unit tests of Core's terminal model: what the reading rules say becomes
//! the flood policy's speech, and each key input becomes the rules' key.

use verbatim_model::{
    CaretKey, CaretMotion, CaretReport, NodeId, OutpostId, Pid, Role, TextAnchor, TextChunk,
    TextUnit, TraceId,
};
use verbatim_text::terminal_reading::{Settings, Speech, Step};
use verbatim_text::terminal_rows::Lost;

use super::{Said, SaidItem, rules_key, speech_of};
use crate::state::SrState;

use crate::terminal::{Skipped, TerminalItem, TerminalKeyKind, TerminalUnit};

#[test]
fn what_the_rules_say_is_cores_speech_in_order() {
    let step = Step {
        speech: vec![
            Speech::Caret {
                row: 2,
                unit: verbatim_text::terminal_reading::Unit::Word,
                text: "word".to_owned(),
            },
            Speech::LostRead(Lost::About { above: 41 }),
            Speech::Line {
                row: 3,
                text: "first".into(),
            },
            Speech::Growth {
                row: 6,
                text: " grown".to_owned(),
            },
            Speech::Echo {
                row: 2,
                text: "x".to_owned(),
            },
        ],
        followed: true,
        ..Step::default()
    };
    assert_eq!(
        speech_of(step),
        Said {
            items: vec![
                SaidItem::Item(TerminalItem::Caret {
                    unit: TerminalUnit::Word,
                    text: "word".to_owned(),
                }),
                // After a count read, its lines follow an "about" count of
                // the rows above it; a line's growth is part of its line.
                SaidItem::Item(TerminalItem::Skipped(Skipped::About(41))),
                SaidItem::Line("first".into()),
                SaidItem::Item(TerminalItem::Growth("grown".to_owned())),
                SaidItem::Item(TerminalItem::Echo("x".to_owned())),
            ],
            followed: true,
        }
    );
}

#[test]
fn each_key_input_is_the_rules_key_and_a_clearing_key_forget() {
    let node = NodeId::in_outpost(OutpostId(1), 12);
    let mut state = SrState::new();
    let window = crate::event_queue::RequestWindow {
        handle: verbatim_model::WindowHandle(5),
        top_level: verbatim_model::WindowHandle(4),
        root_owner: verbatim_model::WindowHandle(4),
        process: Pid(1),
        thread: 41,
        by_class: None,
    };
    crate::terminal::reading::modelled_for_tests(&mut state, node, window);
    let binding = verbatim_model::request::CaretBinding {
        node,
        window: window.served_window(),
        role: Role::Terminal,
        provider: verbatim_model::TextProvider::Uia,
        record: None,
        formatting: verbatim_model::TextAttributes::default(),
    };
    let press = |action, at_ms: u64| {
        crate::keys::entry::PressEntry::new(
            TraceId::from_raw(2),
            verbatim_model::request::Press {
                key_seq: verbatim_model::KeySeq::NONE_YET,
                pressed_at_us: at_ms * 1_000,
                action,
            },
        )
    };
    let caret = |motion, at_ms| {
        press(
            verbatim_model::request::PressAction::CaretKey {
                key: CaretKey {
                    motion,
                    select: false,
                },
                held: None,
                bound: Some(Box::new(binding.clone())),
            },
            at_ms,
        )
    };
    let typed = |text: &str, at_ms| {
        press(
            verbatim_model::request::PressAction::Typed {
                text: text.to_owned(),
            },
            at_ms,
        )
    };
    // Each key is taken as it reaches the inbox.
    for entry in [
        caret(CaretMotion::NextWord, 1),
        caret(CaretMotion::PreviousLine, 2),
        caret(CaretMotion::Delete, 3),
        typed("a", 4),
        typed("\r", 5),
        typed("\t", 6),
        typed("\u{1}", 7),
        // Escape: the hook's clearing key.
        press(verbatim_model::request::PressAction::Clearing, 8),
    ] {
        crate::keys::terminal::arrived(&mut state, &entry);
    }
    let kept: Vec<_> = state.terminals[0]
        .pending
        .iter()
        .map(|key| {
            let key = rules_key(key);
            (key.kind, key.at_ms)
        })
        .collect();
    let expected: Vec<_> = [
        (TerminalKeyKind::Caret(TerminalUnit::Word), 1),
        (TerminalKeyKind::Caret(TerminalUnit::Line), 2),
        (TerminalKeyKind::Delete, 3),
        (TerminalKeyKind::Typed("a".to_owned()), 4),
        (TerminalKeyKind::Enter, 5),
        (TerminalKeyKind::Forget, 6),
        (TerminalKeyKind::Other, 7),
        (TerminalKeyKind::Forget, 8),
    ]
    .iter()
    .map(|(kind, at_ms)| {
        let key = rules_key(&pending(kind.clone(), *at_ms));
        (key.kind, key.at_ms)
    })
    .collect();
    assert_eq!(kept, expected);
    assert_eq!(
        rules_key(&pending(TerminalKeyKind::Caret(TerminalUnit::Line), 42)),
        verbatim_text::terminal_reading::Key {
            kind: verbatim_text::terminal_reading::KeyKind::Caret(
                verbatim_text::terminal_reading::Unit::Line
            ),
            at_ms: 42,
            tag: verbatim_text::terminal_reading::KeyTag {
                trace: 7,
                number: 3,
            },
        }
    );
}

/// A terminal focused with its caret and modelled, then focused again under
/// the same node, whose read says its reader's runtime id now names
/// another element: Core forgets the node's caret and its model, so the
/// new element's text is taken afresh, as a new object's is
/// (`docs/parity/focus-events.md`, "Duplicate focus suppression").
#[test]
fn an_element_gone_under_the_same_node_forgets_its_caret_and_model() {
    use crate::entries::tests::{focus_read, node, ready, uia_focus_read, uia_tell, values};
    use verbatim_model::request::{FocusCaret, FocusCaretRead};
    let mut held = ready();
    let terminal = |caret: FocusCaret, reused: bool| {
        let mut read = focus_read(None);
        read.focus.read.values = values("Terminal", Role::Terminal);
        read.caret = caret;
        read.runtime_id_reused = reused;
        read
    };
    let caret = CaretReport {
        line: TextChunk {
            unit: TextUnit::Line,
            text: "ready> ".to_owned(),
            start: TextAnchor(1),
            offset: 7,
            languages: Vec::new(),
            first: false,
            last: true,
            truncated: false,
            formats: Vec::new(),
        },
        selection: None,
    };
    let read = FocusCaret::Read(Box::new(FocusCaretRead {
        caret,
        selected_text: None,
        baseline: TextAnchor(2),
    }));
    let _ = uia_focus_read(
        &mut held,
        1_000,
        uia_tell(1, Vec::new()),
        terminal(read, false),
    );
    held.state.terminals.add(node(2), Settings::default());
    assert_eq!(
        (
            held.state.focus_caret(node(2)).is_some(),
            held.state.terminals.len()
        ),
        (true, 1),
        "the first element's caret and model are kept"
    );

    let _ = uia_focus_read(
        &mut held,
        5_000,
        uia_tell(1, Vec::new()),
        terminal(FocusCaret::NotRead, true),
    );
    assert_eq!(
        (
            held.state.focus_caret(node(2)).is_some(),
            held.state.terminals.len()
        ),
        (false, 0)
    );
}

// "Identical reads" (`docs/design/readers-tell-core-asks-build3.md`): Core
// gives the rules no read identical to the last one it gave them, unless the
// read waited or is the first after the focus arrived.

/// The terminal the identical-read tests read: node 9 of outpost 1.
fn watched() -> NodeId {
    NodeId::new(9).with_outpost(OutpostId(1))
}

/// The simulated terminal read as the outpost reads it, from the start
/// Core names with the screen's height Core's model found, each read taken
/// by Core's model as Core takes an output read's answer.
struct Watch {
    sim: verbatim_term_sim::Terminal,
    state: SrState,
    /// The start anchors minted, the anchor numbered `n` at `n - 1`.
    anchors: Vec<verbatim_term_sim::Range>,
}

impl Watch {
    fn new() -> Self {
        Self {
            sim: verbatim_term_sim::Terminal::new(
                verbatim_term_sim::Flavor::ConsoleHost,
                20,
                4,
                10,
            ),
            state: SrState::new(),
            anchors: Vec::new(),
        }
    }

    /// Core's model of the terminal.
    fn model(&self) -> &super::TerminalModel {
        super::model_of(&self.state, watched()).expect("a model of the terminal")
    }

    /// Writes `bytes`, then a read begun at `at_ms` from the start Core
    /// names, which `waited` for Core's speech queue: what Core took.
    fn write(&mut self, bytes: &str, at_ms: u64, waited: bool) -> super::Taken {
        self.sim.write(bytes.as_bytes());
        let model = super::model_of(&self.state, watched());
        let named = model.and_then(|model| model.start);
        let height = model.map_or(0, super::TerminalModel::height);
        let snapshot = self
            .sim
            .snapshot(&verbatim_term_sim::Query {
                start: named.map(|anchor| {
                    self.anchors[usize::try_from(anchor.0).expect("a small number") - 1]
                }),
                screen_rows: height,
                count: false,
                above: height,
            })
            .expect("a simulated read");
        self.anchors.push(snapshot.next_start);
        let start = TextAnchor(u64::try_from(self.anchors.len()).expect("a small count"));
        let unpadded = verbatim_model::TerminalText::unpadded;
        let reply = crate::terminal::reader::WindowReply {
            snapshot: verbatim_model::TerminalSnapshot {
                text: unpadded(&snapshot.text),
                screen: unpadded(&snapshot.screen),
                from_start: snapshot.from_start,
                shrank: snapshot.shrank,
                at_document_start: snapshot.at_document_start,
                alternate: snapshot.alternate,
                first_row: unpadded(&snapshot.first_row),
                caret_rows_to_end: snapshot.caret_rows_to_end,
                caret_line: unpadded(&snapshot.caret_line),
                caret_column: u32::try_from(snapshot.caret_column).expect("a short row"),
                above: snapshot.above,
                rows_above: snapshot.rows_above,
                started_ms: at_ms,
                ended_ms: at_ms,
                waited: false,
                colours: None,
                caret: None,
            },
            start,
            started_us: at_ms * 1_000,
            point: None,
        };
        super::take(&mut self.state, watched(), &reply, (named, waited))
    }
}

/// The speech the rules' `speech` becomes.
fn items(speech: Vec<Speech>) -> Vec<SaidItem> {
    speech_of(Step {
        speech,
        ..Step::default()
    })
    .items
}

#[test]
fn a_read_identical_to_the_last_given_is_not_given_unless_it_waited() {
    let mut watch = Watch::new();
    assert!(watch.write("50%", 100, false).speech.is_some());
    // The first read took the screen alone; this one a screen above it too.
    assert!(watch.write("", 200, false).speech.is_some());
    assert!(watch.write("", 300, false).speech.is_none());
    assert!(watch.write("", 400, true).speech.is_some());
    assert!(watch.write("\r60%", 500, false).speech.is_some());
}

/// A read identical to the last one given, and not given, leaves the start
/// Core names at that read's, so the rows of a search are counted from a
/// read Core took (Table 7: the identical read's start is let go).
#[test]
fn a_read_not_given_leaves_the_start_core_names_at_the_last_read_given() {
    let mut watch = Watch::new();
    let _ = watch.write("50%", 100, false);
    assert!(watch.write("", 200, false).speech.is_some());
    assert_eq!(watch.model().start, Some(TextAnchor(2)));
    assert!(watch.write("", 300, false).speech.is_none());
    assert_eq!(watch.model().start, Some(TextAnchor(2)));
    assert!(watch.write("", 400, false).speech.is_none());
    assert_eq!(watch.model().start, Some(TextAnchor(2)));
    assert!(watch.write("\r60%", 500, false).speech.is_some());
    assert_eq!(watch.model().start, Some(TextAnchor(5)));
}

/// A row changes at a read, a read identical to it follows that waited for
/// speech, and the row changes again at a read that waited: the identical
/// read is given, so the two changes are not at reads in a row and the row
/// is not volatile ("Volatile rows").
#[test]
fn a_waited_read_identical_to_the_last_is_given_so_a_row_changed_around_it_is_not_volatile() {
    let mut watch = Watch::new();
    let _ = watch.write("50%", 100, false);
    let _ = watch.write("", 200, false);
    let changed = watch.write("\r60%", 10_000, false).speech.expect("given");
    assert_eq!(
        changed.items,
        items(vec![Speech::Changed {
            row: 0,
            text: "60%".to_owned(),
        }])
    );
    assert!(watch.write("", 15_000, true).speech.is_some());
    let again = watch.write("\r70%", 20_000, true).speech.expect("given");
    assert_eq!(
        again.items,
        items(vec![Speech::Changed {
            row: 0,
            text: "70%".to_owned(),
        }])
    );
    assert_eq!(watch.model().reader.due(), None);
}

/// The read after the focus returns is given though it is identical to the
/// last one given: the rules take the first read after the focus as the
/// one that speaks nothing, so had it been dropped, the next read, showing
/// new output, would have said nothing of it.
#[test]
fn the_read_after_the_focus_returns_is_given_though_identical() {
    let mut watch = Watch::new();
    let _ = watch.write("50%", 100, false);
    let _ = watch.write("", 200, false);
    assert!(watch.write("", 300, false).speech.is_none());
    super::model_or_new(&mut watch.state, watched()).focused(350);
    assert!(watch.write("", 400, false).speech.is_some());
}

/// The focus's exception to the identical-read rule ends at the first read
/// given after the focus: a read identical to that one is not given.
#[test]
fn the_focus_exception_ends_at_the_first_read_given() {
    let mut watch = Watch::new();
    let _ = watch.write("50%", 100, false);
    let _ = watch.write("", 200, false);
    assert!(watch.write("", 300, false).speech.is_none());
    super::model_or_new(&mut watch.state, watched()).focused(350);
    assert!(watch.write("", 400, false).speech.is_some());
    assert!(watch.write("", 500, false).speech.is_none());
}

/// A key of `kind` Core took at `at_ms`, under trace 7, numbered 3.
fn pending(kind: TerminalKeyKind, at_ms: u64) -> super::PendingKey {
    super::PendingKey {
        kind,
        at_ms,
        trace: TraceId::from_raw(7),
        seq: verbatim_model::KeySeq::new(3),
    }
}

/// A key Core holds for `watch`'s model, typed at `at_ms`.
fn hold_key(watch: &mut Watch, at_ms: u64) {
    let model = super::model_or_new(&mut watch.state, watched());
    model
        .pending
        .push_back(pending(TerminalKeyKind::Typed("a".to_owned()), at_ms));
}

// "Colours" (`docs/design/readers-tell-core-asks-build3.md`): a full read
// takes the screen's colours within the newest key's window, given to the
// rules or still held.

#[test]
fn a_pending_keys_window_takes_the_colours_to_its_end() {
    let mut watch = Watch::new();
    let _ = watch.write("$ ", 100, false);
    hold_key(&mut watch, 1000);
    assert!(watch.model().takes_colours(1300));
    assert!(!watch.model().takes_colours(1301));
    // Given to the rules by a read in its window that took no colours:
    // they are owed, so the next read takes them whenever it is sent.
    let _ = watch.write("a", 1010, false);
    assert!(watch.model().pending.is_empty(), "the key was given");
    assert!(watch.model().colours_owed());
    assert!(watch.model().takes_colours(1301));
    // A read ending past the window owes none.
    let _ = watch.write("b", 1400, false);
    assert!(!watch.model().colours_owed());
    assert!(!watch.model().takes_colours(1400));
}

#[test]
fn the_focus_leaving_drops_the_keys_not_yet_given() {
    let mut watch = Watch::new();
    let _ = watch.write("$ ", 100, false);
    hold_key(&mut watch, 1000);
    super::model_or_new(&mut watch.state, watched()).blurred();
    assert!(watch.model().pending.is_empty());
    assert!(!watch.model().takes_colours(1100));
}
