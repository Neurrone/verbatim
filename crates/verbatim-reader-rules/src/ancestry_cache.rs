//! The title check a focus reader makes before it reuses its ancestry
//! cache, its saved ancestors from its last read in the application, to
//! stop a later walk early. The focus listener's reading thread and an
//! outpost's own reads each keep one, and each saves, with it, the window
//! text of every window entry it holds, read by `InternalGetWindowText`, a
//! local call that sends the application nothing. Before reusing it, the
//! reader reads those texts again: any change, or a window gone, means a
//! live walk to the top or the process boundary, which refreshes the
//! cache. The check only saves live re-reads: Core's check before speaking
//! is what keeps a cached ancestor from being spoken, so a title the check
//! cannot see change costs a live re-read, never a wrong word
//! (`books/dev/src/reference/crates/verbatim-core.md`, "The ancestry
//! cache").

/// A saved window entry of an ancestry cache: the window and its text when
/// the cache was saved.
pub type SavedTitle = (isize, Option<String>);

/// A window's text as read now, for the title check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LiveTitle {
    /// The window exists, with this text, `None` for none.
    Text(Option<String>),
    /// The window no longer exists.
    Gone,
}

/// What a focus reader does with its ancestry cache.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CacheUse {
    /// Every saved title is unchanged: the walk may stop at the cache.
    Reuse,
    /// A title changed or a window is gone: the walk reads live to the top
    /// or the process boundary.
    WalkLive,
}

/// The title check of an ancestry cache whose window entries were saved as
/// `saved`, each window's text now read by `live`: [`CacheUse::Reuse`]
/// exactly when every window exists and reads as it was saved.
pub fn title_check(saved: &[SavedTitle], live: impl Fn(isize) -> LiveTitle) -> CacheUse {
    if saved
        .iter()
        .all(|(window, text)| live(*window) == LiveTitle::Text(text.clone()))
    {
        CacheUse::Reuse
    } else {
        CacheUse::WalkLive
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved() -> Vec<SavedTitle> {
        vec![
            (1, Some("A".to_owned())),
            (2, Some("B".to_owned())),
            (3, Some("C".to_owned())),
        ]
    }

    fn reads(texts: [Option<&'static str>; 3]) -> impl Fn(isize) -> LiveTitle {
        move |window| match texts[usize::try_from(window - 1).unwrap()] {
            Some(text) => LiveTitle::Text(Some(text.to_owned())),
            None => LiveTitle::Gone,
        }
    }

    #[test]
    fn an_ancestry_cache_whose_titles_all_match_is_used() {
        assert_eq!(
            title_check(&saved(), reads([Some("A"), Some("B"), Some("C")])),
            CacheUse::Reuse
        );
    }

    #[test]
    fn a_changed_title_walks_to_the_top() {
        assert_eq!(
            title_check(&saved(), reads([Some("A"), Some("B2"), Some("C")])),
            CacheUse::WalkLive
        );
    }

    #[test]
    fn a_gone_window_walks_to_the_top() {
        assert_eq!(
            title_check(&saved(), reads([Some("A"), None, Some("C")])),
            CacheUse::WalkLive
        );
    }
}
