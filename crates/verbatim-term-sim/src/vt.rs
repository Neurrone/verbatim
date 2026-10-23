//! The VT sequences the simulated terminal understands, mapped onto the
//! grid's operations (`docs/design/terminal-model.md`, section 3.2). What
//! it does not understand it ignores, as a terminal does; the conformance
//! expectations show where that differs from the real terminals.

use vte::{Params, Perform};

use crate::grid::{Colour, Grid, PALETTE};

/// Feeds parsed sequences to a grid.
pub(crate) struct Performer<'a> {
    pub(crate) grid: &'a mut Grid,
}

/// The first value of each parameter, with `default` for one missing or 0.
fn values(params: &Params, default: u16) -> Vec<u16> {
    let mut values: Vec<u16> = params
        .iter()
        .map(|param| match param.first() {
            Some(&0) | None => default,
            Some(&value) => value,
        })
        .collect();
    if values.is_empty() {
        values.push(default);
    }
    values
}

/// A count as a signed number of rows.
fn signed(count: usize) -> isize {
    isize::try_from(count).unwrap_or(isize::MAX)
}

/// A 256-colour palette index as UI Automation reports it.
fn indexed(index: u16) -> Colour {
    let index = usize::from(index);
    if let Some(&colour) = PALETTE.get(index) {
        return colour;
    }
    if (16..232).contains(&index) {
        let cube = index - 16;
        let level = |value: usize| -> u32 {
            if value == 0 {
                0
            } else {
                u32::try_from(55 + value * 40).unwrap_or(255)
            }
        };
        let (red, green, blue) = (level(cube / 36), level(cube / 6 % 6), level(cube % 6));
        return red | green << 8 | blue << 16;
    }
    let grey = u32::try_from(8 + (index.saturating_sub(232)) * 10).unwrap_or(255);
    grey | grey << 8 | grey << 16
}

impl Performer<'_> {
    /// Select graphic rendition.
    fn sgr(&mut self, params: &Params) {
        let flat: Vec<u16> = params
            .iter()
            .flat_map(|param| param.iter().copied())
            .collect();
        if flat.is_empty() {
            self.grid.style = crate::grid::Style::default();
            return;
        }
        let mut at = 0;
        while at < flat.len() {
            let code = flat[at];
            match code {
                0 => self.grid.style = crate::grid::Style::default(),
                1 => self.grid.style.bold = true,
                2 => self.grid.style.faint = true,
                22 => {
                    self.grid.style.bold = false;
                    self.grid.style.faint = false;
                }
                7 => self.grid.style.reverse = true,
                27 => self.grid.style.reverse = false,
                30..=37 => {
                    self.grid.style.foreground = PALETTE[usize::from(code - 30)];
                    self.grid.style.brightens = true;
                }
                39 => {
                    self.grid.style.foreground = crate::grid::DEFAULT_FOREGROUND;
                    self.grid.style.brightens = true;
                }
                40..=47 => self.grid.style.background = PALETTE[usize::from(code - 40)],
                49 => self.grid.style.background = crate::grid::DEFAULT_BACKGROUND,
                90..=97 => {
                    self.grid.style.foreground = PALETTE[usize::from(code - 90 + 8)];
                    self.grid.style.brightens = false;
                }
                100..=107 => self.grid.style.background = PALETTE[usize::from(code - 100 + 8)],
                38 | 48 => {
                    let colour = match flat.get(at + 1) {
                        Some(5) => {
                            let colour = flat.get(at + 2).map(|&index| indexed(index));
                            at += 2;
                            colour
                        }
                        Some(2) => {
                            let part = |offset: usize| {
                                u32::from(flat.get(at + offset).copied().unwrap_or(0).min(255))
                            };
                            let colour = part(2) | part(3) << 8 | part(4) << 16;
                            at += 4;
                            Some(colour)
                        }
                        _ => None,
                    };
                    if let Some(colour) = colour {
                        if code == 38 {
                            self.grid.style.foreground = colour;
                            self.grid.style.brightens = false;
                        } else {
                            self.grid.style.background = colour;
                        }
                    }
                }
                _ => {}
            }
            at += 1;
        }
    }

    /// Set or reset a private mode.
    fn private_mode(&mut self, params: &Params, set: bool) {
        for mode in values(params, 0) {
            match mode {
                5 => self.grid.screen_reversed = set,
                25 => self.grid.cursor.visible = set,
                47 | 1047 => {
                    if set {
                        self.grid.enter_alternate(false);
                    } else {
                        self.grid.leave_alternate(false);
                    }
                }
                1049 => {
                    if set {
                        self.grid.enter_alternate(true);
                    } else {
                        self.grid.leave_alternate(true);
                    }
                }
                _ => {}
            }
        }
    }
}

impl Perform for Performer<'_> {
    fn print(&mut self, character: char) {
        self.grid.print(character);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' | 0x0b | 0x0c => self.grid.explicit_line_feed(),
            b'\r' => self.grid.carriage_return(),
            0x08 => {
                self.grid.cursor.pending_wrap = false;
                self.grid.cursor.column = self.grid.cursor.column.saturating_sub(1);
            }
            b'\t' => {
                let next = (self.grid.cursor.column / 8 + 1) * 8;
                self.grid.cursor.column = next.min(self.grid.width - 1);
            }
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, action: char) {
        if intermediates == b"?" {
            match action {
                'h' => self.private_mode(params, true),
                'l' => self.private_mode(params, false),
                _ => {}
            }
            return;
        }
        if !intermediates.is_empty() {
            return;
        }
        let first = usize::from(values(params, 1)[0]);
        let grid = &mut *self.grid;
        let (row, column) = (grid.cursor.row, grid.cursor.column);
        match action {
            'A' => grid.move_vertically(-signed(first)),
            'B' => grid.move_vertically(signed(first)),
            // Vertical position relative is an absolute move from the
            // cursor's row, so the margins do not stop it (both terminals,
            // measured 2026-10-23).
            'e' => grid.move_to(row + first, column),
            'C' | 'a' => grid.move_to(row, column + first),
            'D' => grid.move_to(row, column.saturating_sub(first)),
            'E' => {
                grid.move_vertically(signed(first));
                grid.carriage_return();
            }
            'F' => {
                grid.move_vertically(-signed(first));
                grid.carriage_return();
            }
            'G' | '`' => grid.move_to(row, first - 1),
            'd' => grid.move_to(first - 1, column),
            'H' | 'f' => {
                let both = values(params, 1);
                let column = usize::from(both.get(1).copied().unwrap_or(1));
                grid.move_to(first - 1, column - 1);
            }
            'J' => grid.erase_in_display(values(params, 0)[0]),
            'K' => grid.erase_in_line(values(params, 0)[0]),
            'L' => grid.insert_rows(first),
            'M' => grid.delete_rows(first),
            '@' => grid.insert_cells(first),
            'P' => grid.delete_cells(first),
            'X' => grid.erase_cells(row, column, column + first),
            'S' => grid.scroll_region_up(first),
            'T' => grid.scroll_region_down(first),
            'm' => self.sgr(params),
            'r' => {
                let both = values(params, 1);
                let bottom = usize::from(both.get(1).copied().unwrap_or(0));
                let bottom = if bottom == 0 {
                    self.grid.height
                } else {
                    bottom
                };
                self.grid.set_region(first - 1, bottom - 1);
            }
            's' => grid.save_cursor(),
            'u' => grid.restore_cursor(),
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        if !intermediates.is_empty() {
            return;
        }
        match byte {
            b'7' => self.grid.save_cursor(),
            b'8' => self.grid.restore_cursor(),
            // Index and next line are line feeds the program wrote: the
            // console host clears the row's wrap mark for them as for a line
            // feed (measured 2026-10-23).
            b'D' => self.grid.explicit_line_feed(),
            b'E' => {
                self.grid.carriage_return();
                self.grid.explicit_line_feed();
            }
            b'M' => self.grid.reverse_index(),
            _ => {}
        }
    }
}
