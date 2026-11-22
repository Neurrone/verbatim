//! A trimmed read's text converted in one pass ([`super::unpadded_utf16`])
//! exactly as with two copies, and a point's place in it.

use verbatim_model::TerminalText;

use super::{trimmed_offset, unpadded_utf16};
use verbatim_term_sim::{Flavor, Query};

/// What the outpost made of a read's UTF-16 text before it was converted
/// in one pass: a copy as UTF-8, then a copy of that without the padding.
fn two_copies(units: &[u16]) -> TerminalText {
    TerminalText::unpadded(&String::from_utf16_lossy(units))
}

/// `units` converted in one pass, checked against the two copies field by
/// field and as the bytes sent.
fn converted_alike(units: &[u16], what: &str) {
    let one_pass = super::unpadded_utf16(units);
    let expected = two_copies(units);
    assert_eq!(one_pass, expected, "{what}");
    assert_eq!(
        serde_json::to_vec(&one_pass).expect("serializes"),
        serde_json::to_vec(&expected).expect("serializes"),
        "{what}"
    );
}

/// Every corpus stream's whole document and screen, after each chunk, in
/// both terminals, converts in one pass exactly as with two copies.
#[test]
fn a_reads_text_converted_in_one_pass_is_the_two_copies_exactly() {
    use verbatim_term_sim::conformance::{history, streams};
    for (name, stream) in streams() {
        for flavor in [Flavor::ConsoleHost, Flavor::WindowsTerminal] {
            let mut terminal = verbatim_term_sim::Terminal::new(flavor, 120, 30, history(flavor));
            for (chunk, bytes) in stream.chunks.iter().enumerate() {
                terminal.write(bytes.as_bytes());
                let snapshot = terminal
                    .snapshot(&Query {
                        start: None,
                        screen_rows: 30,
                        whole: true,
                        above: 0,
                    })
                    .expect("a simulated read");
                for (part, text) in [("text", &snapshot.text), ("screen", &snapshot.screen)] {
                    let units: Vec<u16> = text.encode_utf16().collect();
                    converted_alike(
                        &units,
                        &format!("{name} in {flavor:?}, chunk {chunk}, {part}"),
                    );
                }
            }
        }
    }
}

/// Unpaired surrogates become U+FFFD as with two copies, wherever they
/// are, and line breaks, lone carriage returns and line feeds, padding and
/// other white space are kept or left out alike.
#[test]
fn unpaired_surrogates_and_line_ends_convert_as_the_two_copies() {
    let cases: [&[u16]; 14] = [
        &[],
        &[0xD800],
        &[0xDC00, 0x61],
        &[0x61, 0xD83D, 0x20, 0x20, 0x0D, 0x0A, 0x62],
        &[0x61, 0x20, 0xD800, 0x20, 0x0D, 0x0A],
        &[0xD83D, 0xDE00, 0x20, 0x20, 0x0D, 0x0A, 0x20, 0x20],
        &[0x0D],
        &[0x20, 0x0D],
        &[0x0D, 0x0D, 0x0A, 0x0A],
        &[0x61, 0x20, 0x0A, 0x20, 0x0D, 0x0A],
        &[0x0D, 0x0A, 0x0D, 0x0A],
        &[0x61, 0x09, 0x20, 0xA0, 0x20, 0x0D, 0x0A, 0x62, 0x20],
        &[0x20, 0x20, 0x20, 0xDBFF, 0x0D, 0x20, 0x0A],
        &[
            0x6F, 0x6E, 0x65, 0x20, 0x20, 0x0D, 0x0A, 0x74, 0x77, 0x6F, 0x20, 0x20,
        ],
    ];
    for (case, units) in cases.iter().enumerate() {
        converted_alike(units, &format!("case {case}: {units:04X?}"));
    }
}

fn utf16_of(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

/// A point keeps its line and column, and one in a line's padding
/// comes back to the line's end without it.
#[test]
fn a_point_keeps_its_line_and_column_within_the_text() {
    let units = utf16_of("ab   \r\nc d  \r\n");
    let trimmed = unpadded_utf16(&units);
    assert_eq!(trimmed.text, "ab\r\nc d\r\n");
    assert_eq!(trimmed.lengths, vec![(2, 5), (1, 0)]);
    assert_eq!(trimmed_offset(&units, 0, &trimmed.text), 0);
    assert_eq!(trimmed_offset(&units, 1, &trimmed.text), 1);
    assert_eq!(trimmed_offset(&units, 4, &trimmed.text), 2);
    assert_eq!(trimmed_offset(&units, 7, &trimmed.text), 4);
    assert_eq!(trimmed_offset(&units, 9, &trimmed.text), 6);
    assert_eq!(trimmed_offset(&units, 11, &trimmed.text), 7);
    assert_eq!(trimmed_offset(&units, 14, &trimmed.text), 9);
}
