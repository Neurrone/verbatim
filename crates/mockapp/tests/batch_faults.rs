//! The fallback rule of the request list's program builder (the
//! verbatim-uia-rops crate guide, "The program builder and the
//! ordinary-calls executor"), each failure made at a fault point in
//! `verbatim_uia_rops::batch` before the real run, against mockapp's
//! provider: a read failing before review select's act, so that the act is
//! made once, in the rerun; one failing after it, so that it is not made
//! again; a program holding the act whose answer is lost, so that its steps
//! rerun, the act made twice; a run stopped partway, its unreached steps
//! rerun; and a run past the instruction limit, logged as a defect and not
//! run again. Each outcome is the one the calls alone answer, and every
//! rerun step is logged as a fallback. A colour read whose rows its
//! window read's program read and failed at, or whose colour program
//! going on from them failed, is read classically with no second try of
//! the programs, and logged once.

#[path = "common/batch.rs"]
mod batch;
mod common;
#[path = "common/harness.rs"]
mod harness;

use std::sync::Arc;

use batch::{Targets, Text, describe, named};
use verbatim_faults::Fault;
use verbatim_model::{TextPoint, TextUnit};
use verbatim_uia::{Live, Node, Shape as _, Uia};
use verbatim_uia_rops::batch::{run_batch, run_calls};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{TreeScope_Descendants, UIA_NamePropertyId};
use windows::core::BSTR;

/// mockapp's `text.json`, its text, and a client reading it.
struct Fixture {
    app: common::MockApp,
    uia: Uia,
    hwnd: HWND,
    text: Text,
}

impl Fixture {
    fn start() -> Self {
        common::init_com();
        let title = common::unique_title("mockapp-batch-faults");
        let app = common::spawn("text.json", "uia", &title);
        let hwnd = common::find_window(&title);
        let uia = Uia::new().expect("a UIA client");
        let root = uia.window_live(hwnd.0 as isize).expect("the root");
        let request = uia
            .request::<Node>(Node::all_on())
            .expect("a cache request");
        let condition = uia
            .property_condition(UIA_NamePropertyId, &VARIANT::from(BSTR::from("Notes")))
            .expect("a condition");
        let notes = Live::new(root.element().clone())
            .find_first(TreeScope_Descendants, &condition, &request)
            .expect("a search")
            .expect("the text");
        let text = Text::of(notes.live().element().clone());
        Self {
            app,
            uia,
            hwnd,
            text,
        }
    }

    /// `shape` run as programs, with `fault` armed on step `place`, from
    /// the caret `reset` sets: they answer `expected`, or as the calls
    /// alone do from the same caret when it is `None`; the provider selects
    /// `selects` times, and the run logs these lines.
    fn fails(
        &mut self,
        (fault, place): (Fault, u8),
        (reset, expected): (&str, Option<&str>),
        (selects, logged): (u32, &[&str]),
        shape: &dyn Fn(&Text) -> batch::Shape<'_>,
    ) {
        common::apply(&mut self.app, self.hwnd, reset);
        common::reset_hits(self.hwnd);
        let targets = Arc::new(Targets::default());
        let ran = {
            let armed = verbatim_faults::arm(fault, move |subject: &u8| *subject == place, 1);
            let ran = tracing::subscriber::with_default(Arc::clone(&targets), || {
                run_batch(&shape(&self.text).segment(&self.uia))
            });
            drop(armed);
            ran
        };
        let selected = common::read_hits(self.hwnd)
            .into_iter()
            .find(|(method, _)| *method == "ITextRangeProvider::Select")
            .map_or(0, |(_, count)| count);
        let ran = describe(Some(&self.text), &ran);
        common::apply(&mut self.app, self.hwnd, reset);
        let expected = expected.map_or_else(
            || {
                describe(
                    Some(&self.text),
                    &run_calls(&shape(&self.text).segment(&self.uia)),
                )
            },
            str::to_owned,
        );
        assert_eq!(ran, expected, "{fault:?} at {place}: the steps' outcomes");
        assert_eq!(selected, selects, "{fault:?} at {place}: selections made");
        assert_eq!(
            *targets.0.lock().expect("the log"),
            logged,
            "{fault:?} at {place}: logged"
        );
    }
}

/// The review select of `batch::review_select`, from the text's start to its
/// end.
fn review_select(text: &Text) -> batch::Shape<'_> {
    batch::review_select(
        text,
        (&named(TextPoint::Start), &named(TextPoint::End)),
        None,
    )
}

const FALLBACK: &str = verbatim_model::REMOTE_FALLBACK_TARGET;

/// Review select's first caret read fails in the first program: the act,
/// not reached, is made once, in the rerun by calls from that read, which
/// reruns every step after it.
fn a_read_failing_before_the_act_makes_it_once() {
    let mut fixture = Fixture::start();
    let calls = ("caret doc 3", None);
    fixture.fails(
        (Fault::ProgramStepFails, 0),
        calls,
        (1, &[FALLBACK; 4]),
        &review_select,
    );
    fixture.app.quit();
}

/// The caret read after the act fails in the second program: only that
/// program's steps rerun, and the act is not made again.
fn a_read_failing_after_the_act_does_not_repeat_it() {
    let mut fixture = Fixture::start();
    let calls = ("caret doc 3", None);
    fixture.fails(
        (Fault::ProgramStepFails, 2),
        calls,
        (1, &[FALLBACK; 2]),
        &review_select,
    );
    fixture.app.quit();
}

/// The first program, holding the act, ran but its answer was lost, so no
/// status is known: its steps rerun by calls, the act made a second time,
/// and the second program runs as usual. The rerun's first caret read
/// finds the selection the program's act made, so the comparison finds no
/// change: the limit the design states for an act that ran before its
/// program failed.
fn a_lost_answer_reruns_the_act() {
    let mut fixture = Fixture::start();
    let whole = r#"caret (0, 0) selection Some(("(0, 17)", true))"#;
    let expected = format!(
        "{whole}\nselected\n{whole}\ncompared [] Some(\"alpha beta\\ngamma\\n\") Some(\"alpha beta\\ngamma\\n\")\n"
    );
    let rule = ("caret doc 3", Some(expected.as_str()));
    fixture.fails(
        (Fault::ProgramAnswerLost, 0),
        rule,
        (2, &[FALLBACK; 2]),
        &review_select,
    );
    fixture.app.quit();
}

/// A caret key's starting read stopped before its second step by an
/// instruction failing outside any try block: the first step's result
/// stands, and the steps the run did not reach rerun by calls.
fn a_run_stopped_partway_reruns_what_it_did_not_reach() {
    let mut fixture = Fixture::start();
    let calls = ("caret doc 8", None);
    fixture.fails(
        (Fault::ProgramStops, 1),
        calls,
        (0, &[FALLBACK; 2]),
        &|text| batch::starting_read(text, TextUnit::Word, true, false),
    );
    fixture.app.quit();
}

/// A run past the platform's limit is a defect of an emitter's declared
/// worst case: logged as one, never run again, its unreached steps rerun by
/// calls.
fn an_overrun_is_a_defect() {
    let mut fixture = Fixture::start();
    let logged = [verbatim_model::DEFECT_TARGET, FALLBACK, FALLBACK];
    let calls = ("caret doc 8", None);
    fixture.fails((Fault::ProgramOverruns, 1), calls, (0, &logged), &|text| {
        batch::starting_read(text, TextUnit::Word, false, false)
    });
    fixture.app.quit();
}

/// The window read of Core's terminal reader with the screen's colours,
/// of mockapp's terminal `text`.
fn coloured_read(text: &Text) -> batch::Shape<'_> {
    use verbatim_core::terminal::reader::{WindowAsk, WindowRead, window_batch};
    let ask = WindowAsk {
        read: WindowRead::First,
        colours: true,
        starting: false,
        output: false,
    };
    let mut shape = batch::Shape::of(Some(text));
    shape
        .steps
        .clone_from(&window_batch(batch::TEXT, &ask).0.batch().steps);
    shape.culture = false;
    shape
}

/// What a coloured read ran and answered: how many programs, the colour
/// rows, and the lines it logged.
type ColouredRead = (
    usize,
    Option<Vec<Vec<verbatim_uia_rops::ColourRun>>>,
    Vec<String>,
);

/// Runs `shape`, a coloured read, with `armed` in force.
fn read_colours(
    uia: &Uia,
    shape: &batch::Shape<'_>,
    armed: Option<verbatim_faults::Guard>,
) -> ColouredRead {
    use verbatim_uia_rops::batch::{Found, Ran};
    let targets = Arc::new(Targets::default());
    verbatim_uia_rops::counting::start();
    let answer =
        tracing::subscriber::with_default(Arc::clone(&targets), || run_batch(&shape.segment(uia)));
    let programs = verbatim_uia_rops::counting::stop().len();
    drop(armed);
    let rows = answer.steps.into_iter().last().and_then(|ran| match ran {
        Ran::Done(Found::Colours(rows)) => Some(rows),
        _ => None,
    });
    let logged = targets.0.lock().expect("the log").clone();
    (programs, rows, logged)
}

/// mockapp's terminal of `fixture`, with a client reading it.
fn terminal(fixture: &str, name: &str) -> (common::MockApp, HWND, Uia, Text) {
    common::init_com();
    let title = common::unique_title(name);
    let app = common::spawn(fixture, "uia", &title);
    let hwnd = common::find_window(&title);
    let uia = Uia::new().expect("a UIA client");
    let text = Text::of(common::named_element(&uia, hwnd, "Terminal"));
    (app, hwnd, uia, text)
}

/// A window read's program fails at its colour step, the rows read at the
/// size the text's last colour read found: the step is rerun classically,
/// the same rows as the programs read, with no colour program tried and
/// one line logged.
fn a_colour_step_failing_in_its_program_is_read_classically_once() {
    let (app, _, uia, text) = terminal("terminal-colours.json", "mockapp-faults-colour-step");
    let shape = coloured_read(&text);
    let first = read_colours(&uia, &shape, None);
    let again = read_colours(&uia, &shape, None);
    let colours = u8::try_from(shape.steps.len() - 1).expect("a few steps");
    let armed = verbatim_faults::arm(
        Fault::ProgramStepFails,
        move |place: &u8| *place == colours,
        1,
    );
    let failed = read_colours(&uia, &shape, Some(armed));
    app.quit();
    assert_eq!(first.1.as_ref().map(Vec::len), Some(4), "four rows");
    assert_eq!((first.0, &first.2), (2, &Vec::new()), "the first read");
    assert_eq!(again, (1, first.1.clone(), Vec::new()), "the next read");
    assert_eq!(
        failed,
        (1, first.1, vec![FALLBACK.to_owned()]),
        "the read whose colour step failed"
    );
}

/// The colour program that goes on from the rows a window read's program
/// read fails, on a screen whose every cell differs from its neighbours
/// (`terminal-checkered.json`, 9 rows of 120 cells): the colour read is
/// rerun classically, the same rows, with no further program and one line
/// logged.
fn a_colour_program_failing_after_the_reads_own_is_read_classically_once() {
    let (mut app, hwnd, uia, text) =
        terminal("terminal-checkered.json", "mockapp-faults-colour-program");
    common::apply(&mut app, hwnd, "screen term 10");
    let screen = vec!["x".repeat(120); 10].join(r"\n");
    common::apply(&mut app, hwnd, &format!("set-text term {screen}"));
    let shape = coloured_read(&text);
    let first = read_colours(&uia, &shape, None);
    let again = read_colours(&uia, &shape, None);
    let armed = verbatim_faults::arm(Fault::ColourProgramFails, |_: &usize| true, 1);
    let failed = read_colours(&uia, &shape, Some(armed));
    app.quit();
    let runs: Option<Vec<usize>> = first
        .1
        .as_ref()
        .map(|rows| rows.iter().map(Vec::len).collect());
    assert_eq!(runs, Some(vec![120; 9]), "every cell its own run");
    assert_eq!((first.0, &first.2), (3, &Vec::new()), "the first read");
    assert_eq!(again, (2, first.1.clone(), Vec::new()), "the next read");
    assert_eq!(
        failed,
        (1, first.1, vec![FALLBACK.to_owned()]),
        "the read whose colour program failed"
    );
}

fn main() {
    harness::run_isolated(&[
        (
            "a_read_failing_before_the_act_makes_it_once",
            a_read_failing_before_the_act_makes_it_once,
        ),
        (
            "a_read_failing_after_the_act_does_not_repeat_it",
            a_read_failing_after_the_act_does_not_repeat_it,
        ),
        ("a_lost_answer_reruns_the_act", a_lost_answer_reruns_the_act),
        (
            "a_run_stopped_partway_reruns_what_it_did_not_reach",
            a_run_stopped_partway_reruns_what_it_did_not_reach,
        ),
        ("an_overrun_is_a_defect", an_overrun_is_a_defect),
        (
            "a_colour_step_failing_in_its_program_is_read_classically_once",
            a_colour_step_failing_in_its_program_is_read_classically_once,
        ),
        (
            "a_colour_program_failing_after_the_reads_own_is_read_classically_once",
            a_colour_program_failing_after_the_reads_own_is_read_classically_once,
        ),
    ]);
}
