//! A terminal's colours (`books/dev/src/explanation/design/terminal-model.md`, "Lists and
//! colours" and "Costs"): each screen row's runs of one foreground and background, from the
//! screen's top to the document's end, read in the reads in a key's window.
//!
//! What the providers answer, measured in both terminals on 2026-10-24:
//! a single cell's colour attribute is its own colour; a stretch of cells
//! answers its first cell's colour when every cell but its last has that
//! colour, and UIA's "mixed" otherwise, because the providers never compare
//! a stretch's last cell. Their format unit stops only at rows, and Windows
//! Terminal does not implement `FindAttribute`. So a stretch is of one
//! colour exactly when its answer is a colour and its last cell, read on
//! its own, answers the same; such a check costs four attribute reads
//! however long the stretch.
//!
//! A row is checked whole; a row of several colours is checked in chunks
//! of [`COLOUR_CHUNK`] cells, and a chunk of several colours is read cell
//! by cell. A plain row costs one check, a row with a few colours a check
//! per chunk and a walk of the chunks its colours change in, and a row
//! whose every cell differs from its neighbours a walk of all of it, its
//! worst case. Measured on a 120 by 30 screen in that worst case
//! (`books/dev/src/reference/performance.md`, "A terminal's colours"), reading every cell
//! regardless costs the same as this, while it reads an ordinary screen in
//! a fraction of the time.
//!
//! A program reads rows while a worst-case row still fits in its budget
//! ([`COLOUR_BUDGET`] instructions), so it stays under the platform's
//! limit whatever the screen holds, and the next program starts at the
//! row where it stopped. The first rows are read by the window read's own
//! program once the screen's size is known ([`crate::batch`]), so an
//! ordinary screen costs no round trip of its own.

use windows::Win32::UI::Accessibility::{
    IUIAutomationTextRange, TextUnit_Character, TextUnit_Line, UIA_BackgroundColorAttributeId,
    UIA_ForegroundColorAttributeId,
};

use verbatim_uia::text::{Endpoint, TextRangeExt};

use crate::builder::{Builder, Reg, kind};
use crate::error::Error;
use crate::instruction::TypeTest;
use crate::opcode::Comparison;
use crate::operation::{Import, Outcome, ProgramCache, Value};

/// A stretch of one row's cells in one foreground and background, the
/// form the terminal reading rules take (`verbatim-text`'s
/// `terminal_reading::ColourRun`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColourRun {
    /// Its first column, in cells.
    pub column: usize,
    /// Its length, in cells.
    pub length: usize,
    /// Its foreground, `0x00BBGGRR`; `u32::MAX` when the provider gave no
    /// number.
    pub foreground: u32,
    /// Its background, `0x00BBGGRR`; `u32::MAX` when the provider gave no
    /// number.
    pub background: u32,
}

/// How many cells a row of several colours is checked in at a time: a
/// chunk of one colour costs one check, a chunk of several a walk of its
/// cells. Chosen by measurement (`books/dev/src/reference/performance.md`, "A terminal's
/// colours").
pub const COLOUR_CHUNK: usize = 16;

/// How many instructions one program may spend on colours: it reads
/// rows while a worst-case row still fits, so with the snapshot's own
/// instructions a program stays under half the platform's limit of
/// 10,000, where its instructions can be counted
/// (`crates/mockapp/tests/instruction_limit.rs`). A row wider than about
/// 300 cells is read alone, one row per program, and up to about 600
/// cells a program stays under half the limit.
pub const COLOUR_BUDGET: usize = 4_600;

/// A foreground and a background, `0x00BBGGRR`.
type Colours = (u32, u32);

/// What a colour that is not a number reads as.
const NOT_A_NUMBER: u32 = u32::MAX;

/// The chunks of a row `width` cells wide, each its first column and its
/// length.
fn chunks(width: usize) -> Vec<(usize, usize)> {
    (0..width)
        .step_by(COLOUR_CHUNK)
        .map(|column| (column, COLOUR_CHUNK.min(width - column)))
        .collect()
}

/// Adds a cell, or a stretch of one colour, to a row's runs, joining it to
/// the last run when their colours agree.
fn push(runs: &mut Vec<ColourRun>, column: usize, length: usize, colours: (u32, u32)) {
    match runs.last_mut() {
        Some(run) if (run.foreground, run.background) == colours => run.length += length,
        _ => runs.push(ColourRun {
            column,
            length,
            foreground: colours.0,
            background: colours.1,
        }),
    }
}

/// A colour as a program answered it.
fn colour_of(value: &Value) -> u32 {
    match value {
        Value::Int(colour) => colour.cast_unsigned(),
        Value::Uint(colour) => *colour,
        _ => NOT_A_NUMBER,
    }
}

/// The rows the programs' answers describe, appended to `rows`: for each
/// row a flag saying whether it is of one colour, followed by its two
/// colours; or, for a row of several, for each chunk a flag, followed by
/// its two colours when it is of one, and by every cell's two otherwise.
/// `None` when the answer ends early.
fn decode(
    flags: &[Value],
    values: &[Value],
    width: usize,
    rows: &mut Vec<Vec<ColourRun>>,
) -> Option<()> {
    let mut flags = flags.iter().map(|flag| matches!(flag, Value::Bool(true)));
    let mut values = values.iter().map(colour_of);
    let mut pair = || Some((values.next()?, values.next()?));
    while let Some(whole) = flags.next() {
        let mut runs = Vec::new();
        if whole {
            push(&mut runs, 0, width, pair()?);
        } else {
            for (column, length) in chunks(width) {
                if flags.next()? {
                    push(&mut runs, column, length, pair()?);
                } else {
                    for cell in column..column + length {
                        push(&mut runs, cell, 1, pair()?);
                    }
                }
            }
        }
        rows.push(runs);
    }
    Some(())
}

/// The registers a colour read's answer is read from.
#[derive(Clone, Copy)]
pub(crate) struct ColourRegisters {
    flags: Reg<kind::Array>,
    values: Reg<kind::Array>,
    /// The start of the first row not read.
    row: Reg<kind::TextRange>,
    /// Whether the rows reached the limit.
    done: Reg<kind::Bool>,
}

/// The constants the colour code uses.
struct Constants {
    start: Reg<kind::Int>,
    end: Reg<kind::Int>,
    character: Reg<kind::Int>,
    line: Reg<kind::Int>,
    one: Reg<kind::Int>,
    zero: Reg<kind::Int>,
    foreground: Reg<kind::Int>,
    background: Reg<kind::Int>,
}

impl Constants {
    fn new(b: &mut Builder) -> Self {
        Self {
            start: b.int(0),
            end: b.int(1),
            character: b.int(TextUnit_Character.0),
            line: b.int(TextUnit_Line.0),
            one: b.int(1),
            zero: b.int(0),
            foreground: b.int(UIA_ForegroundColorAttributeId.0),
            background: b.int(UIA_BackgroundColorAttributeId.0),
        }
    }
}

/// The most instructions each part of a row's read executes, measured from
/// the code emitted: the row's own part, and each chunk's when it is of one
/// colour and when it is walked.
#[derive(Clone, Default)]
struct Costs {
    row: usize,
    uniform: Vec<usize>,
    walked: Vec<usize>,
}

impl Costs {
    /// A row's worst case: every chunk walked.
    fn worst(&self) -> usize {
        self.row + self.walked.iter().sum::<usize>()
    }
}

fn int(b: &mut Builder, value: usize) -> Reg<kind::Int> {
    b.int(i32::try_from(value).unwrap_or(i32::MAX))
}

/// What a check of a stretch found: whether it is of one foreground and
/// background, and its last cell's two colours.
struct Checked {
    uniform: Reg<kind::Bool>,
    last: (Reg<kind::Any>, Reg<kind::Any>),
}

/// Emits the check of `stretch`, the cells of the row starting at `row`
/// up to `to`: whether they are of one foreground and background, appended
/// to `flags`, and when they are, its colours appended to `values`. The
/// last cell is made as the conformance runner makes a cell, its end moved
/// from the row's start and then its start, so it reads as the runner's
/// does on the buffer's last row, where moving by characters stops a cell
/// short. Only numbers are compared, since comparing UIA's "mixed" fails
/// the program; `whole`, when given, must be true for the stretch to count
/// as of one colour.
fn emit_check(
    b: &mut Builder,
    c: &Constants,
    (row, stretch, to): (Reg<kind::TextRange>, Reg<kind::TextRange>, usize),
    whole: Option<Reg<kind::Bool>>,
    (flags, values): (Reg<kind::Array>, Reg<kind::Array>),
) -> Checked {
    let last = b.text_range_clone(row);
    let end = int(b, to);
    let _ = b.text_range_move_endpoint_by_unit(last, c.end, c.character, end);
    let start = int(b, to - 1);
    let _ = b.text_range_move_endpoint_by_unit(last, c.start, c.character, start);
    let foreground = b.text_range_get_attribute_value(stretch, c.foreground);
    let background = b.text_range_get_attribute_value(stretch, c.background);
    let last_foreground = b.text_range_get_attribute_value(last, c.foreground);
    let last_background = b.text_range_get_attribute_value(last, c.background);
    let numbers = [foreground, background, last_foreground, last_background]
        .map(|value| b.is(TypeTest::Int, value));
    let stretch_numbers = b.and(numbers[0], numbers[1]);
    let last_numbers = b.and(numbers[2], numbers[3]);
    let mut numbers = b.and(stretch_numbers, last_numbers);
    if let Some(whole) = whole {
        numbers = b.and(numbers, whole);
    }
    let uniform = b.new_bool(false);
    b.if_(numbers, |b| {
        // All four were proven integers by `numbers`; equality takes no `Any`.
        let same_foreground = b.equal(foreground.assume::<kind::Int>(), last_foreground.assume());
        let same_background = b.equal(background.assume::<kind::Int>(), last_background.assume());
        let same = b.and(same_foreground, same_background);
        b.set(uniform, same);
    });
    b.array_append(flags, uniform);
    b.if_(uniform, |b| {
        b.array_append(values, foreground);
        b.array_append(values, background);
    });
    Checked {
        uniform,
        last: (last_foreground, last_background),
    }
}

/// Emits the reads of a row starting at `row` (collapsed), adding to
/// `spent` the instructions each part executes at most by `costs`, and
/// returns the costs as emitted. A first pass with no costs measures them.
///
/// On the buffer's last row (the alternate screen's, and the main
/// screen's once the history is full) moving by characters stops a cell
/// short of the width, so a stretch to the row's end leaves out its last
/// two cells and cannot vouch for them: there the row and its last chunk
/// are read cell by cell.
fn emit_row(
    b: &mut Builder,
    c: &Constants,
    (row, spent): (Reg<kind::TextRange>, Reg<kind::Int>),
    (flags, values): (Reg<kind::Array>, Reg<kind::Array>),
    width: usize,
    costs: &Costs,
) -> Costs {
    let mut measured = Costs::default();
    let row_cost = int(b, costs.row);
    b.add_assign(spent, row_cost);
    let whole = b.text_range_clone(row);
    let cells = int(b, width);
    let moved = b.text_range_move_endpoint_by_unit(whole, c.end, c.character, cells);
    let full = b.compare(moved, cells, Comparison::Equal);
    let checked = emit_check(b, c, (row, whole, width), Some(full), (flags, values));
    let several = b.not(checked.uniform);
    let chunks = chunks(width);
    let count = chunks.len();
    b.if_(several, |b| {
        for (index, (column, length)) in chunks.into_iter().enumerate() {
            let begun = b.emitted_count();
            let chunk = b.text_range_clone(row);
            let end = int(b, column + length);
            let _ = b.text_range_move_endpoint_by_unit(chunk, c.end, c.character, end);
            if column > 0 {
                let start = int(b, column);
                let _ = b.text_range_move_endpoint_by_unit(chunk, c.start, c.character, start);
            }
            let last_chunk = (index + 1 == count).then_some(full);
            let checked = emit_check(
                b,
                c,
                (row, chunk, column + length),
                last_chunk,
                (flags, values),
            );
            let fork = b.emitted_count() + 1;
            b.if_else(
                checked.uniform,
                |b| {
                    let cost = int(b, costs.uniform.get(index).copied().unwrap_or(0));
                    b.add_assign(spent, cost);
                    measured.uniform.push(b.emitted_count() + 1 - begun);
                },
                |b| {
                    let walked = b.emitted_count();
                    // Every cell but the last by moving one cell at a
                    // time; the last as the check read it.
                    if length > 1 {
                        let cell = b.text_range_clone(chunk);
                        b.text_range_move_endpoint_by_range(cell, c.end, cell, c.start);
                        let _ = b.text_range_move_endpoint_by_unit(cell, c.end, c.character, c.one);
                        for at in 0..length - 1 {
                            let foreground = b.text_range_get_attribute_value(cell, c.foreground);
                            let background = b.text_range_get_attribute_value(cell, c.background);
                            b.array_append(values, foreground);
                            b.array_append(values, background);
                            if at + 2 < length {
                                let _ = b.text_range_move(cell, c.character, c.one);
                            }
                        }
                    }
                    b.array_append(values, checked.last.0);
                    b.array_append(values, checked.last.1);
                    let cost = int(b, costs.walked.get(index).copied().unwrap_or(0));
                    b.add_assign(spent, cost);
                    measured
                        .walked
                        .push(fork - begun + b.emitted_count() - walked);
                },
            );
        }
    });
    measured
}

/// Emits the loop over rows from `row` (collapsed at a row's start) to
/// `limit` (collapsed), within `budget` instructions, and returns its
/// registers. `row` is moved as the rows are read.
pub(crate) fn emit_rows(
    b: &mut Builder,
    row: Reg<kind::TextRange>,
    limit: Reg<kind::TextRange>,
    width: usize,
) -> ColourRegisters {
    let costs = {
        // A first pass, into a builder of its own, measures what each part
        // of a row executes at most.
        let mut scratch = Builder::new();
        let c = Constants::new(&mut scratch);
        let row = scratch.new_null().assume::<kind::TextRange>();
        let spent = scratch.new_int(0);
        let flags = scratch.new_array();
        let values = scratch.new_array();
        let mut loop_cost = 0;
        let mut measured = Costs::default();
        emit_loop(
            &mut scratch,
            &c,
            (row, row),
            (flags, values, spent),
            width,
            &Costs::default(),
            &mut |emitted, row_costs| {
                loop_cost = emitted;
                measured = row_costs;
            },
        );
        let chunk_parts: usize = measured.walked.iter().sum();
        Costs {
            // The loop's own instructions and the row's, all but the
            // chunks', each executed at most once a row.
            row: loop_cost - chunk_parts.min(loop_cost),
            ..measured
        }
    };
    let c = Constants::new(b);
    let flags = b.new_array();
    let flags = b.add_to_results(flags);
    let values = b.new_array();
    let values = b.add_to_results(values);
    let spent = b.new_int(0);
    let row = b.add_to_results(row);
    let done = emit_loop(
        b,
        &c,
        (row, limit),
        (flags, values, spent),
        width,
        &costs,
        &mut |_, _| {},
    );
    ColourRegisters {
        flags,
        values,
        row,
        done,
    }
}

/// Emits the rows' loop, reporting how many instructions it emitted and
/// the costs its row measured; returns the register saying whether the
/// rows reached the limit.
fn emit_loop(
    b: &mut Builder,
    c: &Constants,
    (row, limit): (Reg<kind::TextRange>, Reg<kind::TextRange>),
    (flags, values, spent): (Reg<kind::Array>, Reg<kind::Array>, Reg<kind::Int>),
    width: usize,
    costs: &Costs,
    report: &mut dyn FnMut(usize, Costs),
) -> Reg<kind::Bool> {
    let done = b.new_bool(false);
    let done = b.add_to_results(done);
    let yes = b.bool(true);
    let worst = int(b, costs.worst());
    let budget = int(b, COLOUR_BUDGET);
    // A start already at the limit reads nothing.
    let order = b.text_range_compare_endpoints(row, c.start, limit, c.start);
    let past = b.compare(order, c.zero, Comparison::GreaterThanOrEqual);
    b.set(done, past);
    let going = b.not(past);
    let begun = b.emitted_count();
    let mut row_costs = Costs::default();
    b.while_(
        |_| going,
        |b| {
            row_costs = emit_row(b, c, (row, spent), (flags, values), width, costs);
            // Past the limit, or on the last row of a provider that moves no
            // further than it.
            let moved = b.text_range_move(row, c.line, c.one);
            let order = b.text_range_compare_endpoints(row, c.start, limit, c.start);
            let past = b.compare(order, c.zero, Comparison::GreaterThanOrEqual);
            let stuck = b.equal(moved, c.zero);
            let past = b.or(past, stuck);
            b.if_(past, |b| {
                b.set(done, yes);
                b.break_loop();
            });
            let total = b.add(spent, worst);
            let over = b.compare(total, budget, Comparison::GreaterThan);
            b.if_(over, Builder::break_loop);
        },
    );
    report(b.emitted_count() - begun, row_costs);
    done
}

thread_local! {
    /// The colour programs that go on from a row, one for each width.
    static GO_ON: ProgramCache<usize, ColourRegisters> = ProgramCache::default();
}

/// What one colour program answered, and where a classic read or the next
/// program starts.
pub(crate) struct ColourBatch {
    /// The start of the first row not read.
    pub(crate) row: Option<IUIAutomationTextRange>,
    /// Whether the rows reached the limit.
    pub(crate) done: bool,
}

impl ColourRegisters {
    /// Reads a program's answer, appending its rows to `rows`.
    pub(crate) fn read(
        &self,
        outcome: &Outcome,
        width: usize,
        rows: &mut Vec<Vec<ColourRun>>,
    ) -> Result<ColourBatch, Error> {
        let flags = outcome.get(self.flags)?;
        let values = outcome.get(self.values)?;
        decode(&flags, &values, width, rows).ok_or_else(|| Error::ResultType {
            operand: self.values.id(),
            error: windows::core::Error::from(windows::Win32::Foundation::E_UNEXPECTED),
        })?;
        Ok(ColourBatch {
            row: outcome.get(self.row)?,
            done: outcome.get(self.done)?,
        })
    }
}

/// Fails when a test armed the colour program that goes on after `read`
/// rows to fail (`verbatim_faults::Fault::ColourProgramFails`).
#[cfg(any(test, feature = "faults"))]
fn injected_failure(read: usize) -> Result<(), Error> {
    if verbatim_faults::hit(verbatim_faults::Fault::ColourProgramFails, &read) {
        return Err(Error::Execute(windows::core::Error::new(
            windows::Win32::Foundation::E_FAIL,
            "a test failed the colour program",
        )));
    }
    Ok(())
}

/// No test arms a failure in a build without fault points.
#[cfg(not(any(test, feature = "faults")))]
#[expect(
    clippy::unnecessary_wraps,
    reason = "the signature of the build with fault points"
)]
const fn injected_failure(_: usize) -> Result<(), Error> {
    Ok(())
}

/// Reads the rows from `row` on, program after program, until they reach
/// `limit` or `height` rows are read in all, appending them to `rows`.
pub(crate) fn go_on_remote(
    mut batch: ColourBatch,
    limit: &IUIAutomationTextRange,
    (width, height): (usize, usize),
    rows: &mut Vec<Vec<ColourRun>>,
) -> Result<(), Error> {
    while !batch.done && rows.len() < height {
        let Some(row) = batch.row.take() else {
            break;
        };
        let (operation, registers) = GO_ON.with(|programs| {
            programs.operation(
                width,
                || {
                    vec![
                        Import::TextRange(row.clone()),
                        Import::TextRange(limit.clone()),
                    ]
                },
                |b| {
                    let row = b.import_text_range(&row);
                    let limit = b.import_text_range(limit);
                    emit_rows(b, row, limit, width)
                },
            )
        });
        let before = rows.len();
        injected_failure(before)?;
        batch = registers.read(&operation.execute()?, width, rows)?;
        if rows.len() == before {
            break;
        }
    }
    rows.truncate(height);
    Ok(())
}

/// A stretch's two colours classically, `None` when either is not a
/// number.
fn colours_classic(range: &IUIAutomationTextRange) -> Result<Option<(u32, u32)>, Error> {
    let values = range.attributes(&[
        UIA_ForegroundColorAttributeId,
        UIA_BackgroundColorAttributeId,
    ])?;
    let number = |at: usize| {
        values
            .get(at)
            .and_then(|value| i32::try_from(value).ok())
            .map(i32::cast_unsigned)
    };
    Ok(number(0).zip(number(1)))
}

/// A cell of the row starting at `row`, made as the conformance runner
/// makes it: its end moved from the row's start, then its start.
fn cell_classic(
    row: &IUIAutomationTextRange,
    column: usize,
) -> Result<IUIAutomationTextRange, Error> {
    let count = |cells: usize| i32::try_from(cells).unwrap_or(i32::MAX);
    let cell = row.clone_range()?;
    cell.move_endpoint_by_unit(Endpoint::End, TextUnit_Character, count(column + 1))?;
    if column > 0 {
        cell.move_endpoint_by_unit(Endpoint::Start, TextUnit_Character, count(column))?;
    }
    Ok(cell)
}

/// Whether `stretch`, the cells of the row starting at `row` up to `to`,
/// is of one foreground and background classically, as the program checks
/// it: its colours when it is, and its last cell's either way.
fn check_classic(
    row: &IUIAutomationTextRange,
    stretch: &IUIAutomationTextRange,
    to: usize,
    whole: bool,
) -> Result<(Option<Colours>, Colours), Error> {
    let colours = colours_classic(stretch)?;
    let last = colours_classic(&cell_classic(row, to - 1)?)?;
    let uniform = colours.filter(|_| whole && colours == last);
    Ok((uniform, last.unwrap_or((NOT_A_NUMBER, NOT_A_NUMBER))))
}

/// The colours of the rows from `row`, collapsed at a row's start, to
/// `limit`, collapsed, `width` cells each, at most `height` rows, the
/// classic way: the same checks as the programs, each a cross-process
/// call. `row` is moved as the rows are read.
pub(crate) fn classic_rows(
    row: &IUIAutomationTextRange,
    limit: &IUIAutomationTextRange,
    width: usize,
    height: usize,
) -> Result<Vec<Vec<ColourRun>>, Error> {
    let count = |cells: usize| i32::try_from(cells).unwrap_or(i32::MAX);
    let mut rows = Vec::new();
    while rows.len() < height && row.compare_endpoints(Endpoint::Start, limit, Endpoint::Start)? < 0
    {
        let mut runs = Vec::new();
        let whole = row.clone_range()?;
        let full = whole.move_endpoint_by_unit(Endpoint::End, TextUnit_Character, count(width))?
            == count(width);
        if let (Some(colours), _) = check_classic(row, &whole, width, full)? {
            push(&mut runs, 0, width, colours);
        } else {
            let chunks = chunks(width);
            let last_chunk = chunks.len() - 1;
            for (index, (column, length)) in chunks.into_iter().enumerate() {
                let chunk = row.clone_range()?;
                chunk.move_endpoint_by_unit(
                    Endpoint::End,
                    TextUnit_Character,
                    count(column + length),
                )?;
                if column > 0 {
                    chunk.move_endpoint_by_unit(
                        Endpoint::Start,
                        TextUnit_Character,
                        count(column),
                    )?;
                }
                let (uniform, last) =
                    check_classic(row, &chunk, column + length, full || index < last_chunk)?;
                if let Some(colours) = uniform {
                    push(&mut runs, column, length, colours);
                    continue;
                }
                if length > 1 {
                    let cell = chunk.clone_range()?;
                    cell.move_endpoint_to(Endpoint::End, &cell, Endpoint::Start)?;
                    cell.move_endpoint_by_unit(Endpoint::End, TextUnit_Character, 1)?;
                    for at in column..column + length - 1 {
                        let colours =
                            colours_classic(&cell)?.unwrap_or((NOT_A_NUMBER, NOT_A_NUMBER));
                        push(&mut runs, at, 1, colours);
                        if at + 2 < column + length {
                            cell.move_by(TextUnit_Character, 1)?;
                        }
                    }
                }
                push(&mut runs, column + length - 1, 1, last);
            }
        }
        rows.push(runs);
        if row.move_by(TextUnit_Line, 1)? == 0 {
            break;
        }
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::{ColourRun, Value, decode};

    #[test]
    fn a_rows_answer_decodes_into_runs_joined_where_colours_agree() {
        let int = |value: i32| Value::Int(value);
        // Row one: one colour. Row two, 20 cells: its first chunk walked,
        // cells of colours 1, 1, 2 and then 3; its second chunk of one
        // colour, 3.
        let flags = [true, false, false, true].map(Value::Bool);
        let mut values = vec![int(9), int(0)];
        for cell in 0..16 {
            let colour = match cell {
                0 | 1 => 1,
                2 => 2,
                _ => 3,
            };
            values.extend([int(colour), int(0)]);
        }
        values.extend([int(3), int(0)]);
        let mut rows = Vec::new();
        decode(&flags, &values, 20, &mut rows).expect("a well-formed answer");
        let run = |column, length, foreground| ColourRun {
            column,
            length,
            foreground,
            background: 0,
        };
        assert_eq!(
            rows,
            vec![
                vec![run(0, 20, 9)],
                vec![run(0, 2, 1), run(2, 1, 2), run(3, 17, 3)]
            ]
        );
    }
}
