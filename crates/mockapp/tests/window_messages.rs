//! The window messages `verbatim-ia2` sends to mockapp's native controls,
//! held inside the control's window procedure by mockapp's `hold-message`
//! until the test releases them: a message has no time limit of its own,
//! and a buffer it carries in mockapp's memory lives until the message is
//! answered (`books/dev/src/reference/crates/verbatim-ia2.md`, "Window
//! messages"). The sends are watched at `verbatim-ia2`'s observation point
//! (`verbatim_faults::Fault::WindowMessageSent`), so this binary is built
//! only with the `faults` feature.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use verbatim_faults::{Fault, Guard};
use verbatim_ia2::edit::EditError;
use verbatim_model::protocol::{DeliveredFact, OutpostToCore, Query, QueryOutcome, QueryResult};
use verbatim_model::{
    CaretMotion, CaretRead, FocusText, KeyFetch, NodeKind, ReadPurpose, TextAnchor, TextOp,
    TextReply,
};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND};
use windows::Win32::System::Memory::{MEM_COMMIT, MEMORY_BASIC_INFORMATION, VirtualQueryEx};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_INFORMATION};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, OBJID_CLIENT};
use windows::core::{PCWSTR, w};

use common::outpost::OutpostUnderTest;

/// `EM_GETSEL`.
const EM_GETSEL: u32 = 0x00B0;

/// `LVM_GETITEMTEXTW`.
const LVM_GETITEMTEXTW: u32 = 0x1073;

/// The name mockapp's list view fixture gives its first item, from its
/// columns.
const README: &str = "readme.txt; Size: 1 KB; Type: Text Document";

/// What one send carried: its time limit and its buffer's address.
type Sent = (Option<u32>, Option<usize>);

/// Watches the sends of `message` to `window`, `times` of them: what each
/// carried, in order, and the guard that checks there were exactly that
/// many.
fn watch_sends(window: isize, message: u32, times: usize) -> (Arc<Mutex<Vec<Sent>>>, Guard) {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&sent);
    let guard = verbatim_faults::arm(
        Fault::WindowMessageSent,
        move |&(hwnd, msg, timeout, buffer): &(isize, u32, Option<u32>, Option<usize>)| {
            let ours = hwnd == window && msg == message;
            if ours {
                recorded
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push((timeout, buffer));
            }
            ours
        },
        times,
    );
    (sent, guard)
}

/// What was sent so far.
fn sent(sent: &Mutex<Vec<Sent>>) -> Vec<Sent> {
    sent.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

/// mockapp's process, opened to query its memory.
struct Process(HANDLE);

impl Process {
    fn open(pid: u32) -> Self {
        // SAFETY: opening a process by id for a query; closed on drop.
        Self(
            unsafe { OpenProcess(PROCESS_QUERY_INFORMATION, false, pid) }
                .expect("mockapp's process, opened"),
        )
    }

    /// Whether `address` is committed memory in the process: a local call.
    fn committed(&self, address: usize) -> bool {
        let mut info = MEMORY_BASIC_INFORMATION::default();
        // SAFETY: a query of the open process into a local of the size
        // given.
        let written = unsafe {
            VirtualQueryEx(
                self.0,
                Some(address as *const std::ffi::c_void),
                &raw mut info,
                size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        written != 0 && info.State == MEM_COMMIT
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        // SAFETY: the handle was opened by `open` and is closed once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// mockapp's child window of `class`.
fn child(host: HWND, class: PCWSTR) -> isize {
    // SAFETY: a local search of the host window's children by class.
    unsafe { FindWindowExW(Some(host), None, class, PCWSTR::null()) }
        .expect("mockapp made the control")
        .0 as isize
}

/// A text query on an edit control whose `EM_GETSEL` waits past twice the
/// old half-second message limit, real time, answers its caret
/// and selection exactly as the focus read them, and its send was made
/// with no time limit.
fn a_slow_edit_control_answers_its_selection() {
    common::init_com();
    let title = common::unique_title("mockapp-slow-edit");
    let mut app = common::spawn("text.json", "msaa", &title);
    let edit = child(common::find_window(&title), w!("EDIT"));
    // The control cannot take the keyboard focus on a desktop of its own,
    // so its host has the focused state, which an MSAA focus needs on the
    // object or an ancestor.
    app.send("set-focus root");
    // "beta" selected, the caret at its end.
    app.send("caret doc 6 10");
    let mut outpost = OutpostUnderTest::new(&app);
    let focus = outpost.focus(DeliveredFact::MsaaFocus {
        hwnd: edit,
        id_object: OBJID_CLIENT.0,
        id_child: 0,
    });
    // The caret as read, its anchors, which each read mints anew, and when
    // it was read left out.
    let unanchored = |caret: &verbatim_model::CaretReport| {
        let mut caret = caret.clone();
        caret.line.start = TextAnchor(0);
        caret.read_at_us = 0;
        if let Some(selection) = &mut caret.selection {
            selection.range.start.anchor = TextAnchor(0);
            selection.range.end.anchor = TextAnchor(0);
        }
        caret
    };
    let Some(FocusText::Caret { caret, .. }) = &focus.text else {
        panic!("the focus read no caret: {:?}", focus.text);
    };
    let expected = unanchored(caret);
    assert!(expected.selection.is_some(), "the focus read the selection");

    let (sends, guard) = watch_sends(edit, EM_GETSEL, 1);
    // The window thread stalled for twice the old half-second limit: the
    // query's `EM_GETSEL` waits in its queue, and the stall's end, which
    // mockapp prints, is the evidence that the old limit passed.
    let stall = Duration::from_secs(1);
    app.stall(stall);
    let request_id = outpost.ask(Query::Text {
        node_id: focus.node.id,
        op: TextOp::ReadCaret(CaretRead {
            fetch: KeyFetch {
                unit: CaretMotion::NextCharacter.unit(),
                motion: CaretMotion::NextCharacter,
                kind: NodeKind::TextField,
            },
            purpose: ReadPurpose::BeforeKey,
        }),
    });
    match outpost.next() {
        OutpostToCore::Reply {
            request_id: answered,
            outcome: QueryOutcome::Done(QueryResult::Text(TextReply::Caret(reply))),
            ..
        } if answered == request_id => assert_eq!(unanchored(&reply.caret), expected),
        other => panic!("the outpost said {other:?}, not the caret"),
    }
    app.stall_ended(stall);
    assert_eq!(sent(&sends), [(None, None)], "sent with no time limit");
    drop(guard);
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// A list view column read sends `LVM_GETITEMTEXTW` with a buffer in
/// mockapp's memory; while mockapp holds the message, the buffer is still
/// allocated there, and once the message is answered the read answers the
/// item's name and the buffer is free.
fn a_buffer_lives_until_its_message_is_answered() {
    common::init_com();
    let title = common::unique_title("mockapp-held-buffer");
    let mut app = common::spawn("list_view.json", "msaa", &title);
    let list = child(common::find_window(&title), w!("SysListView32"));
    let mockapp = Process::open(app.pid());
    // The item's first column, then its others, each its own message.
    let (sends, guard) = watch_sends(list, LVM_GETITEMTEXTW, 3);
    app.send("hold-message LVM_GETITEMTEXTW");
    let read = std::thread::spawn(move || verbatim_ia2::list_view::column_name(list, 1, || None));
    assert_eq!(app.held(), "LVM_GETITEMTEXTW");
    let [(None, Some(held))] = sent(&sends)[..] else {
        panic!("the held send carried no buffer: {:?}", sent(&sends));
    };
    assert!(
        mockapp.committed(held),
        "the held message's buffer, allocated"
    );
    app.release();
    let name = read.join().expect("the read returns");
    assert_eq!(name, Ok(Some(README.to_owned())));
    assert!(!mockapp.committed(held), "the buffer, freed");
    drop(guard);
    app.quit();
}

/// A column read made on a second thread while mockapp holds another
/// read's message sends messages of its own, each with a buffer of its
/// own; once the held message is answered both reads answer the item's
/// name, and every buffer is free.
fn a_second_read_during_a_held_message_sends_its_own() {
    common::init_com();
    let title = common::unique_title("mockapp-second-read");
    let mut app = common::spawn("list_view.json", "msaa", &title);
    let list = child(common::find_window(&title), w!("SysListView32"));
    let mockapp = Process::open(app.pid());
    let (sends, guard) = watch_sends(list, LVM_GETITEMTEXTW, 6);
    app.send("hold-message LVM_GETITEMTEXTW");
    let first = std::thread::spawn(move || verbatim_ia2::list_view::column_name(list, 1, || None));
    assert_eq!(app.held(), "LVM_GETITEMTEXTW");
    let second = std::thread::spawn(move || verbatim_ia2::list_view::column_name(list, 1, || None));
    app.release();
    assert_eq!(
        first.join().expect("the first read returns"),
        Ok(Some(README.to_owned()))
    );
    assert_eq!(
        second.join().expect("the second read returns"),
        Ok(Some(README.to_owned()))
    );
    let buffers: Vec<usize> = sent(&sends)
        .into_iter()
        .map(|(timeout, buffer)| {
            assert_eq!(timeout, None, "sent with no time limit");
            buffer.expect("each send carried a buffer")
        })
        .collect();
    assert_eq!(buffers.len(), 6, "each read sent its own messages");
    for buffer in buffers {
        assert!(!mockapp.committed(buffer), "every buffer, freed");
    }
    drop(guard);
    app.quit();
}

/// A column read whose message mockapp holds is gone once mockapp is
/// ended, its buffer's free, in a process that has exited, no error.
fn the_target_process_ending_during_a_message_is_gone() {
    common::init_com();
    let title = common::unique_title("mockapp-ended-read");
    let mut app = common::spawn("list_view.json", "msaa", &title);
    let list = child(common::find_window(&title), w!("SysListView32"));
    let (_, guard) = watch_sends(list, LVM_GETITEMTEXTW, 1);
    app.send("hold-message LVM_GETITEMTEXTW");
    let read = std::thread::spawn(move || verbatim_ia2::list_view::column_name(list, 1, || None));
    assert_eq!(app.held(), "LVM_GETITEMTEXTW");
    // Dropped, mockapp is ended and waited for.
    drop(app);
    assert_eq!(read.join().expect("the read returns"), Err(EditError::Gone));
    drop(guard);
}

/// Runs each test on a desktop of its own (`common/harness.rs`).
fn main() {
    harness::run_isolated(&[
        (
            "a_slow_edit_control_answers_its_selection",
            a_slow_edit_control_answers_its_selection,
        ),
        (
            "a_buffer_lives_until_its_message_is_answered",
            a_buffer_lives_until_its_message_is_answered,
        ),
        (
            "a_second_read_during_a_held_message_sends_its_own",
            a_second_read_during_a_held_message_sends_its_own,
        ),
        (
            "the_target_process_ending_during_a_message_is_gone",
            the_target_process_ending_during_a_message_is_gone,
        ),
    ]);
}
