//! The thread that installs MSAA `WinEvent` hooks and pumps messages so their
//! out-of-context callbacks are delivered. Both a per-application outpost and
//! the focus listener (decision D13) use it, differing only in their pid and
//! subscription set.

use std::sync::Arc;
use std::thread::{self, JoinHandle};

use crossbeam_channel::{Sender, unbounded};
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, MSG, PostThreadMessageW, TranslateMessage,
};

use verbatim_ia2::{WinEventCallback, WinEventHook, WinEventKind};

/// The event thread: installs the requested MSAA hooks once (scoped to the
/// given pid, or global for pid zero), then pumps messages so the
/// out-of-context callbacks are delivered. Both a per-application outpost and
/// the focus listener (decision D13) use it, differing only in their pid and
/// subscription set.
pub(crate) struct EventThread {
    thread_id: u32,
    join: Option<JoinHandle<()>>,
}

impl EventThread {
    pub(crate) fn spawn(
        target_pid: u32,
        kinds: &'static [WinEventKind],
        make_callback: Arc<dyn Fn() -> WinEventCallback + Send + Sync>,
    ) -> Self {
        let (id_tx, id_rx) = unbounded::<u32>();
        let join = thread::Builder::new()
            .name("verbatim-event".to_owned())
            .spawn(move || event_thread_main(target_pid, kinds, &id_tx, &make_callback))
            .expect("spawn event thread");
        let thread_id = id_rx.recv().unwrap_or(0);
        Self {
            thread_id,
            join: Some(join),
        }
    }
}

impl Drop for EventThread {
    fn drop(&mut self) {
        // SAFETY: WM_QUIT ends the message loop on the event thread.
        unsafe {
            let _ = PostThreadMessageW(
                self.thread_id,
                windows::Win32::UI::WindowsAndMessaging::WM_QUIT,
                WPARAM(0),
                LPARAM(0),
            );
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn event_thread_main(
    target_pid: u32,
    kinds: &'static [WinEventKind],
    id_tx: &Sender<u32>,
    make_callback: &Arc<dyn Fn() -> WinEventCallback + Send + Sync>,
) {
    // SAFETY: GetCurrentThreadId is always sound.
    let thread_id = unsafe { GetCurrentThreadId() };
    let _ = id_tx.send(thread_id);
    // Installed once, for the whole life of the process (decision D9): a
    // second live hook set on the same thread while a first is still
    // registered has been observed to permanently kill WinEvent delivery on
    // that thread for the rest of the process, which is exactly why this
    // pid is fixed at spawn instead of rebindable.
    let hook = match WinEventHook::install(target_pid, kinds, make_callback()) {
        Ok(installed) => Some(installed),
        Err(error) => {
            tracing::warn!(error, target_pid, "failed to install WinEvent hooks");
            None
        }
    };
    let mut message = MSG::default();
    loop {
        // SAFETY: standard message loop; `message` is fully owned here.
        let result = unsafe { GetMessageW(&raw mut message, None, 0, 0) };
        if result.0 <= 0 {
            break; // WM_QUIT (0) or error (-1).
        }
        // SAFETY: dispatching a fully owned message.
        unsafe {
            let _ = TranslateMessage(&raw const message);
            DispatchMessageW(&raw const message);
        }
    }
    drop(hook);
}
