//! The text and SSML Verbatim's synthesizers are given: one escape and one
//! writer, each driver passing how it gives a pitch change.
//!
//! U+0001 is deleted from all text, as NVDA's eSpeak NG driver deletes it:
//! eSpeak NG reads U+0001 as the start of an embedded command, so text
//! such as a window title holding it followed by a V would switch the
//! voice mid-utterance (`docs/nvda/synth-drivers.md`).

use std::borrow::Cow;
use std::fmt::Write as _;

use crate::driver::SpeechItem;

/// The character eSpeak NG starts an embedded command with.
const EMBEDDED: char = '\u{1}';

/// `text` with U+0001 deleted, for text given to a synthesizer as it is.
#[must_use]
pub fn strip_embedded(text: &str) -> Cow<'_, str> {
    if text.contains(EMBEDDED) {
        Cow::Owned(text.replace(EMBEDDED, ""))
    } else {
        Cow::Borrowed(text)
    }
}

/// What [`escape`] does with the other control characters XML 1.0 does not
/// allow (all below U+0020 but tab, line feed and carriage return, and
/// U+FFFE and U+FFFF).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Controls {
    /// Passed as they are, for a synthesizer that reads them leniently.
    Keep,
    /// Each made a space, so one cannot make a strict parser refuse the
    /// whole utterance.
    Space,
}

/// `text` escaped for SSML, with U+0001 deleted ([`strip_embedded`]) and the
/// other control characters as `controls` says.
#[must_use]
pub fn escape(text: &str, controls: Controls) -> String {
    let text = strip_embedded(text);
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            '\t' | '\n' | '\r' => escaped.push(character),
            '\u{0}'..='\u{1f}' | '\u{fffe}' | '\u{ffff}' if controls == Controls::Space => {
                escaped.push(' ');
            }
            other => escaped.push(other),
        }
    }
    escaped
}

/// `items` as SSML, without a `speak` element around them: text escaped
/// ([`escape`]), each mark a `mark` element named by its number, and each
/// pitch change other than a return to the configured pitch a `prosody`
/// element whose pitch, in percent, `pitch_percent` gives for the change's
/// offset; a sound is the mixer's and is left out.
#[must_use]
pub fn ssml(
    items: &[SpeechItem],
    controls: Controls,
    pitch_percent: impl Fn(i32) -> i32,
) -> String {
    let mut ssml = String::new();
    let mut open = false;
    for item in items {
        match item {
            SpeechItem::Text(text) => ssml.push_str(&escape(text, controls)),
            SpeechItem::Mark(mark) => {
                let _ = write!(ssml, "<mark name=\"{}\"/>", mark.0);
            }
            SpeechItem::Pitch(offset) => {
                if open {
                    ssml.push_str("</prosody>");
                    open = false;
                }
                if *offset != 0 {
                    let _ = write!(ssml, "<prosody pitch=\"{}%\">", pitch_percent(*offset));
                    open = true;
                }
            }
            SpeechItem::Sound(_) => {}
        }
    }
    if open {
        ssml.push_str("</prosody>");
    }
    ssml
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::IndexMark;

    #[test]
    fn ssml_escapes_text_deletes_control_a_names_marks_and_gives_the_driver_s_pitch() {
        let items = [
            SpeechItem::Mark(IndexMark(3)),
            SpeechItem::Text("Tom & 'Jerry' <3\u{1}V\u{7}".to_owned()),
            SpeechItem::Pitch(30),
            SpeechItem::Text("B".to_owned()),
            SpeechItem::Pitch(0),
            SpeechItem::Mark(IndexMark(4)),
        ];
        assert_eq!(
            ssml(&items, Controls::Space, |offset| offset * 2),
            "<mark name=\"3\"/>Tom &amp; &apos;Jerry&apos; &lt;3V <prosody pitch=\"60%\">B</prosody><mark name=\"4\"/>"
        );
        assert_eq!(
            escape("\"a\"\u{1}\u{7}", Controls::Keep),
            "&quot;a&quot;\u{7}"
        );
        assert_eq!(strip_embedded("a \u{1}Vb <"), "a Vb <");
    }
}
