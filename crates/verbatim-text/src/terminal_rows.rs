//! The memory of a terminal's rows and the alignment of each read against
//! it (`docs/design/terminal-model.md`, section 4, "Changes as alignments
//! of rows"): what each row of a read is (new, kept and from where,
//! rewritten and from which row, a lingering row returned, a reprint),
//! where each row of the last read went, how far the history rotated, and
//! where the anchors (the review position and the command marks) are now.
//! This is classification, not speech: the reading rules of section 5 are
//! built on [`Outcome`].
//!
//! A read gives no coordinates, only text, so a row's position is its
//! index in the read that found it. Reads start where the last read said
//! to start, the top of the screen as last read, so a row that did not
//! move has a known shift between two reads (its index less the rows the
//! last read held above that start); the history's rotation shows as kept
//! rows shifted up from it.

use std::collections::VecDeque;
use std::ops::Range;

mod align;

use align::Likeness;

/// How long a gone row lingers (section 5.14), in milliseconds.
pub const LINGERING_MS: u64 = 2000;

/// The most row hashes the known history keeps: more than either
/// terminal's buffer holds by default (9,001 rows), so it is the
/// terminal's own history that bounds it in practice.
const KNOWN_LIMIT: usize = 32_768;

/// What one read of a terminal returned, as the read program gives it
/// (`verbatim_uia_rops::Snapshot`), with its text as UTF-8 and the time
/// of the read. The start for the next read is the outpost's to keep.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent answers of one read, as the read program gives them"
)]
pub struct Read {
    /// The text from the read's start to the document's end, padding and
    /// line breaks included.
    pub text: String,
    /// The screen's text, for its width and height.
    pub screen: String,
    /// Whether the read started where the last read said to start.
    pub from_start: bool,
    /// Whether that start was refused: the text shrank, or the screen
    /// switched.
    pub shrank: bool,
    /// Whether the read started at the document's start.
    pub at_document_start: bool,
    /// Whether the screen starts at the document's start: no history
    /// above it.
    pub alternate: bool,
    /// The document's first row.
    pub first_row: String,
    /// How many rows the caret's row and those below it hold, to the
    /// document's end; 0 when there is no caret.
    pub caret_rows_to_end: u32,
    /// The caret's row.
    pub caret_line: String,
    /// The caret's column in its row.
    pub caret_column: usize,
    /// When the read was taken, in milliseconds on the reader's clock.
    pub at_ms: u64,
}

/// A row of the memory: one row of the last read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// Its text, padding left out (section 5.12): only a line's last row
    /// has padding, so the earlier rows of a wrapped line keep the spaces
    /// at their end.
    pub text: String,
    /// A 64-bit hash of `text`.
    pub hash: u64,
    /// Whether its line continues onto the next row.
    pub wraps: bool,
    /// The hash of the whole line it belongs to, its rows joined.
    pub line_hash: u64,
    /// Whether it has been rewritten in place other than by growth, or
    /// taken back while lingering (section 5.2).
    pub redrawn: bool,
    /// When its text last changed, or it was first read.
    pub changed_ms: u64,
}

impl Row {
    /// Whether the row shows nothing: no text, or only the spaces of a
    /// wrapped line's row.
    #[must_use]
    pub fn is_blank(&self) -> bool {
        crate::trim_padding(&self.text).is_empty()
    }
}

/// A remembered place (sections 4.1, 5.6 and 5.13): the review position
/// or a command mark.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Anchor {
    /// Its row, counted from the last read's first row; negative above
    /// it, in the history.
    pub row: isize,
    /// Its column, in characters of the row's text.
    pub column: usize,
    /// The hash of its line.
    pub line_hash: u64,
}

/// What a row of a read is, against the memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NewRow {
    /// Not in the memory: new output, or a row whose text no old row had.
    New,
    /// An old row, its text unchanged.
    Kept {
        /// The old row's index.
        from: usize,
        /// Whether it moved other than with the history: down with the
        /// end of the text, or drawn again elsewhere by its program.
        moved: bool,
    },
    /// An old row rewritten with other text, paired by likeness.
    Rewritten {
        /// The old row's index.
        from: usize,
        /// Whether it moved other than with the history, as a status row
        /// drawn a row lower with its spinner changed.
        moved: bool,
    },
    /// A lingering row taken back: a row a read found gone, back with its
    /// text (section 4.1).
    Returned {
        /// Whether it came back elsewhere than where it was.
        moved: bool,
    },
    /// A row the known history held above the old screen, printed again
    /// after a clear (section 4.4).
    Reprint,
    /// A row above where the memory began, which the reads before never
    /// covered: history, not new output.
    Earlier,
}

/// What became of a row of the last read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OldRow {
    /// Kept, now the read's row at this index.
    Kept(usize),
    /// Rewritten, now the read's row at this index.
    Rewritten(usize),
    /// Gone: erased or written over.
    Gone {
        /// Whether it lingers (section 4.1).
        lingering: bool,
    },
    /// Above the read: it scrolled or rotated past the read's start.
    Above,
}

/// A switch of screens (section 5.8), or a clear that looks like one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Switch {
    /// No switch.
    #[default]
    None,
    /// The document became one screen tall with no history: the alternate
    /// screen opened, or the history was cleared. The memory before it is
    /// kept aside.
    Opened,
    /// The document grew back and matched the memory kept aside, which is
    /// restored.
    Closed,
}

/// A line of a read: rows joined while each wraps onto the next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    /// Its rows.
    pub rows: Range<usize>,
    /// Its text, padding left out.
    pub text: String,
    /// The hash of `text`.
    pub hash: u64,
    /// When the line only gained at its end since the last read, rows
    /// wrapped onto it included, and did not move (section 5.2,
    /// "Growing"): the length in bytes of what it was, so `text[from..]`
    /// is what it gained.
    pub grew_from: Option<usize>,
}

/// What the memory made of one read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent findings of one read"
)]
pub struct Outcome {
    /// No row of the last screen is in this read (or the read's start was
    /// refused), so the caller reads the whole document and gives that
    /// read instead (section 4.2). Nothing else is set, and the memory is
    /// unchanged.
    pub whole_document: bool,
    /// The memory's first read: every row is new, and nothing was heard
    /// before it.
    pub first: bool,
    /// What each row of the read is.
    pub rows: Vec<NewRow>,
    /// What became of each row of the last read.
    pub old: Vec<OldRow>,
    /// The read's lines.
    pub lines: Vec<Line>,
    /// How many rows the history rotated: how far the topmost kept row
    /// moved up from where it would be had nothing moved.
    pub rotation: usize,
    /// A whole-document read did not find the last screen in a document
    /// longer than a screen: the history overflowed, and more went by than
    /// it holds (section 4.3, item 5).
    pub overflowed: bool,
    /// A switch of screens.
    pub switch: Switch,
    /// The caret's row, when it is in the read.
    pub caret: Option<usize>,
    /// Whether the read's first row continues a line that starts above the
    /// read, so the first of [`Outcome::lines`] is that line's rest.
    pub continued: bool,
    /// The screen's width in cells.
    pub width: usize,
    /// The screen's height in rows.
    pub height: usize,
}

/// A row that lingers: gone, kept for a while with its history.
#[derive(Clone, Debug)]
struct Lingering {
    row: Row,
    /// When it was found gone.
    since_ms: u64,
    /// Where it would be now had it stayed, counted as a memory row is.
    at: isize,
}

/// One screen's memory.
#[derive(Clone, Debug, Default)]
struct Screen {
    rows: Vec<Row>,
    /// The screen's height at the last read.
    height: usize,
    at_document_start: bool,
    caret: Option<usize>,
    lingering: Vec<Lingering>,
    /// The hashes of the rows above `rows`, oldest first.
    known: VecDeque<u64>,
    review: Option<Anchor>,
    marks: Vec<Anchor>,
    /// Whether the first row continues a line that starts above it.
    continued: bool,
}

impl Screen {
    /// The rows from the next read's start: the screen as last read.
    fn screen_start(&self) -> usize {
        self.rows.len().saturating_sub(self.height)
    }

    fn has_text(rows: &[Row]) -> bool {
        rows.iter().any(|row| !row.is_blank())
    }
}

/// Why the memory asked for the whole document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Whole {
    /// The read's start was refused: the text shrank, as after a clear.
    Shrank,
    /// No row of the last screen was in the read.
    Lost,
}

/// The memory of one terminal (section 4.1), kept while the terminal
/// exists, across focus changes.
#[derive(Clone, Debug, Default)]
pub struct TerminalMemory {
    screen: Option<Screen>,
    /// Whether the last read's screen started at the document's start.
    alternate: bool,
    /// The main screen's memory, kept aside while the alternate screen is
    /// up.
    aside: Option<Screen>,
    /// Why the whole document was asked for, until it is read.
    pending: Option<Whole>,
}

impl TerminalMemory {
    /// An empty memory, before the terminal's first read.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The rows of the last read.
    #[must_use]
    pub fn rows(&self) -> &[Row] {
        self.screen.as_ref().map_or(&[], |screen| &screen.rows)
    }

    /// The hashes of the rows known above the last read, oldest first.
    #[must_use]
    pub fn known_history(&self) -> Vec<u64> {
        self.screen
            .as_ref()
            .map_or_else(Vec::new, |screen| screen.known.iter().copied().collect())
    }

    /// The review position.
    #[must_use]
    pub fn review(&self) -> Option<Anchor> {
        self.screen.as_ref().and_then(|screen| screen.review)
    }

    /// Puts the review position on row `row` of the last read, at
    /// `column`. Does nothing before the first read or past its rows.
    pub fn set_review(&mut self, row: usize, column: usize) {
        if let Some(screen) = &mut self.screen
            && let Some(found) = screen.rows.get(row)
        {
            screen.review = Some(Anchor {
                row: isize_of(row),
                column,
                line_hash: found.line_hash,
            });
        }
    }

    /// The command marks, in the order they were set.
    #[must_use]
    pub fn marks(&self) -> &[Anchor] {
        self.screen.as_ref().map_or(&[], |screen| &screen.marks)
    }

    /// Sets a command mark on the line holding row `row` of the last read,
    /// at the line's first row. Does nothing before the first read or past
    /// its rows.
    pub fn add_mark(&mut self, row: usize) {
        if let Some(screen) = &mut self.screen
            && row < screen.rows.len()
        {
            let first = line_start(&screen.rows, row);
            let line_hash = screen.rows[first].line_hash;
            screen.marks.push(Anchor {
                row: isize_of(first),
                column: 0,
                line_hash,
            });
        }
    }

    /// Aligns `read` with the memory, and takes it as the memory's last
    /// read, unless [`Outcome::whole_document`] asks for the whole
    /// document first.
    #[expect(
        clippy::too_many_lines,
        reason = "one read's steps in order: switches, the whole document, the alignment, the anchors"
    )]
    pub fn read(&mut self, read: &Read) -> Outcome {
        let last_height = self.screen.as_ref().map(|screen| screen.height);
        let (width, height) = measure(&read.screen, &read.caret_line, last_height);
        let rows = split_rows(&read.text, width, read.at_ms);
        let caret = caret_row(rows.len(), read.caret_rows_to_end);
        let pending = self.pending.take();
        let whole = pending.filter(|_| read.at_document_start && !read.from_start);
        let shape = Outcome {
            caret,
            width,
            height,
            ..Outcome::default()
        };
        let Some(current) = self.screen.take() else {
            self.alternate = read.alternate;
            let screen = Screen {
                rows,
                height,
                at_document_start: read.at_document_start,
                caret,
                ..Screen::default()
            };
            let outcome = Outcome {
                first: true,
                rows: vec![NewRow::New; screen.rows.len()],
                lines: lines(&screen.rows),
                ..shape
            };
            self.screen = Some(screen);
            return outcome;
        };
        let opened = read.alternate && !self.alternate;
        let restored = self
            .aside
            .as_ref()
            .filter(|_| !read.alternate)
            .map(|aside| {
                // The read started where the alternate screen's memory said,
                // so where the kept memory's rows would be is not known.
                let unanchored = Read {
                    from_start: false,
                    ..read.clone()
                };
                classify(aside, &rows, &unanchored, height, caret, whole, true)
            })
            .filter(|attempt| attempt.restores);
        let switch = if restored.is_some() {
            Switch::Closed
        } else if opened {
            Switch::Opened
        } else {
            Switch::None
        };
        let classified = match restored {
            Some(attempt) => attempt,
            None => classify(&current, &rows, read, height, caret, whole, false),
        };
        if whole.is_none() {
            let cause = if read.shrank && !read.at_document_start {
                Some(Whole::Shrank)
            } else if classified.old_had_text && !classified.kept_text && !read.at_document_start {
                Some(Whole::Lost)
            } else {
                None
            };
            if let Some(cause) = cause {
                // The memory is unchanged until the whole document is read.
                self.pending = Some(cause);
                self.screen = Some(current);
                return Outcome {
                    whole_document: true,
                    ..shape
                };
            }
        }
        // A memory kept aside is restored when the read matches it, and is
        // replaced at the next switch when the main screen came back
        // without it, as after a clear (section 5.8).
        let mut reference = if switch == Switch::Closed {
            self.aside.take().unwrap_or(current)
        } else {
            if !read.alternate {
                self.aside = None;
            }
            current
        };
        let Classified {
            new: kinds,
            old: fates,
            rotation,
            shift,
            overflowed,
            ..
        } = classified;
        // The read's first row continues a line from above it when the old
        // row it was did (section 4.3, a row at a read's top).
        let continued = match kinds.first() {
            Some(NewRow::Kept { from, .. } | NewRow::Rewritten { from, .. }) => {
                if *from == 0 {
                    reference.continued
                } else {
                    reference.rows[from - 1].wraps
                }
            }
            _ => false,
        };
        let lines = lines(&rows);
        let lines = lines
            .into_iter()
            .map(|mut line| {
                line.grew_from = grew_from(&line, &kinds, &reference, continued);
                line
            })
            .collect();
        // The known history moves on with the memory, except to a new
        // screen, whose memory starts without it.
        let known = if opened {
            VecDeque::new()
        } else {
            std::mem::take(&mut reference.known)
        };
        let mut next = remember(
            &reference, known, rows, &kinds, &fates, shift, read, height, caret,
        );
        next.continued = continued;
        if overflowed {
            next.review = caret.map(|row| anchor_at(&next.rows, row, read.caret_column));
            next.marks.clear();
        } else {
            next.review = reference
                .review
                .and_then(|anchor| map_anchor(anchor, &fates, shift, &next.rows, caret, read));
            next.marks = reference
                .marks
                .iter()
                .filter_map(|&mark| map_mark(mark, &fates, shift, &next.rows))
                .collect();
        }
        if opened {
            // The memory before the switch is kept aside with its anchors;
            // on the new screen the review position is at the caret.
            self.aside = Some(reference);
            next.review = caret.map(|row| anchor_at(&next.rows, row, read.caret_column));
            next.marks.clear();
            next.known.clear();
        }
        self.alternate = read.alternate;
        self.screen = Some(next);
        Outcome {
            rows: kinds,
            old: fates,
            lines,
            rotation,
            overflowed,
            switch,
            continued,
            ..shape
        }
    }
}

/// A kept pair: an old row's index and the new row's it became.
type Pair = (usize, usize);

/// What the alignment made of a read against one screen's memory.
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent findings of one alignment"
)]
struct Classified {
    new: Vec<NewRow>,
    old: Vec<OldRow>,
    rotation: usize,
    /// The shift of a row that did not move: new index less old index.
    shift: isize,
    /// Whether the old screen had a row with text.
    old_had_text: bool,
    /// Whether a row with text of the old screen was kept.
    kept_text: bool,
    /// Whether every row with text of the old screen was kept, so a memory
    /// kept aside matches the read.
    restores: bool,
    overflowed: bool,
}

/// Aligns the rows of `read` with `memory` (section 4.3).
#[expect(
    clippy::too_many_lines,
    reason = "the alignment's steps in the design's order, each short"
)]
fn classify(
    memory: &Screen,
    rows: &[Row],
    read: &Read,
    height: usize,
    caret: Option<usize>,
    whole: Option<Whole>,
    restoring: bool,
) -> Classified {
    let old = &memory.rows;
    let (count, total) = (old.len(), rows.len());
    // Where a row that did not move is in this read, when the read
    // started where the last one said to.
    let base = if read.from_start {
        if count >= memory.height || memory.at_document_start {
            Some(-isize_of(memory.screen_start()))
        } else if read.at_document_start {
            None
        } else {
            Some(isize_of(memory.height - count))
        }
    } else {
        None
    };
    // The old rows aligned: the old screen; when the read started
    // elsewhere than the last read said, as after a refused start, rows
    // above it may be in the read too, so a screen more, except in a
    // whole-document read, which is searched for the old screen.
    let start = if base.is_none() && whole.is_none() {
        count.saturating_sub(memory.height * 2)
    } else {
        memory.screen_start()
    };
    let old_screen = &old[start..];
    let guess = base.unwrap_or_else(|| isize_of(total) - isize_of(count));
    // The windows (item 2): the old screen and a screen more at the top,
    // and a screen at the end; a whole-document read is aligned whole.
    let window: Vec<usize> = if whole.is_some() {
        (0..total).collect()
    } else {
        let top = total.min(old_screen.len() + height);
        let bottom = top.max(total.saturating_sub(height));
        (0..top).chain(bottom..total).collect()
    };
    let in_window = |index: usize| window.binary_search(&index).is_ok();
    // Rows with text are aligned; a blank row is kept only in place, below.
    let old_text: Vec<(usize, u64)> = (start..count)
        .filter(|&from| !old[from].is_blank())
        .map(|from| (from, old[from].hash))
        .collect();
    let new_hashes: Vec<u64> = rows.iter().map(|row| row.hash).collect();
    let text_window: Vec<usize> = window
        .iter()
        .copied()
        .filter(|&to| !rows[to].is_blank())
        .collect();
    let kept = align::keep(&old_text, &new_hashes, &text_window, guess);
    let topmost = kept
        .first()
        .map(|&(from, to)| isize_of(to) - isize_of(from));
    let (rotation, shift) = match (base, topmost) {
        (Some(base), Some(top)) if top < base => (usize::try_from(base - top).unwrap_or(0), top),
        (Some(base), _) => (0, base),
        (None, Some(top)) => (0, top),
        (None, None) => (0, guess),
    };
    let mut new = vec![NewRow::New; total];
    let mut fates = vec![OldRow::Gone { lingering: false }; count];
    for &(from, to) in &kept {
        new[to] = NewRow::Kept {
            from,
            moved: isize_of(to) - isize_of(from) != shift,
        };
        fates[from] = OldRow::Kept(to);
    }
    // A blank row is kept where the kept row above it, or else below it,
    // puts it.
    let mut placed = kept.iter().peekable();
    let mut shift_above = None;
    for from in start..count {
        while let Some(&&(kept_from, kept_to)) = placed.peek() {
            if kept_from > from {
                break;
            }
            shift_above = Some(isize_of(kept_to) - isize_of(kept_from));
            placed.next();
        }
        if !old[from].is_blank() {
            continue;
        }
        let below = placed
            .peek()
            .map(|&&(kept_from, kept_to)| isize_of(kept_to) - isize_of(kept_from));
        let row_shift = shift_above.or(below).unwrap_or(shift);
        let found = usize::try_from(isize_of(from) + row_shift)
            .ok()
            .filter(|&to| to < total && rows[to].is_blank() && new[to] == NewRow::New);
        if let Some(to) = found {
            new[to] = NewRow::Kept {
                from,
                moved: row_shift != shift,
            };
            fates[from] = OldRow::Kept(to);
        }
    }
    // Rows the last read held above its screen were not aligned: each
    // follows the topmost kept row's shift where its text is found there.
    for from in 0..start {
        let to = isize_of(from) + shift;
        let found = usize::try_from(to)
            .ok()
            .filter(|&to| to < total && new[to] == NewRow::New && rows[to].hash == old[from].hash);
        fates[from] = match found {
            Some(to) => {
                new[to] = NewRow::Kept { from, moved: false };
                OldRow::Kept(to)
            }
            None if to < 0 && !read.at_document_start => OldRow::Above,
            None => OldRow::Gone { lingering: false },
        };
    }
    // The rows left between kept rows, paired by likeness (item 4); a
    // blank row is alike to none.
    let mut bounds: Vec<(Option<Pair>, Option<Pair>)> = Vec::new();
    let mut previous = None;
    for &pair in &kept {
        bounds.push((previous, Some(pair)));
        previous = Some(pair);
    }
    bounds.push((previous, None));
    for (above, below) in bounds {
        let old_range =
            above.map_or(start, |(from, _)| from + 1)..below.map_or(count, |(from, _)| from);
        let new_range = above.map_or(0, |(_, to)| to + 1)..below.map_or(total, |(_, to)| to);
        let prefer = below
            .or(above)
            .map_or(shift, |(from, to)| isize_of(to) - isize_of(from));
        let old_gap: Vec<usize> = old_range
            .filter(|&from| {
                isize_of(from) + shift >= 0
                    && fates[from] == OldRow::Gone { lingering: false }
                    && !old[from].is_blank()
            })
            .collect();
        let (Some(&first_old), Some(&last_old)) = (old_gap.first(), old_gap.last()) else {
            continue;
        };
        // A gap of a whole-document read is compared only near where its
        // old rows would be, so the work stays bounded.
        let near = |to: usize| {
            let to = isize_of(to);
            whole.is_none()
                || (to >= isize_of(first_old) + prefer - isize_of(height)
                    && to <= isize_of(last_old) + prefer + isize_of(height))
        };
        let new_gap: Vec<usize> = new_range
            .filter(|&to| {
                in_window(to) && new[to] == NewRow::New && !rows[to].is_blank() && near(to)
            })
            .collect();
        if new_gap.is_empty() {
            continue;
        }
        let old_likeness: Vec<(usize, Likeness)> = old_gap
            .iter()
            .map(|&from| (from, Likeness::of(&old[from].text)))
            .collect();
        let new_likeness: Vec<(usize, Likeness)> = new_gap
            .iter()
            .map(|&to| (to, Likeness::of(&rows[to].text)))
            .collect();
        let old_gap: Vec<(usize, &Likeness)> =
            old_likeness.iter().map(|(at, like)| (*at, like)).collect();
        let new_gap: Vec<(usize, &Likeness)> =
            new_likeness.iter().map(|(at, like)| (*at, like)).collect();
        for (from, to) in align::pair(&old_gap, &new_gap, prefer) {
            new[to] = NewRow::Rewritten {
                from,
                moved: isize_of(to) - isize_of(from) != shift,
            };
            fates[from] = OldRow::Rewritten(to);
        }
    }
    let old_had_text = Screen::has_text(old_screen);
    let kept_text = !kept.is_empty();
    let restores = old_screen
        .iter()
        .enumerate()
        .all(|(at, row)| row.is_blank() || matches!(fates[start + at], OldRow::Kept(_)));
    // Rows with text that no old row became, above the topmost kept row
    // (every row, when none was kept): history the memory never covered,
    // or rows the known history held, which a whole-document read finds
    // again and a reprint repeats (section 4.4).
    let top_kept = kept.first().map_or(total, |&(_, to)| to);
    // Rows above found again by a whole-document read, or above a memory
    // kept aside and restored, are history; above a normal read, a reprint.
    let earlier = whole == Some(Whole::Lost) || restoring;
    let mut candidates = Vec::new();
    for to in 0..top_kept {
        if new[to] != NewRow::New || rows[to].is_blank() {
            continue;
        }
        if earlier && kept_text && isize_of(to) < shift {
            new[to] = NewRow::Earlier;
        } else {
            candidates.push(to);
        }
    }
    let mut found_known = false;
    if (kept_text || whole.is_some()) && !candidates.is_empty() {
        let first_kept = kept.first().map_or(count, |&(from, _)| from);
        let mut known: Vec<u64> = memory.known.iter().copied().collect();
        known.extend(old[..first_kept].iter().map(|row| row.hash));
        // A reprint repeats the rows directly above the old screen; a
        // whole-document read holds whatever of the history is left.
        let tail = if whole.is_some() {
            0
        } else {
            known.len().saturating_sub(candidates.len() + height)
        };
        let hashes: Vec<u64> = candidates.iter().map(|&to| rows[to].hash).collect();
        for at in align::reprinted(&hashes, &known[tail..]) {
            found_known = true;
            new[candidates[at]] = if earlier {
                NewRow::Earlier
            } else {
                NewRow::Reprint
            };
        }
    }
    let overflowed =
        whole == Some(Whole::Lost) && old_had_text && !kept_text && !found_known && total > height;
    // Gone rows that linger, and lingering rows taken back (section 4.1).
    let blank = !Screen::has_text(rows);
    let old_caret = memory.caret;
    for from in start..count {
        if fates[from] != (OldRow::Gone { lingering: false }) {
            continue;
        }
        let at = isize_of(from) + shift;
        // A row that went past the read's start is in the history, unless
        // the read starts at the document's start, which has none above.
        if at < 0 {
            if !read.at_document_start {
                fates[from] = OldRow::Above;
            }
            continue;
        }
        if old[from].is_blank() {
            continue;
        }
        let below_caret = old_caret.is_some_and(|row| from >= row)
            || caret.is_some_and(|row| at >= isize_of(row));
        if old[from].redrawn || below_caret || blank {
            fates[from] = OldRow::Gone { lingering: true };
        }
    }
    let mut lingering: Vec<(isize, u64)> = memory
        .lingering
        .iter()
        .filter(|row| read.at_ms.saturating_sub(row.since_ms) <= LINGERING_MS)
        .map(|row| (row.at + shift, row.row.hash))
        .collect();
    for to in 0..total {
        if new[to] != NewRow::New || rows[to].is_blank() {
            continue;
        }
        let nearest = lingering
            .iter()
            .enumerate()
            .filter(|(_, (_, hash))| *hash == rows[to].hash)
            .min_by_key(|(_, (at, _))| (at - isize_of(to)).abs())
            .map(|(index, &(at, _))| (index, at));
        if let Some((index, at)) = nearest {
            new[to] = NewRow::Returned {
                moved: at != isize_of(to),
            };
            lingering.remove(index);
        }
    }
    Classified {
        new,
        old: fates,
        rotation,
        shift,
        old_had_text,
        kept_text,
        restores,
        overflowed,
    }
}

/// The memory after a read: its rows, each with its history carried from
/// the old row it was, the lingering rows, the known history.
#[expect(clippy::too_many_arguments, reason = "the parts of one read")]
fn remember(
    memory: &Screen,
    mut known: VecDeque<u64>,
    mut rows: Vec<Row>,
    kinds: &[NewRow],
    fates: &[OldRow],
    shift: isize,
    read: &Read,
    height: usize,
    caret: Option<usize>,
) -> Screen {
    let now = read.at_ms;
    let old = &memory.rows;
    let mut lingering: Vec<Lingering> = memory
        .lingering
        .iter()
        .filter(|row| now.saturating_sub(row.since_ms) <= LINGERING_MS)
        .map(|row| Lingering {
            at: row.at + shift,
            ..row.clone()
        })
        .collect();
    for (to, kind) in kinds.iter().enumerate() {
        let row = &mut rows[to];
        match *kind {
            NewRow::Kept { from, .. } => {
                row.redrawn = old[from].redrawn;
                row.changed_ms = old[from].changed_ms;
            }
            NewRow::Rewritten { from, .. } => {
                row.redrawn = old[from].redrawn || !row.text.starts_with(&old[from].text);
            }
            NewRow::Returned { .. } => {
                let back = lingering
                    .iter()
                    .enumerate()
                    .filter(|(_, gone)| gone.row.hash == row.hash)
                    .min_by_key(|(_, gone)| (gone.at - isize_of(to)).abs())
                    .map(|(index, _)| index);
                if let Some(index) = back {
                    let gone = lingering.remove(index);
                    row.changed_ms = gone.row.changed_ms;
                }
                row.redrawn = true;
            }
            NewRow::New | NewRow::Reprint | NewRow::Earlier => {}
        }
    }
    for (from, fate) in fates.iter().enumerate() {
        if *fate == (OldRow::Gone { lingering: true }) {
            lingering.push(Lingering {
                row: old[from].clone(),
                since_ms: now,
                at: isize_of(from) + shift,
            });
        }
    }
    known.extend(
        fates
            .iter()
            .zip(old)
            .filter(|(fate, _)| **fate == OldRow::Above)
            .map(|(_, row)| row.hash),
    );
    while known.len() > KNOWN_LIMIT {
        known.pop_front();
    }
    Screen {
        rows,
        height,
        at_document_start: read.at_document_start,
        caret,
        lingering,
        known,
        review: None,
        marks: Vec::new(),
        continued: false,
    }
}

/// An anchor on row `row` of `rows`, at `column` clamped to its text.
fn anchor_at(rows: &[Row], row: usize, column: usize) -> Anchor {
    let found = &rows[row];
    Anchor {
        row: isize_of(row),
        column: column.min(found.text.chars().count()),
        line_hash: found.line_hash,
    }
}

/// The review position after a read (section 5.6): a kept row carries it
/// to the row's new place, a rewritten row keeps it with its column
/// clamped to the row's text, and so does a row written over, at the place
/// it had; a gone row whose place is no longer in the read moves it to the
/// nearest surviving row above, or to the caret when nothing above
/// survived; above the read, it moves with the history.
fn map_anchor(
    anchor: Anchor,
    fates: &[OldRow],
    shift: isize,
    rows: &[Row],
    caret: Option<usize>,
    read: &Read,
) -> Option<Anchor> {
    let place = |row: isize| -> Option<isize> {
        let Ok(from) = usize::try_from(row) else {
            return Some(row + shift);
        };
        match fates.get(from)? {
            OldRow::Kept(to) | OldRow::Rewritten(to) => Some(isize_of(*to)),
            OldRow::Above => Some(row + shift),
            // A row written over keeps it at its place.
            OldRow::Gone { .. } => {
                Some(row + shift).filter(|&to| to >= 0 && to < isize_of(rows.len()))
            }
        }
    };
    let mut row = anchor.row;
    let found = loop {
        if let Some(to) = place(row) {
            break Some(to);
        }
        if row < 0 {
            break None;
        }
        row -= 1;
    };
    match found {
        Some(to) => Some(match usize::try_from(to) {
            Ok(to) if to < rows.len() => anchor_at(rows, to, anchor.column),
            _ => Anchor { row: to, ..anchor },
        }),
        None => caret.map(|row| anchor_at(rows, row, read.caret_column)),
    }
}

/// A command mark after a read (section 5.13): it moves with its line,
/// and is dropped when its line is gone.
fn map_mark(mark: Anchor, fates: &[OldRow], shift: isize, rows: &[Row]) -> Option<Anchor> {
    let to = match usize::try_from(mark.row) {
        Err(_) => {
            return Some(Anchor {
                row: mark.row + shift,
                ..mark
            });
        }
        Ok(from) => match fates.get(from)? {
            OldRow::Kept(to) => *to,
            OldRow::Above => {
                return Some(Anchor {
                    row: mark.row + shift,
                    ..mark
                });
            }
            OldRow::Rewritten(_) | OldRow::Gone { .. } => return None,
        },
    };
    let first = line_start(rows, to);
    (rows[first].line_hash == mark.line_hash).then(|| Anchor {
        row: isize_of(first),
        column: 0,
        line_hash: mark.line_hash,
    })
}

/// The first row of the line holding row `row`.
fn line_start(rows: &[Row], row: usize) -> usize {
    let mut first = row;
    while first > 0 && rows[first - 1].wraps {
        first -= 1;
    }
    first
}

/// The lines of `rows`, without their growth, which [`grew_from`] adds.
fn lines(rows: &[Row]) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut first = 0;
    for (at, row) in rows.iter().enumerate() {
        if row.wraps && at + 1 < rows.len() {
            continue;
        }
        let text = line_text(&rows[first..=at]);
        lines.push(Line {
            rows: first..at + 1,
            hash: hash(&text),
            text,
            grew_from: None,
        });
        first = at + 1;
    }
    lines
}

/// The text of a line's rows, joined, padding left out.
fn line_text(rows: &[Row]) -> String {
    let joined: String = rows.iter().map(|row| row.text.as_str()).collect();
    crate::trim_padding(&joined).to_owned()
}

/// Whether `line` only gained at its end since the last read: its first
/// row is an old row that started a line, or for a line continued from
/// above the read the row that started its part in the old read, and did
/// not move, and its text begins with that line's and is longer.
fn grew_from(line: &Line, kinds: &[NewRow], memory: &Screen, continued: bool) -> Option<usize> {
    let old = &memory.rows;
    let from = match kinds.get(line.rows.start)? {
        NewRow::Kept { from, moved: false } | NewRow::Rewritten { from, moved: false } => *from,
        _ => return None,
    };
    let starts = if from == 0 {
        !memory.continued
    } else {
        !old[from - 1].wraps
    };
    if !(starts || continued && line.rows.start == 0) {
        return None;
    }
    let end = (from..old.len())
        .find(|&at| !old[at].wraps)
        .unwrap_or(old.len() - 1);
    let was = line_text(&old[from..=end]);
    (!was.is_empty() && line.text.len() > was.len() && line.text.starts_with(&was))
        .then_some(was.len())
}

/// The screen's width and height (section 4.3, item 1). Every line's
/// padded length is a whole number of rows, except Windows Terminal's last
/// row of the buffer, cut one cell short (section 2.1), so the width is
/// the narrowest of the screen's lines but its last, in cells. When each
/// of those lines wraps, the narrowest is more than a row, so the caret's
/// row, which the read gives as one row, is taken instead when its length,
/// or one cell more if it is the row Windows Terminal cuts, fits every
/// line; when both fit, the one that gives the screen the height it had
/// at the last read.
fn measure(screen: &str, caret_row: &str, last_height: Option<usize>) -> (usize, usize) {
    let screen = screen.strip_suffix("\r\n").unwrap_or(screen);
    let lines: Vec<usize> = screen.split("\r\n").map(crate::cell_width).collect();
    let (last, others) = lines
        .split_last()
        .map_or((0, &[][..]), |(last, others)| (*last, others));
    let height = |width: usize| -> usize {
        lines
            .iter()
            .map(|&cells| cells.div_ceil(width).max(1))
            .sum()
    };
    let fits = |width: usize| {
        width > 0
            && others.iter().all(|cells| cells % width == 0)
            && (last % width == 0 || (last + 1) % width == 0)
    };
    let caret = crate::cell_width(caret_row.strip_suffix("\r\n").unwrap_or(caret_row));
    let fitting: Vec<usize> = [caret, caret + 1]
        .into_iter()
        .filter(|&width| caret > 0 && fits(width))
        .collect();
    let width = fitting
        .iter()
        .copied()
        .find(|&width| Some(height(width)) == last_height)
        .or_else(|| fitting.first().copied())
        .or_else(|| others.iter().copied().filter(|&cells| cells > 0).min())
        .unwrap_or(last);
    if width == 0 {
        return (0, lines.len());
    }
    (width, height(width))
}

/// The rows of a read's text: its lines split at line breaks, each into
/// rows of `width` cells, padding trimmed from each line's last row only
/// (section 5.12).
fn split_rows(text: &str, width: usize, now: u64) -> Vec<Row> {
    let text = text.strip_suffix("\r\n").unwrap_or(text);
    let mut rows = Vec::new();
    for line in text.split("\r\n") {
        let mut pieces: Vec<String> = Vec::new();
        if line.is_ascii() && width > 0 {
            // Every printable ASCII character takes one cell.
            let mut at = 0;
            loop {
                let end = (at + width).min(line.len());
                pieces.push(line[at..end].to_owned());
                at = end;
                if at >= line.len() {
                    break;
                }
            }
        } else {
            let mut piece = String::new();
            let mut cells = 0;
            for c in line.chars() {
                let cell = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
                if width > 0 && cells + cell > width && !piece.is_empty() {
                    pieces.push(std::mem::take(&mut piece));
                    cells = 0;
                }
                piece.push(c);
                cells += cell;
            }
            pieces.push(piece);
        }
        let last = pieces.len() - 1;
        let line_hash = hash(crate::trim_padding(line));
        for (at, mut piece) in pieces.into_iter().enumerate() {
            if at == last {
                piece.truncate(crate::trim_padding(&piece).len());
            }
            rows.push(Row {
                hash: hash(&piece),
                text: piece,
                wraps: at < last,
                line_hash,
                redrawn: false,
                changed_ms: now,
            });
        }
    }
    rows
}

/// The caret's row in a read of `rows` rows, from how many rows the
/// caret's row and those below it hold.
fn caret_row(rows: usize, to_end: u32) -> Option<usize> {
    let to_end = usize::try_from(to_end).ok()?;
    (to_end > 0 && to_end <= rows).then(|| rows - to_end)
}

/// A 64-bit hash of `text` (FNV-1a): the same in every process and build,
/// so hashes kept in the memory compare across reads.
#[must_use]
pub fn hash(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

fn isize_of(value: usize) -> isize {
    isize::try_from(value).unwrap_or(isize::MAX)
}

#[cfg(test)]
mod tests;
