//! A comctl32 list view's columns, read through its window messages, as
//! NVDA's `sysListView32.py` reads them out of process (this crate is GPL
//! like NVDA): an item of a list view that shows columns (the report view,
//! or tiles) is named by its columns' texts in the order they are shown,
//! each after its column's header but the first, "content; Header:
//! content", skipping a column of zero width.
//!
//! The item, column, and rectangle structures these messages take are not
//! marshalled by Windows, so they are written into memory allocated in the
//! list view's process ([`crate::edit::TargetProcess`]), with pointer fields
//! sized for it. Every message goes through `SendMessageTimeoutW`, aborting
//! if the window is hung, and counts as one window message.

use std::ffi::c_void;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    IsWindow, SMTO_ABORTIFHUNG, SMTO_BLOCK, SendMessageTimeoutW,
};

use verbatim_model::CallKind;

use crate::calls::count;
use crate::edit::{EditError, TargetProcess};
use crate::window;

/// How long a message waits for the list view to answer.
const MESSAGE_TIMEOUT_MS: u32 = 500;

const LVM_FIRST: u32 = 0x1000;
const LVM_GETHEADER: u32 = LVM_FIRST + 31;
const LVM_GETSUBITEMRECT: u32 = LVM_FIRST + 56;
const LVM_GETCOLUMNORDERARRAY: u32 = LVM_FIRST + 59;
const LVM_GETCOLUMNW: u32 = LVM_FIRST + 95;
const LVM_GETITEMTEXTW: u32 = LVM_FIRST + 115;
const LVM_GETVIEW: u32 = LVM_FIRST + 143;
const HDM_GETITEMCOUNT: u32 = 0x1200;

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

/// Sends `msg` to the list view, its parameters integers or addresses in
/// the list view's own process.
fn send(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> Result<isize, EditError> {
    count(CallKind::WindowMessage);
    let mut result = 0usize;
    // SAFETY: the parameters are integers or addresses in the target's own
    // memory; any window handle is tolerated.
    let sent = unsafe {
        SendMessageTimeoutW(
            HWND(hwnd as *mut c_void),
            msg,
            WPARAM(wparam),
            LPARAM(lparam),
            SMTO_ABORTIFHUNG | SMTO_BLOCK,
            MESSAGE_TIMEOUT_MS,
            Some(&raw mut result),
        )
    };
    if sent.0 == 0 {
        // SAFETY: IsWindow tolerates any handle.
        return Err(
            if unsafe { IsWindow(Some(HWND(hwnd as *mut c_void))) }.as_bool() {
                EditError::Failed(format!("the list view did not answer message {msg:#x}"))
            } else {
                EditError::Gone
            },
        );
    }
    Ok(result.cast_signed())
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
/// or is owner drawn with no column text (`description` empty, NVDA's
/// sign that it has none), so the item keeps its own name. Columns that
/// cannot be read are left out.
///
/// # Errors
///
/// The list view's error when it is gone or does not answer.
pub(crate) fn column_name(
    hwnd: isize,
    child_id: i32,
    description: Option<&str>,
) -> Result<Option<String>, EditError> {
    if window::style(hwnd) & LVS_OWNERDRAWFIXED != 0 && description.is_none_or(str::is_empty) {
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
    let process = TargetProcess::open(hwnd)?;
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
    if send(hwnd, LVM_GETCOLUMNORDERARRAY, columns, buffer.address())? == 0 {
        return Err(EditError::Failed("no column order".to_owned()));
    }
    let bytes = buffer.read(columns * 4)?;
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
    buffer.write(&rect)?;
    if send(hwnd, LVM_GETSUBITEMRECT, item, buffer.address())? == 0 {
        return Ok(None);
    }
    let bytes = buffer.read(16)?;
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
    let text = process.allocate(MAX_TEXT * 2)?;
    // `LVITEMW`: mask, item, subitem, state, state mask, then the text
    // pointer, aligned for the process, and the text's capacity.
    let pointer_at = if process.pointer_size == 8 { 24 } else { 20 };
    let mut structure = vec![0u8; 96];
    structure[..4].copy_from_slice(&LVIF_TEXT.to_le_bytes());
    structure[8..12].copy_from_slice(&i32::try_from(column).unwrap_or(0).to_le_bytes());
    write_pointer(
        &mut structure,
        pointer_at,
        text.address(),
        process.pointer_size,
    );
    let capacity = i32::try_from(MAX_TEXT).unwrap_or(0);
    let capacity_at = pointer_at + process.pointer_size;
    structure[capacity_at..capacity_at + 4].copy_from_slice(&capacity.to_le_bytes());
    let buffer = process.allocate(structure.len())?;
    buffer.write(&structure)?;
    let length = send(hwnd, LVM_GETITEMTEXTW, item, buffer.address())?;
    let length = usize::try_from(length).unwrap_or(0).min(MAX_TEXT);
    read_text(&text, length)
}

/// The header text of `column` (`LVM_GETCOLUMNW`), `None` when it has none.
fn header_text(
    hwnd: isize,
    process: &TargetProcess,
    column: usize,
) -> Result<Option<String>, EditError> {
    let text = process.allocate(MAX_TEXT * 2)?;
    // `LVCOLUMNW`: mask, format, width, then the text pointer, aligned for
    // the process, and the text's capacity.
    let pointer_at = if process.pointer_size == 8 { 16 } else { 12 };
    let mut structure = vec![0u8; 64];
    structure[..4].copy_from_slice(&LVCF_TEXT.to_le_bytes());
    write_pointer(
        &mut structure,
        pointer_at,
        text.address(),
        process.pointer_size,
    );
    let capacity = i32::try_from(MAX_TEXT).unwrap_or(0);
    let capacity_at = pointer_at + process.pointer_size;
    structure[capacity_at..capacity_at + 4].copy_from_slice(&capacity.to_le_bytes());
    let buffer = process.allocate(structure.len())?;
    buffer.write(&structure)?;
    if send(hwnd, LVM_GETCOLUMNW, column, buffer.address())? == 0 {
        return Ok(None);
    }
    read_text(&text, MAX_TEXT)
}

/// Writes `address` as a pointer of `size` bytes at `at`.
fn write_pointer(structure: &mut [u8], at: usize, address: isize, size: usize) {
    let bytes = address.to_le_bytes();
    structure[at..at + size].copy_from_slice(&bytes[..size]);
}

/// The text of up to `length` UTF-16 units in `buffer`, up to its first
/// null, `None` when empty.
fn read_text(
    buffer: &crate::edit::RemoteBuffer<'_>,
    length: usize,
) -> Result<Option<String>, EditError> {
    let bytes = buffer.read(length * 2)?;
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&pair| u16::from_le_bytes(pair))
        .take_while(|&unit| unit != 0)
        .collect();
    Ok(Some(String::from_utf16_lossy(&units)).filter(|text| !text.is_empty()))
}
