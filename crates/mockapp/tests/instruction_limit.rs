//! UIA's limit on the instructions one remote operation may execute, and how
//! many each of Verbatim's programs executes, against mockapp's real,
//! out-of-process provider (`books/dev/src/reference/performance.md`, "The instruction
//! limit").
//!
//! The limit is not published (NVDA's local emulator of remote operations
//! assumes 10,000), so it is found by running a loop of growing length until
//! UIA stops it, and pinned with the status it gives. Each program's count
//! is taken by running it as Verbatim runs it with `verbatim_uia_rops`'s
//! counting on, which runs the program's counting form, at its worst case
//! against mockapp and at a typical size, and pinned exactly; every worst
//! case but the caret read's must stay under half the limit, the margin the
//! design asks for, and the caret read's is pinned where it comes closer.

#[path = "common/batch.rs"]
mod batch;
mod common;
#[path = "common/harness.rs"]
mod harness;

use verbatim_uia::text::{Endpoint, TextPatternExt};
use verbatim_uia::{
    Ancestor, Cached, DialogText, Fill, FocusInput, Live, Node, NodeGates, Shape, Uia,
};
use verbatim_uia_rops::batch::{Answer, Found, Ran, run_batch};
use verbatim_uia_rops::{
    Attributes, Builder, Comparison, Error, FocusAncestry, FocusQuery, PathQuery, StatesQuery,
    Status, TextAttribute, counting, element_by_path_remote, focus_ancestry_remote,
    focus_states_remote,
};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, IUIAutomationTextRange, TreeScope_Descendants, UIA_NamePropertyId,
};
use windows::core::BSTR;

/// The most instructions UIA lets one remote operation execute, as
/// [`the_instruction_limit_is_measured`] finds it.
const LIMIT: u32 = 10_000;

/// A running mockapp and a client reading it.
struct Fixture {
    app: common::MockApp,
    uia: Uia,
    root: IUIAutomationElement,
    hwnd: HWND,
}

impl Fixture {
    fn start(fixture: &str, prefix: &str) -> Self {
        let title = common::unique_title(prefix);
        let app = common::spawn(fixture, "uia", &title);
        let hwnd = common::find_window(&title);
        let uia = Uia::new().expect("a UIA client");
        let root = uia
            .window_live(hwnd.0 as isize)
            .expect("mockapp's root element")
            .element()
            .clone();
        Self {
            app,
            uia,
            root,
            hwnd,
        }
    }

    /// The element named `name`, for a program to import or a call into its
    /// application: nothing of its cache is read.
    fn find(&self, name: &str) -> IUIAutomationElement {
        self.find_node(name).live().element().clone()
    }

    /// The element named `name`, read as what the focus query reads of it.
    fn find_input(&self, name: &str) -> Cached<FocusInput> {
        self.find_node(name).narrow()
    }

    /// The element named `name`, filled as a node.
    fn find_node(&self, name: &str) -> Cached<Node> {
        let request = self
            .uia
            .request::<Node>(Node::all_on())
            .expect("a cache request");
        let value = VARIANT::from(BSTR::from(name));
        let condition = self
            .uia
            .property_condition(UIA_NamePropertyId, &value)
            .expect("a condition");
        Live::new(self.root.clone())
            .find_first(TreeScope_Descendants, &condition, &request)
            .unwrap_or_else(|error| panic!("no element named {name:?}: {error}"))
            .unwrap_or_else(|| panic!("no element named {name:?}"))
    }

    /// The fill of every element a focus query returns, with every detail.
    fn ancestors(&self) -> Fill<Ancestor> {
        Fill::new(
            &self
                .uia
                .request::<Ancestor>(common::ancestor_gates())
                .expect("a cache request"),
        )
    }

    /// The fill of the focused element itself, read with its place.
    fn focus(&self) -> Fill<Ancestor> {
        Fill::new(
            &self
                .uia
                .request::<Ancestor>(verbatim_uia::AncestorGates {
                    node: NodeGates {
                        counts: None,
                        ..Node::all_on()
                    },
                    ..Ancestor::all_on()
                })
                .expect("a cache request"),
        )
    }

    /// The fill of a node, with every detail a remote read takes.
    fn nodes(&self) -> Fill<Node> {
        Fill::new(
            &self
                .uia
                .request::<Node>(NodeGates {
                    counts: None,
                    ..Node::all_on()
                })
                .expect("a cache request"),
        )
    }
}

/// `element`'s runtime id, which every element of mockapp's has.
fn runtime_id(element: &windows::Win32::UI::Accessibility::IUIAutomationElement) -> Vec<i32> {
    verbatim_uia::runtime_id(element).expect("a runtime id")
}

/// Runs `run` with counting on and returns how many instructions the one
/// program it ran executed.
fn counted(run: impl FnOnce()) -> u32 {
    counting::start();
    run();
    match counting::stop().as_slice() {
        [Some(count)] => *count,
        counts => panic!("one counted program, not {counts:?}"),
    }
}

/// Runs a loop of `passes` passes and `extra` further instructions in
/// mockapp's process: `Ok` with the instructions it executed, or the
/// status that stopped it.
fn run_loop(element: &IUIAutomationElement, passes: i32, extra: u32) -> Result<u32, Status> {
    let mut b = Builder::new();
    let _ = b.import_element(element);
    for _ in 0..extra {
        let _ = b.new_int(0);
    }
    let count = b.new_int(0);
    let passes_reg = b.int(passes);
    let one = b.int(1);
    b.while_(
        |b| b.compare(count, passes_reg, Comparison::LessThan),
        |b| b.add_assign(count, one),
    );
    // Two constants, the counter, the loop block, four instructions a pass
    // (the comparison, the fork, the add, the jump back), the last
    // comparison and fork, the loop block's end, and the halt.
    let executed = 4 * passes.cast_unsigned() + 8 + extra;
    match b.finish().execute() {
        Ok(_) => Ok(executed),
        Err(Error::Failed(failure)) => Err(failure.status),
        Err(error) => panic!("the loop did not run: {error}"),
    }
}

/// The limit: the loop lengthened until UIA stops it, found to the
/// instruction, and the status it stops it with. A loop's count is checked
/// against a counted run first, so the arithmetic the search relies on is
/// the platform's.
fn the_instruction_limit_is_measured() {
    common::init_com();
    let fixture = Fixture::start("counts.json", "mockapp-instruction-limit");
    let element = fixture.root.clone();
    assert_eq!(
        counted(|| {
            run_loop(&element, 10, 0).expect("a short loop runs");
        }),
        48
    );

    // The fewest passes that fail, by halving the range.
    let (mut low, mut high) = (0, 1 << 20);
    assert!(
        run_loop(&element, high, 0).is_err(),
        "a million passes fail"
    );
    while high - low > 1 {
        let middle = i32::midpoint(low, high);
        if run_loop(&element, middle, 0).is_ok() {
            low = middle;
        } else {
            high = middle;
        }
    }
    // Then to the instruction: up to three more after the longest loop
    // that runs.
    let mut limit = run_loop(&element, low, 0).expect("the longest loop that runs");
    let mut stopped = None;
    for extra in 1..=3 {
        match run_loop(&element, low, extra) {
            Ok(executed) => limit = executed,
            Err(status) => {
                stopped = Some(status);
                break;
            }
        }
    }
    assert_eq!((low, limit), (2498, LIMIT));
    assert_eq!(stopped, Some(Status::InstructionLimitExceeded));
    fixture.app.quit();
}

/// The focus ancestry at its worst in mockapp: a looping selector, a list
/// whose selected item is its value, in sixty nested groups (`deep.json`; a
/// fixture's nesting is bounded by its JSON parser's), every ancestor up to
/// the window walked under the depth limit the outpost uses (64), against
/// 64 known ancestors none of which it meets, with the list's selected item
/// read; the same walk stopped by a depth limit of 30 and walking on to the
/// window; and the list whose group is known, which stops the walk at once
/// but for the window, still sixty-one levels up (mockapp's elements below
/// its window have none of their own, where an application's controls
/// usually do). Navigation: a
/// step from the innermost item, whose window is sixty-two levels up, and
/// from the outermost group, right under it.
fn focus_and_navigation_execute_exactly() {
    common::init_com();
    let mut fixture = Fixture::start("deep.json", "mockapp-instructions-focus");
    // mockapp acknowledges the command once it has taken effect.
    fixture.app.send("focus fruits");
    let list = fixture.find_input("Fruits");
    let known: Vec<Vec<i32>> = (0..64).map(|index| vec![42, 7, index]).collect();
    let fill = fixture.ancestors();
    let focus = fixture.focus();
    let query = FocusQuery {
        element: &list,
        known: &known,
        depth_limit: 64,
        fill: &fill,
        element_fill: &focus,
        require_focus: true,
        refresh: false,
        nearest_window: None,
        before_call: None,
        value_available: None,
        is_cell: None,
        plain: None,
        progress: None,
    };
    let ancestry = |query: &FocusQuery<'_>| {
        let answer = focus_ancestry_remote(&fixture.uia, query).expect("the program runs");
        let FocusAncestry::Focused(ancestry) = answer else {
            panic!("not focused");
        };
        (ancestry.ancestors.len(), ancestry.depth_limited)
    };
    let worst = counted(|| assert_eq!(ancestry(&query), (61, false)));
    let limited = counted(|| {
        let limited = FocusQuery {
            depth_limit: 30,
            ..query
        };
        assert_eq!(ancestry(&limited), (30, true));
    });
    let group = fixture.find("Group 60");
    let near = vec![runtime_id(&group)];
    let typical = counted(|| {
        let typical = FocusQuery {
            known: &near,
            ..query
        };
        assert_eq!(ancestry(&typical), (1, false));
    });
    fixture.app.quit();

    assert_eq!(
        [worst, limited, typical],
        [3072, 2237, 1276],
        "focus ancestry worst, at depth limit 30, typical"
    );
    assert!(worst < LIMIT / 2);
}

/// The outpost's walk to its own copy of an element by its path (stage A
/// step 6): from mockapp's window down sixty nested groups and the list
/// to its second item, the deepest path in mockapp, each level's first
/// child the one wanted but for the item; one level down; and a path whose
/// first runtime id no child has, which ends the walk at once.
fn paths_execute_exactly() {
    common::init_com();
    let mut fixture = Fixture::start("deep.json", "mockapp-instructions-path");
    fixture.app.send("focus fruits");
    let list = fixture.find_input("Fruits");
    let second = fixture.find("Banana");

    let mut path: Vec<Vec<i32>> = (1..=60)
        .map(|group| runtime_id(&fixture.find(&format!("Group {group}"))))
        .collect();
    path.push(list.runtime_id());
    path.push(runtime_id(&second));
    let root = Live::new(fixture.root.clone());
    let nodes = fixture.nodes();
    let walk = |path: &[Vec<i32>]| {
        let query = PathQuery {
            root: &root,
            path,
            fill: &nodes,
            out_of_time: None,
        };
        element_by_path_remote(&fixture.uia, &query)
            .expect("the program runs")
            .map(|found| found.runtime_id())
    };
    let deepest = counted(|| assert_eq!(walk(&path).as_ref(), path.last()));
    let near = counted(|| assert_eq!(walk(&path[..1]).as_ref(), path.first()));
    let missing = counted(|| assert_eq!(walk(&[vec![42, -1]]), None));
    fixture.app.quit();

    assert_eq!(
        [deepest, near, missing],
        [1219, 233, 218],
        "a path down sixty-two levels, down one, and to nothing"
    );
    assert!(deepest < LIMIT / 2);
}

/// The focus's caret read, generated from the request list's `Caret`,
/// `ReadText` of the line and `ReadRange` of the selection when text is
/// selected: a line of 64
/// stretches, reached by walking one mixed stretch by words and a mixed
/// word by characters (`italic.json`), every attribute read and learned,
/// with ten characters selected; and, typically, a line of plain text
/// with the default theme's two attributes, the annotation types and the
/// link, whose support is known.
fn caret_reads_execute_exactly() {
    common::init_com();
    let mut fixture = Fixture::start("italic.json", "mockapp-instructions-caret");
    let text = batch::Text::of(fixture.find("Notes"));
    let run = |shape: &batch::Shape<'_>| {
        let mut answer = None;
        let count = counted(|| answer = Some(run_batch(&shape.segment(&fixture.uia))));
        (count, answer.expect("an answer"))
    };
    let stretches = |answer: &Answer| match answer.steps.get(1) {
        Some(Ran::Done(Found::ReadText(line))) => line.runs.len(),
        other => panic!("the line: {other:?}"),
    };
    common::apply(&mut fixture.app, fixture.hwnd, "caret doc 20 30");
    let (worst, answer) = run(&batch::focus_caret(&text));
    assert_eq!(stretches(&answer), 64);
    common::apply(&mut fixture.app, fixture.hwnd, "caret doc 0");
    let mut typical = batch::focus_caret(&text);
    typical.formats = Some((
        Attributes::of(&[TextAttribute::Annotations, TextAttribute::Link]),
        Attributes::NONE,
    ));
    let (typical, answer) = run(&typical);
    assert_eq!(stretches(&answer), 1);
    fixture.app.quit();
    // 4378 and 91 by the focus's hand-written caret program, which this
    // generated read replaced, with the same calls (2026-11-22): the
    // generated read compares each of the caret's and the selection's ends
    // with the text's, and reads the selection between two points only when
    // text is selected.
    assert_eq!(
        [worst, typical],
        [4454, 134],
        "caret read worst and typical"
    );
}

/// The focus's caret read where every format stretch is two characters
/// with one in italics (`mixed.json`), so each is walked by words and its
/// word by characters: lines of 16 and 32 characters, and their line
/// feeds, counted, which grow by the same amount for each stretch; and a
/// line of 80, cut at 64 stretches, about 8,000 by that growth, which the
/// counting form cannot run, since it executes twice as many: run as
/// Verbatim runs it, it stays under the limit.
fn densely_mixed_formatting_executes_exactly() {
    common::init_com();
    let mut fixture = Fixture::start("mixed.json", "mockapp-instructions-mixed");
    let text = batch::Text::of(fixture.find("Notes"));
    let shape = batch::focus_caret(&text);
    let stretches = |answer: &Answer| match answer.steps.get(1) {
        Some(Ran::Done(Found::ReadText(line))) => line.runs.len(),
        other => panic!("the line: {other:?}"),
    };
    common::apply(&mut fixture.app, fixture.hwnd, "caret doc 0");
    let sixteen = counted(|| assert_eq!(stretches(&run_batch(&shape.segment(&fixture.uia))), 17));
    common::apply(&mut fixture.app, fixture.hwnd, "caret doc 17");
    let thirty_two =
        counted(|| assert_eq!(stretches(&run_batch(&shape.segment(&fixture.uia))), 33));
    common::apply(&mut fixture.app, fixture.hwnd, "caret doc 50");
    let _ = verbatim_win32::calls::take();
    let eighty = stretches(&run_batch(&shape.segment(&fixture.uia)));
    let calls = verbatim_win32::calls::take();
    fixture.app.quit();
    // 2152 and 4144 by the focus's hand-written caret program, which this
    // generated read replaced (2026-11-22).
    assert_eq!([sixteen, thirty_two], [2198, 4190], "17 and 33 stretches");
    assert_eq!(eighty, 64, "80 characters, cut at 64 stretches");
    assert_eq!(calls.uia, 1, "the 80 characters' read, one program");
}

/// Say-all's batch of twenty lines a line on from a held position, its
/// lines in more than one language, so each line's is read; and the first
/// batch, from the caret, in one language: each one `ReadText` of twenty
/// lines, one generated program.
fn say_all_batches_execute_exactly() {
    use verbatim_model::{TextPoint, TextUnit};
    use verbatim_uia_rops::batch::run_batch;
    common::init_com();
    let mut fixture = Fixture::start("languages.json", "mockapp-instructions-say-all");
    let text = batch::Text::of(fixture.find("Notes"));
    let more = numbered(30);
    common::apply(
        &mut fixture.app,
        fixture.hwnd,
        &format!(r"set-text doc hello\nbonjour\nhallo\n{more}"),
    );
    common::apply(&mut fixture.app, fixture.hwnd, "caret doc 0");
    let document = text.pattern.document_range().expect("the document range");
    let held = batch::held(&document, Endpoint::Start, false);
    let lines = |at: &batch::At, movement| {
        let shape = batch::one_read(&text, at, (movement, TextUnit::Line, 20));
        let answer = run_batch(&shape.segment(&fixture.uia));
        match answer.steps.into_iter().next() {
            Some(verbatim_uia_rops::batch::Ran::Done(
                verbatim_uia_rops::batch::Found::ReadText(read),
            )) => read.units,
            other => panic!("twenty lines, not {other:?}"),
        }
    };
    let languages = counted(|| {
        let units = lines(&held, batch::by(TextUnit::Line, 1));
        assert_eq!(units.len(), 20);
        assert_eq!(units[1].language.as_deref(), Some("de-DE"));
    });
    common::apply(
        &mut fixture.app,
        fixture.hwnd,
        &format!("set-text doc {more}"),
    );
    let first = counted(|| {
        assert_eq!(lines(&batch::named(TextPoint::Caret), None).len(), 20);
    });
    fixture.app.quit();
    assert_eq!(
        [languages, first],
        [694, 720],
        "a later batch in three languages, a first batch in one"
    );
    assert!(languages < LIMIT / 2 && first < LIMIT / 2);
}

/// The review copy's first press: "alpha" selected back from the review
/// cursor, on its first "a", through the marker, on its last, every point
/// but the document's start found by the text before it as Verbatim finds
/// a held position's, from no selection: review select's batch, the caret,
/// the select, the caret again and the two compared, the reads before the
/// act and the act one program, the reads after it another.
fn range_selections_execute_exactly() {
    use verbatim_uia_rops::FoundPoint;
    use verbatim_uia_rops::batch::{Found, HeldAt, Ran, run_batch};
    common::init_com();
    let mut fixture = Fixture::start("text.json", "mockapp-instructions-range");
    let text = batch::Text::of(fixture.find("Notes"));
    common::apply(&mut fixture.app, fixture.hwnd, "caret doc 0");
    let document = text.pattern.document_range().expect("the document range");
    let start = FoundPoint {
        range: document.clone(),
        endpoint: Endpoint::Start,
        collapsed: false,
    };
    let texts: Vec<Vec<u16>> = ["alph", "alpha", "a"]
        .iter()
        .map(|text| text.encode_utf16().collect())
        .collect();
    let counts: Vec<[i32; 1]> = [4, 5, 1].map(|count| [count]).to_vec();
    let after = |index: usize| {
        batch::At::Held(HeldAt {
            at: start.clone(),
            prefix: texts[index].clone(),
            counts: counts[index].to_vec(),
        })
    };
    let end = batch::held(&document, Endpoint::Start, false);
    let shape = batch::review_select(&text, (&after(0), &end), Some((&after(1), &after(2))));
    let mut press = Vec::new();
    verbatim_uia_rops::counting::start();
    let answer = run_batch(&shape.segment(&fixture.uia));
    press.extend(
        verbatim_uia_rops::counting::stop()
            .into_iter()
            .map(|count| count.expect("a counted program")),
    );
    let Some(Ran::Done(Found::CompareSelection(compared))) = answer.steps.get(3) else {
        panic!("the selections compared, not {answer:?}");
    };
    let changes: Vec<(bool, String)> = compared
        .changes
        .iter()
        .map(|change| (change.selected, String::from_utf16_lossy(&change.text)))
        .collect();
    assert_eq!(changes, [(true, "alpha".to_owned())]);
    fixture.app.quit();
    assert_eq!(press, [123, 109], "the review copy's first press");
    assert!(press.iter().all(|&count| count < LIMIT / 2));
}

/// Brands the window read's batches.
enum WindowSteps {}

/// The steps of a window read (`common/window_steps.rs`) of mockapp's terminal `text` as `ask`
/// names it, its marks the positions `marks` holds, with the screen's
/// colours when `colours`, as a shape the executor runs here.
fn window_shape<'t>(
    text: &'t batch::Text,
    (height, marks): (u32, &[&IUIAutomationTextRange]),
    colours: bool,
) -> batch::Shape<'t> {
    use common::window_steps::{CurrentMark, EndMark, WindowAsk, add_steps};
    let mut shape = batch::Shape::of(Some(text));
    let mut held = marks.iter().map(|mark| {
        let point = shape.point(&batch::held(mark, Endpoint::Start, true));
        let verbatim_model::batch::Point::At(verbatim_model::TextPoint::At(position)) = point
        else {
            unreachable!("a held position")
        };
        EndMark {
            anchor: position.anchor,
            line: String::new(),
            line_above: 0,
            rows: 0,
            row: String::new(),
        }
    });
    let ask = WindowAsk {
        end: held.next().map(|mark| CurrentMark { mark, find: false }),
        aside: held.next(),
        height,
        opening: 30,
        colours,
    };
    let mut steps = verbatim_core::batch::Builder::<WindowSteps>::named();
    add_steps(&mut steps, verbatim_model::NodeId::new(1), &ask);
    let verbatim_model::request::Question::Batch(sent) = steps.finish().question() else {
        unreachable!("a builder's batch")
    };
    shape.steps = sent.steps;
    shape.culture = false;
    shape
}

/// A terminal's window read, the request list's generic steps: a first
/// read naming no mark, and its cap, a read naming both end marks with
/// more rows written past each than the opening rows and the window hold,
/// with the caret, each one program. No step loops, so a long text costs
/// the same instructions as a short one.
fn generic_window_reads_execute_exactly() {
    common::init_com();
    let mut fixture = Fixture::start("terminal.json", "mockapp-instructions-generic-window");
    let text = batch::Text::of(fixture.find("Terminal"));
    common::apply(&mut fixture.app, fixture.hwnd, "screen term 30");
    common::apply(
        &mut fixture.app,
        fixture.hwnd,
        &format!(r"set-text term {}ready>\n", numbered(10)),
    );
    let first_shape = window_shape(&text, (0, &[]), false);
    let mut end = None;
    let first = each_counted(|| {
        let answer = run_batch(&first_shape.segment(&fixture.uia));
        end = answer.steps.into_iter().nth(4).and_then(|ran| match ran {
            Ran::Done(Found::Move(moved)) => Some(moved.from),
            _ => None,
        });
    });
    let end = end.expect("the document's end");
    common::apply(
        &mut fixture.app,
        fixture.hwnd,
        &format!(r"set-text term {}ready>\n", numbered(200)),
    );
    let capped_shape = window_shape(&text, (30, &[&end, &end]), false);
    let capped = each_counted(|| {
        let _ = run_batch(&capped_shape.segment(&fixture.uia));
    });
    fixture.app.quit();
    assert!(first.iter().chain(&capped).all(|&count| count < LIMIT / 2));
    assert_eq!(
        [first, capped],
        [vec![233], vec![353]],
        "terminal window first and capped"
    );
}

/// A terminal's colours, `ReadColours` after a window read on a 120 by 30
/// screen: in the worst case, every cell's colours differing from its
/// neighbours' (`terminal-checkered.json`), each colour program reads rows
/// while a worst row still fits its budget and the next goes on from
/// there; a plain screen's colours are one program. Each text is read
/// once, so neither read has a size kept to read colours at.
fn generic_colours_execute_exactly() {
    common::init_com();
    let screen = vec!["x".repeat(120); 30].join(r"\n");
    let read = |fixture: &str, prefix: &str| {
        let mut fixture = Fixture::start(fixture, prefix);
        let text = batch::Text::of(fixture.find("Terminal"));
        common::apply(&mut fixture.app, fixture.hwnd, "screen term 30");
        common::apply(
            &mut fixture.app,
            fixture.hwnd,
            &format!("set-text term {screen}"),
        );
        let shape = window_shape(&text, (0, &[]), true);
        let mut colours = None;
        let counts = each_counted(|| {
            let answer = run_batch(&shape.segment(&fixture.uia));
            colours = answer.steps.into_iter().last().and_then(|ran| match ran {
                Ran::Done(Found::Colours(rows)) => Some(rows),
                _ => None,
            });
        });
        fixture.app.quit();
        let runs: Vec<usize> = colours.expect("the colours").iter().map(Vec::len).collect();
        (counts, runs)
    };
    let (worst, worst_runs) = read(
        "terminal-checkered.json",
        "mockapp-instructions-generic-colours-worst",
    );
    let (plain, plain_runs) = read(
        "terminal-plain-colours.json",
        "mockapp-instructions-generic-colours-plain",
    );
    // Thirty rows from the screen's top, which stop at the document's end
    // after twenty-nine, as mockapp counts the empty end of the text as the
    // screen's last row.
    assert_eq!(worst_runs, vec![120; 29], "every cell its own run");
    assert_eq!(plain_runs, vec![1; 29], "every row one run");
    assert_eq!(
        worst,
        vec![233, 4106, 4099, 4099, 4099, 4099, 3285],
        "the window read's program, then the worst case's colour programs"
    );
    assert_eq!(
        plain,
        vec![233, 1262],
        "the window read's program, then a plain screen's"
    );
    assert!(worst.iter().all(|&count| count < LIMIT / 2));
}

/// A batch Core's terminal reader wrote (`verbatim_core::terminal::reader`)
/// for mockapp's terminal `text`, as a shape the executor runs here:
/// `batch_of` is given the anchor the shape holds `start` under, when a
/// start is given.
fn reader_shape<'t>(
    text: &'t batch::Text,
    start: Option<&IUIAutomationTextRange>,
    batch_of: impl FnOnce(verbatim_model::TextAnchor) -> verbatim_model::batch::Batch,
) -> batch::Shape<'t> {
    let mut shape = batch::Shape::of(Some(text));
    let anchor = match start {
        Some(start) => {
            let point = shape.point(&batch::held(start, Endpoint::Start, true));
            let verbatim_model::batch::Point::At(verbatim_model::TextPoint::At(position)) = point
            else {
                unreachable!("a held position")
            };
            position.anchor
        }
        None => verbatim_model::TextAnchor(0),
    };
    shape.steps = batch_of(anchor).steps;
    shape.culture = false;
    shape
}

/// The window read `read` of Core's terminal reader, as a batch.
fn reader_window(
    read: verbatim_core::terminal::reader::WindowRead,
    (colours, starting): (bool, bool),
) -> verbatim_model::batch::Batch {
    let ask = verbatim_core::terminal::reader::WindowAsk {
        read,
        colours,
        starting,
        output: false,
    };
    verbatim_core::terminal::reader::window_batch(verbatim_model::NodeId::new(1), &ask)
        .0
        .batch()
        .clone()
}

/// The start a read of Core's terminal reader names next on a screen of 30
/// rows: where its history block read's move from the held end landed.
fn reader_next_start(fixture: &Fixture, text: &batch::Text) -> IUIAutomationTextRange {
    use verbatim_core::terminal::reader::WindowRead;
    let height = std::num::NonZeroU32::new(30).expect("a height");
    let block = reader_shape(text, None, |_| {
        reader_window(WindowRead::HistoryBlock { height }, (false, false))
    });
    let answer = run_batch(&block.segment(&fixture.uia));
    match answer.steps.into_iter().nth(2) {
        Some(Ran::Done(Found::Move(moved))) => moved.to,
        other => panic!("the next start's move, not {other:?}"),
    }
}

/// The batches of Core's terminal reader, each one program: a first read;
/// a first read again with its history block; the whole text; a read from
/// a start, the largest, whose named start is used; the same as a caret
/// key's starting read; the same read after a clear, when the next
/// start's steps run instead; a history read from a start; and a whole
/// history read. No step loops, so a long text costs the same
/// instructions as a short one.
fn terminal_reader_batches_execute_exactly() {
    use verbatim_core::terminal::reader::{HistoryAsk, WindowRead, history_batch};
    common::init_com();
    let mut fixture = Fixture::start("terminal.json", "mockapp-instructions-reader");
    let text = batch::Text::of(fixture.find("Terminal"));
    common::apply(&mut fixture.app, fixture.hwnd, "screen term 30");
    common::apply(
        &mut fixture.app,
        fixture.hwnd,
        &format!(r"set-text term {}ready>\n", numbered(200)),
    );
    let height = std::num::NonZeroU32::new(30).expect("a height");
    let next = reader_next_start(&fixture, &text);
    let from = |anchor| WindowRead::From {
        start: anchor,
        height: 30,
    };
    let node = verbatim_model::NodeId::new(1);
    let shapes = [
        reader_shape(&text, None, |_| {
            reader_window(WindowRead::First, (false, false))
        }),
        reader_shape(&text, None, |_| {
            reader_window(WindowRead::HistoryBlock { height }, (false, false))
        }),
        reader_shape(&text, None, |_| {
            reader_window(WindowRead::Whole { height: 30 }, (false, false))
        }),
        reader_shape(&text, Some(&next), |start| {
            reader_window(from(start), (false, false))
        }),
        reader_shape(&text, Some(&next), |start| {
            reader_window(from(start), (false, true))
        }),
        reader_shape(&text, Some(&next), |start| {
            let ask = HistoryAsk::Above {
                start,
                above: 60,
                height,
            };
            history_batch(node, &ask).0.batch().clone()
        }),
        reader_shape(&text, None, |_| {
            history_batch(node, &HistoryAsk::Whole).0.batch().clone()
        }),
    ];
    let mut counts: Vec<Vec<u32>> = shapes
        .iter()
        .map(|shape| {
            each_counted(|| {
                let _ = run_batch(&shape.segment(&fixture.uia));
            })
        })
        .collect();
    // The text cleared below the start: the next start's steps run.
    common::apply(&mut fixture.app, fixture.hwnd, r"set-text term cleared\n");
    counts.push(each_counted(|| {
        let _ = run_batch(&shapes[3].segment(&fixture.uia));
    }));
    fixture.app.quit();
    assert!(counts.iter().flatten().all(|&count| count < LIMIT / 2));
    assert_eq!(
        counts,
        [
            vec![207],
            vec![242],
            vec![206],
            vec![303],
            vec![303],
            vec![153],
            vec![39],
            vec![294]
        ],
        "first, history block, whole, from a start, starting, history above, whole history, \
         and from a start past the text's end"
    );
}

/// The command search of Core's terminal reader
/// (`verbatim_core::terminal::reader::search_batch`) for `needle`, each
/// way from the row `from` rows above `start` in mockapp's terminal
/// `text`, as a shape the executor runs here; each way makes at most
/// `calls` calls where the reader's hundred are not asked.
fn search_shape<'t>(
    text: &'t batch::Text,
    start: &IUIAutomationTextRange,
    (needle, from): (&str, i32),
    calls: u8,
) -> batch::Shape<'t> {
    use verbatim_core::terminal::reader::{SearchAsk, search_batch};
    let mut shape = reader_shape(text, Some(start), |start| {
        let ask = SearchAsk::new(start, needle.to_owned(), from, 1);
        search_batch(verbatim_model::NodeId::new(1), &ask)
            .0
            .batch()
            .clone()
    });
    for step in &mut shape.steps {
        if let verbatim_model::batch::Op::FindText { calls: most, .. } = &mut step.op {
            *most = calls;
        }
    }
    shape
}

/// What a run of a search's shape gave: each program's instructions, the
/// rows to the text's end and the text of each match toward the text's
/// end and of each toward its start, and the provider's `FindText` calls
/// and the calls it answered with `RPC_E_SERVERFAULT`.
#[derive(Debug, PartialEq, Eq)]
struct Searched {
    programs: Vec<u32>,
    down: Vec<(u32, String)>,
    up: Vec<(u32, String)>,
    find_calls: u32,
    faults: u32,
}

/// Runs `shape`, a search's ([`search_shape`]), against `fixture`.
fn searched(fixture: &Fixture, shape: &batch::Shape<'_>) -> Searched {
    common::reset_hits(fixture.hwnd);
    let mut steps = Vec::new();
    let programs = each_counted(|| steps = run_batch(&shape.segment(&fixture.uia)).steps);
    let hits = common::read_hits(fixture.hwnd);
    let hit = |method: &str| {
        hits.iter()
            .find_map(|(name, count)| (*name == method).then_some(*count))
            .unwrap_or(0)
    };
    let matches = |step: &Ran| match step {
        Ran::Done(Found::FindText(matches)) => matches
            .iter()
            .map(|found| (found.to_end, String::from_utf16_lossy(&found.text)))
            .collect(),
        other => panic!("a search's matches, not {other:?}"),
    };
    Searched {
        programs,
        down: matches(&steps[2]),
        up: matches(&steps[3]),
        find_calls: hit("FindText"),
        faults: hit("RPC_E_SERVERFAULT"),
    }
}

/// A collapsed range at the start of `text`'s text, a search's start.
fn text_start(text: &batch::Text) -> IUIAutomationTextRange {
    use verbatim_uia::text::TextRangeExt;
    let start = text.pattern.document_range().expect("the document range");
    start
        .move_endpoint_to(Endpoint::End, &start, Endpoint::Start)
        .expect("a collapsed range");
    start
}

/// The command search of Core's terminal reader, `FindText` each way from
/// a row, is one program that makes exactly the calls its steps ask: typically, a
/// line found once at its own row, each way ending at its first miss; a
/// line found nowhere, which makes no call but the search of the whole
/// text for each way; and at its bound, each way's hundred calls spent
/// and its twenty matches taken, eighty hits within rows passed over
/// between them. The three costs the emitter declares are these counts'
/// differences: the bound's run stays under the limit.
fn searches_execute_exactly() {
    common::init_com();
    let mut fixture = Fixture::start("terminal.json", "mockapp-instructions-search");
    let text = batch::Text::of(fixture.find("Terminal"));
    let within = " y y y y";
    let mut rows = vec!["y top"];
    rows.extend([within; 20]);
    rows.extend(["y up"; 19]);
    rows.extend(["y down"; 19]);
    rows.extend([within; 20]);
    rows.extend(["y bottom", "end"]);
    common::apply(&mut fixture.app, fixture.hwnd, "screen term 3");
    common::apply(
        &mut fixture.app,
        fixture.hwnd,
        &format!("set-text term {}", rows.join(r"\n")),
    );
    let start = text_start(&text);
    let run =
        |needle_from, calls| searched(&fixture, &search_shape(&text, &start, needle_from, calls));
    let typical = run(("end", -80), 100);
    let nowhere = run(("q", -40), 100);
    let bound = run(("y", -40), 100);
    // Each way's first 19 hits are matches and its next 80 are within
    // rows: 19 calls take 19 matches, and 23 pass 4 more hits over.
    let counts = [1, 19, 23].map(|calls| {
        let ran = run(("y", -40), calls);
        assert_eq!(
            (ran.down.len(), ran.up.len(), ran.find_calls),
            (
                usize::from(calls.min(19)),
                usize::from(calls.min(19)),
                2 * u32::from(calls) + 3
            ),
            "{calls} calls each way"
        );
        ran.programs
    });
    fixture.app.quit();
    assert_eq!(
        typical,
        Searched {
            programs: vec![223],
            down: vec![(0, "end".to_owned())],
            up: Vec::new(),
            // Toward the end: the whole text, the text no terminal holds,
            // the line and a miss. Toward the start: the whole text and a
            // miss.
            find_calls: 6,
            faults: 0,
        },
        "a line found once"
    );
    assert_eq!(
        nowhere,
        Searched {
            programs: vec![150],
            down: Vec::new(),
            up: Vec::new(),
            find_calls: 2,
            faults: 0,
        },
        "a line found nowhere"
    );
    assert_eq!(
        (
            bound.down.len(),
            bound.up.len(),
            bound.find_calls,
            bound.faults
        ),
        (20, 20, 203, 0),
        "the bound: a hundred calls each way, and three over the whole text"
    );
    // A call whose hit is passed over executes 20 instructions, and a
    // match 8 more: (1389 - 1229) / 8 and (1229 - 220) / 36, the costs the
    // emitter declares (`FIND_CALL` and `FIND_MATCH`, `batch/program.rs`).
    assert_eq!(
        (counts, bound.programs),
        ([vec![220], vec![1229], vec![1389]], vec![4485]),
        "one call each way, 19, 23, and the bound"
    );
}

/// The same search in a provider whose `FindText` answers a search that
/// finds nothing with a range, as Windows Terminal's does
/// (`terminal_find_misses_answer_a_range.json`): a call on that range
/// fails with `RPC_E_SERVERFAULT`, which ends a remote run in Windows
/// Terminal. The program makes no such call, so it runs alone, with
/// exactly its own calls: a line found each way; a line found toward the
/// text's end only, where the search toward its start ends at the
/// comparison of its span with the hit (the `CompareEndpoints` guard);
/// and a line found nowhere, which ends each way at the comparison with
/// the answer for text no terminal holds (the sentinel comparison).
fn searches_whose_misses_answer_a_range_run_alone() {
    common::init_com();
    let mut fixture = Fixture::start(
        "terminal_find_misses_answer_a_range.json",
        "mockapp-instructions-search-misses",
    );
    let text = batch::Text::of(fixture.find("Terminal"));
    let rows = ["y 1", "y 2", "y 3", "y 4", "y 5", "end"];
    common::apply(&mut fixture.app, fixture.hwnd, "screen term 3");
    common::apply(
        &mut fixture.app,
        fixture.hwnd,
        &format!("set-text term {}", rows.join(r"\n")),
    );
    let start = text_start(&text);
    let run = |needle| searched(&fixture, &search_shape(&text, &start, (needle, -2), 100));
    let (both, one, nowhere) = (run("y"), run("y 5"), run("q"));
    fixture.app.quit();
    let row = |to_end, text: &str| (to_end, format!("{text}\n"));
    assert_eq!(
        both,
        Searched {
            programs: vec![337],
            down: vec![row(3, "y 3"), row(2, "y 4"), row(1, "y 5")],
            up: vec![row(4, "y 2"), row(5, "y 1")],
            // The whole text twice and the text no terminal holds, three
            // lines and a miss toward the end, two and a miss toward the
            // start.
            find_calls: 10,
            faults: 0,
        },
        "a line found each way"
    );
    assert_eq!(
        one,
        Searched {
            programs: vec![225],
            down: vec![row(1, "y 5")],
            up: Vec::new(),
            find_calls: 6,
            faults: 0,
        },
        "a line found toward the end only"
    );
    assert_eq!(
        nowhere,
        Searched {
            programs: vec![167],
            down: Vec::new(),
            up: Vec::new(),
            // The whole text for each way, and the text no terminal holds.
            find_calls: 3,
            faults: 0,
        },
        "a line found nowhere"
    );
}

/// The colours of a read of Core's terminal reader from a start, its
/// largest batch, on a 120 by 30 screen, in the worst case, every cell's
/// colours differing from its neighbours' (`terminal-checkered.json`), and
/// on a plain screen. A text's first colour read has no size kept to read
/// colours at: the read's program, then the colour programs. Every later
/// read of that text reads its first rows' colours in its own program, at
/// the size its last read found, so a plain screen's read is one program,
/// and the worst case's first program stops at the colours' budget and
/// the colour programs go on from there.
fn terminal_reader_colours_execute_exactly() {
    use verbatim_core::terminal::reader::WindowRead;
    common::init_com();
    let screen = vec!["x".repeat(120); 30].join(r"\n");
    let read = |fixture: &str, prefix: &str| {
        let mut fixture = Fixture::start(fixture, prefix);
        let text = batch::Text::of(fixture.find("Terminal"));
        common::apply(&mut fixture.app, fixture.hwnd, "screen term 30");
        common::apply(
            &mut fixture.app,
            fixture.hwnd,
            &format!("set-text term {screen}"),
        );
        let next = reader_next_start(&fixture, &text);
        let shape = reader_shape(&text, Some(&next), |start| {
            reader_window(WindowRead::From { start, height: 30 }, (true, false))
        });
        let reads: Vec<(Vec<u32>, Vec<usize>)> = (0..2)
            .map(|_| {
                let mut colours = None;
                let counts = each_counted(|| {
                    let answer = run_batch(&shape.segment(&fixture.uia));
                    colours = answer.steps.into_iter().last().and_then(|ran| match ran {
                        Ran::Done(Found::Colours(rows)) => Some(rows),
                        _ => None,
                    });
                });
                let runs = colours.expect("the colours").iter().map(Vec::len).collect();
                (counts, runs)
            })
            .collect();
        fixture.app.quit();
        reads
    };
    let plain = read(
        "terminal-plain-colours.json",
        "mockapp-instructions-reader-colours-plain",
    );
    let worst = read(
        "terminal-checkered.json",
        "mockapp-instructions-reader-colours-worst",
    );
    // Thirty rows from the screen's top, which stop at the document's end
    // after twenty-nine, as mockapp counts the empty end of the text as the
    // screen's last row.
    for (_, runs) in &worst {
        assert_eq!(*runs, vec![120; 29], "every cell its own run");
    }
    for (_, runs) in &plain {
        assert_eq!(*runs, vec![1; 29], "every row one run");
    }
    let counts = |reads: &[(Vec<u32>, Vec<usize>)]| -> Vec<Vec<u32>> {
        reads.iter().map(|(counts, _)| counts.clone()).collect()
    };
    assert_eq!(
        counts(&plain),
        [vec![330, 1262], vec![1599]],
        "a plain screen: a first read's program and its colour program, then a read that is \
         one program, colours and all"
    );
    assert_eq!(
        counts(&worst),
        [
            vec![330, 4106, 4099, 4099, 4099, 4099, 3285],
            vec![4443, 4099, 4099, 4099, 4099, 3285]
        ],
        "the worst case: a first read's program and its colour programs, then the read's \
         program with the first rows' colours and the colour programs that go on"
    );
    assert!(
        counts(&worst)
            .iter()
            .chain(&counts(&plain))
            .flatten()
            .all(|&count| count < LIMIT / 2)
    );
}

/// The calls the program of a read of Core's terminal reader makes to the
/// provider, a read from a start on a screen of 4 rows, without the
/// screen's colours and, once a read has found the size, with them: the
/// text's element found once (`ProviderOptions`, `GetPropertyValue`,
/// `HostRawElementProvider` and `Navigate`), its pattern and its document
/// range read once, the comparison of the start with the next start made
/// once, and no position copied that a step only reads. Core takes this
/// read twice for every key, and Windows Terminal answers each call
/// slowly while it echoes one.
fn terminal_reader_reads_call_the_provider_exactly() {
    use verbatim_core::terminal::reader::WindowRead;
    common::init_com();
    let mut fixture = Fixture::start("terminal.json", "mockapp-instructions-reader-provider");
    let text = batch::Text::of(fixture.find("Terminal"));
    common::apply(&mut fixture.app, fixture.hwnd, "screen term 4");
    common::apply(
        &mut fixture.app,
        fixture.hwnd,
        &format!(
            r"set-text term {}ready>
",
            numbered(8)
        ),
    );
    let height = std::num::NonZeroU32::new(3).expect("a height");
    let block = reader_shape(&text, None, |_| {
        reader_window(WindowRead::HistoryBlock { height }, (false, false))
    });
    let next = match run_batch(&block.segment(&fixture.uia))
        .steps
        .into_iter()
        .nth(2)
    {
        Some(Ran::Done(Found::Move(moved))) => moved.to,
        other => panic!("the next start's move, not {other:?}"),
    };
    let hits = |colours: bool| {
        let shape = reader_shape(&text, Some(&next), |start| {
            reader_window(WindowRead::From { start, height: 3 }, (colours, false))
        });
        common::reset_hits(fixture.hwnd);
        let mut rows = None;
        let programs = each_counted(|| {
            let answer = run_batch(&shape.segment(&fixture.uia));
            rows = answer.steps.into_iter().last().and_then(|ran| match ran {
                Ran::Done(Found::Colours(rows)) => Some(rows.len()),
                _ => None,
            });
        })
        .len();
        (programs, rows, common::read_hits(fixture.hwnd))
    };
    let plain = hits(false);
    // The first coloured read finds the size; the second is the one pinned.
    let first = hits(true);
    let coloured = hits(true);
    fixture.app.quit();
    assert_eq!((first.0, first.1), (2, Some(3)), "the first coloured read");
    let element = [
        ("ProviderOptions", 2),
        ("GetPatternProvider", 1),
        ("GetPropertyValue", 1),
        ("HostRawElementProvider", 1),
        ("Navigate", 1),
        ("ITextProvider::GetSelection", 1),
        ("GetVisibleRanges", 1),
        ("DocumentRange", 1),
    ];
    let with = |ranges: &[(&'static str, u32)]| -> Vec<(&'static str, u32)> {
        element.iter().chain(ranges).copied().collect()
    };
    assert_eq!(
        plain,
        (
            1,
            None,
            with(&[
                ("Clone", 14),
                ("CompareEndpoints", 5),
                ("ExpandToEnclosingUnit", 2),
                ("GetText", 6),
                ("Move", 5),
                ("MoveEndpointByRange", 6),
            ])
        ),
        "a read from a start: 47 calls"
    );
    // The colours of 3 rows of 10 cells, which this fixture's provider
    // answers cell by cell.
    assert_eq!(
        coloured,
        (
            1,
            Some(3),
            with(&[
                ("Clone", 34),
                ("CompareEndpoints", 9),
                ("ExpandToEnclosingUnit", 3),
                ("GetAttributeValue", 66),
                ("GetText", 8),
                ("Move", 28),
                ("MoveEndpointByUnit", 21),
                ("MoveEndpointByRange", 14),
            ])
        ),
        "the same with the colours, in the one program"
    );
}

/// A colour read after a step whose declared worst case leaves no room for
/// the colours' budget under the instruction limit: a line read with its
/// formatting (7,700 instructions declared for the formatting), then the
/// screen, the line at its top and the colours, on a plain 120 by 30
/// screen. The size is kept by the first read, yet the second's program
/// ends before the colours as the first's did, and the colour program
/// reads them; the same steps without the formatting fit, and are one
/// program.
fn a_colour_read_that_does_not_fit_its_program_runs_the_colour_programs() {
    use verbatim_model::batch::{Op, Place, Subject};
    use verbatim_model::{TextPoint, TextUnit};
    common::init_com();
    let screen = vec!["x".repeat(120); 30].join(r"\n");
    let mut fixture = Fixture::start(
        "terminal-plain-colours.json",
        "mockapp-instructions-colours-no-room",
    );
    let text = batch::Text::of(fixture.find("Terminal"));
    common::apply(&mut fixture.app, fixture.hwnd, "screen term 30");
    common::apply(
        &mut fixture.app,
        fixture.hwnd,
        &format!("set-text term {screen}"),
    );
    let subject = Subject::Node(batch::TEXT);
    let shape = |formats: bool| {
        let mut shape = batch::Shape::of(Some(&text));
        shape.culture = false;
        shape
            .step(batch::read_text(
                verbatim_model::batch::Point::At(TextPoint::Caret),
                (None, TextUnit::Line, 1),
                formats,
            ))
            .step(Op::Visible {
                subject: subject.clone(),
            })
            .step(Op::ReadText {
                subject: subject.clone(),
                at: verbatim_model::batch::Point::Of(
                    Place(1),
                    verbatim_model::batch::PointOf::VisibleStart,
                ),
                movement: None,
                unit: TextUnit::Line,
                count: 1,
                details: verbatim_model::TextDetails::NONE,
                whole: false,
                trim: true,
            })
            .step(Op::ReadColours {
                subject: subject.clone(),
                screen: Place(1),
                top: Place(2),
            });
        shape
    };
    let read = |shape: &batch::Shape<'_>| {
        let mut rows = None;
        let counts = each_counted(|| {
            let answer = run_batch(&shape.segment(&fixture.uia));
            rows = answer.steps.into_iter().last().and_then(|ran| match ran {
                Ran::Done(Found::Colours(rows)) => Some(rows.len()),
                _ => None,
            });
        });
        assert_eq!(rows, Some(29), "the screen's rows");
        counts
    };
    let (formatted, plain) = (shape(true), shape(false));
    let counts = [
        read(&formatted),
        read(&formatted),
        read(&plain),
        read(&formatted),
    ];
    fixture.app.quit();
    assert_eq!(
        counts,
        [
            vec![234, 1262],
            vec![234, 1262],
            vec![1379],
            vec![234, 1262]
        ],
        "with the formatting, twice; without it, once the size is kept; and with it again"
    );
}

/// A caret wait's poll (`docs/design/readers-tell-core-asks-build1.md`,
/// "The watch"; the request list's design, section 5, `WaitForCaret`):
/// one program, the check followed by the batch's later reads, the caret
/// and its line with every attribute's formatting, gated on the check's
/// caret or selection having moved or its being at the bound. Typically a
/// poll that finds nothing, its later reads skipped by their gate; a line
/// key's poll that finds the caret moved, reading the line's one stretch
/// in `text.json`; a deleting key's at the bound with a selection made
/// since the starting point, reading the word too; and in `mixed.json`,
/// where every stretch is walked by words and characters, the poll at the
/// bound over a line of 16 stretches, counted, and the worst, over a line
/// of 80 cut at 64 stretches, which the counting form cannot run, since it
/// executes twice as many: run as Verbatim runs it, it stays under the
/// limit, one program. The check and the reads it gates were the watch
/// check's one program before (91, 190, 110 without formatting, and 2,181
/// for the 17 stretches): a poll that finds nothing executes 139, its
/// gated reads' tests among them.
fn wait_polls_execute_exactly() {
    use verbatim_uia_rops::FoundPoint;
    use verbatim_uia_rops::batch::{Found, Ran, run_batch};
    common::init_com();
    let mut plain = Fixture::start("text.json", "mockapp-instructions-poll");
    let text = batch::Text::of(plain.find("Notes"));
    common::apply(&mut plain.app, plain.hwnd, "caret doc 0");
    let before = verbatim_uia::text::TextPatternExt::selection(&text.pattern)
        .expect("the selection")
        .remove(0);
    let start = FoundPoint {
        range: before,
        endpoint: Endpoint::Start,
        collapsed: true,
    };
    let poll = |text: &batch::Text, (word, last): (bool, bool)| {
        let shape = batch::wait_poll(text, start.clone(), (word, last));
        run_batch(&shape.segment(&plain.uia))
    };
    let read_after = |answer: &verbatim_uia_rops::batch::Answer| match &answer.steps[..] {
        [Ran::Done(Found::WaitCheck(checked)), ..] => checked.reads_after,
        other => panic!("the check: {other:?}"),
    };
    let typical = counted(|| assert!(!read_after(&poll(&text, (false, false)))));
    common::apply(&mut plain.app, plain.hwnd, "caret doc 11");
    let moved = counted(|| assert!(read_after(&poll(&text, (false, false)))));
    common::apply(&mut plain.app, plain.hwnd, "caret doc 6 10");
    let deleting = counted(|| assert!(read_after(&poll(&text, (true, true)))));
    plain.app.quit();

    let mut mixed = Fixture::start("mixed.json", "mockapp-instructions-poll-mixed");
    let text = batch::Text::of(mixed.find("Notes"));
    common::apply(&mut mixed.app, mixed.hwnd, "caret doc 0");
    let before = verbatim_uia::text::TextPatternExt::selection(&text.pattern)
        .expect("the selection")
        .remove(0);
    let start = FoundPoint {
        range: before,
        endpoint: Endpoint::Start,
        collapsed: true,
    };
    let at_bound = |text: &batch::Text| {
        run_batch(&batch::wait_poll(text, start.clone(), (false, true)).segment(&mixed.uia))
    };
    let stretches = |answer: &verbatim_uia_rops::batch::Answer| match answer.steps.get(2) {
        Some(Ran::Done(Found::ReadText(line))) => line.runs.len(),
        other => panic!("the line: {other:?}"),
    };
    let sixteen = counted(|| assert_eq!(stretches(&at_bound(&text)), 17));
    common::apply(&mut mixed.app, mixed.hwnd, "caret doc 50");
    let _ = verbatim_win32::calls::take();
    let worst = at_bound(&text);
    let worst_calls = verbatim_win32::calls::take();
    mixed.app.quit();
    assert_eq!(
        [typical, moved, deleting, sixteen],
        [141, 306, 530, 2297],
        "a poll that finds nothing, a moved line key's, a deleting key's at the bound, \
         and 17 mixed stretches at the bound"
    );
    assert_eq!(stretches(&worst), 64, "80 characters at the bound");
    assert_eq!(worst_calls.uia, 1, "the worst poll, one program");
}

/// A focus completion's read after the focus's registration
/// (`verbatim_uia_rops::focus_states`): typically, a button's states with
/// its dialog window's four children, `dialog_class_window.json`'s three
/// and the window's title bar; at its worst,
/// a focus's states with the children of `wide.json`'s list of a hundred
/// items read as a dialog's, which the run stops at its sixty and answers
/// unread.
fn focus_states_execute_exactly() {
    common::init_com();
    let run = |name: &str, prefix: &str, focus: &str, dialog: Option<&str>| {
        let fixture = Fixture::start(name, prefix);
        let nodes = fixture.nodes();
        let children = Fill::new(
            &fixture
                .uia
                .request_fixed::<DialogText>()
                .expect("a cache request"),
        );
        let focus = Live::new(fixture.find(focus));
        let dialogs = vec![Live::new(
            dialog.map_or_else(|| fixture.root.clone(), |name| fixture.find(name)),
        )];
        let query = StatesQuery {
            focus: &focus,
            fill: &nodes,
            dialogs: &dialogs,
            children: &children,
        };
        let mut answered = None;
        let count = counted(|| {
            let states = focus_states_remote(&fixture.uia, &query).expect("the program runs");
            answered = Some(
                states
                    .children
                    .iter()
                    .map(|read| read.as_ref().map(Vec::len))
                    .collect::<Vec<_>>(),
            );
        });
        fixture.app.quit();
        (count, answered.expect("an answer"))
    };
    let typical = run(
        "dialog_class_window.json",
        "mockapp-instructions-dialog-states",
        "No",
        None,
    );
    let worst = run(
        "wide.json",
        "mockapp-instructions-wide-states",
        "Item 000",
        Some("Many"),
    );
    assert_eq!(
        (typical.clone(), worst.clone()),
        ((442, vec![Some(4)]), (2233, vec![None])),
        "a dialog window's four children; a hundred, past the run's sixty"
    );
    assert!(typical.0 < LIMIT / 2 && worst.0 < LIMIT / 2);
}

/// Runs `run` with counting on and returns how many instructions each
/// program it ran executed, in order.
fn each_counted(run: impl FnOnce()) -> Vec<u32> {
    counting::start();
    run();
    counting::stop()
        .into_iter()
        .map(|count| count.expect("a counted program"))
        .collect()
}

/// The request list's generated text programs (`verbatim_uia_rops::batch`)
/// at their caps: a caret tell whose line has 64 format stretches, every
/// attribute read and learned, the caret read's worst; a caret key's
/// starting read for Delete, the largest; review's next word from a point
/// found by its text; review select's two programs, through a character;
/// a copy; and say-all's 32 lines.
fn batch_text_shapes_execute_exactly() {
    use verbatim_model::batch::Point;
    use verbatim_model::{TextPoint, TextUnit};
    use verbatim_uia_rops::batch::run_batch;
    common::init_com();
    let mut fixture = Fixture::start("italic.json", "mockapp-instructions-batch-text");
    let text = batch::Text::of(fixture.find("Notes"));
    let run = |shape: batch::Shape<'_>| {
        each_counted(|| {
            let _ = run_batch(&shape.segment(&fixture.uia));
        })
    };
    common::apply(&mut fixture.app, fixture.hwnd, "caret doc 18");
    let before = text.pattern.document_range().expect("the document");
    common::apply(&mut fixture.app, fixture.hwnd, "caret doc 20 30");
    let baseline = batch::held(&before, Endpoint::Start, true);
    let tell = run(batch::caret_tell(&text, &baseline));
    common::apply(&mut fixture.app, fixture.hwnd, "caret doc 8");
    let delete = run(batch::starting_read(&text, TextUnit::Word, false, true));
    let prefix: Vec<u16> = "plain ".encode_utf16().collect();
    let counts = [6, 5, 4];
    let at = batch::after(&before, &prefix, &counts);
    let next_word = run(batch::next_word(&text, &at));
    let end = batch::named(TextPoint::End);
    let select = run(batch::review_select(&text, (&at, &end), Some((&at, &end))));
    let mut copy = batch::Shape::of(Some(&text));
    copy.step(batch::read_range(
        Point::At(TextPoint::SelectionStart),
        Point::At(TextPoint::SelectionEnd),
        None,
    ));
    let copy = run(copy);
    common::apply(
        &mut fixture.app,
        fixture.hwnd,
        &format!("set-text doc {}", numbered(40)),
    );
    let say_all = run(batch::one_read(
        &text,
        &batch::named(TextPoint::Start),
        (None, TextUnit::Line, 32),
    ));
    fixture.app.quit();
    let counts = [tell, delete, next_word, select, copy, say_all];
    assert_eq!(
        counts,
        [
            // 4493 and 147 while the test gave a unit read and a range or
            // comparison one cap, which the outpost never did: the
            // program builder's own caps differ, so each takes its own
            // constant (2026-11-22).
            vec![4482],
            vec![149],
            vec![150],
            vec![123, 109],
            vec![58],
            vec![757],
        ],
        "the caret tell, Delete's starting read, next word, review select, a copy, \
         say-all's lines"
    );
    assert!(counts.iter().flatten().all(|&count| count < LIMIT / 2));
}

/// The request list's generated object programs at their caps: object
/// navigation from the deepest item of `deep.json`, whose window is
/// sixty-two levels up; and the walks down the tree of
/// `wide.json`: the controlled selection whose list holds a hundred items,
/// and the tree dump, each run at its sixty children or sixty-four waiting
/// elements.
fn batch_object_shapes_execute_exactly() {
    use verbatim_model::batch::{Relation, View};
    use verbatim_uia_rops::batch::{Judged, run_batch};
    common::init_com();
    let run = |uia: &Uia, shape: batch::Shape<'_>| {
        each_counted(|| {
            let _ = run_batch(&shape.segment(uia));
        })
    };
    let fixture = Fixture::start("deep.json", "mockapp-instructions-batch-deep");
    let fill = batch::node_fill(&fixture.uia);
    let apple = fixture.find("Apple");
    let navigation = run(
        &fixture.uia,
        batch::related(&apple, Relation::Parent, &fill),
    );
    fixture.app.quit();

    let mut fixture = Fixture::start("wide.json", "mockapp-instructions-batch-wide");
    let fill = batch::node_fill(&fixture.uia);
    let many = batch::many_node_fill(&fixture.uia);
    common::apply(
        &mut fixture.app,
        fixture.hwnd,
        "set-controller-for item0 many",
    );
    let item = fixture.find("Item 000");
    let root = fixture.root.clone();
    let judged = |matched| {
        move |_: &Cached<Node>| Judged {
            matched,
            pruned: false,
        }
    };
    let (none, every) = (judged(false), judged(true));
    let controlled = run(
        &fixture.uia,
        batch::controlled_selection(&item, (&fill, &many), &none),
    );
    let walk = (View::Raw, false, false, 4_096);
    let dump = run(&fixture.uia, batch::walk(&root, &many, &every, walk));
    fixture.app.quit();

    let counts = [navigation, controlled, dump];
    assert_eq!(
        counts,
        [
            vec![978],
            // The list's hundred items fill no run alone, and are read by
            // a call between the first program and the second.
            vec![3366, 3448, 2020],
            vec![3188, 3192, 3529, 2225],
        ],
        "navigation, the controlled selection, the tree dump"
    );
    assert!(counts.iter().flatten().all(|&count| count < LIMIT / 2));
}

/// `count` numbered lines, `line 000` on, as mockapp's `set-text` takes
/// them, each line feed escaped.
fn numbered(count: u32) -> String {
    use std::fmt::Write as _;
    (0..count).fold(String::new(), |mut lines, line| {
        let _ = write!(lines, r"line {line:03}\n");
        lines
    })
}

/// Runs this file's tests through the UIA test runner, which explains why
/// these binaries do not exit normally (`common/harness.rs`).
fn main() {
    harness::run_isolated(&[
        (
            "the_instruction_limit_is_measured",
            the_instruction_limit_is_measured,
        ),
        (
            "focus_and_navigation_execute_exactly",
            focus_and_navigation_execute_exactly,
        ),
        ("paths_execute_exactly", paths_execute_exactly),
        ("caret_reads_execute_exactly", caret_reads_execute_exactly),
        (
            "densely_mixed_formatting_executes_exactly",
            densely_mixed_formatting_executes_exactly,
        ),
        (
            "say_all_batches_execute_exactly",
            say_all_batches_execute_exactly,
        ),
        (
            "range_selections_execute_exactly",
            range_selections_execute_exactly,
        ),
        (
            "generic_window_reads_execute_exactly",
            generic_window_reads_execute_exactly,
        ),
        (
            "generic_colours_execute_exactly",
            generic_colours_execute_exactly,
        ),
        (
            "terminal_reader_batches_execute_exactly",
            terminal_reader_batches_execute_exactly,
        ),
        (
            "terminal_reader_colours_execute_exactly",
            terminal_reader_colours_execute_exactly,
        ),
        ("searches_execute_exactly", searches_execute_exactly),
        (
            "searches_whose_misses_answer_a_range_run_alone",
            searches_whose_misses_answer_a_range_run_alone,
        ),
        (
            "terminal_reader_reads_call_the_provider_exactly",
            terminal_reader_reads_call_the_provider_exactly,
        ),
        (
            "a_colour_read_that_does_not_fit_its_program_runs_the_colour_programs",
            a_colour_read_that_does_not_fit_its_program_runs_the_colour_programs,
        ),
        ("wait_polls_execute_exactly", wait_polls_execute_exactly),
        ("focus_states_execute_exactly", focus_states_execute_exactly),
        (
            "batch_text_shapes_execute_exactly",
            batch_text_shapes_execute_exactly,
        ),
        (
            "batch_object_shapes_execute_exactly",
            batch_object_shapes_execute_exactly,
        ),
    ]);
}
