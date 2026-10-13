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
    DispatchMessageW, GetMessageW, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW,
    TranslateMessage, WM_QUIT,
};

use verbatim_ia2::{WinEventCallback, WinEventHook, WinEventKind};

/// The event thread: installs the requested MSAA hooks once (scoped to the
/// given pid, or global for pid zero), then pumps messages so the
/// out-of-context callbacks are delivered. Both a per-application outpost and
/// the focus listener (decision D13) use it, differing only in their pid and
/// subscription set. [`EventThread::spawn`] returns once the hooks are
/// installed, so its caller may report itself ready.
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
        // SAFETY: posting a message with no pointer parameters; the call
        // fails safely for a thread that has ended or has no queue.
        let posted = unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
        let Some(join) = self.join.take() else {
            return;
        };
        // The thread makes its message queue before it reports its id, so a
        // failed post means it has already ended, and joining it returns at
        // once. Should it still be running, nothing would end its loop, so
        // it is left to end with the process rather than waited on forever.
        if posted.is_ok() || join.is_finished() {
            let _ = join.join();
        } else {
            tracing::warn!("the event thread could not be told to stop; it is not waited on");
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
    // Makes this thread's message queue now, before its id is reported, so
    // the WM_QUIT that `Drop` posts always has a queue to land in.
    let mut message = MSG::default();
    // SAFETY: `message` is owned here; the call only peeks, removing
    // nothing.
    let _ = unsafe { PeekMessageW(&raw mut message, None, 0, 0, PM_NOREMOVE) };
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
    // `spawn` returns only now, with the hooks installed, so a caller that
    // reports itself ready after it really sees the events from then on.
    let _ = id_tx.send(thread_id);
    loop {
        // SAFETY: standard message loop; `message` is fully owned here.
        let result = unsafe { GetMessageW(&raw mut message, None, 0, 0) };
        if result.0 <= 0 {
            break; // WM_QUIT (0) or error (-1).
        }
        // SAFETY: translating a fully owned message.
        let _ = unsafe { TranslateMessage(&raw const message) };
        // SAFETY: dispatching a fully owned message.
        unsafe { DispatchMessageW(&raw const message) };
    }
    drop(hook);
}
