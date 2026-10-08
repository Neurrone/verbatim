//! The one shared clipboard-copy path (NVDA's `api.copyToClip` analog).
//!
//! Every gesture that copies text routes through [`Clipboard::copy`], so
//! the Win32 clipboard interaction and the spoken confirmation are
//! implemented once and stay consistent. The report-object triple-press is
//! the first caller; later copying gestures use this same path rather than
//! rolling their own.
//!
//! The copy runs on a thread of its own, never on the reducer thread that
//! asks for it: emptying the clipboard sends its previous owner, a window
//! of whichever application copied last, a message (`WM_DESTROYCLIPBOARD`)
//! and waits for the answer with no bound, so an owner that has stopped
//! answering would otherwise hold every event and every command behind the
//! copy.

use std::sync::Arc;
use std::thread;

use crossbeam_channel::{Sender, unbounded};

use verbatim_model::{SpeechPriority, TraceId, Utterance, UtteranceSegment};
use verbatim_speech::SpeechManager;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GHND, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, HWND_MESSAGE, WINDOW_EX_STYLE, WINDOW_STYLE,
};
use windows::core::{PCWSTR, w};

/// The clipboard thread: copies handed to it are made in order, each
/// confirmed in speech, while whoever asked goes on.
pub struct Clipboard {
    copies: Sender<String>,
}

impl Clipboard {
    /// Starts the clipboard thread, which speaks through `manager`.
    ///
    /// # Errors
    ///
    /// The error spawning the thread.
    pub fn start(manager: Arc<SpeechManager>) -> std::io::Result<Self> {
        let (copies, waiting) = unbounded::<String>();
        thread::Builder::new()
            .name("verbatim-clipboard".to_owned())
            .spawn(move || {
                while let Ok(text) = waiting.recv() {
                    copy(&manager, &text);
                }
            })?;
        Ok(Self { copies })
    }

    /// Copies `text` to the system clipboard on the clipboard thread and
    /// returns at once; the thread speaks whether it worked.
    pub fn copy(&self, text: String) {
        // An unbounded send never waits; it fails only once the thread has
        // gone, with the process.
        let _ = self.copies.send(text);
    }
}

/// Copies `text` to the system clipboard and speaks a localized
/// confirmation. A failure to reach the clipboard is logged and announced
/// as such rather than silently swallowed, so a user who pressed copy
/// always hears whether it worked. Runs on the clipboard thread.
fn copy(manager: &Arc<SpeechManager>, text: &str) {
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
        say_all: false,
        validity: None,
    });
}

/// Places `text` on the clipboard as `CF_UNICODETEXT`, the standard Win32
/// copy dance: open the clipboard with an owner window, empty it, allocate
/// movable global memory holding the UTF-16 string plus its null
/// terminator, and hand ownership to the clipboard. The clipboard owns the
/// memory once `SetClipboardData` succeeds; on any failure before that, the
/// memory is freed here.
///
/// The owner window is needed because emptying a clipboard opened with no
/// owner window leaves the clipboard with no owner, and `SetClipboardData`
/// then fails, as Microsoft's documentation of `OpenClipboard` says. The
/// caller is the clipboard thread, which runs no message loop, so the window
/// is a message-only window made for this write and destroyed after it
/// ([`OwnerWindow`]).
fn set_clipboard_text(text: &str) -> Result<(), String> {
    let mut utf16: Vec<u16> = text.encode_utf16().collect();
    utf16.push(0);
    let bytes = std::mem::size_of_val(utf16.as_slice());

    let owner = OwnerWindow::new()?;
    // SAFETY: opens the clipboard for this thread, owned by `owner`, a
    // window of this thread that outlives the clipboard being open; it is
    // closed below on every path.
    unsafe { OpenClipboard(Some(owner.0)) }.map_err(|error| format!("OpenClipboard: {error}"))?;
    // A guard-free early exit would leak the open clipboard; every error
    // path below closes it before returning.
    let result = (|| {
        // SAFETY: the clipboard is open on this thread.
        unsafe { EmptyClipboard() }.map_err(|error| format!("EmptyClipboard: {error}"))?;
        let memory = OwnedGlobal::holding(&utf16, bytes)?;
        // SAFETY: the clipboard is open and emptied by this thread, with an
        // owner; on success it owns the memory, which is then released from
        // `memory` so it is not freed here.
        unsafe { SetClipboardData(CF_UNICODETEXT.0.into(), Some(HANDLE(memory.0.0))) }
            .map_err(|error| format!("SetClipboardData: {error}"))?;
        memory.release();
        Ok(())
    })();
    // SAFETY: closes the clipboard this thread opened above.
    let _ = unsafe { CloseClipboard() };
    result
}

/// A hidden message-only window owned by the calling thread, to open the
/// clipboard with, destroyed when dropped. It lives only as long as one
/// write: the clipboard sends its owner messages, such as the
/// `WM_DESTROYCLIPBOARD` another application's `EmptyClipboard` sends, and
/// a window kept on a thread that pumps no messages would leave that
/// application waiting for an answer. Once the window is destroyed the
/// clipboard keeps the text with no owner.
struct OwnerWindow(HWND);

impl OwnerWindow {
    /// Creates the window, of the system's predefined static class, so no
    /// class is registered for it.
    fn new() -> Result<Self, String> {
        // SAFETY: creates a window of a class the system registers in every
        // process, with no name, menu, module, or creation data, as a
        // message-only window (its parent `HWND_MESSAGE`) owned by this
        // thread; it is destroyed in `drop`.
        let window = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                PCWSTR::null(),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                None,
                None,
            )
        }
        .map_err(|error| format!("CreateWindowExW: {error}"))?;
        Ok(Self(window))
    }
}

impl Drop for OwnerWindow {
    fn drop(&mut self) {
        // SAFETY: destroys the window this thread created in `new`, on the
        // same thread, as `DestroyWindow` requires; nothing uses it after.
        let _ = unsafe { DestroyWindow(self.0) };
    }
}

/// Global memory allocated for the clipboard, freed when dropped unless
/// [`release`](Self::release) handed it to the clipboard.
struct OwnedGlobal(HGLOBAL);

impl OwnedGlobal {
    /// Allocates `bytes` bytes of movable global memory holding `units`,
    /// which are exactly `bytes` long.
    fn holding(units: &[u16], bytes: usize) -> Result<Self, String> {
        // SAFETY: allocates `bytes` bytes of movable, zeroed global memory,
        // owned by the value made from it and freed when that drops.
        let handle =
            unsafe { GlobalAlloc(GHND, bytes) }.map_err(|error| format!("GlobalAlloc: {error}"))?;
        let memory = Self(handle);
        // SAFETY: locks the allocation just made, unlocked below.
        let destination = unsafe { GlobalLock(memory.0) };
        if destination.is_null() {
            return Err("GlobalLock returned null".to_owned());
        }
        // SAFETY: the locked allocation holds `bytes` bytes, exactly
        // `units`, and is aligned for `u16`, as global memory always is;
        // the source is a separate slice.
        unsafe {
            std::ptr::copy_nonoverlapping(units.as_ptr(), destination.cast::<u16>(), units.len());
        }
        // SAFETY: unlocks the lock taken above.
        let _ = unsafe { GlobalUnlock(memory.0) };
        Ok(memory)
    }

    /// Gives the memory up to the clipboard, which now owns it.
    fn release(self) {
        std::mem::forget(self);
    }
}

impl Drop for OwnedGlobal {
    fn drop(&mut self) {
        // SAFETY: frees the unlocked allocation `holding` made, which the
        // clipboard never took; nothing uses it after. `GlobalFree` returns
        // null on success, which the binding reports as an error, so its
        // result says nothing and is ignored.
        let _ = unsafe { GlobalFree(Some(self.0)) };
    }
}
