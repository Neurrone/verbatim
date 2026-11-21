//! The batch shapes the request list's conversions send, built against
//! mockapp's elements for `verbatim_uia_rops::batch`'s two executors, and a
//! description of each step's outcome that two runs can be compared by.
//! Each test binary that includes it uses a part of it, as with `mod.rs`.
#![allow(dead_code)]

use std::fmt::Write as _;

use verbatim_model::batch::{Check, Condition, Place, PointOf, Relation, View};
use verbatim_uia::text::{Endpoint, TextPatternExt, TextRangeExt};
use verbatim_uia::{Cached, Fill, Node, Shape, Uia};
use verbatim_uia_rops::batch::{
    Answer, Descend, Ends, Found, Fused, Input, Judged, Object, Point, Prior, Ran, UnitsRead,
    WaitCheck,
};
use verbatim_uia_rops::{Attributes, Movement, Position, TextFrom, TextTarget};
use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, IUIAutomationTextPattern, IUIAutomationTextRange, TextUnit,
    TextUnit_Character, TextUnit_Line, TextUnit_Word,
};

/// The most UTF-16 code units of a unit's text the outpost reads.
pub const MAX_TEXT: i32 = 1025;

/// An element with text, and its text pattern.
pub struct Text {
    pub element: IUIAutomationElement,
    pub pattern: IUIAutomationTextPattern,
}

impl Text {
    pub fn of(element: IUIAutomationElement) -> Self {
        let pattern = verbatim_uia::text::text_pattern(&element).expect("a text pattern");
        Self { element, pattern }
    }

    pub fn target(&self) -> TextTarget<'_> {
        TextTarget {
            element: &self.element,
            pattern: &self.pattern,
        }
    }

    /// Where `range`'s ends are, in UTF-16 code units from the text's
    /// start, read by calls.
    pub fn offsets(&self, range: &IUIAutomationTextRange) -> (usize, usize) {
        let at = |end| {
            let before = self.pattern.document_range().expect("the document");
            before
                .move_endpoint_to(Endpoint::End, range, end)
                .expect("a range");
            before.text(-1).expect("its text").len()
        };
        (at(Endpoint::Start), at(Endpoint::End))
    }
}

/// A node's fill with every detail but a grid's counts.
pub fn node_fill(uia: &Uia) -> Fill<Node> {
    Fill::new(
        &uia.request::<Node>(verbatim_uia::NodeGates {
            counts: None,
            ..Node::all_on()
        })
        .expect("a cache request"),
    )
}

/// A step at `place`, with no condition.
pub fn step(place: u8, input: Input<'_>) -> Fused<'_> {
    Fused {
        place: Place(place),
        when: None,
        input,
        calls_only: false,
    }
}

/// A step at `place` run when step `on` found something.
pub fn when_found(place: u8, on: u8, input: Input<'_>) -> Fused<'_> {
    Fused {
        when: Some(Condition {
            on: Place(on),
            check: Check::Found,
            negated: false,
        }),
        ..step(place, input)
    }
}

/// A unit read of `unit`, `count` of them, after `movement`.
pub fn units(movement: Option<Movement>, unit: TextUnit, count: u32) -> UnitsRead {
    UnitsRead {
        movement,
        unit,
        count,
        max_text: MAX_TEXT,
        max_total: 32 * 1024,
        culture: true,
        formats: Attributes::NONE,
        learning: Attributes::NONE,
    }
}

/// A text read at `at`.
pub fn read_text<'a>(text: &'a Text, at: Point<'a>, read: UnitsRead) -> Input<'a> {
    Input::ReadText {
        text: text.target(),
        at,
        read,
    }
}

/// The point `which` of step `place`.
pub fn of(place: u8, which: PointOf) -> Point<'static> {
    Point::Of(Place(place), which)
}

/// A caret key's starting read: the caret, its line, and `unit` at it, or
/// one character back expanded to `unit` (Backspace), and for Delete the
/// word at the caret too.
pub fn starting_read(text: &Text, unit: TextUnit, back: bool, delete: bool) -> Vec<Fused<'_>> {
    let movement = back.then_some(Movement::By(TextUnit_Character, -1));
    let mut steps = vec![
        step(
            0,
            Input::Caret {
                text: text.target(),
            },
        ),
        step(
            1,
            read_text(text, of(0, PointOf::Caret), units(None, TextUnit_Line, 1)),
        ),
        step(
            2,
            read_text(text, of(0, PointOf::Caret), units(movement, unit, 1)),
        ),
    ];
    if delete {
        steps.push(step(
            3,
            read_text(text, of(0, PointOf::Caret), units(None, TextUnit_Word, 1)),
        ));
    }
    steps
}

/// A caret tell: the caret, its line with every attribute's formatting,
/// and the selection compared with `baseline`.
pub fn caret_tell<'a>(text: &'a Text, baseline: Position<'a>) -> Vec<Fused<'a>> {
    let mut line = units(None, TextUnit_Line, 1);
    line.formats = Attributes::ALL;
    line.learning = Attributes::ALL;
    vec![
        step(
            0,
            Input::Caret {
                text: text.target(),
            },
        ),
        step(1, read_text(text, of(0, PointOf::Caret), line)),
        step(
            2,
            Input::CompareSelection {
                text: text.target(),
                before: Prior::Baseline(baseline),
                after: Place(0),
                max_text: MAX_TEXT,
            },
        ),
    ]
}

/// A caret wait's poll: the check of the caret against the starting
/// caret `start`, nothing selected there, with the word when `word` and at
/// the bound when `last`; then the caret and its line with every
/// attribute's formatting, each gated on the check.
pub fn wait_poll<'a>(
    text: &'a Text,
    start: Position<'a>,
    (word, last): (bool, bool),
) -> Vec<Fused<'a>> {
    let mut line = units(None, TextUnit_Line, 1);
    line.formats = Attributes::ALL;
    line.learning = Attributes::ALL;
    vec![
        step(
            0,
            Input::WaitCheck(WaitCheck {
                text: text.target(),
                start,
                start_selection: (start, start),
                word,
                last,
                max_text: MAX_TEXT,
            }),
        ),
        when_found(
            1,
            0,
            Input::Caret {
                text: text.target(),
            },
        ),
        when_found(2, 0, read_text(text, of(1, PointOf::Caret), line)),
    ]
}

/// Review's previous word: the word at `at`, the word before it, and the
/// line it lands on.
pub fn previous_word<'a>(text: &'a Text, at: Point<'a>) -> Vec<Fused<'a>> {
    vec![
        step(0, read_text(text, at, units(None, TextUnit_Word, 1))),
        step(
            1,
            read_text(
                text,
                of(0, PointOf::ChunkStart),
                units(Some(Movement::By(TextUnit_Word, -1)), TextUnit_Word, 1),
            ),
        ),
        step(
            2,
            read_text(
                text,
                of(1, PointOf::ChunkStart),
                units(None, TextUnit_Line, 1),
            ),
        ),
    ]
}

/// Review's next word: the word after `at`, the text between, and the line
/// it lands on.
pub fn next_word<'a>(text: &'a Text, at: Point<'a>) -> Vec<Fused<'a>> {
    vec![
        step(
            0,
            read_text(
                text,
                at,
                units(Some(Movement::By(TextUnit_Word, 1)), TextUnit_Word, 1),
            ),
        ),
        step(
            1,
            Input::ReadRange {
                text: text.target(),
                ends: Ends {
                    start: at,
                    end: of(0, PointOf::ChunkStart),
                    through: None,
                },
                max_text: MAX_TEXT,
            },
        ),
        step(
            2,
            read_text(
                text,
                of(0, PointOf::ChunkStart),
                units(None, TextUnit_Line, 1),
            ),
        ),
    ]
}

/// Review select: the selection before, the select, the selection after,
/// and the two compared.
pub fn review_select<'a>(text: &'a Text, ends: Ends<'a>) -> Vec<Fused<'a>> {
    vec![
        step(
            0,
            Input::Caret {
                text: text.target(),
            },
        ),
        step(
            1,
            Input::Select {
                text: text.target(),
                ends,
            },
        ),
        step(
            2,
            Input::Caret {
                text: text.target(),
            },
        ),
        step(
            3,
            Input::CompareSelection {
                text: text.target(),
                before: Prior::SelectionOf(Place(0)),
                after: Place(2),
                max_text: MAX_TEXT,
            },
        ),
    ]
}

/// Say-all's caret move to `at`, and the caret read after it.
pub fn say_all_move<'a>(text: &'a Text, at: Point<'a>) -> Vec<Fused<'a>> {
    vec![
        step(
            0,
            Input::Select {
                text: text.target(),
                ends: Ends {
                    start: at,
                    end: at,
                    through: None,
                },
            },
        ),
        step(
            1,
            Input::Caret {
                text: text.target(),
            },
        ),
    ]
}

/// A walk down the tree from `subject`.
pub fn descendants<'a>(
    subject: Object<'a>,
    fill: &'a Fill<Node>,
    judge: &'a dyn Fn(&Cached<Node>) -> Judged,
    (view, prunes, include_subject, limit): (View, bool, bool, u32),
) -> Input<'a> {
    Input::Descendants {
        subject,
        walk: Descend {
            depth: 64,
            view,
            include_subject,
            prunes,
            limit,
            nodes: 4_096,
            fill,
            judge,
        },
    }
}

/// The controlled selection: what `focus` controls, and its subtree
/// searched for the element whose runtime id is in `judge`.
pub fn controlled_selection<'a>(
    focus: &'a IUIAutomationElement,
    fill: &'a Fill<Node>,
    judge: &'a dyn Fn(&Cached<Node>) -> Judged,
) -> Vec<Fused<'a>> {
    vec![
        step(
            0,
            Input::Related {
                subject: Object::Held {
                    element: focus,
                    window: 0,
                },
                relation: Relation::ControllerFor,
                fill,
            },
        ),
        when_found(
            1,
            0,
            descendants(
                Object::Result(Place(0)),
                fill,
                judge,
                (View::Raw, false, true, 1),
            ),
        ),
    ]
}

/// A node as its parts.
fn node(node: &Cached<Node>) -> String {
    format!("{:?}", node.read().parts(node.runtime_id()))
}

/// Each step's outcome described, every range by where its ends are in
/// `text`.
pub fn describe(text: Option<&Text>, answer: &Answer) -> String {
    let mut out = String::new();
    if answer.read_classically {
        out.push_str("read classically\n");
    }
    let range = |range: &IUIAutomationTextRange| {
        text.map_or_else(
            || "a range".to_owned(),
            |text| format!("{:?}", text.offsets(range)),
        )
    };
    for ran in &answer.steps {
        let line = match ran {
            Ran::Done(Found::Read(read)) => format!("read {}", node(read)),
            Ran::Done(Found::Related(related)) => format!(
                "related {:?} window {:?} table {:?}",
                related.nodes.iter().map(node).collect::<Vec<_>>(),
                related.window,
                related.table
            ),
            Ran::Done(Found::Ancestors(ancestors)) => format!(
                "ancestors {:?} {:?} {:?} {:?}",
                ancestors.chain, ancestors.crossed, ancestors.walk, ancestors.parent_automation_id
            ),
            Ran::Done(Found::Descendants(walk)) => format!(
                "descendants {:?} complete {}",
                walk.nodes
                    .iter()
                    .map(|d| (node(&d.node), d.parent, d.root))
                    .collect::<Vec<_>>(),
                walk.complete
            ),
            Ran::Done(Found::Caret(caret)) => format!(
                "caret {} {:?} selection {:?}",
                range(&caret.caret),
                caret.edges,
                caret.selection.as_ref().map(|(selection, start, end)| (
                    range(selection),
                    start,
                    end
                ))
            ),
            Ran::Done(Found::ReadText(read)) => {
                let mut line = format!("units moved {} ended {}", read.moved, read.ended);
                for unit in &read.units {
                    let _ = write!(
                        line,
                        " [{} {:?} at {} {:?}]",
                        range(&unit.range),
                        String::from_utf16_lossy(&unit.text),
                        unit.offset,
                        unit.language
                    );
                }
                let _ = write!(
                    line,
                    " runs {:?} unsupported {:?}",
                    read.runs, read.unsupported
                );
                line
            }
            Ran::Done(Found::ReadRange(text)) => {
                format!("range {:?}", String::from_utf16_lossy(text))
            }
            Ran::Done(Found::CompareSelection(compared)) => format!(
                "compared {:?} {:?} {:?}",
                compared
                    .changes
                    .iter()
                    .map(|change| (change.selected, String::from_utf16_lossy(&change.text)))
                    .collect::<Vec<_>>(),
                compared
                    .selection_text
                    .as_deref()
                    .map(String::from_utf16_lossy),
                compared
                    .previous_text
                    .as_deref()
                    .map(String::from_utf16_lossy)
            ),
            Ran::Done(Found::Select) => "selected".to_owned(),
            Ran::Done(Found::Locate(at)) => format!("located {at:?}"),
            other => format!("{other:?}"),
        };
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// The point some characters after a held position, as Core names a point
/// inside a chunk it was sent.
pub fn after<'a>(
    range: &'a IUIAutomationTextRange,
    prefix: &'a [u16],
    counts: &'a [i32],
) -> Point<'a> {
    Point::From(TextFrom::After {
        from: Position {
            range,
            endpoint: Endpoint::Start,
            collapsed: true,
        },
        prefix,
        counts,
    })
}
