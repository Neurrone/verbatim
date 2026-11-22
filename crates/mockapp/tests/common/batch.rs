//! The batch shapes the request list's conversions send, as the model's
//! steps, with what they name that the outpost would hold ([`Shape`], a
//! `Resolve` of mockapp's elements), for `verbatim_uia_rops::batch`'s two
//! executors, and a description of each step's outcome that two runs can
//! be compared by. Each test binary that includes it uses a part of it, as
//! with `mod.rs`.
#![allow(dead_code)]

use std::fmt::Write as _;

use verbatim_model::batch::{
    Before, Check, Condition, Op, Place, Point, PointOf, Relation, Step, StepRefusal, Subject,
    Through, View, Walk,
};
use verbatim_model::matcher::Matcher;
use verbatim_model::{
    NodeId, TextAnchor, TextDetails, TextMovement, TextPoint, TextPosition, TextUnit,
};
use verbatim_uia::text::{Endpoint, TextPatternExt, TextRangeExt};
use verbatim_uia::{Cached, Fill, Node, Shape as _, Uia};
use verbatim_uia_rops::batch::{
    Answer, Element, Found, HeldAt, Judged, Ran, Segment, UnitsRead, Unrun, WaitCheck,
};
use verbatim_uia_rops::{Attributes, FoundPoint, Movement, Position, TextTarget};
use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, IUIAutomationTextPattern, IUIAutomationTextRange,
};

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

/// A node's fill among many, as the outpost's walks read: every detail
/// but a grid's counts, its place and its cell. For a window's title bar
/// and its buttons UI Automation asks the application's UI thread and the
/// window manager for the place, which waits while the window manager is
/// behind, as it is with many test desktops made at once.
pub fn many_node_fill(uia: &Uia) -> Fill<Node> {
    Fill::new(
        &uia.request::<Node>(verbatim_uia::NodeGates {
            counts: None,
            place: None,
            cell: None,
            ..Node::all_on()
        })
        .expect("a cache request"),
    )
}

/// The node every text step names.
pub const TEXT: NodeId = NodeId::new(1);

/// A point a test names: one the program finds, a position it holds, or
/// a point of an earlier step's result.
#[derive(Clone)]
pub enum At {
    /// The caret, an end of the selection or of the text.
    Named(TextPoint),
    /// A position held, or some text after it.
    Held(HeldAt),
    /// A point of an earlier step's result.
    Of(u8, PointOf),
}

/// The position at `range`'s `endpoint`, `collapsed` when the range is.
pub fn held(range: &IUIAutomationTextRange, endpoint: Endpoint, collapsed: bool) -> At {
    At::Held(HeldAt {
        at: FoundPoint {
            range: range.clone(),
            endpoint,
            collapsed,
        },
        prefix: Vec::new(),
        counts: Vec::new(),
    })
}

/// The point some characters after a held position, as Core names a point
/// inside a chunk it was sent.
pub fn after(range: &IUIAutomationTextRange, prefix: &[u16], counts: &[i32]) -> At {
    At::Held(HeldAt {
        at: FoundPoint {
            range: range.clone(),
            endpoint: Endpoint::Start,
            collapsed: true,
        },
        prefix: prefix.to_vec(),
        counts: counts.to_vec(),
    })
}

/// The point `which` of step `place`.
pub fn of(place: u8, which: PointOf) -> At {
    At::Of(place, which)
}

/// A point the program or the calls find themselves.
pub fn named(point: TextPoint) -> At {
    At::Named(point)
}

/// A walk's judgement of each node it reads, standing for its matchers.
pub type Judge<'a> = &'a dyn Fn(&Cached<Node>) -> Judged;

/// A batch's steps and what they name that the outpost would hold: the
/// text, the elements by node, the fills by step, a walk's matchers, the
/// positions by anchor, a poll's starting point, and how a text read reads
/// beyond its unit: whether with each unit's language, and the most it
/// reads ahead (the outpost's, by default); and whether each object step
/// runs by calls alone ([`Element::calls_only`]).
pub struct Shape<'a> {
    pub steps: Vec<Step>,
    pub text: Option<&'a Text>,
    pub elements: Vec<(NodeId, &'a IUIAutomationElement, isize)>,
    pub fills: Vec<&'a Fill<Node>>,
    pub judge: Option<Judge<'a>>,
    pub points: Vec<HeldAt>,
    pub wait: Option<(FoundPoint, bool, bool)>,
    pub culture: bool,
    pub max_total: u32,
    pub calls_only: bool,
    /// The attributes a read with formatting reads and learns, every one
    /// unless set.
    pub formats: Option<(Attributes, Attributes)>,
}

impl<'a> Shape<'a> {
    /// A shape of no steps on `text`.
    pub fn of(text: Option<&'a Text>) -> Self {
        Self {
            steps: Vec::new(),
            text,
            elements: Vec::new(),
            fills: Vec::new(),
            judge: None,
            points: Vec::new(),
            wait: None,
            culture: true,
            formats: None,
            max_total: 32 * 1024,
            calls_only: false,
        }
    }

    /// `at` as a model's point: a held position under a new anchor.
    pub fn point(&mut self, at: &At) -> Point {
        match at {
            At::Named(point) => Point::At(*point),
            At::Of(place, which) => Point::Of(Place(*place), *which),
            At::Held(held) => {
                self.points.push(held.clone());
                let anchor = TextAnchor(self.points.len() as u64 - 1);
                Point::At(TextPoint::At(TextPosition::at(anchor)))
            }
        }
    }

    /// The anchor a held baseline is under.
    pub fn baseline(&mut self, at: &At) -> Before {
        match self.point(at) {
            Point::At(TextPoint::At(position)) => Before::Baseline(position.anchor),
            _ => panic!("a baseline is a held position"),
        }
    }

    /// Adds `op` with no condition.
    pub fn step(&mut self, op: Op) -> &mut Self {
        self.steps.push(Step { op, when: None });
        self
    }

    /// Adds `op`, run when step `on` found something.
    pub fn when_found(&mut self, on: u8, op: Op) -> &mut Self {
        self.steps.push(Step {
            op,
            when: Some(Condition {
                on: Place(on),
                check: Check::Found,
                negated: false,
            }),
        });
        self
    }

    /// `element` as an object subject, with no window of its own.
    pub fn object(&mut self, element: &'a IUIAutomationElement) -> Subject {
        self.object_in(element, 0)
    }

    /// `element`, whose own native window is `window`, as an object
    /// subject.
    pub fn object_in(&mut self, element: &'a IUIAutomationElement, window: isize) -> Subject {
        let node = NodeId::new(100 + self.elements.len() as u64);
        self.elements.push((node, element, window));
        Subject::Node(node)
    }

    /// The segment of every step, from the batch's start.
    pub fn segment<'s>(&'s self, uia: &'s Uia) -> Segment<'s> {
        Segment {
            uia,
            steps: &self.steps,
            first: 0,
            earlier: &[],
            resolver: self,
            remote: true,
            window: None,
        }
    }
}

/// Named in full, not imported, so that `Shape`'s own `point`, `baseline` and
/// `object` are the ones its users call.
impl verbatim_uia_rops::batch::Resolve for Shape<'_> {
    fn object(&self, subject: &Subject) -> Result<Element<'_>, Ran> {
        let found = self
            .elements
            .iter()
            .find(|(node, ..)| Subject::Node(*node) == *subject)
            .expect("a test names each object it holds");
        Ok(Element {
            element: found.1,
            window: found.2,
            calls_only: self.calls_only,
        })
    }

    fn fill(&self, place: Place) -> Option<&Fill<Node>> {
        self.fills.get(place.index()).copied()
    }

    fn judge(&self, _place: Place, node: &Cached<Node>) -> Judged {
        (self.judge.expect("a walk's matchers"))(node)
    }

    fn text(&self, subject: &Subject) -> Result<TextTarget<'_>, Ran> {
        assert_eq!(
            *subject,
            Subject::Node(TEXT),
            "a test's text steps name its text"
        );
        Ok(self.text.expect("a text").target())
    }

    fn point(&self, point: &Point) -> Result<HeldAt, Ran> {
        let Point::At(TextPoint::At(position)) = point else {
            panic!("a test resolves only the positions it holds")
        };
        Ok(self.points[usize::try_from(position.anchor.0).expect("an anchor")].clone())
    }

    fn baseline(&self, before: &Before) -> Result<FoundPoint, Ran> {
        let Before::Baseline(anchor) = before else {
            panic!("a test's baselines are held")
        };
        Ok(self.points[usize::try_from(anchor.0).expect("an anchor")]
            .at
            .clone())
    }

    fn units(
        &self,
        (movement, unit, count): (Option<TextMovement>, TextUnit, u8),
        details: TextDetails,
    ) -> Result<UnitsRead, Ran> {
        let provider = |unit| {
            verbatim_uia::text::uia_text_unit(unit)
                .ok_or(Ran::Unrun(Unrun::Refused(StepRefusal::NoSuchUnit)))
        };
        let movement = match movement {
            None => None,
            Some(TextMovement {
                unit: TextUnit::Document,
                count,
            }) => Some(Movement::Document(count)),
            Some(TextMovement { unit, count }) => Some(Movement::By(provider(unit)?, count)),
        };
        let (formats, learning) = if details == TextDetails::NONE {
            (Attributes::NONE, Attributes::NONE)
        } else {
            self.formats.unwrap_or((Attributes::ALL, Attributes::ALL))
        };
        Ok(UnitsRead {
            movement,
            unit: provider(unit)?,
            count: u32::from(count),
            max_total: self.max_total,
            culture: self.culture,
            formats,
            learning,
        })
    }

    fn wait_check(&self) -> Result<WaitCheck<'_>, Ran> {
        let (start, word, last) = self.wait.as_ref().expect("a poll's starting point");
        let start = Position {
            range: &start.range,
            endpoint: start.endpoint,
            collapsed: start.collapsed,
        };
        Ok(WaitCheck {
            text: self.text.expect("a text").target(),
            start,
            start_selection: (start, start),
            word: *word,
            last: *last,
        })
    }
}

/// The caret and the selection of the text.
pub fn caret() -> Op {
    Op::Caret {
        subject: Subject::Node(TEXT),
        adopt: None,
    }
}

/// `count` units of `unit` read from `at` after `movement`, with every
/// attribute's formatting when `formats`.
pub fn read_text(
    at: Point,
    (movement, unit, count): (Option<TextMovement>, TextUnit, u8),
    formats: bool,
) -> Op {
    Op::ReadText {
        subject: Subject::Node(TEXT),
        at,
        movement,
        unit,
        count,
        details: if formats {
            TextDetails::ALL
        } else {
            TextDetails::NONE
        },
    }
}

/// The text between `start` and `end`, through the characters at
/// `through`'s points when given.
pub fn read_range(start: Point, end: Point, through: Option<(Point, Point)>) -> Op {
    Op::ReadRange {
        subject: Subject::Node(TEXT),
        start,
        end,
        through: through.map(|(start, end)| Through { start, end }),
    }
}

/// Selects between `start` and `end`.
pub fn select(start: Point, end: Point, through: Option<(Point, Point)>) -> Op {
    Op::Select {
        subject: Subject::Node(TEXT),
        start,
        end,
        through: through.map(|(start, end)| Through { start, end }),
    }
}

/// The selection step `after` read compared with `before`.
pub fn compare(before: Before, after: u8) -> Op {
    Op::CompareSelection {
        subject: Subject::Node(TEXT),
        before,
        after: Place(after),
    }
}

/// Where `at` is.
pub fn locate(at: Point) -> Op {
    Op::Locate {
        subject: Subject::Node(TEXT),
        at,
    }
}

/// A move of `count` units of `unit`.
#[expect(
    clippy::unnecessary_wraps,
    reason = "a read's movement is optional, and this is one"
)]
pub fn by(unit: TextUnit, count: i32) -> Option<TextMovement> {
    Some(TextMovement { unit, count })
}

/// One read of `at`: `count` units of `unit` after `movement`.
pub fn one_read<'a>(
    text: &'a Text,
    at: &At,
    read: (Option<TextMovement>, TextUnit, u8),
) -> Shape<'a> {
    let mut shape = Shape::of(Some(text));
    let at = shape.point(at);
    shape.step(read_text(at, read, false));
    shape
}

/// A caret key's starting read: the caret, its line, and `unit` at it, or
/// one character back expanded to `unit` (Backspace), and for Delete the
/// word at the caret too.
pub fn starting_read(text: &Text, unit: TextUnit, back: bool, delete: bool) -> Shape<'_> {
    let movement = if back {
        by(TextUnit::Character, -1)
    } else {
        None
    };
    let at = Point::Of(Place(0), PointOf::Caret);
    let mut shape = Shape::of(Some(text));
    shape
        .step(caret())
        .step(read_text(at, (None, TextUnit::Line, 1), false))
        .step(read_text(at, (movement, unit, 1), false));
    if delete {
        shape.step(read_text(at, (None, TextUnit::Word, 1), false));
    }
    shape
}

/// A caret tell: the caret, its line with every attribute's formatting,
/// and the selection compared with `baseline`.
pub fn caret_tell<'a>(text: &'a Text, baseline: &At) -> Shape<'a> {
    let mut shape = Shape::of(Some(text));
    let before = shape.baseline(baseline);
    shape
        .step(caret())
        .step(read_text(
            Point::Of(Place(0), PointOf::Caret),
            (None, TextUnit::Line, 1),
            true,
        ))
        .step(compare(before, 0));
    shape
}

/// A caret wait's poll: the check of the caret against the starting
/// caret `start`, nothing selected there, with the word when `word` and at
/// the bound when `last`; then the caret and its line with every
/// attribute's formatting, each gated on the check.
pub fn wait_poll(text: &Text, start: FoundPoint, (word, last): (bool, bool)) -> Shape<'_> {
    let mut shape = Shape::of(Some(text));
    shape.wait = Some((start, word, last));
    let wait = verbatim_model::batch::CaretWait {
        from: TextAnchor(0),
        line: String::new(),
        offset: 0,
        word: None,
        effect: verbatim_model::batch::EffectTest {
            unit: false,
            line: false,
        },
        passed_at_us: 0,
        poll_us: verbatim_model::batch::MIN_POLL_US,
        bound_us: verbatim_model::batch::MAX_BOUND_US,
        events: false,
    };
    shape
        .step(Op::WaitForCaret {
            subject: Subject::Node(TEXT),
            wait: Box::new(wait),
        })
        .when_found(0, caret())
        .when_found(
            0,
            read_text(
                Point::Of(Place(1), PointOf::Caret),
                (None, TextUnit::Line, 1),
                true,
            ),
        );
    shape
}

/// A focus's caret read, as the outpost runs it: the caret, its line with
/// every attribute's formatting and no language, and the selection's text.
pub fn focus_caret(text: &Text) -> Shape<'_> {
    let mut shape = Shape::of(Some(text));
    shape.culture = false;
    let of = |which| Point::Of(Place(0), which);
    shape
        .step(caret())
        .step(read_text(
            of(PointOf::Caret),
            (None, TextUnit::Line, 1),
            true,
        ))
        .step(read_range(
            of(PointOf::SelectionStart),
            of(PointOf::SelectionEnd),
            None,
        ));
    shape
}

/// Review's previous word: the word at `at`, the word before it, and the
/// line it lands on.
pub fn previous_word<'a>(text: &'a Text, at: &At) -> Shape<'a> {
    let mut shape = Shape::of(Some(text));
    let at = shape.point(at);
    shape
        .step(read_text(at, (None, TextUnit::Word, 1), false))
        .step(read_text(
            Point::Of(Place(0), PointOf::ChunkStart),
            (by(TextUnit::Word, -1), TextUnit::Word, 1),
            false,
        ))
        .step(read_text(
            Point::Of(Place(1), PointOf::ChunkStart),
            (None, TextUnit::Line, 1),
            false,
        ));
    shape
}

/// Review's next word: the word after `at`, the text between, and the line
/// it lands on.
pub fn next_word<'a>(text: &'a Text, at: &At) -> Shape<'a> {
    let mut shape = Shape::of(Some(text));
    let at = shape.point(at);
    let landed = Point::Of(Place(0), PointOf::ChunkStart);
    shape
        .step(read_text(
            at,
            (by(TextUnit::Word, 1), TextUnit::Word, 1),
            false,
        ))
        .step(read_range(at, landed, None))
        .step(read_text(landed, (None, TextUnit::Line, 1), false));
    shape
}

/// Review select between `start` and `end`, through the characters at
/// `through`'s points when given: the selection before, the select, the
/// selection after, and the two compared.
pub fn review_select<'a>(
    text: &'a Text,
    (start, end): (&At, &At),
    through: Option<(&At, &At)>,
) -> Shape<'a> {
    let mut shape = Shape::of(Some(text));
    let start = shape.point(start);
    let end = shape.point(end);
    let through = through.map(|(start, end)| (shape.point(start), shape.point(end)));
    shape
        .step(caret())
        .step(select(start, end, through))
        .step(caret())
        .step(compare(Before::SelectionOf(Place(0)), 2));
    shape
}

/// Say-all's caret move to `at`, and the caret read after it.
pub fn say_all_move<'a>(text: &'a Text, at: &At) -> Shape<'a> {
    let mut shape = Shape::of(Some(text));
    let at = shape.point(at);
    shape.step(select(at, at, None)).step(caret());
    shape
}

/// A walk down the tree from `subject`: `prunes` gives it a pruning
/// matcher, which the shape's judgement stands for.
pub fn descendants(
    subject: Subject,
    (view, prunes, include_subject, limit): (View, bool, bool, u32),
) -> Op {
    Op::Descendants {
        subject,
        walk: Box::new(Walk {
            depth: 64,
            view,
            matcher: None,
            prune: prunes.then(|| Matcher::Any(Vec::new())),
            include_subject,
            limit,
            nodes: 4_096,
            set: Vec::new(),
        }),
    }
}

/// A walk down the tree from `root`, read with `fill`, judged by `judge`.
pub fn walk<'a>(
    root: &'a IUIAutomationElement,
    fill: &'a Fill<Node>,
    judge: &'a dyn Fn(&Cached<Node>) -> Judged,
    how: (View, bool, bool, u32),
) -> Shape<'a> {
    let mut shape = Shape::of(None);
    let subject = shape.object(root);
    shape.fills = vec![fill];
    shape.judge = Some(judge);
    shape.step(descendants(subject, how));
    shape
}

/// One step `relation` from `from`, each node reached read with `fill`.
pub fn related<'a>(
    from: &'a IUIAutomationElement,
    relation: Relation,
    fill: &'a Fill<Node>,
) -> Shape<'a> {
    let mut shape = Shape::of(None);
    let subject = shape.object(from);
    shape.fills = vec![fill];
    shape.step(Op::Related {
        subject,
        relation,
        set: Vec::new(),
    });
    shape
}

/// The controlled selection: what `focus` controls, read with `fill`, and
/// its subtree, read with `many`, searched for the element whose runtime
/// id is in `judge`.
pub fn controlled_selection<'a>(
    focus: &'a IUIAutomationElement,
    (fill, many): (&'a Fill<Node>, &'a Fill<Node>),
    judge: &'a dyn Fn(&Cached<Node>) -> Judged,
) -> Shape<'a> {
    let mut shape = related(focus, Relation::ControllerFor, fill);
    shape.fills.push(many);
    shape.judge = Some(judge);
    shape.when_found(
        0,
        descendants(Subject::Result(Place(0)), (View::Raw, false, true, 1)),
    );
    shape
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
            Ran::Done(Found::Related(related)) => format!(
                "related {:?} window {:?} table {:?}",
                related.nodes.iter().map(node).collect::<Vec<_>>(),
                related.window,
                related.table
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
