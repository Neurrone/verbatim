//! UIA's limit on the instructions one remote operation may execute, and how
//! many each of Verbatim's programs executes, against mockapp's real,
//! out-of-process provider (`docs/performance.md`, "The instruction
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

mod common;
#[path = "common/harness.rs"]
mod harness;

use verbatim_uia::text::{Endpoint, TextPatternExt};
use verbatim_uia::{Ancestor, Cached, Fill, FocusInput, Kept, Live, Node, Shape, Uia, runtime_id};
use verbatim_uia_rops::{
    Attributes, Builder, CaretLineQuery, CaretQuery, Comparison, ControlledQuery, Error,
    FocusAncestry, FocusQuery, FormatSpan, Movement, NavigationDirection, PathQuery, Position,
    RangeAction, RangeEnd, RangeQuery, ScreenAnchor, ScreenQuery, Status, StepQuery, TextAttribute,
    TextFrom, TextTarget, UnitsQuery, caret_read_remote, controlled_selection_remote, counting,
    element_by_path_remote, focus_ancestry_remote, navigation_step_remote, terminal_screen_remote,
    text_range_remote, text_units_remote,
};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, IUIAutomationTextPattern, IUIAutomationTextPattern2, TextUnit_Line,
    TextUnit_Word, TreeScope_Descendants, UIA_NamePropertyId,
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

    /// The element named `name`, read as the node registry keeps it, as the
    /// navigation step query reads it.
    fn find_kept(&self, name: &str) -> Cached<Kept> {
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
                .request::<Ancestor>(Ancestor::all_on())
                .expect("a cache request"),
        )
    }

    /// The fill of a node, with every detail.
    fn nodes(&self) -> Fill<Node> {
        Fill::new(
            &self
                .uia
                .request::<Node>(Node::all_on())
                .expect("a cache request"),
        )
    }
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

/// The focus ancestry at its worst in mockapp: a list in sixty nested
/// groups (`deep.json`; a fixture's nesting is bounded by its JSON
/// parser's), every ancestor up to the window walked under the depth limit
/// the outpost uses (64), against 64 known ancestors none of which it
/// meets, with the list's selected item and a held element's focus read;
/// the same walk stopped by a depth limit of 30 and walking on to the
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
    let previous = fixture.find("Banana");
    let known: Vec<Vec<i32>> = (0..64).map(|index| vec![42, 7, index]).collect();
    let fill = fixture.ancestors();
    let query = FocusQuery {
        element: &list,
        known: &known,
        previous: Some(&previous),
        depth_limit: 64,
        fill: &fill,
        out_of_time: None,
        require_focus: true,
        refresh: false,
        nearest_window: None,
        before_call: None,
        value_available: None,
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
            previous: None,
            ..query
        };
        assert_eq!(ancestry(&typical), (1, false));
    });

    let nodes = fixture.nodes();
    let step = |element: &Cached<Kept>, direction| {
        let query = StepQuery {
            element,
            direction,
            fill: &nodes,
        };
        let step = navigation_step_remote(&fixture.uia, &query).expect("the program runs");
        assert_eq!(step.window, Some(fixture.hwnd.0 as isize));
    };
    let item = fixture.find_kept("Apple");
    let far = counted(|| step(&item, NavigationDirection::Parent));
    let outer = fixture.find_kept("Group 1");
    let near_step = counted(|| step(&outer, NavigationDirection::FirstChild));
    fixture.app.quit();

    assert_eq!(
        [worst, limited, typical, far, near_step],
        [2366, 1779, 1044, 870, 139],
        "focus ancestry worst, at depth limit 30, typical; navigation far and near"
    );
    assert!(worst < LIMIT / 2 && far < LIMIT / 2);
}

/// The focus listener's read of a focus, and the outpost's walk to its own
/// copy of an element by its path (stage A step 6). The read: the worst
/// case of `focus_and_navigation_execute_exactly`, with the focus's own
/// cache filled afresh in the same run, as the listener reads every focus.
/// The walk: from mockapp's window down sixty nested groups and the list
/// to its second item, the deepest path in mockapp, each level's first
/// child the one wanted but for the item; one level down; and a path whose
/// first runtime id no child has, which ends the walk at once.
fn reads_and_paths_execute_exactly() {
    common::init_com();
    let mut fixture = Fixture::start("deep.json", "mockapp-instructions-path");
    fixture.app.send("focus fruits");
    let list = fixture.find_input("Fruits");
    let previous = fixture.find("Banana");
    let known: Vec<Vec<i32>> = (0..64).map(|index| vec![42, 7, index]).collect();
    let fill = fixture.ancestors();
    let query = FocusQuery {
        element: &list,
        known: &known,
        previous: Some(&previous),
        depth_limit: 64,
        fill: &fill,
        out_of_time: None,
        require_focus: true,
        refresh: true,
        nearest_window: None,
        before_call: None,
        value_available: None,
    };
    let read = counted(|| {
        let answer = focus_ancestry_remote(&fixture.uia, &query).expect("the program runs");
        let FocusAncestry::Focused(ancestry) = answer else {
            panic!("not focused");
        };
        assert!(
            ancestry.element.is_some(),
            "the focus's own cache, refreshed"
        );
    });

    let mut path: Vec<Vec<i32>> = (1..=60)
        .map(|group| runtime_id(&fixture.find(&format!("Group {group}"))))
        .collect();
    path.push(list.runtime_id());
    path.push(runtime_id(&previous));
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
        [read, deepest, near, missing],
        [2379, 1134, 148, 141],
        "the listener's refreshed read; a path down sixty-two levels, down one, and to nothing"
    );
    assert!(read < LIMIT / 2 && deepest < LIMIT / 2);
}

/// A selection inside the list a search box controls
/// (`tests/fixtures/controller.json`), found inside a group of the list,
/// found as the list's second child, and not found, every element of the
/// list walked and then the list's selection, empty, read: the program
/// walks the controlled list depth first, so its count grows with the
/// elements it passes before the selected one.
fn controlled_selections_execute_exactly() {
    common::init_com();
    let fixture = Fixture::start("controller.json", "mockapp-instructions-controlled");
    let search = fixture.find("Search box");
    let selected = |name: &str| runtime_id(&fixture.find(name));
    let nodes = fixture.nodes();
    let run = |selected: &[i32]| {
        let query = ControlledQuery {
            focused: &search,
            selected,
            fill: &nodes,
        };
        controlled_selection_remote(&fixture.uia, &query)
            .expect("the program runs")
            .selected
            .is_some()
    };
    let grouped = selected("Display settings");
    let deep = counted(|| assert!(run(&grouped)));
    let second = selected("Sound settings");
    let near = counted(|| assert!(run(&second)));
    let outside = selected("Elsewhere");
    let missing = counted(|| assert!(!run(&outside)));
    fixture.app.quit();

    assert_eq!(
        [deep, near, missing],
        [174, 206, 229],
        "controlled selection inside a group, second child, not found"
    );
}

/// A caret read of the line at the caret with every attribute, the
/// selection's change from `old` and the evidence against it when given.
fn caret_query<'a>(
    element: &'a IUIAutomationElement,
    pattern: &'a IUIAutomationTextPattern,
    pattern2: Option<&'a IUIAutomationTextPattern2>,
    old: Option<RangeEnd<'a>>,
) -> CaretQuery<'a> {
    CaretQuery {
        element,
        pattern,
        pattern2,
        since: old,
        previous_selection: old.map(|old| (old, old)),
        unit: None,
        formats: Some(FormatSpan::Line),
        attributes: Attributes::ALL,
        learning: Attributes::ALL,
        max_text: 1024,
        max_change_text: 1024,
        recheck: false,
    }
}

/// The caret read: a line of 64 stretches, reached by walking one mixed
/// stretch by words and a mixed word by characters (`italic.json`), every
/// attribute read and learned, with a word, the evidence, and a selection's
/// change; and, typically, the report after a focus on a line of plain text
/// with the default theme's two attributes, the annotation types and the
/// link, whose support is known.
fn caret_reads_execute_exactly() {
    common::init_com();
    let mut fixture = Fixture::start("italic.json", "mockapp-instructions-caret");
    let notes = fixture.find("Notes");
    let (pattern, pattern2) = verbatim_uia::text::text_pattern(&notes).expect("a text pattern");
    common::apply(&mut fixture.app, fixture.hwnd, "caret doc 18");
    let before = caret_read_remote(&caret_query(&notes, &pattern, pattern2.as_ref(), None))
        .expect("the caret");
    common::apply(&mut fixture.app, fixture.hwnd, "caret doc 20 30");
    let old = RangeEnd {
        range: &before.caret,
        endpoint: Endpoint::Start,
    };
    let worst = counted(|| {
        let mut worst = caret_query(&notes, &pattern, pattern2.as_ref(), Some(old));
        worst.unit = Some(TextUnit_Word);
        let answer = caret_read_remote(&worst).expect("the program runs");
        assert_eq!(answer.runs.len(), 64);
        assert_eq!(answer.changes.map(|changes| changes.len()), Some(1));
    });
    common::apply(&mut fixture.app, fixture.hwnd, "caret doc 0");
    let typical = counted(|| {
        let mut typical = caret_query(&notes, &pattern, pattern2.as_ref(), None);
        typical.attributes = Attributes::of(&[TextAttribute::Annotations, TextAttribute::Link]);
        typical.learning = Attributes::NONE;
        let answer = caret_read_remote(&typical).expect("the program runs");
        assert_eq!(answer.runs.len(), 1);
    });
    fixture.app.quit();
    assert_eq!([worst, typical], [4425, 84], "caret read worst and typical");
}

/// The caret read where every format stretch is two characters with one
/// in italics (`mixed.json`), so each is walked by words and its word by
/// characters: lines of 16 and 32 characters, and their line feeds,
/// counted, which grow by the same amount, 249 instructions, for each
/// stretch; and a line of 80, cut at 64 stretches, about 8,000 by that
/// growth, which the counting form cannot run, since it executes twice as
/// many: run as Verbatim runs it, it stays under the limit.
fn densely_mixed_formatting_executes_exactly() {
    common::init_com();
    let mut fixture = Fixture::start("mixed.json", "mockapp-instructions-mixed");
    let notes = fixture.find("Notes");
    let (pattern, pattern2) = verbatim_uia::text::text_pattern(&notes).expect("a text pattern");
    let query = caret_query(&notes, &pattern, pattern2.as_ref(), None);
    common::apply(&mut fixture.app, fixture.hwnd, "caret doc 0");
    let sixteen = counted(|| {
        let answer = caret_read_remote(&query).expect("the program runs");
        assert_eq!(answer.runs.len(), 17);
    });
    common::apply(&mut fixture.app, fixture.hwnd, "caret doc 17");
    let thirty_two = counted(|| {
        let answer = caret_read_remote(&query).expect("the program runs");
        assert_eq!(answer.runs.len(), 33);
    });
    common::apply(&mut fixture.app, fixture.hwnd, "caret doc 50");
    let eighty = caret_read_remote(&query).map(|answer| answer.runs.len());
    fixture.app.quit();
    assert_eq!([sixteen, thirty_two], [2148, 4140], "17 and 33 stretches");
    match eighty {
        Ok(runs) => assert_eq!(runs, 64),
        Err(error) => panic!("80 characters: {error}"),
    }
}

/// Say-all's batch of twenty lines a line on from a held position, its
/// lines in more than one language, so each line's is read; and the first
/// batch, from the caret, in one language.
fn say_all_batches_execute_exactly() {
    common::init_com();
    let mut fixture = Fixture::start("languages.json", "mockapp-instructions-say-all");
    let notes = fixture.find("Notes");
    let (pattern, pattern2) = verbatim_uia::text::text_pattern(&notes).expect("a text pattern");
    let more = numbered(30);
    common::apply(
        &mut fixture.app,
        fixture.hwnd,
        &format!(r"set-text doc hello\nbonjour\nhallo\n{more}"),
    );
    common::apply(&mut fixture.app, fixture.hwnd, "caret doc 0");
    let target = TextTarget {
        element: &notes,
        pattern: &pattern,
        pattern2: pattern2.as_ref(),
    };
    let document = pattern.document_range().expect("the document range");
    let units = |from, movement| UnitsQuery {
        target,
        from,
        movement,
        unit: TextUnit_Line,
        count: 20,
        max_text: 1024,
        max_total: 32_768,
        culture: true,
    };
    let held = TextFrom::At(Position {
        range: &document,
        endpoint: Endpoint::Start,
        collapsed: false,
    });
    let languages = counted(|| {
        let answer = text_units_remote(&units(held, Some(Movement::By(TextUnit_Line, 1))))
            .expect("the program runs");
        assert_eq!(answer.units.len(), 20);
        assert_eq!(answer.units[1].language.as_deref(), Some("de-DE"));
    });
    common::apply(
        &mut fixture.app,
        fixture.hwnd,
        &format!("set-text doc {more}"),
    );
    let first = counted(|| {
        let answer = text_units_remote(&units(TextFrom::Caret, None)).expect("the program runs");
        assert_eq!(answer.units.len(), 20);
    });
    fixture.app.quit();
    assert_eq!(
        [languages, first],
        [626, 647],
        "a later batch in three languages, a first batch in one"
    );
    assert!(languages < LIMIT / 2 && first < LIMIT / 2);
}

/// The review copy's first press: "alpha" selected back from the review
/// cursor, on its first "a", through the marker, on its last, every point
/// but the document's start found by the text before it as Verbatim finds
/// a held position's, from no selection, and the selection read again and
/// compared.
fn range_selections_execute_exactly() {
    common::init_com();
    let mut fixture = Fixture::start("text.json", "mockapp-instructions-range");
    let notes = fixture.find("Notes");
    let (pattern, pattern2) = verbatim_uia::text::text_pattern(&notes).expect("a text pattern");
    common::apply(&mut fixture.app, fixture.hwnd, "caret doc 0");
    let document = pattern.document_range().expect("the document range");
    let start = Position {
        range: &document,
        endpoint: Endpoint::Start,
        collapsed: false,
    };
    let texts: Vec<Vec<u16>> = ["alph", "alpha", "a"]
        .iter()
        .map(|text| text.encode_utf16().collect())
        .collect();
    let counts: Vec<[i32; 1]> = [4, 5, 1].map(|count| [count]).to_vec();
    let after = |index: usize| TextFrom::After {
        from: start,
        prefix: &texts[index],
        counts: &counts[index],
    };
    let query = RangeQuery {
        target: TextTarget {
            element: &notes,
            pattern: &pattern,
            pattern2: pattern2.as_ref(),
        },
        start: after(0),
        end: Some(TextFrom::At(start)),
        through: Some((after(1), after(2))),
        action: RangeAction::SelectReporting(1024),
    };
    let press = counted(|| {
        let answer = text_range_remote(&query).expect("the program runs");
        assert!(answer.selected);
        let changes: Vec<(bool, String)> = answer
            .changes
            .expect("the changes, read")
            .into_iter()
            .map(|change| (change.selected, String::from_utf16_lossy(&change.text)))
            .collect();
        assert_eq!(changes, [(true, "alpha".to_owned())]);
    });
    fixture.app.quit();
    assert_eq!(press, 160, "the review copy's first press");
    assert!(press < LIMIT / 2);
}

/// A terminal's screen read: an anchor whose top row is nowhere as a row of
/// its own, under 90 rows that hold its text as part of theirs, so the
/// search checks its 20 matches and gives up and the text's rows are
/// counted; and, typically, an anchor found where it was under new output.
fn terminal_screens_execute_exactly() {
    common::init_com();
    let mut fixture = Fixture::start("terminal.json", "mockapp-instructions-terminal");
    let terminal = fixture.find("Terminal");
    let (pattern, pattern2) = verbatim_uia::text::text_pattern(&terminal).expect("a text pattern");
    common::apply(&mut fixture.app, fixture.hwnd, "screen term 30");
    let decoys: String = (0..90).map(|_| r"x ready>\n").collect();
    common::apply(
        &mut fixture.app,
        fixture.hwnd,
        &format!(r"set-text term {decoys}one\ntwo"),
    );
    // With the caret read in the same program, as the outpost reads a
    // focused terminal on each change of its text.
    let query = |top, next| ScreenQuery {
        element: &terminal,
        pattern: &pattern,
        anchor: Some(ScreenAnchor {
            top,
            next,
            range: None,
            first: "",
            no_history: false,
        }),
        matches_padding: false,
        seen_rows: 30,
        last_rows: 1,
        head_wanted: 30,
        caret: Some(CaretLineQuery {
            element: &terminal,
            pattern: &pattern,
            pattern2: pattern2.as_ref(),
            max_text: 4096,
            since: None,
        }),
    };
    let worst = counted(|| {
        let screen = terminal_screen_remote(&fixture.uia, &query("ready>\n", "ready>\n"))
            .expect("the program runs");
        assert_eq!(screen.shift, None);
    });
    let lines = numbered(80);
    common::apply(
        &mut fixture.app,
        fixture.hwnd,
        &format!(r"set-text term {lines}ready> ls\na\nb"),
    );
    let typical = counted(|| {
        let screen = terminal_screen_remote(&fixture.uia, &query("line 052\n", "line 053\n"))
            .expect("the program runs");
        assert_eq!(screen.shift, Some(1));
    });
    fixture.app.quit();
    assert_eq!(
        [worst, typical],
        [441, 166],
        "terminal screen worst and typical"
    );
    assert!(worst < LIMIT / 2);
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
        (
            "reads_and_paths_execute_exactly",
            reads_and_paths_execute_exactly,
        ),
        (
            "controlled_selections_execute_exactly",
            controlled_selections_execute_exactly,
        ),
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
            "terminal_screens_execute_exactly",
            terminal_screens_execute_exactly,
        ),
    ]);
}
