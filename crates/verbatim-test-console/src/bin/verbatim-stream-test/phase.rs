//! The phases: what each frame of each way of streaming writes, worked out
//! from the frame's number and the text typed so far, with no input or
//! output of its own, so a phase writes the same bytes whatever its pace.

use std::fmt::Write as _;

/// A way a program streams text (`docs/design/terminal-model.md`, section
/// 12, item 13).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Way {
    /// Each token appended to the last line where it ends, split anywhere,
    /// a character's bytes included; the terminal wraps it.
    Append,
    /// The whole line written again for each token, after a carriage return
    /// or after erasing the row, one sentence a line.
    Rewrite,
    /// The whole region erased and drawn again each frame, as Ink does, the
    /// streamed paragraph inside it growing and wrapping.
    Ink,
    /// The program wrapping its own text, so a partly written word that
    /// grows past the row's end is erased there and written on the next.
    Wrap,
    /// Tokens appended into a scroll region above fixed status rows.
    ScrollRegion,
    /// The program wrapping its own text above fixed status rows, each new
    /// row made by deleting the output's first row and inserting one above
    /// the status rows.
    Insert,
    /// The screen erased and painted whole each frame.
    Repaint,
    /// The streamed text rendered as Markdown: a line restyled once its
    /// markup closes, and a table's columns widened as its rows come, the
    /// rows from the first that changed drawn again each frame.
    Markdown,
}

impl Way {
    /// Every way, by name.
    pub const ALL: [(&'static str, Way); 8] = [
        ("append", Way::Append),
        ("rewrite", Way::Rewrite),
        ("ink", Way::Ink),
        ("wrap", Way::Wrap),
        ("scroll-region", Way::ScrollRegion),
        ("insert", Way::Insert),
        ("repaint", Way::Repaint),
        ("markdown", Way::Markdown),
    ];
}

/// Where the status row, with its spinner and timer, and the prompt are
/// drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    /// Below the output: the prompt, then the status row last.
    Footer,
    /// The status row on the screen's first row, the output in a scroll
    /// region below it, whose scrolled-out lines never reach the history,
    /// and the prompt on the last row.
    Header,
    /// Between the output and the input, as Pi draws its status row: the
    /// status row, then the prompt last.
    Middle,
}

impl Placement {
    /// Every placement, by name.
    pub const ALL: [(&'static str, Placement); 3] = [
        ("footer", Placement::Footer),
        ("header", Placement::Header),
        ("middle", Placement::Middle),
    ];
}

/// A phase to run: a way of streaming, where its status row goes, and
/// whether it runs on the alternate screen, which only repainting does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spec {
    /// The way of streaming.
    pub way: Way,
    /// Where the status row goes.
    pub placement: Placement,
    /// Whether the phase runs on the alternate screen.
    pub alternate: bool,
}

impl Spec {
    /// A phase named `way:placement`, with `:alternate` after it for
    /// repainting on the alternate screen.
    pub fn parse(name: &str) -> Option<Self> {
        let mut parts = name.split(':');
        let way = parts.next()?;
        let placement = parts.next()?;
        let alternate = match parts.next() {
            None => false,
            Some("alternate") => true,
            Some(_) => return None,
        };
        let way = Way::ALL.iter().find(|(n, _)| *n == way)?.1;
        let placement = Placement::ALL.iter().find(|(n, _)| *n == placement)?.1;
        if parts.next().is_some() || (alternate && way != Way::Repaint) {
            return None;
        }
        // A header is drawn above a scroll region in every way, so the
        // scroll region's own way has nothing more to show there.
        if way == Way::ScrollRegion && placement == Placement::Header {
            return None;
        }
        Some(Self {
            way,
            placement,
            alternate,
        })
    }
}

/// The text the ways stream: three sentences, each shorter than a row of
/// 120 cells, together three rows, with words long enough to be split
/// between tokens and characters of two and three bytes.
pub const PLAIN: &str = "Streaming begins: each token arrives as it is written, and words split between tokens. \
Some are long, like incomprehensibilities, and some are not ASCII: naïve, café, crème brûlée. \
The last sentence ends with a dash — an ellipsis… and a check mark ✓";

/// The text the Markdown way streams: a heading, a list with bold and code
/// spans, a table whose first column widens as its rows come, and a
/// paragraph of two rows whose bold span closes on its second.
pub const MARKDOWN: &str = "## Plan\n\
- Read the **terminal model** first, then the **oracle**.\n\
- Run `cargo xtask ci` and fix what fails.\n\
| File | Lines |\n\
| --- | --- |\n\
| terminal_rows.rs | 812 |\n\
| align.rs | 95 |\n\
Each line is restyled once its markup closes, so rows drawn earlier change as the text streams, \
and **a bold span that starts on one row closes on the next one**, which rewraps the paragraph.";

/// The lines of earlier output each phase starts with, below a cleared
/// screen: enough to fill the screen but four rows, so that the streamed
/// text scrolls it.
fn prelude(height: usize) -> Vec<String> {
    (1..=height.saturating_sub(4))
        .map(|line| format!("earlier output {line:02}: step {line} of the build finished"))
        .collect()
}

/// Pi's spinner, one symbol every 80 milliseconds.
const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// How a token's end is chosen: a linear congruential generator from a
/// fixed seed, so every run streams the same tokens.
fn next(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    *state >> 33
}

/// The ends of the tokens `text` is streamed in, as byte offsets, the last
/// the text's end: 2 to 12 bytes each. With `inside` a token may end inside
/// a character's encoding, and does at least inside "é" and twice inside
/// "✓"; without it each end moves on to the next character's start.
pub fn token_ends(text: &str, inside: bool) -> Vec<usize> {
    let mut state = 20_261_024;
    let mut ends = Vec::new();
    let mut at = 0;
    while at < text.len() {
        at = (at + 2 + usize::try_from(next(&mut state) % 11).unwrap_or(0)).min(text.len());
        ends.push(at);
    }
    if inside {
        for character in ["é", "✓"] {
            let start = text.find(character).unwrap_or(0);
            ends.push(start + 1);
            if character.len() == 3 {
                ends.push(start + 2);
            }
        }
    } else {
        for end in &mut ends {
            while !text.is_char_boundary(*end) {
                *end += 1;
            }
        }
    }
    ends.sort_unstable();
    ends.dedup();
    ends
}

/// The rows a program wrapping `chars` at `width` draws them in: as many
/// words as fit on each row, a word longer than a row split, the spaces
/// where a row ends left out. Ranges into `chars`.
pub fn wrap(chars: &[char], width: usize) -> Vec<std::ops::Range<usize>> {
    let mut rows = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        if chars.len() - start <= width {
            rows.push(start..chars.len());
            break;
        }
        let limit = start + width;
        let end = (start + 1..=limit).rev().find(|&at| chars[at] == ' ');
        let (mut row_end, mut next_start) = match end {
            Some(space) => (space, space + 1),
            None => (limit, limit),
        };
        while row_end > start && chars[row_end - 1] == ' ' {
            row_end -= 1;
        }
        while next_start < chars.len() && chars[next_start] == ' ' {
            next_start += 1;
        }
        rows.push(start..row_end);
        start = next_start;
    }
    rows
}

/// `text` wrapped at `width`, as the rows' text.
fn wrapped(text: &str, width: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    wrap(&chars, width)
        .into_iter()
        .map(|row| chars[row].iter().collect())
        .collect()
}

/// How many characters two texts start with alike.
fn common(left: &str, right: &str) -> usize {
    left.chars()
        .zip(right.chars())
        .take_while(|(left, right)| left == right)
        .count()
}

/// A style the Markdown renderer gives a stretch of text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Style {
    Plain,
    Bold,
    Code,
}

/// The rows `text`, as much of [`MARKDOWN`] as has streamed, renders to at
/// `width`, each with its colours, as a renderer draws them: a heading is
/// bold once its line is complete, a list item's "- " a bullet, a bold or
/// code span styled once it closes, its markers left out, and a table's
/// cells padded to its widest so far, its rule drawn in box lines.
fn markdown(text: &str, width: usize) -> Vec<String> {
    let lines: Vec<&str> = text.split('\n').collect();
    let complete = |index: usize| index + 1 < lines.len();
    let mut rendered: Vec<Vec<(char, Style)>> = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        if line.starts_with('|') {
            let end = (index..lines.len())
                .find(|&at| !lines[at].starts_with('|'))
                .unwrap_or(lines.len());
            rendered.extend(table(&lines[index..end]));
            index = end;
            continue;
        }
        if let Some(heading) = line.strip_prefix("## ").filter(|_| complete(index)) {
            rendered.push(heading.chars().map(|c| (c, Style::Bold)).collect());
        } else if let Some(item) = line.strip_prefix("- ") {
            let mut row = vec![('•', Style::Plain), (' ', Style::Plain)];
            row.extend(inline(item));
            rendered.push(row);
        } else if !line.is_empty() || complete(index) {
            rendered.push(inline(line));
        }
        index += 1;
    }
    let mut rows = Vec::new();
    for line in rendered {
        let chars: Vec<char> = line.iter().map(|(c, _)| *c).collect();
        if chars.is_empty() {
            rows.push(String::new());
        }
        for range in wrap(&chars, width) {
            rows.push(styled(&line[range]));
        }
    }
    rows
}

/// A line's text with its closed bold and code spans styled and their
/// markers left out; an unclosed span's marker stays as written.
fn inline(text: &str) -> Vec<(char, Style)> {
    let mut out = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let bold = rest.find("**");
        let code = rest.find('`');
        let (at, marker, style) = match (bold, code) {
            (Some(b), Some(c)) if c < b => (c, "`", Style::Code),
            (Some(b), _) => (b, "**", Style::Bold),
            (None, Some(c)) => (c, "`", Style::Code),
            (None, None) => (rest.len(), "", Style::Plain),
        };
        out.extend(rest[..at].chars().map(|c| (c, Style::Plain)));
        rest = &rest[at..];
        if marker.is_empty() {
            break;
        }
        let inner = &rest[marker.len()..];
        if let Some(close) = inner.find(marker) {
            out.extend(inner[..close].chars().map(|c| (c, style)));
            rest = &inner[close + marker.len()..];
        } else {
            out.extend(rest.chars().map(|c| (c, Style::Plain)));
            break;
        }
    }
    out
}

/// A table's rows: each cell padded to its column's widest cell so far,
/// columns two spaces apart, and a rule row drawn in box lines.
fn table(lines: &[&str]) -> Vec<Vec<(char, Style)>> {
    let cells: Vec<Vec<String>> = lines
        .iter()
        .map(|line| {
            let inner = line.trim_start_matches('|');
            let inner = inner.strip_suffix('|').unwrap_or(inner);
            inner
                .split('|')
                .map(|cell| cell.trim().to_owned())
                .collect()
        })
        .collect();
    let rule = |row: &[String]| {
        !row.is_empty()
            && row
                .iter()
                .all(|cell| !cell.is_empty() && cell.chars().all(|c| c == '-'))
    };
    let mut widths: Vec<usize> = Vec::new();
    for row in cells.iter().filter(|row| !rule(row)) {
        for (column, cell) in row.iter().enumerate() {
            let cells = cell.chars().count();
            match widths.get_mut(column) {
                Some(width) => *width = (*width).max(cells),
                None => widths.push(cells),
            }
        }
    }
    cells
        .iter()
        .map(|row| {
            let mut text = String::new();
            for (column, cell) in row.iter().enumerate() {
                let width = widths.get(column).copied().unwrap_or(0);
                if column > 0 {
                    text.push_str("  ");
                }
                if rule(row) {
                    text.push_str(&"─".repeat(width));
                } else {
                    let _ = write!(text, "{cell:width$}");
                }
            }
            text.trim_end().chars().map(|c| (c, Style::Plain)).collect()
        })
        .collect()
}

/// A row's characters with the colours of their styles.
fn styled(row: &[(char, Style)]) -> String {
    let mut out = String::new();
    let mut current = Style::Plain;
    for &(c, style) in row {
        if style != current {
            out.push_str(match (current, style) {
                (Style::Bold, _) => "\x1b[22m",
                (Style::Code, _) => "\x1b[39m",
                (Style::Plain, _) => "",
            });
            out.push_str(match style {
                Style::Bold => "\x1b[1m",
                Style::Code => "\x1b[36m",
                Style::Plain => "",
            });
            current = style;
        }
        out.push(c);
    }
    out.push_str(match current {
        Style::Bold => "\x1b[22m",
        Style::Code => "\x1b[39m",
        Style::Plain => "",
    });
    out
}

/// How many cells a row drawn with colours fills.
fn cells(row: &str) -> usize {
    let mut count = 0;
    let mut escape = false;
    for c in row.chars() {
        match c {
            '\x1b' => escape = true,
            'm' if escape => escape = false,
            _ if escape => {}
            _ => count += 1,
        }
    }
    count
}

/// A cursor position, counted from 1.
fn to(row: usize, column: usize) -> String {
    format!("\x1b[{row};{column}H")
}

/// The cursor moved up `rows`, nothing for none.
fn up(rows: usize) -> String {
    if rows == 0 {
        String::new()
    } else {
        format!("\x1b[{rows}A")
    }
}

/// How the status row and the prompt stay where they are as the output
/// changes.
#[derive(Clone, Copy, Debug)]
enum Rows {
    /// Drawn below the output after each change, erased and drawn again,
    /// moving down as the output grows; the cursor parked at the prompt's
    /// end, `to_output` rows below the output's last row.
    Moving {
        /// Rows from the prompt up to the output's last row.
        to_output: usize,
    },
    /// On fixed rows outside a scroll region, the output's cursor saved
    /// while they are drawn and restored before the output changes.
    Fixed {
        /// The status row's row.
        status: usize,
        /// The prompt's row.
        prompt: usize,
    },
}

/// A running phase: what it has drawn so far, and what it writes next.
pub struct Phase {
    spec: Spec,
    width: usize,
    height: usize,
    fps: u32,
    text: &'static str,
    ends: Vec<usize>,
    rows: Rows,
    /// The frames written so far.
    frame: usize,
    /// How much of the text the last frame showed, in bytes.
    shown: usize,
    /// Whether the cursor is parked at the prompt (or the output's saved),
    /// rather than at the output's end after a character split.
    parked: bool,
    /// The cells on the output's last row, `None` before the streamed text
    /// has a row.
    column: Option<usize>,
    /// The streamed text's rows as last drawn, for the ways that compare.
    drawn: Vec<String>,
    /// The output's last row on the screen, for the inserting way.
    output_row: usize,
    /// The rows Ink drew last, its region's height.
    ink_rows: usize,
}

/// The writes of one frame: usually one, two where a token ends inside a
/// character and the character's first bytes go alone, and none for a
/// frame that changes nothing.
pub type Writes = Vec<Vec<u8>>;

impl Phase {
    /// A phase for a screen of `width` by `height`, at `fps` frames a
    /// second.
    pub fn new(spec: Spec, width: usize, height: usize, fps: u32) -> Self {
        let text = if spec.way == Way::Markdown {
            MARKDOWN
        } else {
            PLAIN
        };
        let ends = token_ends(text, spec.way == Way::Append);
        let rows = match (spec.way, spec.placement) {
            (_, Placement::Header) => Rows::Fixed {
                status: 1,
                prompt: height,
            },
            (Way::ScrollRegion | Way::Insert | Way::Repaint, Placement::Footer) => Rows::Fixed {
                status: height,
                prompt: height - 1,
            },
            (Way::ScrollRegion | Way::Insert | Way::Repaint, Placement::Middle) => Rows::Fixed {
                status: height - 1,
                prompt: height,
            },
            (_, Placement::Footer) => Rows::Moving { to_output: 1 },
            (_, Placement::Middle) => Rows::Moving { to_output: 2 },
        };
        Self {
            spec,
            width,
            height,
            fps,
            text,
            ends,
            rows,
            frame: 0,
            shown: 0,
            parked: true,
            column: None,
            drawn: Vec::new(),
            output_row: 0,
            ink_rows: 0,
        }
    }

    /// How many frames the phase writes: its setup, one a token, the frame
    /// that says it is done, and the one that ends it.
    pub fn frames(&self) -> usize {
        self.ends.len() + 3
    }

    /// The status row at frame `frame`: the spinner and the time so far,
    /// then, once the text is done, the time it took.
    fn status(&self, frame: usize) -> String {
        let ms = frame * 1000 / usize::try_from(self.fps).unwrap_or(60);
        let tenths = ms / 100;
        if frame > self.ends.len() {
            format!("✓ Done in {}.{}s", tenths / 10, tenths % 10)
        } else {
            let symbol = SPINNER[(ms / 80) % SPINNER.len()];
            format!("{symbol} Working... ({}.{}s)", tenths / 10, tenths % 10)
        }
    }

    /// The prompt with what has been typed at it.
    fn prompt(typed: &str) -> String {
        format!("> {typed}")
    }

    /// The scroll region's top and bottom rows, for the fixed rows.
    fn region(&self) -> (usize, usize) {
        match self.spec.placement {
            Placement::Header => (2, self.height - 1),
            Placement::Footer | Placement::Middle => (1, self.height - 2),
        }
    }

    /// The next frame's writes, with what has been typed so far. Called
    /// once a frame, [`Phase::frames`] times.
    pub fn next(&mut self, typed: &str) -> Writes {
        let frame = self.frame;
        self.frame += 1;
        let status = self.status(frame);
        let prompt = Self::prompt(typed);
        if frame == 0 {
            return vec![self.setup(&status, &prompt).into_bytes()];
        }
        if frame == self.ends.len() + 2 {
            return self.end(&prompt);
        }
        let shown = self.ends.get(frame - 1).copied().unwrap_or(self.text.len());
        let before = self.shown;
        self.shown = shown;
        match self.spec.way {
            Way::Append | Way::ScrollRegion => self.append(before, shown, &status, &prompt),
            Way::Rewrite => vec![self.rewrite(before, shown, &status, &prompt).into_bytes()],
            Way::Wrap | Way::Insert => vec![self.wrapping(shown, &status, &prompt).into_bytes()],
            Way::Ink => vec![self.ink(shown, &status, &prompt).into_bytes()],
            Way::Repaint => vec![self.repaint(shown, &status, &prompt).into_bytes()],
            Way::Markdown => vec![self.markdown(shown, &status, &prompt).into_bytes()],
        }
    }

    /// What typing at the prompt writes once the phase has ended, the
    /// prompt drawn again where it is.
    pub fn typed(&self, typed: &str) -> Vec<u8> {
        let prompt = Self::prompt(typed);
        match (self.spec.way, self.rows) {
            _ if self.spec.alternate => Vec::new(),
            (Way::Ink, _) if self.spec.placement != Placement::Header => {
                let above = match self.spec.placement {
                    Placement::Footer => 2,
                    _ => 1,
                };
                format!("{}\r\x1b[2K{prompt}\x1b[{above}B\r", up(above)).into_bytes()
            }
            (_, Rows::Moving { .. }) => format!("\r\x1b[2K{prompt}").into_bytes(),
            (_, Rows::Fixed { prompt: row, .. }) => {
                format!("{}\x1b[2K{prompt}", to(row, 1)).into_bytes()
            }
        }
    }

    /// The status row and prompt below the output, in the placement's
    /// order.
    fn below(&self, status: &str, prompt: &str) -> [String; 2] {
        match self.spec.placement {
            Placement::Footer => [prompt.to_owned(), status.to_owned()],
            Placement::Header | Placement::Middle => [status.to_owned(), prompt.to_owned()],
        }
    }

    /// The status row and prompt drawn after the output changed, from the
    /// output's end, the cursor left at the prompt's end.
    fn after(&mut self, status: &str, prompt: &str) -> String {
        self.parked = true;
        match self.rows {
            Rows::Moving { .. } => {
                let [first, second] = self.below(status, prompt);
                let back = match self.spec.placement {
                    Placement::Footer => format!("\x1b[1A\x1b[{}G", cells(prompt) + 1),
                    _ => String::new(),
                };
                format!("\r\n\x1b[J{first}\r\n{second}{back}")
            }
            Rows::Fixed {
                status: status_row,
                prompt: prompt_row,
            } => format!(
                "\x1b7{}\x1b[2K{status}{}\x1b[2K{prompt}",
                to(status_row, 1),
                to(prompt_row, 1)
            ),
        }
    }

    /// From the parked cursor to the start of the output's row `above`
    /// rows above its last.
    fn to_row(&self, above: usize) -> String {
        match self.rows {
            Rows::Moving { to_output } => format!("{}\r", up(to_output + above)),
            Rows::Fixed { .. } => format!("\x1b8{}\r", up(above)),
        }
    }

    /// From the parked cursor to the start of a new row below the output's
    /// last, the row erased.
    fn new_row(&self) -> String {
        match self.rows {
            Rows::Moving { to_output } => format!("{}\r\x1b[K", up(to_output - 1)),
            Rows::Fixed { .. } => "\x1b8\r\n\x1b[K".to_owned(),
        }
    }

    /// From the parked cursor to the output's end.
    fn to_end(&self) -> String {
        match self.column {
            None => self.new_row(),
            Some(column) if column >= self.width => self.new_row(),
            Some(column) => format!("{}\x1b[{}G", self.to_row(0), column + 1),
        }
    }

    /// The first frame: the screen cleared, the earlier output, and the
    /// status row and prompt.
    fn setup(&mut self, status: &str, prompt: &str) -> String {
        let mut out = String::new();
        if self.spec.alternate {
            out.push_str("\x1b[?1049h");
        }
        out.push_str("\x1b[H\x1b[2J");
        let prelude = prelude(self.height);
        match (self.spec.way, self.rows) {
            (Way::Repaint, _) => {
                out.push_str(&self.screen(&prelude, status, prompt));
                return out;
            }
            (Way::Insert, _) => {
                let (top, _) = self.region();
                out.push_str(&to(top, 1));
                out.push_str(&prelude.join("\r\n"));
                self.output_row = top + prelude.len() - 1;
                out.push_str(&self.after(status, prompt));
                return out;
            }
            (_, Rows::Fixed { .. }) => {
                let (top, bottom) = self.region();
                let _ = write!(out, "\x1b[{top};{bottom}r{}", to(top, 1));
            }
            (_, Rows::Moving { .. }) => {}
        }
        out.push_str(&prelude.join("\r\n"));
        if self.spec.way == Way::Ink {
            out.push_str("\r\n");
            match self.rows {
                Rows::Fixed { .. } => {
                    out.push_str(&self.ink_region(""));
                    out.push_str(&self.after(status, prompt));
                }
                Rows::Moving { .. } => {
                    let region = self
                        .ink_region(&format!("{}\r\n", self.below(status, prompt).join("\r\n")));
                    out.push_str(&region);
                }
            }
            return out;
        }
        out.push_str(&self.after(status, prompt));
        out
    }

    /// The frame after the one that says the text is done: the alternate
    /// screen left, or the scroll region removed and the cursor parked at
    /// the prompt again; nothing for the others.
    fn end(&mut self, prompt: &str) -> Writes {
        if self.spec.alternate {
            return vec![b"\x1b[?1049l".to_vec()];
        }
        match (self.spec.way, self.rows) {
            (Way::Insert | Way::Repaint, _) | (_, Rows::Moving { .. }) => Vec::new(),
            (_, Rows::Fixed { prompt: row, .. }) => {
                vec![format!("\x1b[r{}", to(row, cells(prompt) + 1)).into_bytes()]
            }
        }
    }

    /// Appending: the token where the output ends, the terminal wrapping
    /// it; a character split by the token's end is written in two, its
    /// first bytes alone, the status row and prompt drawn only once it is
    /// whole.
    fn append(&mut self, before: usize, shown: usize, status: &str, prompt: &str) -> Writes {
        let bytes = &self.text.as_bytes()[before..shown];
        let mut writes = Vec::new();
        let mut out = Vec::new();
        // The bytes that end a character the last frame split.
        let ending = if self.parked {
            0
        } else {
            bytes.iter().take_while(|byte| *byte & 0xc0 == 0x80).count()
        };
        if ending > 0 {
            writes.push(bytes[..ending].to_vec());
            if !self.text.is_char_boundary(before + ending) {
                return writes;
            }
            self.count(1);
        }
        let rest = before + ending;
        let mut whole = shown;
        while !self.text.is_char_boundary(whole) {
            whole -= 1;
        }
        if self.parked {
            out.extend_from_slice(self.to_end().as_bytes());
            if self.column.is_none_or(|column| column >= self.width) {
                self.column = Some(0);
            }
        }
        let text = &self.text[rest..whole.max(rest)];
        out.extend_from_slice(text.as_bytes());
        self.count(text.chars().count());
        if whole < shown {
            writes.push(out);
            writes.push(self.text.as_bytes()[whole..shown].to_vec());
            self.parked = false;
            return writes;
        }
        out.extend_from_slice(self.after(status, prompt).as_bytes());
        writes.push(out);
        writes
    }

    /// The output's last row's cells moved on by `characters` written, as
    /// the terminal wraps them: a row filled to its end waits for the next
    /// character to wrap.
    fn count(&mut self, characters: usize) {
        let mut column = self.column.unwrap_or(0);
        for _ in 0..characters {
            if column >= self.width {
                column = 0;
            }
            column += 1;
        }
        self.column = Some(column);
    }

    /// Rewriting: the sentence the text has reached written again whole,
    /// after a carriage return for the first and third sentences and after
    /// erasing the row for the second; a sentence once done stays, and the
    /// next starts on a new row.
    fn rewrite(&mut self, before: usize, shown: usize, status: &str, prompt: &str) -> String {
        let sentences = sentences(self.text);
        let index = |at: usize| {
            sentences
                .iter()
                .position(|range| at < range.end)
                .unwrap_or(sentences.len() - 1)
        };
        let from = if before == 0 { 0 } else { index(before - 1) };
        let to = index(shown - 1);
        let mut out = if self.column.is_none() {
            self.new_row()
        } else {
            self.to_row(0)
        };
        for (sentence, range) in sentences.iter().enumerate().take(to + 1).skip(from) {
            if sentence > from {
                out.push_str("\r\n\x1b[K");
            }
            let text = self.text[range.start..shown.min(range.end)].trim_end();
            out.push_str(if sentence % 2 == 1 { "\x1b[2K\r" } else { "\r" });
            out.push_str(text);
            self.column = Some(text.chars().count());
        }
        out.push_str(&self.after(status, prompt));
        out
    }

    /// Wrapping the text itself: the last row's change written from where
    /// it first differs, a partly written word that no longer fits erased
    /// there, and each new row below; the inserting way makes each new
    /// row by deleting the output's first row and inserting one above the
    /// status rows.
    fn wrapping(&mut self, shown: usize, status: &str, prompt: &str) -> String {
        let rows = wrapped(&self.text[..shown], self.width);
        let mut out = String::new();
        let mut from = 0;
        if let Some(last) = self.drawn.last() {
            let index = self.drawn.len() - 1;
            let new = rows.get(index).map_or("", String::as_str);
            let same = common(last, new);
            out.push_str(&self.to_column(index, same));
            if same < last.chars().count() {
                out.push_str("\x1b[K");
            }
            out.extend(new.chars().skip(same));
            from = index + 1;
        }
        for (index, row) in rows.iter().enumerate().skip(from) {
            if index == 0 {
                out.push_str(&self.new_output_row());
            } else if self.spec.way == Way::Insert {
                out.push_str(&self.inserted_row());
            } else {
                out.push_str("\r\n\x1b[K");
            }
            out.push_str(row);
        }
        self.drawn = rows;
        out.push_str(&self.after(status, prompt));
        out
    }

    /// From the parked cursor to column `column` of the streamed text's
    /// last row, `index`.
    fn to_column(&self, index: usize, column: usize) -> String {
        if self.spec.way == Way::Insert {
            to(self.output_row, column + 1)
        } else {
            debug_assert_eq!(index + 1, self.drawn.len());
            format!("{}\x1b[{}G", self.to_row(0), column + 1)
        }
    }

    /// From the parked cursor to the streamed text's first row.
    fn new_output_row(&mut self) -> String {
        if self.spec.way == Way::Insert {
            self.inserted_row()
        } else {
            self.new_row()
        }
    }

    /// A new row for the inserting way: the next row while the output has
    /// room, else the output's first row deleted and a row inserted above
    /// the status rows.
    fn inserted_row(&mut self) -> String {
        let (top, bottom) = self.region();
        if self.output_row < bottom {
            self.output_row += 1;
            to(self.output_row, 1)
        } else {
            format!(
                "{}\x1b[M{}\x1b[L{}",
                to(top, 1),
                to(bottom, 1),
                to(bottom, 1)
            )
        }
    }

    /// Ink's frame: its region erased from the row below it upward, then
    /// drawn again whole, the cursor left on the row below it.
    fn ink(&mut self, shown: usize, status: &str, prompt: &str) -> String {
        let mut rows = wrapped(&self.text[..shown], self.width);
        match self.rows {
            Rows::Fixed { .. } => {
                let mut out = "\x1b8".to_owned();
                let mut region = rows.join("\r\n");
                region.push_str("\r\n");
                out.push_str(&self.ink_region(&region));
                out.push_str(&self.after(status, prompt));
                out
            }
            Rows::Moving { .. } => {
                rows.extend(self.below(status, prompt));
                let mut region = rows.join("\r\n");
                region.push_str("\r\n");
                self.ink_region(&region)
            }
        }
    }

    /// Ink's region erased and `region` drawn in its place.
    fn ink_region(&mut self, region: &str) -> String {
        let erase = self.ink_rows + 1;
        let mut out = "\x1b[2K\x1b[1A".repeat(erase - 1);
        out.push_str("\x1b[2K\x1b[G");
        out.push_str(region);
        self.ink_rows = region.matches("\r\n").count();
        out
    }

    /// The whole screen: the earlier output and the text so far, as many of
    /// their last rows as fit, with the status row and prompt where the
    /// placement puts them.
    fn screen(&self, prelude: &[String], status: &str, prompt: &str) -> String {
        let mut output: Vec<String> = prelude.to_vec();
        output.extend(wrapped(&self.text[..self.shown], self.width));
        let room = self.height - 2;
        let tail = &output[output.len().saturating_sub(room)..];
        let mut screen: Vec<String> = Vec::new();
        let prompt_row;
        if self.spec.placement == Placement::Header {
            screen.push(status.to_owned());
            screen.extend(tail.iter().cloned());
            screen.resize(self.height - 1, String::new());
            screen.push(prompt.to_owned());
            prompt_row = self.height;
        } else {
            screen.extend(tail.iter().cloned());
            screen.resize(room, String::new());
            let below = self.below(status, prompt);
            prompt_row = screen.len()
                + if self.spec.placement == Placement::Footer {
                    1
                } else {
                    2
                };
            screen.extend(below);
        }
        format!(
            "{}{}",
            screen.join("\r\n"),
            to(prompt_row, cells(prompt) + 1)
        )
    }

    /// Repainting: the screen erased and painted whole.
    fn repaint(&mut self, _shown: usize, status: &str, prompt: &str) -> String {
        format!(
            "\x1b[H\x1b[2J{}",
            self.screen(&prelude(self.height), status, prompt)
        )
    }

    /// Markdown: the rendered rows drawn again from the first that changed,
    /// each erased first, the rows below the last erased.
    fn markdown(&mut self, shown: usize, status: &str, prompt: &str) -> String {
        let rows = markdown(&self.text[..shown], self.width);
        let first = self
            .drawn
            .iter()
            .zip(&rows)
            .position(|(drawn, row)| drawn != row)
            .unwrap_or_else(|| self.drawn.len().min(rows.len()));
        // Nothing changed: the last row drawn again, to reach the
        // output's end.
        let first = if first == rows.len() && first == self.drawn.len() {
            first.saturating_sub(1)
        } else {
            first
        };
        let mut out = if first < self.drawn.len() {
            self.to_row(self.drawn.len() - 1 - first)
        } else {
            self.new_row()
        };
        let redrawn: Vec<String> = rows[first..]
            .iter()
            .map(|row| format!("\x1b[2K{row}"))
            .collect();
        out.push_str(&redrawn.join("\r\n"));
        if rows.len() < self.drawn.len() && matches!(self.rows, Rows::Fixed { .. }) {
            out.push_str("\x1b[J");
        }
        self.drawn = rows;
        out.push_str(&self.after(status, prompt));
        out
    }
}

/// The byte ranges of `text`'s sentences, each with the space after it.
fn sentences(text: &str) -> Vec<std::ops::Range<usize>> {
    let mut ranges = Vec::new();
    let mut start = 0;
    for (at, _) in text.match_indices(". ") {
        ranges.push(start..at + 2);
        start = at + 2;
    }
    ranges.push(start..text.len());
    ranges
}
