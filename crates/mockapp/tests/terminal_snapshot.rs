//! The terminal read program (`verbatim-uia-rops`'s `terminal_snapshot`,
//! `docs/design/terminal-model.md`, section 4.2) against mockapp's UIA text
//! provider, whose `set-text` command rewrites the terminal's text and whose
//! `screen` command puts only the text's last lines on screen. Every read is
//! taken by the remote program and by the classic implementation, which
//! must agree on every field, and each case pins the fields exactly: a
//! first read, a read from where the last one said to start with nothing
//! new, one with output written since, one whose start the text no longer
//! reaches, one asked for fewer rows than the screen holds, a read of the
//! whole text, and reads taking rows above their start, as many as asked
//! and as far as the document's start.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use verbatim_uia::{Node, NodeIdRegistry, Shape, Uia};
use verbatim_uia_rops::{
    Snapshot, SnapshotQuery, terminal_snapshot_classic, terminal_snapshot_remote,
};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{IUIAutomationElement, IUIAutomationTextRange};

/// mockapp's terminal's element.
fn terminal_element(uia: &Uia, hwnd: HWND) -> IUIAutomationElement {
    let nodes = uia
        .request::<Node>(Node::all_on())
        .expect("a cache request");
    let root = uia
        .window_element(hwnd.0 as isize, &nodes)
        .expect("mockapp's root element");
    let registry = NodeIdRegistry::new(Arc::new(AtomicU64::new(1)));
    let (tree, _) = uia
        .tree(&root, &nodes, &registry, 8, 64)
        .expect("mockapp's tree");
    let terminal = tree
        .children
        .iter()
        .find(|child| child.snapshot.name.as_deref() == Some("Terminal"))
        .expect("the terminal")
        .snapshot
        .id;
    registry
        .element_of(terminal)
        .and_then(|agile| agile.resolve().ok())
        .map(|kept| kept.live().element().clone())
        .expect("its element")
}

/// The fields a test checks, both paths' alike.
#[derive(Debug, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "the read program's independent answers"
)]
struct Fields {
    text: String,
    screen: String,
    from_start: bool,
    shrank: bool,
    at_document_start: bool,
    alternate: bool,
    first_row: String,
    caret_rows_to_end: u32,
    above: u32,
}

impl From<&Snapshot> for Fields {
    fn from(snapshot: &Snapshot) -> Self {
        Self {
            text: String::from_utf16_lossy(&snapshot.text),
            screen: String::from_utf16_lossy(&snapshot.screen),
            from_start: snapshot.from_start,
            shrank: snapshot.shrank,
            at_document_start: snapshot.at_document_start,
            alternate: snapshot.alternate,
            first_row: snapshot.first_row.clone(),
            caret_rows_to_end: snapshot.caret_rows_to_end,
            above: snapshot.above,
        }
    }
}

/// Reads both ways, checks they agree, and returns the fields and where
/// the next read starts, as the remote program found it.
fn both(uia: &Uia, query: &SnapshotQuery<'_>) -> (Fields, IUIAutomationTextRange) {
    let remote = terminal_snapshot_remote(uia, query).expect("the remote program runs");
    let classic = terminal_snapshot_classic(uia, query).expect("the classic reads run");
    let fields = Fields::from(&remote);
    assert_eq!(fields, Fields::from(&classic), "remote and classic differ");
    (
        fields,
        remote.next_start.expect("where the next read starts"),
    )
}

/// The fields of a read of the eight numbered lines and the prompt, the
/// screen its last four rows, the caret at the text's start, eight rows from
/// its end: `text` and the three flags as given.
fn eight_lines(text: &str, from_start: bool, at_document_start: bool) -> Fields {
    Fields {
        text: text.to_owned(),
        screen: "line 005\nline 006\nline 007\nready>".to_owned(),
        from_start,
        shrank: false,
        at_document_start,
        alternate: false,
        first_row: "line 000\n".to_owned(),
        caret_rows_to_end: 8,
        above: 0,
    }
}

/// `count` numbered lines from `first`, as `set-text` takes them.
fn numbered(numbers: std::ops::Range<u32>) -> String {
    use std::fmt::Write as _;
    numbers.fold(String::new(), |mut text, number| {
        let _ = write!(text, r"line {number:03}\n");
        text
    })
}

/// mockapp's terminal with its screen four rows high, showing the eight
/// numbered lines and the prompt.
fn eight_line_terminal(name: &str) -> (common::MockApp, HWND, Uia, IUIAutomationElement) {
    common::init_com();
    let title = common::unique_title(name);
    let mut app = common::spawn("terminal.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let uia = Uia::new().expect("a UIA client");
    let terminal = terminal_element(&uia, hwnd);
    common::apply(&mut app, hwnd, "screen term 4");
    common::apply(
        &mut app,
        hwnd,
        &format!(r"set-text term {}ready>", numbered(0..8)),
    );
    (app, hwnd, uia, terminal)
}

fn a_kept_start_is_used_until_the_text_clears_below_it_both_ways() {
    let (mut app, hwnd, uia, terminal) = eight_line_terminal("mockapp-snapshot-kept");
    let (pattern, _) = verbatim_uia::text::text_pattern(&terminal).expect("a text pattern");
    let first = SnapshotQuery {
        element: &terminal,
        pattern: &pattern,
        start: None,
        screen_rows: 0,
        whole: false,
        above: 0,
        caret: None,
    };
    // A first read starts at the screen's top.
    let (fields, next) = both(&uia, &first);
    assert_eq!(
        fields,
        eight_lines("line 005\nline 006\nline 007\nready>", false, false)
    );
    let again = SnapshotQuery {
        start: Some(&next),
        screen_rows: 4,
        ..first
    };
    // With nothing new, the start is exactly where the next read would
    // start, and is used.
    let (fields, next) = both(&uia, &again);
    assert_eq!(
        fields,
        eight_lines("line 005\nline 006\nline 007\nready>", true, false)
    );
    common::apply(
        &mut app,
        hwnd,
        &format!(r"set-text term {}ready> ls\na\nb\nready>", numbered(0..8)),
    );
    let since = SnapshotQuery {
        start: Some(&next),
        screen_rows: 4,
        ..first
    };
    // Output written since: the read runs from the old screen's top to the
    // end, past the screen.
    let (fields, next) = both(&uia, &since);
    assert_eq!(
        fields,
        Fields {
            text: "line 005\nline 006\nline 007\nready> ls\na\nb\nready>".to_owned(),
            screen: "ready> ls\na\nb\nready>".to_owned(),
            from_start: true,
            shrank: false,
            at_document_start: false,
            alternate: false,
            first_row: "line 000\n".to_owned(),
            caret_rows_to_end: 11,
            above: 0,
        }
    );
    common::apply(&mut app, hwnd, r"set-text term cleared\nready>");
    let cleared = SnapshotQuery {
        start: Some(&next),
        screen_rows: 4,
        ..first
    };
    // The text cleared below the kept start: the start is refused, and the
    // read starts where the next will.
    let (fields, _) = both(&uia, &cleared);
    assert_eq!(
        fields,
        Fields {
            text: "cleared\nready>".to_owned(),
            screen: "cleared\nready>".to_owned(),
            from_start: false,
            shrank: true,
            at_document_start: true,
            alternate: true,
            first_row: "cleared\n".to_owned(),
            caret_rows_to_end: 1,
            above: 0,
        }
    );
    app.quit();
}

fn a_read_with_no_start_takes_the_rows_asked_for_or_the_whole_text_both_ways() {
    let (app, _, uia, terminal) = eight_line_terminal("mockapp-snapshot-unkept");
    let (pattern, _) = verbatim_uia::text::text_pattern(&terminal).expect("a text pattern");
    let first = SnapshotQuery {
        element: &terminal,
        pattern: &pattern,
        start: None,
        screen_rows: 0,
        whole: false,
        above: 0,
        caret: None,
    };
    let fewer = SnapshotQuery {
        screen_rows: 2,
        ..first
    };
    // Asked for two rows with no start: the read starts two rows from the
    // end, not at the screen's top.
    let (fields, _) = both(&uia, &fewer);
    assert_eq!(fields, eight_lines("line 007\nready>", false, false));
    let whole = SnapshotQuery {
        whole: true,
        ..first
    };
    // The whole text from the document's start.
    let (fields, _) = both(&uia, &whole);
    app.quit();
    assert_eq!(
        fields,
        eight_lines(
            "line 000\nline 001\nline 002\nline 003\nline 004\nline 005\nline 006\nline 007\nready>",
            false,
            true
        )
    );
}

fn a_read_takes_the_rows_asked_for_above_its_start_as_far_as_the_documents_start_both_ways() {
    let (app, _, uia, terminal) = eight_line_terminal("mockapp-snapshot-above");
    let (pattern, _) = verbatim_uia::text::text_pattern(&terminal).expect("a text pattern");
    let first = SnapshotQuery {
        element: &terminal,
        pattern: &pattern,
        start: None,
        screen_rows: 0,
        whole: false,
        above: 0,
        caret: None,
    };
    let (_, next) = both(&uia, &first);
    let above = SnapshotQuery {
        start: Some(&next),
        screen_rows: 4,
        above: 2,
        ..first
    };
    // Two rows above the kept start, which is still used.
    let (fields, _) = both(&uia, &above);
    assert_eq!(
        fields,
        Fields {
            above: 2,
            ..eight_lines(
                "line 003\nline 004\nline 005\nline 006\nline 007\nready>",
                true,
                false
            )
        }
    );
    let past = SnapshotQuery { above: 20, ..above };
    // Asked for more rows than lie above the start: the read stops at the
    // document's start, five rows up.
    let (fields, _) = both(&uia, &past);
    app.quit();
    assert_eq!(
        fields,
        Fields {
            above: 5,
            ..eight_lines(
                "line 000\nline 001\nline 002\nline 003\nline 004\nline 005\nline 006\nline 007\nready>",
                true,
                true
            )
        }
    );
}

/// Runs this file's tests through the UIA test runner, which explains why
/// these binaries do not exit normally (`common/harness.rs`).
fn main() {
    harness::run_isolated(&[
        (
            "a_kept_start_is_used_until_the_text_clears_below_it_both_ways",
            a_kept_start_is_used_until_the_text_clears_below_it_both_ways,
        ),
        (
            "a_read_with_no_start_takes_the_rows_asked_for_or_the_whole_text_both_ways",
            a_read_with_no_start_takes_the_rows_asked_for_or_the_whole_text_both_ways,
        ),
        (
            "a_read_takes_the_rows_asked_for_above_its_start_as_far_as_the_documents_start_both_ways",
            a_read_takes_the_rows_asked_for_above_its_start_as_far_as_the_documents_start_both_ways,
        ),
    ]);
}
