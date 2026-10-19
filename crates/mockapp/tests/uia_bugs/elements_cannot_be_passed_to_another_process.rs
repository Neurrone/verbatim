//! `docs/uia-bugs.md`, "Elements cannot be passed to another process"
//! (E-marshal in its experiments folder): a UIA element marshaled by COM
//! for another process cannot be unmarshaled there, in either of COM's
//! forms, though the same bytes work in the process that made them, and an
//! object of the test's own passed the same way works across the two
//! processes until a call passes it an element; and that a sender stays
//! usable in its own process while its provider keeps the element.
//!
//! The element's owner is this test binary started again ([`owner`]), on
//! the test's desktop, which reads mockapp's window element, marshals it,
//! and writes the bytes on its standard output as hexadecimal lines.

use std::ffi::c_void;
use std::fmt::Write as _;
use std::io::{BufRead as _, BufReader, Write as _};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};

use windows::Win32::Foundation::{E_FAIL, E_UNEXPECTED, HGLOBAL, HWND};
use windows::Win32::System::Com::StructuredStorage::CreateStreamOnHGlobal;
use windows::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, CoInitializeEx, IStream, MSHCTX_LOCAL, MSHLFLAGS_NORMAL,
    STREAM_SEEK_END, STREAM_SEEK_SET,
};
use windows::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, IUIAutomationEventHandler, UIA_E_ELEMENTNOTAVAILABLE,
    UIA_MenuOpenedEventId,
};
use windows::core::{GUID, HRESULT, Interface};

use crate::{common, support};

/// The environment variable that makes this binary the element's owner,
/// naming mockapp's window.
pub const OWNER: &str = "VERBATIM_TEST_ELEMENT_OWNER";

// The workspace's `windows` crate leaves out COM's marshaling functions
// (`Win32_System_Com_Marshal`), so they are declared here.
#[link(name = "ole32")]
unsafe extern "system" {
    fn CoMarshalInterface(
        stream: *mut c_void,
        riid: *const GUID,
        unknown: *mut c_void,
        context: u32,
        reserved: *const c_void,
        flags: u32,
    ) -> HRESULT;
    fn CoUnmarshalInterface(
        stream: *mut c_void,
        riid: *const GUID,
        out: *mut *mut c_void,
    ) -> HRESULT;
    fn CoMarshalInterThreadInterfaceInStream(
        riid: *const GUID,
        unknown: *mut c_void,
        out: *mut *mut c_void,
    ) -> HRESULT;
}

/// `T`'s interface on `object` marshaled for another process on this
/// machine, as COM's standard marshaling writes it.
fn marshal_for_another_process<T: Interface>(object: &T) -> Vec<u8> {
    // SAFETY: a new stream that owns its memory.
    let stream = unsafe { CreateStreamOnHGlobal(HGLOBAL::default(), true) }.expect("a stream");
    // SAFETY: a live stream and object, and the object's own interface id.
    unsafe {
        CoMarshalInterface(
            stream.as_raw(),
            &T::IID,
            object.as_raw(),
            MSHCTX_LOCAL.0.cast_unsigned(),
            std::ptr::null(),
            MSHLFLAGS_NORMAL.0.cast_unsigned(),
        )
    }
    .ok()
    .expect("the interface marshaled");
    stream_bytes(&stream)
}

/// Everything written to `stream`.
fn stream_bytes(stream: &IStream) -> Vec<u8> {
    let mut size = 0u64;
    // SAFETY: a live stream; a plain out-pointer.
    unsafe { stream.Seek(0, STREAM_SEEK_END, Some(&raw mut size)) }.expect("the stream's end");
    // SAFETY: as above.
    unsafe { stream.Seek(0, STREAM_SEEK_SET, None) }.expect("the stream's start");
    let mut bytes = vec![0u8; usize::try_from(size).expect("the size fits")];
    let mut read = 0u32;
    // SAFETY: the buffer is as long as the count given.
    unsafe {
        stream.Read(
            bytes.as_mut_ptr().cast(),
            u32::try_from(bytes.len()).expect("the size fits"),
            Some(&raw mut read),
        )
    }
    .ok()
    .expect("the stream read");
    bytes.truncate(read as usize);
    bytes
}

/// `T`'s interface unmarshaled from `bytes` in this process.
fn unmarshal<T: Interface>(bytes: &[u8]) -> windows::core::Result<T> {
    // SAFETY: a new stream that owns its memory.
    let stream = unsafe { CreateStreamOnHGlobal(HGLOBAL::default(), true) }?;
    // SAFETY: the buffer is as long as the count given.
    unsafe {
        stream.Write(
            bytes.as_ptr().cast(),
            u32::try_from(bytes.len()).expect("the size fits"),
            None,
        )
    }
    .ok()?;
    // SAFETY: a live stream.
    unsafe { stream.Seek(0, STREAM_SEEK_SET, None) }?;
    let mut out = std::ptr::null_mut();
    // SAFETY: a live stream, the interface's id, and an out-pointer for it.
    unsafe { CoUnmarshalInterface(stream.as_raw(), &T::IID, &raw mut out) }.ok()?;
    // SAFETY: the owned reference CoUnmarshalInterface returned.
    Ok(unsafe { T::from_raw(out) })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut text, byte| {
        let _ = write!(text, "{byte:02x}");
        text
    })
}

fn from_hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).expect("a hexadecimal byte"))
        .collect()
}

/// An event handler of the test's own, which does nothing.
mod ignoring {
    // The `implement` macro's expansion.
    #![allow(clippy::inline_always, clippy::ref_as_ptr)]

    use windows::Win32::UI::Accessibility::{
        IUIAutomationElement, IUIAutomationEventHandler, IUIAutomationEventHandler_Impl,
        UIA_EVENT_ID,
    };

    pub(super) fn new() -> IUIAutomationEventHandler {
        Ignoring.into()
    }

    #[windows_core::implement(IUIAutomationEventHandler)]
    struct Ignoring;

    impl IUIAutomationEventHandler_Impl for Ignoring_Impl {
        fn HandleAutomationEvent(
            &self,
            _sender: windows_core::Ref<IUIAutomationElement>,
            _event: UIA_EVENT_ID,
        ) -> windows_core::Result<()> {
            Ok(())
        }
    }
}

/// The element's owner, run as this binary with [`OWNER`] naming mockapp's
/// window: reads the window's element and writes, one line each, `element`
/// and the element marshaled for another process; `in-process` and the
/// element marshaled for another apartment of this process, the
/// free-threaded marshaler's form; `own-apartment` and the name read
/// through a second copy marshaled for another process, unmarshaled on a
/// single-threaded apartment thread of this process; and `handler` and an
/// event handler of its own marshaled for another process. It then keeps
/// them until its standard input closes, and ends without running any
/// DLL's detach code, as the test runner does (`common/harness.rs`).
pub fn owner(window: &str) -> ! {
    let hwnd = HWND(window.parse::<isize>().expect("a window handle") as *mut c_void);
    let uia = support::client();
    let element = support::element(uia.client(), hwnd);
    let mut out = std::io::stdout().lock();
    writeln!(
        out,
        "element {}",
        hex(&marshal_for_another_process(&element))
    )
    .expect("written");

    let mut stream = std::ptr::null_mut();
    // SAFETY: a live element, its interface's id, and an out-pointer for the
    // stream.
    unsafe {
        CoMarshalInterThreadInterfaceInStream(
            &IUIAutomationElement::IID,
            element.as_raw(),
            &raw mut stream,
        )
    }
    .ok()
    .expect("the element marshaled for another apartment");
    // SAFETY: the owned stream reference the call returned.
    let in_process = stream_bytes(&unsafe { IStream::from_raw(stream) });
    writeln!(out, "in-process {}", hex(&in_process)).expect("written");

    let copy = marshal_for_another_process(&element);
    let name = std::thread::spawn(move || {
        // SAFETY: joins this new thread to a single-threaded apartment.
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
            .ok()
            .expect("a single-threaded apartment");
        let proxy: IUIAutomationElement = unmarshal(&copy).expect("unmarshaled in its process");
        // SAFETY: a live element; a cross-process read.
        unsafe { proxy.CurrentName() }
            .expect("its name")
            .to_string()
    })
    .join()
    .expect("the apartment's thread");
    writeln!(out, "own-apartment {name}").expect("written");

    let handler = ignoring::new();
    writeln!(
        out,
        "handler {}",
        hex(&marshal_for_another_process(&handler))
    )
    .expect("written");
    out.flush().expect("flushed");
    drop(out);
    for _ in std::io::stdin().lock().lines() {}
    // SAFETY: the pseudo-handle of this process; no preconditions.
    let this_process = unsafe { GetCurrentProcess() };
    // SAFETY: ends this process; nothing runs after it.
    let _ = unsafe { TerminateProcess(this_process, 0) };
    std::process::abort();
}

/// The element's owner, started for mockapp's window, and its lines.
struct Owner {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
}

impl Owner {
    fn start(hwnd: HWND) -> Self {
        common::contain_children();
        let mut child = Command::new(std::env::current_exe().expect("this test binary"))
            .env(OWNER, (hwnd.0 as isize).to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("the element's owner started");
        let stdin = child.stdin.take().expect("the owner's input");
        let stdout = child.stdout.take().expect("the owner's output");
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { return };
                if sender.send(line).is_err() {
                    return;
                }
            }
        });
        Self {
            child,
            stdin,
            lines,
        }
    }

    /// The owner's next line, which must start with `label` and a space;
    /// the rest of it.
    fn line(&self, label: &str) -> String {
        let line = self
            .lines
            .recv_timeout(common::WAIT_TIMEOUT)
            .unwrap_or_else(|error| panic!("the owner wrote no {label} line: {error}"));
        line.strip_prefix(&format!("{label} "))
            .unwrap_or_else(|| panic!("the owner wrote {line:?}, not its {label} line"))
            .to_owned()
    }

    /// Every line, up to the handler's: the element marshaled for another
    /// process, for another apartment, the name its own apartment read,
    /// and the handler.
    fn read(&self) -> (Vec<u8>, Vec<u8>, String, Vec<u8>) {
        (
            from_hex(&self.line("element")),
            from_hex(&self.line("in-process")),
            self.line("own-apartment"),
            from_hex(&self.line("handler")),
        )
    }

    /// Closes the owner's input, which ends it, and waits for it.
    fn end(mut self) {
        drop(self.stdin);
        let status = self.child.wait().expect("the owner ended");
        assert!(status.success(), "the owner ended with {status}");
    }
}

/// The element marshaled for another process cannot be unmarshaled in the
/// test's process (`E_FAIL`), though a second copy unmarshaled in the
/// owner's own process gives a working element.
pub fn an_element_marshaled_for_another_process_cannot_be_unmarshaled_there() {
    let (app, hwnd) = support::mockapp("marshal-element", "uia");
    let _uia = support::client();
    let owner = Owner::start(hwnd);
    let (element, _, own_apartment, _) = owner.read();
    assert_eq!(own_apartment, "Mockapp Events Fixture");
    let unmarshaled = unmarshal::<IUIAutomationElement>(&element).map(|_| ());
    assert_eq!(unmarshaled.map_err(|error| error.code()), Err(E_FAIL));
    owner.end();
    app.quit();
}

/// The element marshaled in the free-threaded marshaler's form, for
/// another apartment of its process, cannot be unmarshaled in the test's
/// process either (`E_UNEXPECTED`).
pub fn an_element_marshaled_for_another_apartment_cannot_be_unmarshaled_in_another_process() {
    let (app, hwnd) = support::mockapp("marshal-apartment", "uia");
    let _uia = support::client();
    let owner = Owner::start(hwnd);
    let (_, in_process, _, _) = owner.read();
    let unmarshaled = unmarshal::<IUIAutomationElement>(&in_process).map(|_| ());
    assert_eq!(unmarshaled.map_err(|error| error.code()), Err(E_UNEXPECTED));
    owner.end();
    app.quit();
}

/// The owner's own event handler, marshaled the same way, is unmarshaled
/// in the test's process, and a call through it with no element succeeds;
/// the same call passing it an element of the test's fails (`E_FAIL`):
/// COM between the two processes works, and only the element cannot
/// travel.
pub fn an_object_passed_to_another_process_works_until_a_call_passes_it_an_element() {
    let (app, hwnd) = support::mockapp("marshal-handler", "uia");
    let uia = support::client();
    let owner = Owner::start(hwnd);
    let (_, _, _, handler) = owner.read();
    let handler: IUIAutomationEventHandler =
        unmarshal(&handler).expect("the owner's handler unmarshaled here");
    let element = support::element(uia.client(), hwnd);
    // SAFETY: a call through a live proxy, with no element.
    let without = unsafe { handler.HandleAutomationEvent(None, UIA_MenuOpenedEventId) };
    // SAFETY: a call through a live proxy, with a live element.
    let with = unsafe { handler.HandleAutomationEvent(&element, UIA_MenuOpenedEventId) };
    assert_eq!(
        (without, with.map_err(|error| error.code())),
        (Ok(()), Err(E_FAIL))
    );
    owner.end();
    app.quit();
}

/// The current name of the element `kept`, read live.
fn live_name(
    kept: &windows::core::AgileReference<IUIAutomationElement>,
) -> Result<String, HRESULT> {
    let element = kept.resolve().expect("the kept sender");
    // SAFETY: a live element; a cross-process read.
    unsafe { element.CurrentName() }
        .map(|name| name.to_string())
        .map_err(|error| error.code())
}

/// A sender stays usable in the process that received it for as long as
/// the provider keeps its element (P3 in this limitation's experiments
/// folder): the senders of two focus events in mockapp, kept, read their
/// names live; once mockapp removes one's element (`take-runtime-id`), its
/// read fails with `UIA_E_ELEMENTNOTAVAILABLE` while the other still
/// answers; once mockapp has exited, the other's fails the same way.
pub fn a_kept_sender_is_read_live_until_its_provider_removes_it() {
    let (mut app, hwnd) = support::mockapp("marshal-kept", "uia");
    let uia = support::client();
    let handler = support::Handler::new();
    let _group = support::group_on(
        uia.client(),
        &support::element(uia.client(), hwnd),
        &handler,
        false,
    );
    app.send("focus btn1");
    let (_, button) = handler.next_with_sender();
    app.send("focus item1");
    let (_, item) = handler.next_with_sender();
    let gone = Err(HRESULT(UIA_E_ELEMENTNOTAVAILABLE.cast_signed()));
    assert_eq!(
        (live_name(&button), live_name(&item)),
        (Ok("Original Name".to_owned()), Ok("First".to_owned()))
    );
    app.send("take-runtime-id item2 btn1");
    assert_eq!(
        (live_name(&button), live_name(&item)),
        (gone.clone(), Ok("First".to_owned()))
    );
    app.quit();
    assert_eq!(live_name(&item), gone);
}
