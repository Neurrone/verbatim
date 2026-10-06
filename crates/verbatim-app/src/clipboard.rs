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
use windows::Win32::System::Memory::{GHND, GlobalAlloc, GlobalLock, GlobalUnlock};
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
/// cannot be opened.
fn clipboard_text() -> Option<String> {
    // SAFETY: the clipboard is opened for this thread and always closed;
    // the global handle is locked only while its null-terminated UTF-16
    // contents are copied out, and never freed, since the clipboard owns it.
    unsafe {
        OpenClipboard(Some(HWND::default())).ok()?;
        let read = (|| {
            let handle = GetClipboardData(u32::from(CF_UNICODETEXT.0)).ok()?;
            let memory = HGLOBAL(handle.0);
            let pointer = GlobalLock(memory).cast::<u16>();
            if pointer.is_null() {
                return None;
            }
            let mut length = 0usize;
            while *pointer.add(length) != 0 {
                length += 1;
            }
            let text = String::from_utf16_lossy(std::slice::from_raw_parts(pointer, length));
            let _ = GlobalUnlock(memory);
            Some(text)
        })();
        let _ = CloseClipboard();
        read
    }
}

/// Speaks one line of confirmation text at Interrupt priority, with no
/// source node (this is Core-originated speech, not an object announcement).
fn speak(manager: &Arc<SpeechManager>, text: String) {
    manager.speak(Utterance {
        trace_id: TraceId::mint(),
        priority: SpeechPriority::Interrupt,
        segments: vec![UtteranceSegment::text(text)],
        source: None,
        say_all: false,
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

    // SAFETY: each call is checked; the global buffer is sized to hold the
    // whole null-terminated UTF-16 string and is written only within that
    // size while locked. `HWND(null)` opens the clipboard for this thread
    // with no owner window, which is valid.
    unsafe {
        OpenClipboard(Some(HWND::default())).map_err(|error| format!("OpenClipboard: {error}"))?;
        // A guard-free early exit would leak the open clipboard; every error
        // path below closes it before returning.
        let result = (|| {
            EmptyClipboard().map_err(|error| format!("EmptyClipboard: {error}"))?;
            let handle: HGLOBAL =
                GlobalAlloc(GHND, bytes).map_err(|error| format!("GlobalAlloc: {error}"))?;
            let destination = GlobalLock(handle);
            if destination.is_null() {
                return Err("GlobalLock returned null".to_owned());
            }
            std::ptr::copy_nonoverlapping(utf16.as_ptr(), destination.cast::<u16>(), utf16.len());
            let _ = GlobalUnlock(handle);
            SetClipboardData(CF_UNICODETEXT.0.into(), Some(HANDLE(handle.0)))
                .map_err(|error| format!("SetClipboardData: {error}"))?;
            Ok(())
        })();
        let _ = CloseClipboard();
        result
    }
}
