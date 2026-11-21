//! The request list's two executors on UI Automation subjects
//! (`verbatim_uia_rops::batch`), against mockapp's real, out-of-process
//! provider: every batch shape a conversion sends, run as generated
//! programs (`run_batch`) and call by call (`run_calls`), answers the same,
//! each program run counted as the one call it is; a shape run again binds
//! the program it built; and a property an element lacks reads absent on
//! both paths.

#[path = "common/batch.rs"]
mod batch;
mod common;
#[path = "common/harness.rs"]
mod harness;

use batch::{Text, after, describe, node_fill, read_text, step, units};
use verbatim_model::batch::{Relation, View};
use verbatim_uia::text::Endpoint;
use verbatim_uia::{AncestorStops, Cached, Live, Node, NodeIdRegistry, Shape, Uia, runtime_id};
use verbatim_uia_rops::batch::{
    CaretFound, Ends, Found, Fused, Input, Judged, Object, Point, Ran, Segment, run_batch,
    run_calls,
};
use verbatim_uia_rops::{Movement, Position, TextFrom, work};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, TextUnit_Character, TextUnit_Line, TextUnit_Paragraph, TextUnit_Word,
    TreeScope_Descendants, UIA_NamePropertyId,
};
use windows::core::BSTR;

/// A running mockapp and a client reading it.
struct Fixture {
    app: common::MockApp,
    uia: Uia,
    root: IUIAutomationElement,
    hwnd: HWND,
}

impl Fixture {
    fn start(fixture: &str, prefix: &str) -> Self {
        common::init_com();
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

    /// The element named `name`.
    fn find(&self, name: &str) -> IUIAutomationElement {
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
            .expect("a search")
            .unwrap_or_else(|| panic!("no element named {name:?}"))
            .live()
            .element()
            .clone()
    }

    fn apply(&mut self, line: &str) {
        common::apply(&mut self.app, self.hwnd, line);
    }

    fn segment<'a>(&'a self, steps: Vec<Fused<'a>>) -> Segment<'a> {
        Segment {
            uia: &self.uia,
            steps,
            remote: true,
            window: None,
        }
    }

    /// `steps` run as programs and by calls, each after `reset`, answering
    /// the same; the run as programs makes `calls_made` calls into
    /// mockapp, one for each program.
    fn agree<'s>(
        &mut self,
        name: &str,
        text: Option<&Text>,
        (reset, calls_made): (&str, u32),
        steps: &dyn Fn() -> Vec<Fused<'s>>,
    ) {
        if !reset.is_empty() {
            self.apply(reset);
        }
        let _ = verbatim_win32::calls::take();
        let remote = run_batch(&self.segment(steps()));
        let calls = verbatim_win32::calls::take().uia;
        let remote = describe(text, &remote);
        if !reset.is_empty() {
            self.apply(reset);
        }
        let classic = describe(text, &run_calls(&self.segment(steps())));
        assert_eq!(remote, classic, "{name}: the two executors differ");
        assert_eq!(calls, calls_made, "{name}: the calls made\n{remote}");
    }
}

/// A caret read at the caret `reset` sets, by calls.
fn caret_at(fixture: &mut Fixture, text: &Text, reset: &str) -> CaretFound {
    fixture.apply(reset);
    let read = run_calls(&fixture.segment(vec![step(
        0,
        Input::Caret {
            text: text.target(),
        },
    )]));
    let Some(Ran::Done(Found::Caret(caret))) = read.steps.into_iter().next() else {
        panic!("a caret read")
    };
    caret
}

/// The caret keys' starting reads, for each unit and for Backspace and
/// Delete, at the text's start, inside a word, at a line's start and at
/// the text's end, and a caret tell, on `text.json`'s "alpha beta",
/// "gamma".
fn caret_shapes_agree() {
    let mut fixture = Fixture::start("text.json", "mockapp-batch-caret");
    let text = Text::of(fixture.find("Notes"));
    for (unit, back, delete) in [
        (TextUnit_Character, false, false),
        (TextUnit_Word, false, false),
        (TextUnit_Line, false, false),
        (TextUnit_Paragraph, false, false),
        (TextUnit_Character, true, false),
        (TextUnit_Word, true, false),
        (TextUnit_Character, false, true),
    ] {
        for caret in ["caret doc 0", "caret doc 8", "caret doc 11", "caret doc 17"] {
            let name = format!("starting read {unit:?} back {back} delete {delete} at {caret}");
            fixture.agree(&name, Some(&text), (caret, 1), &|| {
                batch::starting_read(&text, unit, back, delete)
            });
        }
    }
    // A caret tell against the selection a read before it held.
    let before = caret_at(&mut fixture, &text, "caret doc 2");
    let baseline = Position {
        range: &before.caret,
        endpoint: Endpoint::Start,
        collapsed: true,
    };
    fixture.agree("caret tell", Some(&text), ("caret doc 6 10", 1), &|| {
        batch::caret_tell(&text, baseline)
    });
    fixture.app.quit();
}

/// Review's reads from a position found by its text, its words, say-all's
/// lines, a copy, a location, review select and say-all's caret move, on
/// `text.json`.
fn review_shapes_agree() {
    let mut fixture = Fixture::start("text.json", "mockapp-batch-review");
    let text = Text::of(fixture.find("Notes"));
    // Review's reads from a position held at "beta", and its words.
    let held = caret_at(&mut fixture, &text, "caret doc 2").caret;
    let prefix: Vec<u16> = "pha ".encode_utf16().collect();
    let counts = [4, 3];
    let at = after(&held, &prefix, &counts);
    for (name, movement, unit) in [
        (
            "next line",
            Some(Movement::By(TextUnit_Line, 1)),
            TextUnit_Line,
        ),
        (
            "previous character",
            Some(Movement::By(TextUnit_Character, -1)),
            TextUnit_Character,
        ),
        ("to the end", Some(Movement::Document(1)), TextUnit_Line),
        ("current word", None, TextUnit_Word),
    ] {
        fixture.agree(name, Some(&text), ("", 1), &|| {
            vec![step(0, read_text(&text, at, units(movement, unit, 1)))]
        });
    }
    fixture.agree("previous word", Some(&text), ("", 1), &|| {
        batch::previous_word(&text, at)
    });
    fixture.agree("next word", Some(&text), ("", 1), &|| {
        batch::next_word(&text, at)
    });
    fixture.agree("say-all's lines", Some(&text), ("", 1), &|| {
        vec![step(
            0,
            read_text(
                &text,
                Point::From(TextFrom::Start),
                units(None, TextUnit_Line, 20),
            ),
        )]
    });
    let selection = Ends {
        start: Point::From(TextFrom::SelectionStart),
        end: Point::From(TextFrom::SelectionEnd),
        through: None,
    };
    fixture.agree("copy", Some(&text), ("caret doc 6 13", 1), &|| {
        vec![step(
            0,
            Input::ReadRange {
                text: text.target(),
                ends: selection,
                max_text: batch::MAX_TEXT,
            },
        )]
    });
    fixture.agree("location", Some(&text), ("caret doc 6", 1), &|| {
        vec![step(
            0,
            Input::Locate {
                text: text.target(),
                at: Point::From(TextFrom::Caret),
            },
        )]
    });
    let review = Ends {
        start: at,
        end: Point::From(TextFrom::End),
        through: None,
    };
    // Review select and say-all's caret move each end a program at their
    // act: two programs.
    fixture.agree("review select", Some(&text), ("caret doc 0", 2), &|| {
        batch::review_select(&text, review)
    });
    fixture.agree("say-all's caret", Some(&text), ("caret doc 0", 2), &|| {
        batch::say_all_move(&text, at)
    });
    fixture.app.quit();
}

/// Object navigation, the controlled selection and a node read on
/// `controller.json`, and the listener's ancestors.
fn object_shapes_agree() {
    let mut fixture = Fixture::start("controller.json", "mockapp-batch-objects");
    let fill = node_fill(&fixture.uia);
    let display = fixture.find("Display settings");
    // From elements of the application's own: "System" would name the
    // title bar's menu, which UI Automation's proxies serve.
    for (relation, from) in [
        (Relation::Parent, "Sound settings"),
        (Relation::FirstChild, "Suggestions"),
        (Relation::LastChild, "Suggestions"),
        (Relation::NextSibling, "Search box"),
        (Relation::PreviousSibling, "Sound settings"),
    ] {
        let from = fixture.find(from);
        fixture.agree(&format!("navigation {relation:?}"), None, ("", 1), &|| {
            vec![step(
                0,
                Input::Related {
                    subject: held(&from),
                    relation,
                    fill: &fill,
                },
            )]
        });
    }
    fixture.agree("read", None, ("", 1), &|| {
        vec![step(
            0,
            Input::Read {
                subject: held(&display),
                fill: &fill,
            },
        )]
    });
    let target = runtime_id(&display).expect("a runtime id");
    let judge = |node: &Cached<Node>| Judged {
        matched: node.runtime_id() == target,
        pruned: false,
    };
    let search = fixture.find("Search box");
    fixture.agree("controlled selection", None, ("", 1), &|| {
        batch::controlled_selection(&search, &fill, &judge)
    });
    let registry = NodeIdRegistry::new(std::sync::Arc::default());
    let request = fixture
        .uia
        .request::<verbatim_uia::Ancestor>(common::ancestor_gates())
        .expect("a cache request");
    let stops = AncestorStops {
        read_by_other_api: &|_| false,
        known: &|_| false,
        out_of_time: None,
        visited: None,
    };
    // The ancestors run by calls on both paths, a call a hop: four to
    // mockapp's window, and the desktop's root above it.
    fixture.agree("ancestors", None, ("", 4), &|| {
        vec![step(
            0,
            Input::Ancestors(Box::new(verbatim_uia_rops::batch::AncestorsInput {
                subject: held(&display),
                nearest: 0,
                request: &request,
                registry: &registry,
                max_hops: 64,
                stops: &stops,
            })),
        )]
    });
    fixture.app.quit();
}

/// Walks down the tree: the tree dump's of `tree.json` and of `wide.json`,
/// whose hundred items pass one run's sixty, and the tray list's of the
/// control view, pruned.
fn walks_agree() {
    // The tree dump of `wide.json`: a program reads the root's child and
    // stops in the list's hundred items; a second finds they fill no run
    // alone, and one call reads them; two more read whether each of them
    // has children, sixty-four a run. The tray list's walk of the control view
    // runs by calls, a call a step, as today's tray walk does.
    for (fixture_name, programs, steps) in [("tree.json", 1, 59), ("wide.json", 5, 215)] {
        let mut fixture = Fixture::start(fixture_name, "mockapp-batch-walk");
        let fill = node_fill(&fixture.uia);
        let every = |_: &Cached<Node>| Judged {
            matched: true,
            pruned: false,
        };
        let root = fixture.root.clone();
        fixture.agree(
            &format!("tree dump of {fixture_name}"),
            None,
            ("", programs),
            &|| {
                vec![step(
                    0,
                    batch::descendants(
                        held(&root),
                        &fill,
                        &every,
                        (View::Raw, false, false, 4_096),
                    ),
                )]
            },
        );
        let shown = |node: &Cached<Node>| {
            let name = node.read().name.unwrap_or_default();
            Judged {
                matched: node.read().role.role() == verbatim_model::Role::Button,
                pruned: name == "Group Two",
            }
        };
        fixture.agree(
            &format!("tray walk of {fixture_name}"),
            None,
            ("", steps),
            &|| {
                vec![step(
                    0,
                    batch::descendants(
                        held(&root),
                        &fill,
                        &shown,
                        (View::Control, true, false, 1_024),
                    ),
                )]
            },
        );
        fixture.app.quit();
    }
}

/// A caret key's starting read run twice: the second binds the program
/// the first built and checked, building and checking nothing.
fn a_shape_run_again_binds_its_program() {
    let mut fixture = Fixture::start("text.json", "mockapp-batch-kept");
    let text = Text::of(fixture.find("Notes"));
    fixture.apply("caret doc 3");
    let first =
        run_batch(&fixture.segment(batch::starting_read(&text, TextUnit_Word, true, false)));
    let built = (work::programs_built(), work::support_checks());
    fixture.apply("caret doc 8");
    let second =
        run_batch(&fixture.segment(batch::starting_read(&text, TextUnit_Word, true, false)));
    assert_eq!((work::programs_built(), work::support_checks()), built);
    assert_ne!(
        describe(Some(&text), &first),
        describe(Some(&text), &second)
    );
    fixture.app.quit();
}

/// A node read whose element lacks properties a filled cache would hold
/// defaults for (a group: no value, no position, no level): the program's
/// read and the read by calls agree, the lacking ones absent on both.
fn absent_properties_agree() {
    let mut fixture = Fixture::start("tree.json", "mockapp-batch-absent");
    let fill = node_fill(&fixture.uia);
    let group = fixture.find("Group One");
    fixture.agree("absent", None, ("", 1), &|| {
        vec![step(
            0,
            Input::Read {
                subject: Object::Held {
                    element: &group,
                    window: 0,
                },
                fill: &fill,
            },
        )]
    });
    let read = run_batch(&fixture.segment(vec![step(
        0,
        Input::Read {
            subject: Object::Held {
                element: &group,
                window: 0,
            },
            fill: &fill,
        },
    )]));
    let Ran::Done(Found::Read(node)) = &read.steps[0] else {
        panic!("a read")
    };
    let parts = node.read().parts(node.runtime_id());
    assert_eq!(
        (
            parts.details.position_in_set,
            parts.details.level,
            parts.value
        ),
        (None, None, None)
    );
    fixture.app.quit();
}

/// An element the outpost holds, with no window of its own.
fn held(element: &IUIAutomationElement) -> Object<'_> {
    Object::Held { element, window: 0 }
}

fn main() {
    harness::run_isolated(&[
        ("caret_shapes_agree", caret_shapes_agree),
        ("review_shapes_agree", review_shapes_agree),
        ("object_shapes_agree", object_shapes_agree),
        ("walks_agree", walks_agree),
        (
            "a_shape_run_again_binds_its_program",
            a_shape_run_again_binds_its_program,
        ),
        ("absent_properties_agree", absent_properties_agree),
    ]);
}
