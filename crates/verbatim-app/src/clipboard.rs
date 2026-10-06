//! The one shared clipboard-copy path (NVDA's `api.copyToClip` analog).
//!
//! Every gesture that copies text routes through [`copy`], so the Win32
//! clipboard interaction and the spoken confirmation are implemented once
//! and stay consistent. The report-object triple-press is the first caller;
//! later copying gestures use this same function rather than rolling their
//! own.

use std::sync::Arc;

use verbatim_model::{SpeechPriority, TraceId, Utterance, UtteranceSegment};
use verbatim_speech::SpeechManager;
use windows::Win32::Foundation::{HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GHND, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Ole::CF_UNICODETEXT;

/// Copies `text` to the system clipboard and speaks a localized
/// confirmation. A failure to reach the clipboard is logged and announced
/// as such rather than silently swallowed, so a user who pressed copy
/// always hears whether it worked.
pub fn copy(manager: &Arc<SpeechManager>, text: &str) {
    // As NVDA does, the copy is confirmed by reading the clipboard back.
    let copied = set_clipboard_text(text).and_then(|()| match clipboard_text() {
        Some(read) if read == text => Ok(()),
        _ => Err("the clipboard does not hold the copied text".to_owned()),
    });
    match copied {
        Ok(()) => speak(manager, verbatim_i18n::messages::clipboard_copied(text)),
        Err(error) => {
            tracing::warn!(%error, "failed to copy to the clipboard");
            speak(manager, verbatim_i18n::messages::clipboard_copy_failed());
        }
    }
}

/// The clipboard's text (`CF_UNICODETEXT`), `None` when it holds none or
/// cannot be opened. The data may have been put there by any process, so
/// it is read up to its terminator or the end of its allocation, whichever
/// comes first.
fn clipboard_text() -> Option<String> {
    // SAFETY: opening the clipboard for this thread, with no owner window;
    // it is closed below on every path.
    unsafe { OpenClipboard(Some(HWND::default())) }.ok()?;
    let read = (|| {
        // SAFETY: the clipboard is open on this thread; the handle stays
        // owned by the clipboard and is never freed here.
        let handle = unsafe { GetClipboardData(u32::from(CF_UNICODETEXT.0)) }.ok()?;
        let memory = HGLOBAL(handle.0);
        // SAFETY: `memory` is the clipboard's global handle; its size, in
        // bytes, bounds the read below.
        let units = unsafe { GlobalSize(memory) } / size_of::<u16>();
        // SAFETY: locking the clipboard's global handle, unlocked below.
        let pointer = unsafe { GlobalLock(memory) }.cast::<u16>();
        if pointer.is_null() {
            return None;
        }
        // SAFETY: `pointer` is the locked allocation of at least `units`
        // UTF-16 units, aligned for them as global memory always is, and
        // unchanged while it is locked.
        let data = unsafe { std::slice::from_raw_parts(pointer, units) };
        let length = data.iter().position(|&unit| unit == 0).unwrap_or(units);
        let text = String::from_utf16_lossy(&data[..length]);
        // SAFETY: unlocks the lock taken above; `data` is not used after.
        let _ = unsafe { GlobalUnlock(memory) };
        Some(text)
    })();
    // SAFETY: closes the clipboard this thread opened above.
    let _ = unsafe { CloseClipboard() };
    read
}

/// Speaks one line of confirmation text at Interrupt priority, with no
/// source node (this is Core-originated speech, not an object announcement).
fn speak(manager: &Arc<SpeechManager>, text: String) {
    manager.speak(Utterance {
        trace_id: TraceId::mint(),
        priority: SpeechPriority::Interrupt,
        segments: vec![UtteranceSegment::text(text)],
        source: None,
        validity: None,
    });
}

/// Places `text` on the clipboard as `CF_UNICODETEXT`, the standard Win32
/// copy dance: open the clipboard, empty it, allocate movable global memory
/// holding the UTF-16 string plus its null terminator, and hand ownership to
/// the clipboard. The clipboard owns the memory after `SetClipboardData`
/// succeeds, so it is not freed here.
fn set_clipboard_text(text: &str) -> Result<(), String> {
    let mut utf16: Vec<u16> = text.encode_utf16().collect();
    utf16.push(0);
    let bytes = std::mem::size_of_val(utf16.as_slice());

    // SAFETY: `HWND(null)` opens the clipboard for this thread with no
    // owner window, which is valid; it is closed below on every path.
    unsafe { OpenClipboard(Some(HWND::default())) }
        .map_err(|error| format!("OpenClipboard: {error}"))?;
    // A guard-free early exit would leak the open clipboard; every error
    // path below closes it before returning.
    let result = (|| {
        // SAFETY: the clipboard is open on this thread.
        unsafe { EmptyClipboard() }.map_err(|error| format!("EmptyClipboard: {error}"))?;
        // SAFETY: allocates `bytes` bytes of movable, zeroed global memory.
        let handle: HGLOBAL =
            unsafe { GlobalAlloc(GHND, bytes) }.map_err(|error| format!("GlobalAlloc: {error}"))?;
        // SAFETY: locks the allocation just made, unlocked below.
        let destination = unsafe { GlobalLock(handle) };
        if destination.is_null() {
            return Err("GlobalLock returned null".to_owned());
        }
        // SAFETY: the locked allocation holds `bytes` bytes, exactly the
        // whole null-terminated UTF-16 string, and is aligned for `u16`, as
        // global memory always is; the source is a separate vector.
        unsafe {
            std::ptr::copy_nonoverlapping(utf16.as_ptr(), destination.cast::<u16>(), utf16.len());
        }
        // SAFETY: unlocks the lock taken above.
        let _ = unsafe { GlobalUnlock(handle) };
        // SAFETY: the clipboard is open and emptied by this thread; on
        // success it owns the memory.
        unsafe { SetClipboardData(CF_UNICODETEXT.0.into(), Some(HANDLE(handle.0))) }
            .map_err(|error| format!("SetClipboardData: {error}"))?;
        Ok(())
    })();
    // SAFETY: closes the clipboard this thread opened above.
    let _ = unsafe { CloseClipboard() };
    result
}
