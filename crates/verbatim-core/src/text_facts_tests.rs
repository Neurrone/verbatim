//! The ids a request of the event queue names, held while it waits or is
//! out (design, "Held anchors").

use verbatim_model::batch::{Batch, CaretWait, EffectTest, Op, Placement, Step, Subject};
use verbatim_model::request::Request as Wire;
use verbatim_model::{TextAnchor, TextDetails, TextOp, TextPoint, TextPosition, TextThrough};

use super::named_by;
use crate::entries::tests::node;

fn at(anchor: u64) -> TextPoint {
    TextPoint::At(TextPosition::at(TextAnchor(anchor)))
}

fn on_node(op: TextOp) -> Wire {
    Wire::Text { node: node(2), op }
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
    let select = TextOp::Select {
        start: at(1),
        end: at(2),
        through: Some(TextThrough {
            start: at(3),
            end: at(4),
        }),
    };
    assert_eq!(
        named_by(&verbatim_model::request::Question::from(on_node(select))),
        [(
            node(2),
            vec![TextAnchor(1), TextAnchor(2), TextAnchor(3), TextAnchor(4)]
        )]
    );
    assert_eq!(
        named_by(&verbatim_model::request::Question::from(on_node(
            TextOp::MoveCaret(TextPoint::Caret)
        ))),
        [(node(2), Vec::new())]
    );
}
