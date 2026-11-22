//! The outpost's text sources (milestone M4): UI Automation's
//! `TextPattern` ([`uia::UiaText`]), read through the program builder
//! ([`fused`]). An edit control's text is the MSAA executor's
//! (`verbatim_ia2::text`). The [`TextSource`] trait and the algorithms
//! over it are `verbatim_reader_rules::text`'s, re-exported here.

#![forbid(unsafe_code)]

mod color;
pub mod fused;
pub mod uia;

pub use verbatim_reader_rules::text::*;

/// A source's caret, line and selection read in one go, where it can: UI
/// Automation's program ([`fused::caret_line`]) when the outpost has a
/// client; `None` for an edit control, read by parts.
pub trait InOneGo: TextSource + Sized {
    /// The read, `None` where the source is read by parts.
    ///
    /// # Errors
    ///
    /// When the application does not answer or the node is gone.
    fn in_one_go(
        &mut self,
        uia: Option<&verbatim_uia::Uia>,
        node: verbatim_model::NodeId,
    ) -> TextResult<Option<CaretRead<Self::Pos>>>;
}

impl InOneGo for uia::UiaText {
    fn in_one_go(
        &mut self,
        uia: Option<&verbatim_uia::Uia>,
        node: verbatim_model::NodeId,
    ) -> TextResult<Option<CaretRead<uia::UiaPos>>> {
        match uia {
            Some(uia) => fused::caret_line(uia, (node, self)).map(Some),
            None => Ok(None),
        }
    }
}

impl InOneGo for verbatim_ia2::text::EditText {
    fn in_one_go(
        &mut self,
        _uia: Option<&verbatim_uia::Uia>,
        _node: verbatim_model::NodeId,
    ) -> TextResult<Option<CaretRead<u32>>> {
        Ok(None)
    }
}
