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
//! Every message is sent with `SendMessageTimeoutW`, aborting if the
//! application is hung and waiting at most half a second, and counts as one
//! window message (`docs/performance.md`); allocating and copying the
//! target's memory are calls into the kernel, not into the application,
//! and are not counted. A password field's text reads as stars, as NVDA
//! reads it.

use std::ffi::c_void;

use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, POINT, WPARAM};
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
    IsWow64Process2, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_OPERATION,
    PROCESS_VM_READ, PROCESS_VM_WRITE,
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
    ES_PASSWORD, GWL_STYLE, GetWindowLongPtrW, GetWindowThreadProcessId, IsWindow, IsWindowUnicode,
    SMTO_ABORTIFHUNG, SMTO_BLOCK, SendMessageTimeoutW, WM_GETTEXT, WM_GETTEXTLENGTH,
};

use verbatim_model::CallKind;

use crate::calls::count;

/// How long one message waits for the control's answer.
const MESSAGE_TIMEOUT_MS: u32 = 500;

/// The most UTF-16 code units `EM_GETLINE` can return: its buffer's size
/// travels in the buffer's first word.
const MAX_LINE_UNITS: usize = 0xFFFF;

/// Why a read of an edit control failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditError {
    /// The window no longer exists.
    Gone,
    /// The control did not answer in time, or its process could not be
    /// read.
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

    /// Sends `msg` with plain integer parameters, or pointers into the
    /// target process or ones Windows marshals.
    fn send(&self, msg: u32, wparam: usize, lparam: isize) -> EditResult<isize> {
        count(CallKind::WindowMessage);
        let mut result = 0usize;
        // SAFETY: the parameters are integers, addresses in the target's own
        // memory, or pointers to locals Windows marshals for this message,
        // which outlive the call; any window handle is tolerated.
        let sent = unsafe {
            SendMessageTimeoutW(
                window(self.hwnd),
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
            return Err(if unsafe { IsWindow(Some(window(self.hwnd))) }.as_bool() {
                EditError::Failed(format!("the edit control did not answer message {msg:#x}"))
            } else {
                EditError::Gone
            });
        }
        Ok(result.cast_signed())
    }

    /// Opens the target process, on first use.
    fn open_process(&mut self) -> EditResult<()> {
        if self.process.is_none() {
            self.process = Some(TargetProcess::open(self.hwnd)?);
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
            self.send(EM_EXGETSEL, 0, buffer.address())?;
            let bytes = buffer.read(8)?;
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
            buffer.write(&bytes)?;
            self.send(EM_EXSETSEL, 0, buffer.address())?;
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
        Ok(u32::try_from(line).unwrap_or(0))
    }

    /// The offset where line `line` starts, `None` past the last line
    /// (`EM_LINEINDEX`).
    ///
    /// # Errors
    ///
    /// When the control does not answer or is gone.
    pub fn line_start(&self, line: u32) -> EditResult<Option<u32>> {
        Ok(u32::try_from(self.send(EM_LINEINDEX, line as usize, 0)?).ok())
    }

    /// How many code units the line containing `offset` has, without its
    /// line break (`EM_LINELENGTH`).
    ///
    /// # Errors
    ///
    /// When the control does not answer or is gone.
    pub fn line_length(&self, offset: u32) -> EditResult<u32> {
        Ok(u32::try_from(self.send(EM_LINELENGTH, offset as usize, 0)?).unwrap_or(0))
    }

    /// How many lines the control has (`EM_GETLINECOUNT`).
    ///
    /// # Errors
    ///
    /// When the control does not answer or is gone.
    pub fn line_count(&self) -> EditResult<u32> {
        Ok(u32::try_from(self.send(EM_GETLINECOUNT, 0, 0)?).unwrap_or(1))
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
            buffer.write(&bytes)?;
            self.send(EM_GETTEXTLENGTHEX, buffer.address().cast_unsigned(), 0)?
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

    /// `EM_GETTEXTRANGE`, with its structure and buffer in the target.
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
        let text = process.allocate(text_bytes)?;
        let mut range = Vec::with_capacity(16);
        range.extend_from_slice(&start.to_le_bytes());
        range.extend_from_slice(&end.to_le_bytes());
        let address = text.address().cast_unsigned() as u64;
        if pointer_size == 4 {
            range.extend_from_slice(&u32::try_from(address).unwrap_or(0).to_le_bytes());
        } else {
            range.extend_from_slice(&address.to_le_bytes());
        }
        let structure = process.allocate(range.len())?;
        structure.write(&range)?;
        let copied = self.send(EM_GETTEXTRANGE, 0, structure.address())?;
        let copied = usize::try_from(copied).unwrap_or(0).min(units);
        let units = if unicode {
            let bytes = text.read(copied * 2)?;
            bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_le_bytes(*pair))
                .collect()
        } else {
            ansi_to_utf16(&text.read(copied)?)
        };
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
            buffer.write(&[0; 8])?;
            self.send(
                EM_POSFROMCHAR,
                buffer.address().cast_unsigned(),
                param(offset),
            )?;
            let bytes = buffer.read(8)?;
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

/// An offset as a message parameter.
fn param(value: u32) -> isize {
    isize::try_from(value).unwrap_or(isize::MAX)
}

/// A little-endian `u32` from four bytes.
fn le_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
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

/// The process that owns an edit control, opened to allocate, read, and
/// write its memory.
struct TargetProcess {
    handle: HANDLE,
    /// The size of a pointer in that process: 4 for a 32-bit process.
    pointer_size: usize,
}

impl TargetProcess {
    fn open(hwnd: isize) -> EditResult<Self> {
        let mut pid = 0u32;
        // SAFETY: a local read into a local; any handle is tolerated.
        unsafe { GetWindowThreadProcessId(window(hwnd), Some(&raw mut pid)) };
        if pid == 0 {
            return Err(EditError::Gone);
        }
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
        Ok(Self {
            handle,
            pointer_size,
        })
    }

    /// Allocates `size` bytes in the process, freed when the buffer drops.
    fn allocate(&self, size: usize) -> EditResult<RemoteBuffer<'_>> {
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

/// Memory allocated in another process.
struct RemoteBuffer<'a> {
    process: &'a TargetProcess,
    address: *mut c_void,
    size: usize,
}

impl RemoteBuffer<'_> {
    /// The buffer's address in the target, as a message parameter.
    fn address(&self) -> isize {
        self.address as isize
    }

    /// Writes `bytes` at the buffer's start.
    fn write(&self, bytes: &[u8]) -> EditResult<()> {
        let length = bytes.len().min(self.size);
        // SAFETY: writing at most the buffer's size from a local slice into
        // memory allocated in the target for it.
        unsafe {
            WriteProcessMemory(
                self.process.handle,
                self.address,
                bytes.as_ptr().cast(),
                length,
                None,
            )
        }
        .map_err(|error| EditError::Failed(format!("could not write the process: {error}")))
    }

    /// Reads `length` bytes from the buffer's start.
    fn read(&self, length: usize) -> EditResult<Vec<u8>> {
        let length = length.min(self.size);
        let mut bytes = vec![0u8; length];
        // SAFETY: reading at most the buffer's size from memory allocated in
        // the target into a local of that length.
        unsafe {
            ReadProcessMemory(
                self.process.handle,
                self.address,
                bytes.as_mut_ptr().cast(),
                length,
                None,
            )
        }
        .map_err(|error| EditError::Failed(format!("could not read the process: {error}")))?;
        Ok(bytes)
    }
}

impl Drop for RemoteBuffer<'_> {
    fn drop(&mut self) {
        // SAFETY: the memory was allocated by `allocate` in this process and
        // is released once.
        unsafe {
            let _ = VirtualFreeEx(self.process.handle, self.address, 0, MEM_RELEASE);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes_map_to_nvdas_edit_api_versions() {
        assert_eq!(edit_api_version("Edit"), Some(0));
        assert_eq!(edit_api_version("RichEdit20"), Some(2));
        assert_eq!(edit_api_version("RICHEDIT50W"), Some(5));
        assert_eq!(edit_api_version("Button"), None);
    }

    #[test]
    fn ansi_text_converts_to_utf16() {
        assert_eq!(
            ansi_to_utf16(b"abc"),
            "abc".encode_utf16().collect::<Vec<_>>()
        );
        assert_eq!(ansi_to_utf16(&[]), Vec::<u16>::new());
    }
}
