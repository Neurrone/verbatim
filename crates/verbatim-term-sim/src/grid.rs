//! The terminal's state: a buffer of rows with fixed coordinates, the
//! screen at its bottom, the cursor, and the alternate screen
//! (`docs/design/terminal-model.md`, section 3.1). Every row carries an
//! identity that follows it through scrolls, rotation and clears, which the
//! real terminals do not expose and the oracle's checks rest on.

use std::collections::VecDeque;

/// A colour as UI Automation reports it: `0x00BBGGRR`.
pub type Colour = u32;

/// The Campbell scheme both terminals use by default: the sixteen colours
/// of the palette, as UI Automation reports them.
pub const PALETTE: [Colour; 16] = [
    0x000c_0c0c,
    0x001f_0fc5,
    0x000e_a113,
    0x0000_9cc1,
    0x00da_3700,
    0x0098_1788,
    0x00dd_963a,
    0x00cc_cccc,
    0x0076_7676,
    0x0056_48e7,
    0x000c_c616,
    0x00a5_f1f9,
    0x00ff_783b,
    0x009e_00b4,
    0x00d6_d661,
    0x00f2_f2f2,
];

/// The default foreground and background.
pub const DEFAULT_FOREGROUND: Colour = PALETTE[7];
/// The default background.
pub const DEFAULT_BACKGROUND: Colour = PALETTE[0];

/// A cell's formatting as written: colours before reverse video is
/// applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Style {
    /// The foreground colour.
    pub foreground: Colour,
    /// The background colour.
    pub background: Colour,
    /// Whether foreground and background are swapped when shown.
    pub reverse: bool,
    /// Whether the text is bold, which both terminals show as the bright
    /// version of one of the eight basic foregrounds (measured 2026-10-23).
    pub bold: bool,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            foreground: DEFAULT_FOREGROUND,
            background: DEFAULT_BACKGROUND,
            reverse: false,
            bold: false,
        }
    }
}

impl Style {
    /// The foreground and background as shown: bold brightening a basic
    /// foreground, then reverse video applied.
    #[must_use]
    pub fn shown(self) -> (Colour, Colour) {
        let foreground = match PALETTE[..8]
            .iter()
            .position(|&basic| basic == self.foreground)
        {
            Some(index) if self.bold => PALETTE[index + 8],
            _ => self.foreground,
        };
        if self.reverse {
            (self.background, foreground)
        } else {
            (foreground, self.background)
        }
    }
}

/// One cell: a character and its formatting. A blank cell is a space in the
/// default style.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    /// The character shown.
    pub character: char,
    /// Its formatting.
    pub style: Style,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            character: ' ',
            style: Style::default(),
        }
    }
}

/// A row's identity, unique over a terminal's life: rows keep theirs when
/// they move, and a row made blank by a scroll gets a new one.
pub type RowId = u64;

/// One row of the grid.
#[derive(Clone, Debug)]
pub struct Row {
    /// Its identity.
    pub id: RowId,
    /// Its cells, as many as the width.
    pub cells: Vec<Cell>,
    /// Whether its line continues onto the next row (it was written past
    /// its last column).
    pub wraps: bool,
    /// How many times a write changed its cells.
    pub writes: u32,
}

impl Row {
    fn blank(id: RowId, width: usize) -> Self {
        Self {
            id,
            cells: vec![Cell::default(); width],
            wraps: false,
            writes: 0,
        }
    }

    /// The row's characters, padding included.
    #[must_use]
    pub fn text(&self) -> String {
        self.cells.iter().map(|cell| cell.character).collect()
    }

    /// Whether any cell shows something other than a space.
    #[must_use]
    pub fn has_text(&self) -> bool {
        self.cells.iter().any(|cell| cell.character != ' ')
    }
}

/// One buffer: the main buffer, with its history, or the alternate one,
/// one screen tall. Coordinates are indices into `rows` and never move:
/// when a full main buffer takes another row, every row's content moves up
/// one coordinate (rotation).
#[derive(Clone, Debug)]
pub struct Buffer {
    /// Every row the buffer holds, history then screen.
    pub rows: VecDeque<Row>,
    /// The most rows it holds.
    pub capacity: usize,
    /// The coordinate of the screen's first row.
    pub top: usize,
}

/// The cursor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cursor {
    /// The row, counted from the screen's top.
    pub row: usize,
    /// The column.
    pub column: usize,
    /// Whether the last write filled the last column, so the next
    /// character wraps first.
    pub pending_wrap: bool,
    /// Whether the cursor is shown.
    pub visible: bool,
}

/// What the cursor saved with DECSC or CSI s, and the alternate screen's
/// switch.
#[derive(Clone, Copy, Debug, Default)]
pub struct Saved {
    /// The cursor.
    pub cursor: Cursor,
    /// The style.
    pub style: Style,
}

/// The whole state.
#[derive(Clone, Debug)]
pub struct Grid {
    /// The width in cells.
    pub width: usize,
    /// The screen's height in rows.
    pub height: usize,
    /// The main buffer.
    pub main: Buffer,
    /// The alternate buffer, while the alternate screen is up.
    pub alternate: Option<Buffer>,
    /// The cursor on the active buffer's screen.
    pub cursor: Cursor,
    /// The main screen's cursor, kept while the alternate one is up.
    pub main_cursor: Cursor,
    /// The style the next character is written in.
    pub style: Style,
    /// The main screen's saved cursor.
    pub saved: Saved,
    /// The alternate screen's saved cursor: each screen keeps its own.
    pub saved_alternate: Saved,
    /// The scroll region, first and last screen row, inclusive.
    pub region: (usize, usize),
    /// Whether the whole screen is shown in reverse video (mode 5, a
    /// visual bell's flash), which swaps every cell's colours as the
    /// providers report them (measured 2026-10-23).
    pub screen_reversed: bool,
    /// Whether writing a row's last column marks the row as wrapping at
    /// once, before anything wraps onto the next row, as the console host
    /// does; an explicit line feed on the row clears the mark. Windows
    /// Terminal marks a row only when a character wraps (measured
    /// 2026-10-23).
    pub marks_wrap_when_full: bool,
    next_id: RowId,
}

impl Grid {
    /// An empty terminal `width` cells wide and `height` rows tall, its
    /// history holding up to `history` rows above the screen.
    #[must_use]
    pub fn new(width: usize, height: usize, history: usize, marks_wrap_when_full: bool) -> Self {
        let mut grid = Self {
            width,
            height,
            main: Buffer {
                rows: VecDeque::new(),
                capacity: history + height,
                top: 0,
            },
            alternate: None,
            cursor: Cursor {
                visible: true,
                ..Cursor::default()
            },
            main_cursor: Cursor::default(),
            style: Style::default(),
            saved: Saved::default(),
            saved_alternate: Saved::default(),
            region: (0, height - 1),
            screen_reversed: false,
            marks_wrap_when_full,
            next_id: 0,
        };
        for _ in 0..height {
            let row = grid.new_row();
            grid.main.rows.push_back(row);
        }
        grid
    }

    fn new_row(&mut self) -> Row {
        let id = self.next_id;
        self.next_id += 1;
        Row::blank(id, self.width)
    }

    /// A row scrolled in at a region's edge: blank, in the current
    /// background, as erasing leaves cells (measured 2026-10-23).
    fn scrolled_in_row(&mut self) -> Row {
        let blank = self.erased();
        let mut row = self.new_row();
        row.cells.fill(blank);
        row
    }

    /// The active buffer.
    #[must_use]
    pub fn buffer(&self) -> &Buffer {
        self.alternate.as_ref().unwrap_or(&self.main)
    }

    fn buffer_mut(&mut self) -> &mut Buffer {
        self.alternate.as_mut().unwrap_or(&mut self.main)
    }

    /// The coordinate of the screen row `row`.
    #[must_use]
    pub fn coordinate(&self, row: usize) -> usize {
        self.buffer().top + row
    }

    /// The cursor's coordinate.
    #[must_use]
    pub fn cursor_coordinate(&self) -> usize {
        self.coordinate(self.cursor.row)
    }

    fn row_mut(&mut self, row: usize) -> &mut Row {
        let at = self.coordinate(row);
        &mut self.buffer_mut().rows[at]
    }

    /// Writes one character at the cursor, wrapping first when the last
    /// write filled the row.
    pub fn print(&mut self, character: char) {
        if self.cursor.pending_wrap {
            self.row_mut(self.cursor.row).wraps = true;
            self.cursor.pending_wrap = false;
            self.cursor.column = 0;
            self.line_feed();
        }
        let (row, column, style) = (self.cursor.row, self.cursor.column, self.style);
        let cell = Cell { character, style };
        let target = self.row_mut(row);
        if target.cells[column] != cell {
            target.cells[column] = cell;
            target.writes += 1;
        }
        if column + 1 == self.width {
            self.cursor.pending_wrap = true;
            if self.marks_wrap_when_full {
                self.row_mut(row).wraps = true;
            }
        } else {
            self.cursor.column += 1;
        }
    }

    /// A carriage return.
    pub fn carriage_return(&mut self) {
        self.cursor.column = 0;
        self.cursor.pending_wrap = false;
    }

    /// A line feed written by the program, as opposed to one a wrap makes:
    /// it ends the row's line where the console host marked it as wrapping.
    pub fn explicit_line_feed(&mut self) {
        if self.marks_wrap_when_full {
            self.row_mut(self.cursor.row).wraps = false;
        }
        self.line_feed();
    }

    /// Moves the cursor up or down `delta` rows, stopping at the scroll
    /// region's margins while it is inside the region, and at the screen's
    /// edges otherwise.
    pub fn move_vertically(&mut self, delta: isize) {
        let (top, bottom) = self.region;
        let row = self.cursor.row;
        let (low, high) = if (top..=bottom).contains(&row) {
            (top, bottom)
        } else {
            (0, self.height - 1)
        };
        let target = row.saturating_add_signed(delta).clamp(low, high);
        let column = self.cursor.column;
        self.move_to(target, column);
    }

    /// A line feed: down a row, or the screen or the region scrolls.
    pub fn line_feed(&mut self) {
        self.cursor.pending_wrap = false;
        let (top, bottom) = self.region;
        if self.cursor.row == bottom {
            if top == 0 && self.alternate.is_none() {
                // A region from the screen's top scrolls into the history
                // as the whole screen does, the rows below it keeping their
                // places on the screen and the new blank row coming in at
                // the region's bottom (both terminals, measured
                // 2026-10-23).
                self.scroll_screen();
                if bottom < self.height - 1 {
                    let last = self.coordinate(self.height - 1);
                    let into = self.coordinate(bottom);
                    let buffer = self.buffer_mut();
                    if let Some(blank) = buffer.rows.remove(last) {
                        buffer.rows.insert(into, blank);
                    }
                }
            } else {
                self.scroll_region_up(1);
            }
        } else if self.cursor.row + 1 < self.height {
            self.cursor.row += 1;
        }
    }

    /// The whole screen moves up a row: on the main buffer the screen moves
    /// down the buffer, or, with the buffer full, every row's content moves
    /// up one coordinate; on the alternate buffer the top row goes.
    fn scroll_screen(&mut self) {
        let row = self.scrolled_in_row();
        let alternate = self.alternate.is_some();
        let buffer = self.buffer_mut();
        if alternate {
            buffer.rows.pop_front();
            buffer.rows.push_back(row);
            return;
        }
        if buffer.rows.len() < buffer.capacity {
            buffer.rows.push_back(row);
            buffer.top += 1;
        } else {
            buffer.rows.pop_front();
            buffer.rows.push_back(row);
        }
    }

    /// The scroll region's rows move up `count`, its top rows going, blank
    /// rows coming in at its bottom.
    pub fn scroll_region_up(&mut self, count: usize) {
        let (top, bottom) = self.region;
        for _ in 0..count {
            let row = self.scrolled_in_row();
            let first = self.coordinate(top);
            let last = self.coordinate(bottom);
            let buffer = self.buffer_mut();
            buffer.rows.remove(first);
            buffer.rows.insert(last, row);
        }
    }

    /// The scroll region's rows move down `count`, its bottom rows going.
    pub fn scroll_region_down(&mut self, count: usize) {
        let (top, bottom) = self.region;
        for _ in 0..count {
            let row = self.scrolled_in_row();
            let first = self.coordinate(top);
            let last = self.coordinate(bottom);
            let buffer = self.buffer_mut();
            buffer.rows.remove(last);
            buffer.rows.insert(first, row);
        }
    }

    /// Reverse index: up a row, or the region scrolls down.
    pub fn reverse_index(&mut self) {
        self.cursor.pending_wrap = false;
        if self.cursor.row == self.region.0 {
            self.scroll_region_down(1);
        } else {
            self.cursor.row = self.cursor.row.saturating_sub(1);
        }
    }

    /// Moves the cursor to `row` and `column` of the screen, clamped.
    pub fn move_to(&mut self, row: usize, column: usize) {
        self.cursor.row = row.min(self.height - 1);
        self.cursor.column = column.min(self.width - 1);
        self.cursor.pending_wrap = false;
    }

    /// A cell as erasing leaves it: a space in the current colours, without
    /// reverse video.
    fn erased(&self) -> Cell {
        Cell {
            character: ' ',
            style: Style {
                reverse: false,
                ..self.style
            },
        }
    }

    /// Erases cells `from` to `to` (exclusive) of screen row `row`.
    pub fn erase_cells(&mut self, row: usize, from: usize, to: usize) {
        let blank = self.erased();
        let width = self.width;
        let target = self.row_mut(row);
        let mut changed = false;
        for cell in &mut target.cells[from.min(width)..to.min(width)] {
            if *cell != blank {
                *cell = blank;
                changed = true;
            }
        }
        if changed {
            target.writes += 1;
        }
    }

    /// Erase in line: 0 from the cursor, 1 to the cursor, 2 the whole row.
    pub fn erase_in_line(&mut self, mode: u16) {
        let (row, column) = (self.cursor.row, self.cursor.column);
        match mode {
            1 => self.erase_cells(row, 0, column + 1),
            2 => self.erase_cells(row, 0, self.width),
            _ => self.erase_cells(row, column, self.width),
        }
    }

    /// Erase in display: 0 from the cursor, 1 to the cursor, 2 the screen,
    /// which both terminals push into the history on the main buffer, 3 the
    /// history.
    pub fn erase_in_display(&mut self, mode: u16) {
        let (row, column) = (self.cursor.row, self.cursor.column);
        match mode {
            0 => {
                self.erase_cells(row, column, self.width);
                for below in row + 1..self.height {
                    self.erase_cells(below, 0, self.width);
                }
            }
            1 => {
                for above in 0..row {
                    self.erase_cells(above, 0, self.width);
                }
                self.erase_cells(row, 0, column + 1);
            }
            2 => self.clear_screen(),
            3 => self.clear_history(),
            _ => {}
        }
    }

    /// Erases the screen: on the main buffer, its rows with text move into
    /// the history as a screenful of blank rows follows them.
    fn clear_screen(&mut self) {
        if self.alternate.is_none() {
            let last = (0..self.height)
                .rev()
                .find(|&row| self.main.rows[self.coordinate(row)].has_text());
            if let Some(last) = last {
                for _ in 0..=last {
                    self.scroll_screen();
                }
            }
        }
        // Every cell left on the screen is erased too, rows of colour with
        // no text included (measured 2026-10-23).
        for row in 0..self.height {
            self.erase_cells(row, 0, self.width);
        }
    }

    /// Erases the history above the screen: the screen's rows become the
    /// buffer's first.
    fn clear_history(&mut self) {
        if self.alternate.is_some() {
            return;
        }
        let top = self.main.top;
        self.main.rows.drain(..top);
        self.main.top = 0;
    }

    /// Inserts `count` blank rows at the cursor's row, within the region.
    pub fn insert_rows(&mut self, count: usize) {
        let region = self.region;
        if self.cursor.row < region.0 || self.cursor.row > region.1 {
            return;
        }
        let wraps = self.marks_wrap_when_full && self.row_mut(self.cursor.row).wraps;
        self.region = (self.cursor.row, region.1);
        self.scroll_region_down(count);
        self.region = region;
        // The console host's inserted rows take the wrap mark of the row
        // they were inserted at (measured 2026-10-23).
        for offset in 0..count.min(region.1 + 1 - self.cursor.row) {
            self.row_mut(self.cursor.row + offset).wraps = wraps;
        }
        self.cursor.column = 0;
        self.cursor.pending_wrap = false;
    }

    /// Deletes `count` rows at the cursor's row, within the region.
    pub fn delete_rows(&mut self, count: usize) {
        let region = self.region;
        if self.cursor.row < region.0 || self.cursor.row > region.1 {
            return;
        }
        self.region = (self.cursor.row, region.1);
        self.scroll_region_up(count);
        self.region = region;
        self.cursor.column = 0;
        self.cursor.pending_wrap = false;
    }

    /// Inserts `count` blank cells at the cursor, shifting the rest right.
    pub fn insert_cells(&mut self, count: usize) {
        let (row, column, width) = (self.cursor.row, self.cursor.column, self.width);
        let blank = self.erased();
        let target = self.row_mut(row);
        for _ in 0..count.min(width - column) {
            target.cells.pop();
            target.cells.insert(column, blank);
        }
        target.writes += 1;
    }

    /// Deletes `count` cells at the cursor, shifting the rest left.
    pub fn delete_cells(&mut self, count: usize) {
        let (row, column, width) = (self.cursor.row, self.cursor.column, self.width);
        let target = self.row_mut(row);
        for _ in 0..count.min(width - column) {
            target.cells.remove(column);
            target.cells.push(Cell::default());
        }
        target.writes += 1;
    }

    /// Switches to the alternate screen, a blank one; `save` keeps the
    /// cursor too (mode 1049).
    pub fn enter_alternate(&mut self, save: bool) {
        if self.alternate.is_some() {
            return;
        }
        if save {
            self.saved = Saved {
                cursor: self.cursor,
                style: self.style,
            };
        }
        self.main_cursor = self.cursor;
        let mut buffer = Buffer {
            rows: VecDeque::new(),
            capacity: self.height,
            top: 0,
        };
        for _ in 0..self.height {
            buffer.rows.push_back(self.new_row());
        }
        self.alternate = Some(buffer);
        self.region = (0, self.height - 1);
    }

    /// Switches back to the main screen; `restore` restores the cursor
    /// saved on the way in (mode 1049).
    pub fn leave_alternate(&mut self, restore: bool) {
        if self.alternate.take().is_none() {
            return;
        }
        self.cursor = self.main_cursor;
        if restore {
            self.cursor = self.saved.cursor;
            self.style = self.saved.style;
        }
        self.region = (0, self.height - 1);
    }

    /// Sets the scroll region, 0-based and inclusive, and sends the cursor
    /// home; margins out of order are ignored.
    pub fn set_region(&mut self, top: usize, bottom: usize) {
        let bottom = bottom.min(self.height - 1);
        if top < bottom {
            self.region = (top, bottom);
            self.move_to(0, 0);
        }
    }

    /// The active screen's saved-cursor slot.
    fn slot(&mut self) -> &mut Saved {
        if self.alternate.is_some() {
            &mut self.saved_alternate
        } else {
            &mut self.saved
        }
    }

    /// Saves the cursor and style in the active screen's slot.
    pub fn save_cursor(&mut self) {
        let saved = Saved {
            cursor: self.cursor,
            style: self.style,
        };
        *self.slot() = saved;
    }

    /// Restores the cursor and style saved last on the active screen.
    pub fn restore_cursor(&mut self) {
        let saved = *self.slot();
        let visible = self.cursor.visible;
        self.cursor = saved.cursor;
        self.cursor.visible = visible;
        self.style = saved.style;
    }
}
