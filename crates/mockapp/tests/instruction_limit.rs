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
use verbatim_uia::{CACHED_PROPERTIES, ElementExt, Uia, runtime_id};
use verbatim_uia_rops::{
    Attributes, Builder, CaretQuery, Comparison, Error, Fingerprint, FocusAncestry, FocusQuery,
    FormatSpan, Found, Movement, NavigationDirection, Position, RangeEnd, Status, StepQuery,
    TailQuery, TailStart, TextAttribute, TextFrom, TextTarget, UnitsQuery, caret_read_remote,
    counting, focus_ancestry_remote, navigation_step_remote, terminal_tail_remote,
    text_units_remote,
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
        let cache = uia.base_cache_request().expect("a cache request");
        let root = uia
            .element_from_handle(hwnd.0 as isize, &cache)
            .expect("mockapp's root element");
        Self {
            app,
            uia,
            root,
            hwnd,
        }
    }

    /// The element named `name`, built with the base cache request as a
    /// focus event's sender is.
    fn find(&self, name: &str) -> IUIAutomationElement {
        let cache = self.uia.base_cache_request().expect("a cache request");
        let value = VARIANT::from(BSTR::from(name));
        let condition = self
            .uia
            .property_condition(UIA_NamePropertyId, &value)
            .expect("a condition");
        self.root
            .find_first_build_cache(TreeScope_Descendants, &condition, &cache)
            .unwrap_or_else(|error| panic!("no element named {name:?}: {error}"))
            .unwrap_or_else(|| panic!("no element named {name:?}"))
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
    let list = fixture.find("Fruits");
    let previous = fixture.find("Banana");
    let known: Vec<Vec<i32>> = (0..64).map(|index| vec![42, 7, index]).collect();
    let query = FocusQuery {
        element: &list,
        known: &known,
        previous: Some(&previous),
        depth_limit: 64,
        properties: CACHED_PROPERTIES,
        deadline: None,
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

    let step = |element: &IUIAutomationElement, direction| {
        let query = StepQuery {
            element,
            direction,
            properties: CACHED_PROPERTIES,
        };
        let step = navigation_step_remote(&fixture.uia, &query).expect("the program runs");
        assert_eq!(step.window, Some(fixture.hwnd.0 as isize));
    };
    let item = fixture.find("Apple");
    let far = counted(|| step(&item, NavigationDirection::Parent));
    let outer = fixture.find("Group 1");
    let near_step = counted(|| step(&outer, NavigationDirection::FirstChild));
    fixture.app.quit();

    assert_eq!(
        [worst, limited, typical, far, near_step],
        [2180, 1686, 1038, 875, 144],
        "focus ancestry worst, at depth limit 30, typical; navigation far and near"
    );
    assert!(worst < LIMIT / 2 && far < LIMIT / 2);
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

/// A terminal's tail: an anchor whose fingerprint is nowhere, under 80
/// lines that hold its line before as part of their text, so the search
/// checks its 64 matches and gives up; and, typically, an anchor in place
/// under new output.
fn terminal_tails_execute_exactly() {
    common::init_com();
    let mut fixture = Fixture::start("terminal.json", "mockapp-instructions-terminal");
    let terminal = fixture.find("Terminal");
    let pattern = verbatim_uia::text::text_pattern(&terminal)
        .expect("a text pattern")
        .0;
    let lines = numbered(80);
    common::apply(
        &mut fixture.app,
        fixture.hwnd,
        &format!("set-text term {lines}ready>"),
    );
    let document = pattern.document_range().expect("the document range");
    let fresh = TailQuery {
        start: TailStart::Document(&document),
        lines_wanted: 30,
    };
    let last = terminal_tail_remote(&fixture.uia, &fresh)
        .expect("the program runs")
        .last;
    let decoys: String = (0..90).map(|_| r"x ready>\n").collect();
    common::apply(
        &mut fixture.app,
        fixture.hwnd,
        &format!(r"set-text term {decoys}one\ntwo"),
    );
    let anchored = |line, previous| TailQuery {
        start: TailStart::Anchor {
            range: &last,
            fingerprint: Fingerprint { line, previous },
        },
        lines_wanted: 30,
    };
    let worst = counted(|| {
        let tail = terminal_tail_remote(&fixture.uia, &anchored("ready>", "ready>\n"))
            .expect("the program runs");
        assert_eq!(tail.found, Found::NotFound);
    });
    common::apply(
        &mut fixture.app,
        fixture.hwnd,
        &format!(r"set-text term {lines}ready> ls\na\nb"),
    );
    let typical = counted(|| {
        let tail = terminal_tail_remote(&fixture.uia, &anchored("ready>", "line 079\n"))
            .expect("the program runs");
        assert_eq!(tail.found, Found::AtAnchor);
    });
    fixture.app.quit();
    assert_eq!(
        [worst, typical],
        [1128, 101],
        "terminal tail worst and typical"
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
    harness::run(&[
        (
            "the_instruction_limit_is_measured",
            the_instruction_limit_is_measured,
        ),
        (
            "focus_and_navigation_execute_exactly",
            focus_and_navigation_execute_exactly,
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
            "terminal_tails_execute_exactly",
            terminal_tails_execute_exactly,
        ),
    ]);
}
