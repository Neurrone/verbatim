//! The ids a request of the event queue names, held while it waits or is
//! out (design, "Held anchors").

use verbatim_model::batch::{Batch, CaretWait, EffectTest, Op, Placement, Step, Subject};
use verbatim_model::{TextAnchor, TextDetails, TextPoint, TextPosition};

use super::named_by;
use crate::entries::tests::node;

fn at(anchor: u64) -> TextPoint {
    TextPoint::At(TextPosition::at(TextAnchor(anchor)))
}

/// A batch of the one operation `op`.
fn one(op: Op) -> verbatim_model::request::Question {
    verbatim_model::request::Question::Batch(Batch {
        steps: vec![Step { op, when: None }],
        placement: Placement::Queue,
        spare: None,
    })
}

#[test]
fn a_wait_and_a_sentence_move_name_their_starting_point() {
    let wait = Op::WaitForCaret {
        subject: Subject::Node(node(2)),
        wait: Box::new(CaretWait {
            from: TextAnchor(50),
            line: String::new(),
            offset: 0,
            word: None,
            effect: EffectTest {
                unit: false,
                line: false,
            },
            passed_at_us: 0,
            poll_us: 10_000,
            bound_us: 100_000,
            events: true,
        }),
    };
    assert_eq!(named_by(&one(wait)), [(node(2), vec![TextAnchor(50)])]);
    let moving = Op::MoveBySentence {
        subject: Subject::Node(node(2)),
        from: TextAnchor(51),
        forward: true,
        details: TextDetails::ALL,
    };
    assert_eq!(named_by(&one(moving)), [(node(2), vec![TextAnchor(51)])]);
}

#[test]
fn a_selection_names_both_ends_and_both_points_past_them() {
    let point = |anchor| verbatim_model::batch::Point::At(at(anchor));
    let select = Op::Select {
        subject: Subject::Node(node(2)),
        start: point(1),
        end: point(2),
        through: Some(verbatim_model::batch::Through {
            start: point(3),
            end: point(4),
        }),
    };
    assert_eq!(
        named_by(&one(select)),
        [(
            node(2),
            vec![TextAnchor(1), TextAnchor(2), TextAnchor(3), TextAnchor(4)]
        )]
    );
    let to_caret = Op::Select {
        subject: Subject::Node(node(2)),
        start: verbatim_model::batch::Point::At(TextPoint::Caret),
        end: verbatim_model::batch::Point::At(TextPoint::Caret),
        through: None,
    };
    assert_eq!(named_by(&one(to_caret)), [(node(2), Vec::new())]);
}
