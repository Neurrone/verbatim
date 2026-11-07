//! A comctl32 list view's columns, read through its window messages, as
//! NVDA's `sysListView32.py` reads them out of process (this crate is GPL
//! like NVDA): an item of a list view that shows columns (the report view,
//! or tiles) is named by its columns' texts in the order they are shown,
//! each after its column's header but the first, "content; Header:
//! content", skipping a column of zero width.
//!
//! The item, column, and rectangle structures these messages take are not
//! marshalled by Windows, so they are written into memory allocated in the
//! list view's process (`crate::edit::TargetProcess`), with pointer fields
//! sized for it: one buffer per message, its structure at its start and
//! its text 16-byte aligned after it, freed by the thread that sent the
//! message once the send has returned (`crate::edit::RemoteBuffer`). Every
//! message goes through `crate::window::send_message`, so a hung window
//! fails at once and the message has no time limit of its own, and counts
//! as one window message.

use crate::edit::{EditError, RemoteBuffer, TargetProcess, message_error, text_offset};
use crate::window;

const LVM_FIRST: u32 = 0x1000;
pub(crate) const LVM_GETHEADER: u32 = LVM_FIRST + 31;
pub(crate) const LVM_GETSUBITEMRECT: u32 = LVM_FIRST + 56;
pub(crate) const LVM_GETCOLUMNORDERARRAY: u32 = LVM_FIRST + 59;
pub(crate) const LVM_GETCOLUMNW: u32 = LVM_FIRST + 95;
pub(crate) const LVM_GETITEMTEXTW: u32 = LVM_FIRST + 115;
pub(crate) const LVM_GETVIEW: u32 = LVM_FIRST + 143;
pub(crate) const HDM_GETITEMCOUNT: u32 = 0x1200;

const LV_VIEW_DETAILS: isize = 1;
const LV_VIEW_TILE: isize = 4;
const LVS_TYPEMASK: isize = 0x3;
const LVS_REPORT: isize = 0x1;
const LVS_OWNERDRAWFIXED: isize = 0x400;
const LVIF_TEXT: u32 = 0x1;
const LVCF_TEXT: u32 = 0x4;
const LVIR_LABEL: i32 = 2;

/// The longest text read from a column or its header, in UTF-16 units:
/// NVDA's `CBEMAXSTRLEN`.
const MAX_TEXT: usize = 260;

/// The left-to-right and right-to-left marks some list views put into
/// their texts, which NVDA removes.
const DIRECTION_MARKS: [char; 2] = ['\u{200e}', '\u{200f}'];

/// Sends `msg` to the list view, its parameters integers.
fn send(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> Result<isize, EditError> {
    // SAFETY: the parameters are integers.
    unsafe { window::send_message(hwnd, msg, (wparam, lparam), None) }
        .map_err(|unanswered| message_error(unanswered, "the list view", msg))
}

/// Sends `msg` to the list view carrying `buffer`, whose address is
/// `lparam`.
fn send_with(
    hwnd: isize,
    msg: u32,
    wparam: usize,
    buffer: &RemoteBuffer<'_>,
) -> Result<isize, EditError> {
    // SAFETY: the parameters are an integer and the address of `buffer`,
    // memory allocated in the target for this message, which lives until
    // after the send returns.
    unsafe { window::send_message(hwnd, msg, (wparam, buffer.address()), Some(buffer.sent())) }
        .map_err(|unanswered| message_error(unanswered, "the list view", msg))
}

/// Whether the list view `hwnd` shows its items in columns, as NVDA's
/// `isMultiColumn` asks: the report view or tiles, or, when the control
/// answers no view (comctl32 before version 6), the report style.
fn is_multi_column(hwnd: isize) -> Result<bool, EditError> {
    Ok(match send(hwnd, LVM_GETVIEW, 0, 0)? {
        LV_VIEW_DETAILS | LV_VIEW_TILE => true,
        0 => window::style(hwnd) & LVS_TYPEMASK == LVS_REPORT,
        _ => false,
    })
}

/// The name NVDA gives item `child_id` (1-based) of the list view `hwnd`
/// from its columns, or `None` when the list view does not show columns,
/// or is owner drawn with no column text (the item's description, which
/// `description` reads and only an owner-drawn list view asks for, empty:
/// NVDA's sign that it has none), so the item keeps its own name. Columns
/// that cannot be read are left out.
///
/// # Errors
///
/// The list view's error when it is gone, is not responding, or does not
/// answer.
pub fn column_name(
    hwnd: isize,
    child_id: i32,
    description: impl FnOnce() -> Option<String>,
) -> Result<Option<String>, EditError> {
    if window::style(hwnd) & LVS_OWNERDRAWFIXED != 0 && description().is_none_or(|d| d.is_empty()) {
        return Ok(None);
    }
    if !is_multi_column(hwnd)? {
        return Ok(None);
    }
    let Some(item) = usize::try_from(child_id)
        .ok()
        .and_then(|id| id.checked_sub(1))
    else {
        return Ok(None);
    };
    let header = send(hwnd, LVM_GETHEADER, 0, 0)?;
    let columns = match send(header, HDM_GETITEMCOUNT, 0, 0)? {
        count if count > 0 => usize::try_from(count).unwrap_or(1),
        _ => 1,
    };
    let process = TargetProcess::open(crate::edit::window_process(hwnd)?)?;
    let order = column_order(hwnd, &process, columns)?;
    let mut texts = Vec::new();
    for (position, &column) in order.iter().enumerate() {
        if column_width(hwnd, &process, item, column)? == Some(0) {
            continue;
        }
        let Some(content) = item_text(hwnd, &process, item, column)? else {
            continue;
        };
        let header = if position == 0 {
            None
        } else {
            header_text(hwnd, &process, column)?
        };
        texts.push(match header {
            Some(header) => format!("{header}: {content}"),
            None => content,
        });
    }
    Ok(Some(texts.join("; ").replace(DIRECTION_MARKS, "")))
}

/// The columns' indexes in the order they are shown, from left to right
/// (`LVM_GETCOLUMNORDERARRAY`); a single column is column 0.
fn column_order(
    hwnd: isize,
    process: &TargetProcess,
    columns: usize,
) -> Result<Vec<usize>, EditError> {
    if columns == 1 {
        return Ok(vec![0]);
    }
    let buffer = process.allocate(columns * 4)?;
    if send_with(hwnd, LVM_GETCOLUMNORDERARRAY, columns, &buffer)? == 0 {
        return Err(EditError::Failed("no column order".to_owned()));
    }
    let bytes = buffer.read(0, columns * 4)?;
    Ok(bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|&chunk| usize::try_from(i32::from_le_bytes(chunk)).unwrap_or(0))
        .collect())
}

/// The width of `column` of `item` (`LVM_GETSUBITEMRECT`), `None` when the
/// control does not say.
fn column_width(
    hwnd: isize,
    process: &TargetProcess,
    item: usize,
    column: usize,
) -> Result<Option<i32>, EditError> {
    let mut rect = [0u8; 16];
    rect[..4].copy_from_slice(&LVIR_LABEL.to_le_bytes());
    rect[4..8].copy_from_slice(&i32::try_from(column).unwrap_or(0).to_le_bytes());
    let buffer = process.allocate(rect.len())?;
    buffer.write(0, &rect)?;
    if send_with(hwnd, LVM_GETSUBITEMRECT, item, &buffer)? == 0 {
        return Ok(None);
    }
    let bytes = buffer.read(0, 16)?;
    let field =
        |at: usize| i32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
    Ok(Some(field(8) - field(0)))
}

/// The text of `column` of `item` (`LVM_GETITEMTEXTW`), `None` when it
/// has none.
fn item_text(
    hwnd: isize,
    process: &TargetProcess,
    item: usize,
    column: usize,
) -> Result<Option<String>, EditError> {
    // `LVITEMW`: mask, item, subitem, state, state mask, then the text
    // pointer, aligned for the process, and the text's capacity; the text
    // after it.
    let mut structure = vec![0u8; 96];
    let text_at = text_offset(structure.len());
    let buffer = process.allocate(text_at + MAX_TEXT * 2)?;
    let pointer_at = if process.pointer_size == 8 { 24 } else { 20 };
    structure[..4].copy_from_slice(&LVIF_TEXT.to_le_bytes());
    structure[8..12].copy_from_slice(&i32::try_from(column).unwrap_or(0).to_le_bytes());
    write_pointer(
        &mut structure,
        pointer_at,
        buffer.address_at(text_at),
        process.pointer_size,
    );
    let capacity = i32::try_from(MAX_TEXT).unwrap_or(0);
    let capacity_at = pointer_at + process.pointer_size;
    structure[capacity_at..capacity_at + 4].copy_from_slice(&capacity.to_le_bytes());
    buffer.write(0, &structure)?;
    let length = send_with(hwnd, LVM_GETITEMTEXTW, item, &buffer)?;
    let length = usize::try_from(length).unwrap_or(0).min(MAX_TEXT);
    read_text(&buffer, text_at, length)
}

/// The header text of `column` (`LVM_GETCOLUMNW`), `None` when it has none.
fn header_text(
    hwnd: isize,
    process: &TargetProcess,
    column: usize,
) -> Result<Option<String>, EditError> {
    // `LVCOLUMNW`: mask, format, width, then the text pointer, aligned for
    // the process, and the text's capacity; the text after it.
    let mut structure = vec![0u8; 64];
    let text_at = text_offset(structure.len());
    let buffer = process.allocate(text_at + MAX_TEXT * 2)?;
    let pointer_at = if process.pointer_size == 8 { 16 } else { 12 };
    structure[..4].copy_from_slice(&LVCF_TEXT.to_le_bytes());
    write_pointer(
        &mut structure,
        pointer_at,
        buffer.address_at(text_at),
        process.pointer_size,
    );
    let capacity = i32::try_from(MAX_TEXT).unwrap_or(0);
    let capacity_at = pointer_at + process.pointer_size;
    structure[capacity_at..capacity_at + 4].copy_from_slice(&capacity.to_le_bytes());
    buffer.write(0, &structure)?;
    if send_with(hwnd, LVM_GETCOLUMNW, column, &buffer)? == 0 {
        return Ok(None);
    }
    read_text(&buffer, text_at, MAX_TEXT)
}

/// Writes `address` as a pointer of `size` bytes at `at`.
fn write_pointer(structure: &mut [u8], at: usize, address: isize, size: usize) {
    let bytes = address.to_le_bytes();
    structure[at..at + size].copy_from_slice(&bytes[..size]);
}

/// The text of up to `length` UTF-16 units at `offset` in `buffer`, up to
/// its first null, `None` when empty.
fn read_text(
    buffer: &RemoteBuffer<'_>,
    offset: usize,
    length: usize,
) -> Result<Option<String>, EditError> {
    let bytes = buffer.read(offset, length * 2)?;
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&pair| u16::from_le_bytes(pair))
        .take_while(|&unit| unit != 0)
        .collect();
    Ok(Some(String::from_utf16_lossy(&units)).filter(|text| !text.is_empty()))
}

#[cfg(all(test, target_pointer_width = "64"))]
mod tests {
    use std::mem::offset_of;

    use windows::Win32::UI::Controls::{LVCOLUMNW, LVITEMW};

    /// The offsets written into a 64-bit process are the structures' own.
    #[test]
    fn the_structures_are_laid_out_as_windows_lays_them_out() {
        assert_eq!(offset_of!(LVITEMW, pszText), 24);
        assert_eq!(offset_of!(LVITEMW, cchTextMax), 32);
        assert_eq!(offset_of!(LVCOLUMNW, pszText), 16);
        assert_eq!(offset_of!(LVCOLUMNW, cchTextMax), 24);
    }
}
