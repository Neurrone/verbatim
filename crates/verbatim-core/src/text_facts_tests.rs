//! The ids a request of the event queue names, held while it waits or is
//! out (design, "Held anchors").

use verbatim_model::request::Request as Wire;
use verbatim_model::{
    CaretMotion, SentenceMove, TextAnchor, TextDetails, TextOp, TextPoint, TextPosition,
    TextProvider, TextThrough, Watch, WatchFinalRead,
};

use super::named_by;
use crate::entries::tests::node;

fn at(anchor: u64) -> TextPoint {
    TextPoint::At(TextPosition::at(TextAnchor(anchor)))
}

fn on_node(op: TextOp) -> Wire {
    Wire::Text { node: node(2), op }
}

#[test]
fn a_watch_and_a_sentence_move_name_their_starting_point() {
    let watch = TextOp::Watch(Watch {
        provider: TextProvider::Uia,
        point: TextAnchor(50),
        motion: CaretMotion::NextCharacter,
        passed_at_us: 0,
        poll_us: 10_000,
        events: true,
        bound_us: 100_000,
        final_read: WatchFinalRead::Unit {
            unit: CaretMotion::NextCharacter.unit(),
            details: TextDetails::ALL,
        },
    });
    assert_eq!(
        named_by(&verbatim_model::request::Question::from(on_node(watch))),
        [(node(2), vec![TextAnchor(50)])]
    );
    let moving = TextOp::MoveBySentence(SentenceMove {
        provider: TextProvider::EditMessages,
        point: TextAnchor(51),
        forward: true,
        details: TextDetails::ALL,
    });
    assert_eq!(
        named_by(&verbatim_model::request::Question::from(on_node(moving))),
        [(node(2), vec![TextAnchor(51)])]
    );
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
