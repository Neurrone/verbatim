//! The fallback rule of the request list's program builder
//! (`target/captures-2026-11-20/build8-design/design.md`, section 5, "The
//! fallback rule, per operation"), each failure made at a fault point in
//! `verbatim_uia_rops::batch` before the real run, against mockapp's
//! provider: a read failing before review select's act, so that the act is
//! made once, in the rerun; one failing after it, so that it is not made
//! again; a program holding the act whose answer is lost, so that its steps
//! rerun, the act made twice; a run stopped partway, its unreached steps
//! rerun; and a run past the instruction limit, logged as a defect and not
//! run again. Each outcome is the one the calls alone answer, and every
//! rerun step is logged as a fallback.

#[path = "common/batch.rs"]
mod batch;
mod common;
#[path = "common/harness.rs"]
mod harness;

use std::sync::{Arc, Mutex};

use batch::{Text, describe};
use verbatim_faults::Fault;
use verbatim_uia::{Live, Node, Shape, Uia};
use verbatim_uia_rops::TextFrom;
use verbatim_uia_rops::batch::{Ends, Fused, Point, Segment, run_batch, run_calls};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{TreeScope_Descendants, UIA_NamePropertyId};
use windows::core::BSTR;

/// The targets of the log lines a run wrote.
#[derive(Default)]
struct Targets(Mutex<Vec<String>>);

impl tracing::Subscriber for Targets {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        let target = event.metadata().target();
        if target.starts_with("verbatim::") {
            self.0.lock().expect("the log").push(target.to_owned());
        }
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

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

    fn segment<'a>(&'a self, steps: Vec<Fused<'a>>) -> Segment<'a> {
        Segment {
            uia: &self.uia,
            steps,
            remote: true,
            window: None,
        }
    }

    /// `steps` run as programs, with `fault` armed on step `place`, from
    /// the caret `reset` sets: they answer `expected`, or as the calls
    /// alone do from the same caret when it is `None`; the provider selects
    /// `selects` times, and the run logs these lines.
    fn fails(
        &mut self,
        (fault, place): (Fault, u8),
        (reset, expected): (&str, Option<&str>),
        (selects, logged): (u32, &[&str]),
        steps: &dyn Fn(&Text) -> Vec<Fused<'_>>,
    ) {
        common::apply(&mut self.app, self.hwnd, reset);
        common::reset_hits(self.hwnd);
        let targets = Arc::new(Targets::default());
        let ran = {
            let armed = verbatim_faults::arm(fault, move |subject: &u8| *subject == place, 1);
            let ran = tracing::subscriber::with_default(Arc::clone(&targets), || {
                run_batch(&self.segment(steps(&self.text)))
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
                    &run_calls(&self.segment(steps(&self.text))),
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
fn review_select(text: &Text) -> Vec<Fused<'_>> {
    batch::review_select(
        text,
        Ends {
            start: Point::From(TextFrom::Start),
            end: Point::From(TextFrom::End),
            through: None,
        },
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
    let whole = r#"caret (0, 0) Edges { start: true, end: false } selection Some(("(0, 17)", Edges { start: true, end: false }, Edges { start: false, end: true }))"#;
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
        &|text| {
            batch::starting_read(
                text,
                windows::Win32::UI::Accessibility::TextUnit_Word,
                true,
                false,
            )
        },
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
        batch::starting_read(
            text,
            windows::Win32::UI::Accessibility::TextUnit_Word,
            false,
            false,
        )
    });
    fixture.app.quit();
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
    ]);
}
