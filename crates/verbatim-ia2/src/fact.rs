//! The fact a `WinEvent` makes, the same whether the focus listener's hook
//! or an outpost's own took it in.

use verbatim_model::protocol::DeliveredFact;

use crate::WinEventKind;

/// The fact an MSAA `WinEvent` of `kind` at the address `(hwnd, id_object,
/// id_child)` makes, whether the listener's hook or an outpost's own took it
/// in: a focus, a foreground change, a menu opening, an alert, or a tooltip
/// shown. `None` for any other kind, and for an event with no window, which
/// neither routes nor reads.
#[must_use]
pub fn fact_of_win_event(
    kind: WinEventKind,
    (hwnd, id_object, id_child): (isize, i32, i32),
) -> Option<DeliveredFact> {
    if hwnd == 0 {
        return None;
    }
    Some(match kind {
        WinEventKind::Focus => DeliveredFact::MsaaFocus {
            hwnd,
            id_object,
            id_child,
        },
        // Taken unchecked: whether the window is still the foreground
        // window is checked when the fact is handled, as NVDA checks it
        // when it processes the event rather than in its event callback.
        WinEventKind::Foreground => DeliveredFact::Foreground { hwnd },
        WinEventKind::MenuPopupStart => DeliveredFact::MenuPopup {
            hwnd,
            id_object,
            id_child,
        },
        WinEventKind::Alert => DeliveredFact::Alert {
            hwnd,
            id_object,
            id_child,
        },
        WinEventKind::Show => DeliveredFact::Show {
            hwnd,
            id_object,
            id_child,
        },
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_five_win_event_kinds_with_a_window_make_a_fact() {
        for kind in [
            WinEventKind::Focus,
            WinEventKind::Foreground,
            WinEventKind::MenuPopupStart,
            WinEventKind::Alert,
            WinEventKind::Show,
        ] {
            let fact = fact_of_win_event(kind, (7, -4, 0))
                .unwrap_or_else(|| panic!("{kind:?} makes a fact"));
            assert!(fact.is_win_event(), "{kind:?}");
            assert_eq!(fact_of_win_event(kind, (0, -4, 0)), None);
        }
        assert_eq!(
            fact_of_win_event(WinEventKind::NameChange, (7, -4, 0)),
            None
        );
    }
}
