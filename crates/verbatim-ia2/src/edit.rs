//! The standard Win32 edit and rich edit controls, read through their
//! window messages (milestone M4), ported from NVDA's `EditTextInfo`
//! (`NVDAObjects/window/edit.py`; this crate is GPL like NVDA). Edit fields
//! in applications Verbatim otherwise reads through MSAA have text this
//! way, since MSAA itself has no text interface.
//!
//! Offsets are the control's own: UTF-16 code units from the start of its
//! text. Which messages are used depends on the control's edit API version,
//! as in NVDA: a plain edit control (version 0) answers `EM_GETSEL`,
//! `EM_LINEFROMCHAR`, and the rest with plain values or with pointers
//! Windows marshals across processes (`EM_GETSEL`'s two `DWORD`s,
//! `EM_GETLINE`'s buffer, `WM_GETTEXT`'s buffer); a rich edit control
//! (version 1 and later) is asked with `EM_EXGETSEL`, `EM_EXSETSEL`,
//! `EM_EXLINEFROMCHAR`, and from version 2 `EM_GETTEXTRANGE` and
//! `EM_FINDWORDBREAK`, whose structures Windows does not marshal: they are
//! written into memory allocated in the control's process
//! (`VirtualAllocEx`, `WriteProcessMemory`, `ReadProcessMemory`), as NVDA
//! does, with pointer fields sized for that process, which can be a 32-bit
//! one.
//!
//! Each message that needs memory in the target gets a buffer of its own
//! ([`RemoteBuffer`]): one allocation holding the message's structure at
//! its start and its text 16-byte aligned after it, allocated before the
//! send and freed whole by the thread that sent the message, after the send
//! returns and never before, so the application never writes into memory
//! freed or reused while its message waited. Before each send that carries
//! such a buffer, the window's process is checked to be the buffer's.
//!
//! Every message goes through `crate::window::send_message`: a hung or
//! ghosted window fails at once ([`EditError::NotResponding`]), and the
//! message is sent with no time limit of its own. Each counts as one window
//! message (`books/dev/src/reference/performance.md`); allocating and
//! copying the target's memory are calls into the kernel, not into the
//! application, and are not counted. A password field's text reads as
//! stars, as NVDA reads it.

use std::ffi::c_void;

use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, POINT, STILL_ACTIVE};
use windows::Win32::Globalization::{CP_ACP, MULTI_BYTE_TO_WIDE_CHAR_FLAGS, MultiByteToWideChar};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::Diagnostics::Debug::{ReadProcessMemory, WriteProcessMemory};
use windows::Win32::System::Memory::{
    MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAllocEx, VirtualFreeEx,
};
use windows::Win32::System::SystemInformation::{
    IMAGE_FILE_MACHINE, IMAGE_FILE_MACHINE_ARMNT, IMAGE_FILE_MACHINE_I386,
    IMAGE_FILE_MACHINE_UNKNOWN,
};
use windows::Win32::System::Threading::{
    GetExitCodeProcess, GetProcessId, IsWow64Process2, OpenProcess,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_OPERATION, PROCESS_VM_READ, PROCESS_VM_WRITE,
};
use windows::Win32::UI::Controls::RichEdit::{
    EM_EXGETSEL, EM_EXLINEFROMCHAR, EM_EXSETSEL, EM_FINDWORDBREAK, EM_GETTEXTLENGTHEX,
    EM_GETTEXTRANGE, GTL_NUMCHARS,
};
use windows::Win32::UI::Controls::{
    EM_GETLINE, EM_GETLINECOUNT, EM_GETSEL, EM_LINEFROMCHAR, EM_LINEINDEX, EM_LINELENGTH,
    EM_POSFROMCHAR, EM_SCROLLCARET, EM_SETSEL, WB_MOVEWORDLEFT, WB_MOVEWORDRIGHT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    ES_PASSWORD, GWL_STYLE, GetWindowLongPtrW, GetWindowThreadProcessId, IsWindowUnicode,
    WM_GETTEXT, WM_GETTEXTLENGTH,
};

use crate::window::{Unanswered, send_message};

/// The most UTF-16 code units `EM_GETLINE` can return: its buffer's size
/// travels in the buffer's first word.
const MAX_LINE_UNITS: usize = 0xFFFF;

/// Why a read of an edit control failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditError {
    /// The window no longer exists, or its handle now names another
    /// process's window.
    Gone,
    /// The window is hung or ghosted, so nothing was sent to it.
    NotResponding,
    /// The control did not answer, or its process could not be read.
    Failed(String),
}

/// What an edit control read can fail with.
pub type EditResult<T> = Result<T, EditError>;

/// The edit API version of a window class, as NVDA numbers it, from the
/// class name normalized by NVDA's class map: 0 for the plain edit control,
/// 1 for rich edit 1.0, 2 for rich edit 2.0 and its combo box, 5 for rich
/// edit 5.0 and later; `None` for a class that is not an edit control.
#[must_use]
pub fn edit_api_version(normalized_class: &str) -> Option<u8> {
    match normalized_class {
        "Edit" => Some(0),
        "RichEdit" => Some(1),
        "RichEdit20" | "REComboBox20W" => Some(2),
        "RICHEDIT50W" => Some(5),
        _ => None,
    }
}

/// One edit control, asked through its window messages.
pub struct EditControl {
    hwnd: isize,
    version: u8,
    process: Option<TargetProcess>,
    /// The process to open in place of the window's own, for a test
    /// ([`EditControl::with_process`]).
    process_id: Option<u32>,
}

impl EditControl {
    /// The edit control `hwnd`, of edit API `version`
    /// ([`edit_api_version`]).
    #[must_use]
    pub fn new(hwnd: isize, version: u8) -> Self {
        Self {
            hwnd,
            version,
            process: None,
            process_id: None,
        }
    }

    /// [`EditControl::new`], its buffers allocated in process `pid` rather
    /// than the window's own: for a test standing for a window handle
    /// reused by another process's window after the control's process was
    /// opened, whose sends the process check refuses. Verbatim never calls
    /// it.
    #[must_use]
    pub fn with_process(hwnd: isize, version: u8, pid: u32) -> Self {
        Self {
            process_id: Some(pid),
            ..Self::new(hwnd, version)
        }
    }

    /// The control's window.
    #[must_use]
    pub fn hwnd(&self) -> isize {
        self.hwnd
    }

    /// The control's edit API version.
    #[must_use]
    pub fn version(&self) -> u8 {
        self.version
    }

    /// Whether the control is a password field, whose text reads as stars.
    /// Local.
    #[must_use]
    pub fn is_password(&self) -> bool {
        // SAFETY: a local read of the window's style; any handle is
        // tolerated.
        let style = unsafe { GetWindowLongPtrW(window(self.hwnd), GWL_STYLE) };
        style & ES_PASSWORD as isize != 0
    }

    /// Sends `msg` with plain integer parameters, or pointers to locals
    /// Windows marshals.
    fn send(&self, msg: u32, wparam: usize, lparam: isize) -> EditResult<isize> {
        // SAFETY: the parameters are integers or pointers to locals Windows
        // marshals for this message, which outlive the call.
        unsafe { send_message(self.hwnd, msg, (wparam, lparam), None) }
            .map_err(|unanswered| message_error(unanswered, "the edit control", msg))
    }

    /// Sends `msg` carrying `buffer`, whose address is one of its
    /// parameters, after checking that the window is still in the buffer's
    /// process.
    fn send_with(
        &self,
        msg: u32,
        (wparam, lparam): (usize, isize),
        buffer: &RemoteBuffer<'_>,
    ) -> EditResult<isize> {
        // SAFETY: the parameters are integers or the address of `buffer`,
        // memory allocated in the target for this message, which lives until
        // after the send returns.
        unsafe { send_message(self.hwnd, msg, (wparam, lparam), Some(buffer.sent())) }
            .map_err(|unanswered| message_error(unanswered, "the edit control", msg))
    }

    /// Opens the target process, on first use.
    fn open_process(&mut self) -> EditResult<()> {
        if self.process.is_none() {
            let pid = match self.process_id {
                Some(pid) => pid,
                None => window_process(self.hwnd)?,
            };
            self.process = Some(TargetProcess::open(pid)?);
        }
        Ok(())
    }

    /// The target process, once [`open_process`](Self::open_process) has
    /// opened it.
    fn process(&self) -> EditResult<&TargetProcess> {
        self.process
            .as_ref()
            .ok_or_else(|| EditError::Failed("the process is not open".to_owned()))
    }

    /// The selection's start and end, equal for none (`EM_EXGETSEL` or
    /// `EM_GETSEL`).
    ///
    /// # Errors
    ///
    /// When the control does not answer or is gone.
    pub fn selection(&mut self) -> EditResult<(u32, u32)> {
        if self.version >= 1 {
            self.open_process()?;
            let buffer = self.process()?.allocate(8)?;
            self.send_with(EM_EXGETSEL, (0, buffer.address()), &buffer)?;
            let bytes = buffer.read(0, 8)?;
            return Ok((le_u32(&bytes[0..4]), le_u32(&bytes[4..8])));
        }
        let (mut start, mut end) = (0u32, 0u32);
        // Windows marshals EM_GETSEL's two DWORD pointers across processes.
        self.send(
            EM_GETSEL,
            (&raw mut start) as usize,
            (&raw mut end) as isize,
        )?;
        Ok((start, end))
    }

    /// Selects from `start` to `end` and scrolls the caret into view
    /// (`EM_EXSETSEL` or `EM_SETSEL`, then `EM_SCROLLCARET`).
    ///
    /// # Errors
    ///
    /// When the control does not answer or is gone.
    pub fn set_selection(&mut self, start: u32, end: u32) -> EditResult<()> {
        if self.version >= 1 {
            let mut bytes = Vec::with_capacity(8);
            bytes.extend_from_slice(&start.to_le_bytes());
            bytes.extend_from_slice(&end.to_le_bytes());
            self.open_process()?;
            let buffer = self.process()?.allocate(8)?;
            buffer.write(0, &bytes)?;
            self.send_with(EM_EXSETSEL, (0, buffer.address()), &buffer)?;
        } else {
            self.send(EM_SETSEL, start as usize, param(end))?;
        }
        self.send(EM_SCROLLCARET, 0, 0)?;
        Ok(())
    }

    /// The line containing `offset`, counted from 0
    /// (`EM_EXLINEFROMCHAR` or `EM_LINEFROMCHAR`).
    ///
    /// # Errors
    ///
    /// When the control does not answer or is gone.
    pub fn line_from_offset(&self, offset: u32) -> EditResult<u32> {
        let line = if self.version >= 1 {
            self.send(EM_EXLINEFROMCHAR, 0, param(offset))?
        } else {
            self.send(EM_LINEFROMCHAR, offset as usize, 0)?
        };
        Ok(u32::try_from(int_answer(line)).unwrap_or(0))
    }

    /// The offset where line `line` starts, `None` past the last line
    /// (`EM_LINEINDEX`, which answers -1 there).
    ///
    /// # Errors
    ///
    /// When the control does not answer or is gone.
    pub fn line_start(&self, line: u32) -> EditResult<Option<u32>> {
        Ok(u32::try_from(int_answer(self.send(EM_LINEINDEX, line as usize, 0)?)).ok())
    }

    /// How many code units the line containing `offset` has, without its
    /// line break (`EM_LINELENGTH`).
    ///
    /// # Errors
    ///
    /// When the control does not answer or is gone.
    pub fn line_length(&self, offset: u32) -> EditResult<u32> {
        Ok(u32::try_from(int_answer(self.send(EM_LINELENGTH, offset as usize, 0)?)).unwrap_or(0))
    }

    /// How many lines the control has (`EM_GETLINECOUNT`).
    ///
    /// # Errors
    ///
    /// When the control does not answer or is gone.
    pub fn line_count(&self) -> EditResult<u32> {
        Ok(u32::try_from(int_answer(self.send(EM_GETLINECOUNT, 0, 0)?)).unwrap_or(1))
    }

    /// How many code units the text has (`EM_GETTEXTLENGTHEX` from rich edit
    /// 2.0, else `WM_GETTEXTLENGTH`).
    ///
    /// # Errors
    ///
    /// When the control does not answer or is gone.
    pub fn text_length(&mut self) -> EditResult<u32> {
        let length = if self.version >= 2 {
            let mut bytes = Vec::with_capacity(8);
            bytes.extend_from_slice(&GTL_NUMCHARS.0.to_le_bytes());
            // UTF-16, code page 1200.
            bytes.extend_from_slice(&1200u32.to_le_bytes());
            self.open_process()?;
            let buffer = self.process()?.allocate(8)?;
            buffer.write(0, &bytes)?;
            self.send_with(
                EM_GETTEXTLENGTHEX,
                (buffer.address().cast_unsigned(), 0),
                &buffer,
            )?
        } else {
            self.send(WM_GETTEXTLENGTH, 0, 0)?
        };
        Ok(u32::try_from(length).unwrap_or(0))
    }

    /// Line `line`'s text without its line break, at most `max` code units
    /// (`EM_GETLINE`, whose buffer Windows marshals).
    ///
    /// # Errors
    ///
    /// When the control does not answer or is gone.
    pub fn line_text(&self, line: u32, max: usize) -> EditResult<Vec<u16>> {
        let size = max.clamp(1, MAX_LINE_UNITS);
        let mut buffer = vec![0u16; size + 1];
        buffer[0] = u16::try_from(size).unwrap_or(u16::MAX);
        let copied = self.send(EM_GETLINE, line as usize, buffer.as_mut_ptr() as isize)?;
        let copied = usize::try_from(copied).unwrap_or(0).min(size);
        buffer.truncate(copied);
        Ok(self.masked(buffer))
    }

    /// The text from `start` to `end`: `EM_GETTEXTRANGE` from rich edit
    /// 2.0, else the whole text (`WM_GETTEXT`, marshalled) cut down, as NVDA
    /// reads a plain edit control's ranges.
    ///
    /// # Errors
    ///
    /// When the control does not answer or is gone.
    pub fn text_range(&mut self, start: u32, end: u32) -> EditResult<Vec<u16>> {
        let (start, end) = (start.min(end), start.max(end));
        if end == start {
            return Ok(Vec::new());
        }
        if self.version >= 2 {
            return self.rich_text_range(start, end);
        }
        let length = self.text_length()? as usize;
        let mut buffer = vec![0u16; length + 1];
        let copied = self.send(WM_GETTEXT, buffer.len(), buffer.as_mut_ptr() as isize)?;
        buffer.truncate(usize::try_from(copied).unwrap_or(0).min(length));
        let start = (start as usize).min(buffer.len());
        let end = (end as usize).min(buffer.len());
        Ok(self.masked(buffer[start..end].to_vec()))
    }

    /// `EM_GETTEXTRANGE`, with its structure and its text in one buffer in
    /// the target.
    fn rich_text_range(&mut self, start: u32, end: u32) -> EditResult<Vec<u16>> {
        let unicode = {
            // SAFETY: IsWindowUnicode tolerates any handle.
            unsafe { IsWindowUnicode(window(self.hwnd)) }.as_bool()
        };
        let units = (end - start) as usize;
        // Twice the units and a terminator: room for either character size.
        let text_bytes = (units + 1) * 2;
        self.open_process()?;
        let process = self.process()?;
        let pointer_size = process.pointer_size;
        // `TEXTRANGEW`: the range's two ends and the text's pointer.
        let structure_bytes = 8 + pointer_size;
        let text_at = text_offset(structure_bytes);
        let buffer = process.allocate(text_at + text_bytes)?;
        let mut range = Vec::with_capacity(structure_bytes);
        range.extend_from_slice(&start.to_le_bytes());
        range.extend_from_slice(&end.to_le_bytes());
        let address = buffer.address_at(text_at).cast_unsigned() as u64;
        if pointer_size == 4 {
            range.extend_from_slice(&u32::try_from(address).unwrap_or(0).to_le_bytes());
        } else {
            range.extend_from_slice(&address.to_le_bytes());
        }
        buffer.write(0, &range)?;
        let copied = self.send_with(EM_GETTEXTRANGE, (0, buffer.address()), &buffer)?;
        let copied = usize::try_from(copied).unwrap_or(0).min(units);
        // The whole text, which was zeroed when it was committed: a window
        // that is not Unicode may still copy UTF-16, which only the bytes
        // past the first `copied` show.
        let units = decode_text_range(&buffer.read(text_at, text_bytes)?, copied, unicode);
        Ok(self.masked(units))
    }

    /// The word break `action` finds from `offset` (`EM_FINDWORDBREAK`, rich
    /// edit 2.0 and later): the start of the word before or the start of
    /// the next.
    ///
    /// # Errors
    ///
    /// When the control does not answer or is gone.
    pub fn find_word_break(&self, right: bool, offset: u32) -> EditResult<u32> {
        let action = if right {
            WB_MOVEWORDRIGHT
        } else {
            WB_MOVEWORDLEFT
        };
        let found = self.send(
            EM_FINDWORDBREAK,
            action.0.cast_unsigned() as usize,
            param(offset),
        )?;
        Ok(u32::try_from(found).unwrap_or(offset))
    }

    /// The screen position of the character at `offset`, `None` when it is
    /// not in the control's client area (`EM_POSFROMCHAR`).
    ///
    /// # Errors
    ///
    /// When the control does not answer or is gone.
    pub fn position_of(&mut self, offset: u32) -> EditResult<Option<(i32, i32)>> {
        let (x, y) = if self.version == 1 || self.version >= 3 {
            self.open_process()?;
            let buffer = self.process()?.allocate(8)?;
            buffer.write(0, &[0; 8])?;
            self.send_with(
                EM_POSFROMCHAR,
                (buffer.address().cast_unsigned(), param(offset)),
                &buffer,
            )?;
            let bytes = buffer.read(0, 8)?;
            (
                le_u32(&bytes[0..4]).cast_signed(),
                le_u32(&bytes[4..8]).cast_signed(),
            )
        } else {
            // The client coordinates are packed into the result's two low
            // words, each a signed 16-bit value.
            let packed = self
                .send(EM_POSFROMCHAR, offset as usize, 0)?
                .cast_unsigned();
            let low = u16::try_from(packed & 0xFFFF).unwrap_or(0);
            let high = u16::try_from((packed >> 16) & 0xFFFF).unwrap_or(0);
            (i32::from(low.cast_signed()), i32::from(high.cast_signed()))
        };
        if x < 0 || y < 0 {
            return Ok(None);
        }
        let mut point = POINT { x, y };
        // SAFETY: a local conversion of a local point; any handle is
        // tolerated.
        if !unsafe { ClientToScreen(window(self.hwnd), &raw mut point) }.as_bool() {
            return Ok(None);
        }
        Ok(Some((point.x, point.y)))
    }

    /// `text`, as stars in a password field.
    fn masked(&self, text: Vec<u16>) -> Vec<u16> {
        if self.is_password() {
            vec![u16::from(b'*'); text.len()]
        } else {
            text
        }
    }
}

/// The `HWND` for a handle value.
fn window(hwnd: isize) -> HWND {
    HWND(hwnd as *mut c_void)
}

/// What a window message to `what` that was not answered fails with.
pub(crate) fn message_error(unanswered: Unanswered, what: &str, msg: u32) -> EditError {
    match unanswered {
        Unanswered::Gone => EditError::Gone,
        Unanswered::NotResponding => EditError::NotResponding,
        Unanswered::Failed => EditError::Failed(format!("{what} did not answer message {msg:#x}")),
        Unanswered::Abandoned => {
            EditError::Failed("the worker was abandoned; nothing was sent".to_owned())
        }
    }
}

/// The process of window `hwnd`; gone when it has none.
pub(crate) fn window_process(hwnd: isize) -> EditResult<u32> {
    let mut pid = 0u32;
    // SAFETY: a local read into a local; any handle is tolerated.
    unsafe { GetWindowThreadProcessId(window(hwnd), Some(&raw mut pid)) };
    if pid == 0 {
        return Err(EditError::Gone);
    }
    Ok(pid)
}

/// Where a buffer's text starts after a structure of `structure_bytes`:
/// 16-byte aligned.
pub(crate) fn text_offset(structure_bytes: usize) -> usize {
    structure_bytes.next_multiple_of(16)
}

/// The `int` a line message (`EM_LINEINDEX`, `EM_LINEFROMCHAR`,
/// `EM_LINELENGTH`, `EM_GETLINECOUNT`) answers, from the message's
/// pointer-sized result: its low 32 bits, as a signed number. The classic
/// edit control sign-extends its answer into the result, so a -1 reads as
/// -1, but Common Controls version 6's edit control, which Windows Forms
/// text boxes are, zero-extends it, so the same -1 reads as 4294967295
/// unless it is cut back to 32 bits. Read whole, that made the line after
/// the last one start at offset 4294967295, and the last line end there.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the answer is an int in the result's low 32 bits"
)]
fn int_answer(result: isize) -> i32 {
    result as i32
}

/// An offset as a message parameter.
fn param(value: u32) -> isize {
    isize::try_from(value).unwrap_or(isize::MAX)
}

/// A little-endian `u32` from four bytes.
fn le_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

/// The text `EM_GETTEXTRANGE` wrote into `buffer`, a zeroed buffer of
/// twice the range's length and a terminator, read back whole, the call
/// having answered `copied` characters, as NVDA decodes it
/// (`docs/nvda/editable-text-and-terminals.md`, "Rich edit text"): UTF-16
/// from a Unicode window; from any other window, UTF-16 too when more than
/// one character was copied and a byte past the first `copied` is not
/// zero, since some rich edit controls answer an ANSI window in UTF-16,
/// and otherwise ANSI text in the system code page.
fn decode_text_range(buffer: &[u8], copied: usize, unicode_window: bool) -> Vec<u16> {
    let wide = unicode_window
        || (copied > 1
            && buffer
                .get(copied..)
                .is_some_and(|rest| rest.iter().any(|&b| b != 0)));
    if wide {
        let bytes = &buffer[..(copied * 2).min(buffer.len())];
        bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect()
    } else {
        ansi_to_utf16(&buffer[..copied.min(buffer.len())])
    }
}

/// ANSI text, in the system code page, as UTF-16.
fn ansi_to_utf16(bytes: &[u8]) -> Vec<u16> {
    if bytes.is_empty() {
        return Vec::new();
    }
    let mut units = vec![0u16; bytes.len()];
    // SAFETY: a local conversion between two local buffers of the lengths
    // given.
    let length = unsafe {
        MultiByteToWideChar(
            CP_ACP,
            MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0),
            bytes,
            Some(&mut units),
        )
    };
    units.truncate(usize::try_from(length).unwrap_or(0));
    units
}

/// The process that owns a control, opened to allocate, read, and write
/// its memory.
pub(crate) struct TargetProcess {
    handle: HANDLE,
    /// The process's id, which its open handle keeps from being reused
    /// (`GetProcessId`).
    pid: u32,
    /// The size of a pointer in that process: 4 for a 32-bit process.
    pub(crate) pointer_size: usize,
}

impl TargetProcess {
    /// Opens process `pid`.
    pub(crate) fn open(pid: u32) -> EditResult<Self> {
        // SAFETY: opening a process by id with the rights needed; the handle
        // is closed when this value drops.
        let handle = unsafe {
            OpenProcess(
                PROCESS_VM_OPERATION
                    | PROCESS_VM_READ
                    | PROCESS_VM_WRITE
                    | PROCESS_QUERY_LIMITED_INFORMATION,
                false,
                pid,
            )
        }
        .map_err(|error| EditError::Failed(format!("could not open the process: {error}")))?;
        let mut machine = IMAGE_FILE_MACHINE_UNKNOWN;
        // SAFETY: a query of the handle just opened into a local.
        let wow = unsafe { IsWow64Process2(handle, &raw mut machine, None) }.is_ok();
        let pointer_size = if wow && is_32_bit(machine) { 4 } else { 8 };
        // SAFETY: a query of the handle just opened.
        let pid = unsafe { GetProcessId(handle) };
        Ok(Self {
            handle,
            pid,
            pointer_size,
        })
    }

    /// Whether the process has exited. A local call.
    fn exited(&self) -> bool {
        let mut code = 0u32;
        // SAFETY: a query of the open handle into a local.
        unsafe { GetExitCodeProcess(self.handle, &raw mut code) }
            .is_ok_and(|()| code != STILL_ACTIVE.0.cast_unsigned())
    }

    /// Allocates `size` bytes in the process, at least a page committed and
    /// one region of the process's address space reserved, freed whole
    /// when the buffer drops.
    pub(crate) fn allocate(&self, size: usize) -> EditResult<RemoteBuffer<'_>> {
        // SAFETY: allocating in the process the handle opened with
        // `PROCESS_VM_OPERATION`; a null result is checked.
        let address = unsafe {
            VirtualAllocEx(
                self.handle,
                None,
                size,
                MEM_COMMIT | MEM_RESERVE,
                PAGE_READWRITE,
            )
        };
        if address.is_null() {
            return Err(EditError::Failed(
                "could not allocate memory in the edit control's process".to_owned(),
            ));
        }
        Ok(RemoteBuffer {
            process: self,
            address,
            size,
        })
    }
}

impl Drop for TargetProcess {
    fn drop(&mut self) {
        // SAFETY: the handle was opened by `open` and is closed once.
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

/// Whether a WOW64 process machine is a 32-bit one.
fn is_32_bit(machine: IMAGE_FILE_MACHINE) -> bool {
    machine == IMAGE_FILE_MACHINE_I386 || machine == IMAGE_FILE_MACHINE_ARMNT
}

/// Memory allocated in another process for one message, owned by the
/// thread that sends it: freed whole when it drops, after the send has
/// returned ([`RemoteBuffer::free`]).
pub(crate) struct RemoteBuffer<'a> {
    process: &'a TargetProcess,
    address: *mut c_void,
    size: usize,
}

/// How a buffer's free ended, when it did not fail.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Freed {
    /// The memory was released.
    Released,
    /// The process has exited, and its memory went with it; the free
    /// itself failed, which is no error.
    WithItsProcess,
}

impl RemoteBuffer<'_> {
    /// The buffer's address in the target, as a message parameter.
    pub(crate) fn address(&self) -> isize {
        self.address as isize
    }

    /// The address `offset` bytes into the buffer, in the target.
    pub(crate) fn address_at(&self, offset: usize) -> isize {
        self.address().saturating_add_unsigned(offset)
    }

    /// The process the buffer is in and its address, for the send that
    /// carries it, which checks the window is still in that process.
    pub(crate) fn sent(&self) -> (u32, usize) {
        (self.process.pid, self.address as usize)
    }

    /// Writes `bytes` at `offset` into the buffer.
    pub(crate) fn write(&self, offset: usize, bytes: &[u8]) -> EditResult<()> {
        let length = bytes.len().min(self.size.saturating_sub(offset));
        // SAFETY: writing at most what is left of the buffer from `offset`,
        // from a local slice into memory allocated in the target for it.
        unsafe {
            WriteProcessMemory(
                self.process.handle,
                self.address.wrapping_byte_add(offset),
                bytes.as_ptr().cast(),
                length,
                None,
            )
        }
        .map_err(|error| EditError::Failed(format!("could not write the process: {error}")))
    }

    /// Reads `length` bytes from `offset` into the buffer.
    pub(crate) fn read(&self, offset: usize, length: usize) -> EditResult<Vec<u8>> {
        let length = length.min(self.size.saturating_sub(offset));
        let mut bytes = vec![0u8; length];
        // SAFETY: reading at most what is left of the buffer from `offset`,
        // from memory allocated in the target into a local of that length.
        unsafe {
            ReadProcessMemory(
                self.process.handle,
                self.address.wrapping_byte_add(offset),
                bytes.as_mut_ptr().cast(),
                length,
                None,
            )
        }
        .map_err(|error| EditError::Failed(format!("could not read the process: {error}")))?;
        Ok(bytes)
    }

    /// Frees the buffer, once. A free that fails because the process has
    /// exited is no error: the process's exit freed the memory with it.
    fn free(&mut self) -> windows::core::Result<Freed> {
        // SAFETY: the memory was allocated by `allocate` in this process and
        // is released once, from the drop.
        match unsafe { VirtualFreeEx(self.process.handle, self.address, 0, MEM_RELEASE) } {
            Ok(()) => Ok(Freed::Released),
            Err(_) if self.process.exited() => Ok(Freed::WithItsProcess),
            Err(error) => Err(error),
        }
    }
}

impl Drop for RemoteBuffer<'_> {
    fn drop(&mut self) {
        match self.free() {
            Ok(Freed::Released) => {}
            Ok(Freed::WithItsProcess) => tracing::debug!(
                address = self.address as usize,
                "a buffer's process exited before it was freed; its memory went with it"
            ),
            Err(error) => tracing::warn!(
                %error,
                address = self.address as usize,
                "a buffer in another process could not be freed"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A buffer in a process that has exited is freed with no error: the
    /// process's exit took its memory, against a child process the test
    /// starts and ends.
    #[test]
    fn freeing_a_buffer_in_a_process_that_has_exited_is_not_an_error() {
        let mut child = std::process::Command::new("cmd.exe")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("a child process");
        let process = TargetProcess::open(child.id()).expect("the child, opened");
        let mut buffer = process.allocate(64).expect("a buffer in the child");
        child.kill().expect("the child, ended");
        child.wait().expect("the child's exit");
        assert!(process.exited());
        assert_eq!(buffer.free().expect("no error"), Freed::WithItsProcess);
        std::mem::forget(buffer);
    }

    #[test]
    fn a_buffers_text_starts_16_byte_aligned_after_its_structure() {
        assert_eq!(text_offset(12), 16);
        assert_eq!(text_offset(16), 16);
        assert_eq!(text_offset(96), 96);
        assert_eq!(text_offset(97), 112);
    }

    #[test]
    fn classes_map_to_nvdas_edit_api_versions() {
        assert_eq!(edit_api_version("Edit"), Some(0));
        assert_eq!(edit_api_version("RichEdit20"), Some(2));
        assert_eq!(edit_api_version("RICHEDIT50W"), Some(5));
        assert_eq!(edit_api_version("Button"), None);
    }

    #[test]
    fn a_line_messages_answer_is_its_low_32_bits_signed() {
        assert_eq!(int_answer(-1), -1, "the classic control's -1");
        assert_eq!(
            int_answer(0xFFFF_FFFF),
            -1,
            "Common Controls 6's -1, zero-extended"
        );
        assert_eq!(int_answer(10), 10);
    }

    #[test]
    fn ansi_text_converts_to_utf16() {
        assert_eq!(
            ansi_to_utf16(b"abc"),
            "abc".encode_utf16().collect::<Vec<_>>()
        );
        assert_eq!(ansi_to_utf16(&[]), Vec::<u16>::new());
    }

    #[test]
    fn a_text_range_is_decoded_by_what_the_buffer_holds() {
        let utf16 = |text: &str| -> Vec<u8> {
            let mut bytes: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
            bytes.resize((text.encode_utf16().count() + 1) * 2, 0);
            bytes
        };
        let units = |text: &str| text.encode_utf16().collect::<Vec<_>>();
        // A window that is not Unicode, answered in UTF-16: "สวัสดี".
        assert_eq!(decode_text_range(&utf16("สวัสดี"), 6, false), units("สวัสดี"));
        // The same window answered in ANSI: the bytes past the text are zero.
        let mut ansi = b"abc".to_vec();
        ansi.resize(8, 0);
        assert_eq!(decode_text_range(&ansi, 3, false), units("abc"));
        // One character cannot be told apart, and is read as ANSI.
        let mut one = b"a".to_vec();
        one.resize(4, 0);
        assert_eq!(decode_text_range(&one, 1, false), units("a"));
        // A Unicode window is always UTF-16.
        assert_eq!(decode_text_range(&utf16("ab"), 2, true), units("ab"));
    }
}
