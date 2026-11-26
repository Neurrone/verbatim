//! A focused terminal's command search, the one terminal operation the
//! outpost answers outside a batch, kept until the command marks' journal
//! replaces it (Dickson, 2026-11-26); public so mockapp's tests drive it as
//! the worker does. A terminal's reads and history reads are batches of
//! the request list's generic steps, which Core's terminal reader builds
//! (`books/dev/src/explanation/design/terminal-model.md`, "Where it
//! runs").

use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, IUIAutomationTextPattern, IUIAutomationTextRange,
};

use verbatim_uia::Uia;
use verbatim_uia_rops::{Error as RopsError, FindQuery, Path, terminal_find};

use crate::text::TextError;

/// Whether a failed remote operation or classic read means the terminal
/// is gone.
fn is_gone(error: &RopsError) -> bool {
    error
        .hresult()
        .is_some_and(|code| verbatim_uia::element_is_gone(&windows::core::Error::from(code)))
}

/// The error a failed remote operation or classic read is.
fn gone_or_failed(error: &RopsError) -> TextError {
    if is_gone(error) {
        TextError::Gone
    } else {
        TextError::Failed(error.to_string())
    }
}

/// What [`find`] found: the matches, and which path answered.
pub struct Found {
    /// The matches.
    pub matches: Vec<verbatim_model::TerminalMatch>,
    /// Which implementation answered.
    pub path: Path,
}

/// Searches a terminal's text for a command mark's line, as Core asks
/// ([`verbatim_model::TextOp::TerminalFind`]): `needle` outward from the
/// row `from` rows above `start`, the start anchor Core named, reading
/// `rows` rows from each match, in one remote operation. Core decides
/// which match, if any, is the mark's line.
///
/// # Errors
///
/// [`TextError::Failed`] when a call before the search failed.
pub fn find(
    uia: &Uia,
    (element, pattern): (&IUIAutomationElement, &IUIAutomationTextPattern),
    start: &IUIAutomationTextRange,
    (needle, from, rows): (&str, i32, u32),
    remote: bool,
) -> Result<Found, TextError> {
    let query = FindQuery {
        element,
        pattern,
        start,
        needle,
        from,
        rows,
    };
    let (found, path) =
        terminal_find(uia, &query, remote).map_err(|error| gone_or_failed(&error))?;
    Ok(Found {
        matches: found
            .into_iter()
            .map(|found| verbatim_model::TerminalMatch {
                above: found.above,
                text: found.text,
            })
            .collect(),
        path,
    })
}

#[cfg(test)]
mod tests;
