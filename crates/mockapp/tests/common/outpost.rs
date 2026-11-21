//! A real outpost in the test process, watching mockapp, as Core would
//! drive it: the tests send it Core's requests through its request driver
//! (`verbatim_outpost::driver`) and read every message it writes to its
//! pipe.
//!
//! The test stands in for the focus listener for UIA focus events: it
//! reads each focus from the element mockapp reports focused with the
//! listener's own read of a kept element ([`verbatim_listener::read_object`],
//! on the test's thread, where its calls are counted apart from the
//! outpost's), and has the outpost complete it from its route, as Core
//! asks it.
//!
//! Core's request for the current focus reads the system's keyboard focus,
//! which a test must not take from the desktop it runs on, so this outpost
//! reads it from the test instead
//! ([`Outpost::with_focused_element_reader`]): the test sets the element
//! mockapp reports focused, and the outpost's read of it counts as the one
//! call the system read is. The window with the keyboard focus is the one
//! the test reads as the foreground ([`Outpost::set_focus_window_reader`]).
//!
//! The outpost stamps its
//! own events' observations, and the start of each probe it reports, by the test's
//! observation clock ([`Outpost::set_observation_clock`]), which a test
//! chooses once, as it makes the outpost ([`ObservationTime`]): the run's
//! shared clock moved on by every time the test has passed
//! ([`OutpostUnderTest::new`]), or that clock frozen when the outpost was
//! made, moved on by the time passed and by a microsecond each time it is
//! read ([`OutpostUnderTest::on_frozen_time`]). On the frozen clock a
//! window's verdict in Core's cache runs out only once the test passes
//! the time, however long its steps take, so every test whose assertions
//! depend on a verdict's keep uses it. The others keep the real one,
//! because the outpost compares the stamps of caret, text and terminal
//! reads, read from the run's clock itself, with events' observations;
//! the facts the test hands over are stamped on the chosen clock
//! ([`OutpostUnderTest::observed_now`]).
//!
//! No window takes the foreground on the desktop a test runs on, so the
//! outpost reads the foreground window from the test too
//! ([`Outpost::set_foreground_reader`]): mockapp's own window unless the
//! test says otherwise ([`OutpostUnderTest::set_foreground`]), as when a
//! user works in mockapp.
//!
//! The test stands in for Core's holding of the outpost's nodes too: each
//! focus an answer carries is committed as Core commits it
//! ([`OutpostUnderTest::commit_focus`]), and the position of every message
//! that carries nodes is counted. Nothing is released until a test
//! delivers Core's list of the nodes it holds
//! ([`OutpostUnderTest::release`], [`OutpostUnderTest::release_all`]).

use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicIsize, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use verbatim_model::property::{PropertyId, PropertyValue, PropertyValues};
use verbatim_model::protocol::{CoreToOutpost, OutpostToCore};
use verbatim_model::request::{
    Answer, FocusNode, FocusRead, KnownAncestor, MsaaIds, NodeRead, NodeWindow, Outcome, Reply,
    Request, RequestNumber, WindowVerdict,
};
use verbatim_model::{CallCounts, NodeId, Pid, QueryKind, Role, StateSet, WindowHandle};
use verbatim_outpost::{Heard, Outpost};
use verbatim_process::arguments::OutpostOptions;
use verbatim_process::message::read_message;
use verbatim_process::overlapped::{End, Flow, pipe};
use verbatim_process::transport::BlockingReader;
use verbatim_uia::{Cached, ListenerEvent, Live};
use windows::Win32::Foundation::{E_FAIL, HWND};
use windows::Win32::UI::Accessibility::IUIAutomationElement;
use windows::core::AgileReference;

/// How much the outpost's pipe to Core holds unread. A test reads the
/// outpost's messages only when it asks for them, and the outpost's
/// [`Outpost::settle`] waits for every write to complete, so the pipe holds
/// everything a test leaves unread before it settles the outpost.
const MESSAGES_PIPE_BUFFER: u32 = 4 * 1024 * 1024;

/// The outpost's pipe to Core, read by the test as Core reads it, frame by
/// frame, on the test's own thread. Once [`Outpost::settle`] returns,
/// every message the outpost wrote before is in the pipe, so
/// [`MessagePipe::try_recv`] finds it.
pub struct MessagePipe {
    reader: RefCell<BlockingReader>,
    /// Bytes read and not yet taken: the start of a frame whose rest has
    /// not been read.
    pending: RefCell<Vec<u8>>,
}

impl MessagePipe {
    fn new(reader: BlockingReader) -> Self {
        Self {
            reader: RefCell::new(reader),
            pending: RefCell::new(Vec::new()),
        }
    }

    /// The first whole message among the bytes read, taken.
    fn take(&self) -> Option<OutpostToCore> {
        let mut pending = self.pending.borrow_mut();
        let mut rest = pending.as_slice();
        let message = match read_message::<_, OutpostToCore>(&mut rest) {
            Ok(message) => message?,
            // A frame not yet whole.
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return None,
            Err(error) => panic!("the outpost wrote something that is not a message: {error}"),
        };
        let taken = pending.len() - rest.len();
        pending.drain(..taken);
        Some(message)
    }

    /// The outpost's next message, waiting at most `timeout` for it.
    ///
    /// # Errors
    ///
    /// [`RecvTimeoutError::Timeout`] when none came in that time, and
    /// [`RecvTimeoutError::Disconnected`] once the outpost has closed its
    /// pipe.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<OutpostToCore, RecvTimeoutError> {
        let deadline = Instant::now() + timeout;
        let mut buffer = vec![0; 64 * 1024];
        loop {
            if let Some(message) = self.take() {
                return Ok(message);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            match self.reader.borrow_mut().read_within(&mut buffer, left) {
                Ok(None) => return Err(RecvTimeoutError::Timeout),
                Ok(Some(0)) => return Err(RecvTimeoutError::Disconnected),
                Ok(Some(read)) => self.pending.borrow_mut().extend_from_slice(&buffer[..read]),
                Err(error) => panic!("the outpost's pipe failed: {error}"),
            }
        }
    }

    /// The outpost's next message if it has written one: what is in the
    /// pipe now, without waiting for more.
    ///
    /// # Errors
    ///
    /// [`TryRecvError::Empty`] when nothing is there, and
    /// [`TryRecvError::Disconnected`] once the outpost has closed its pipe.
    pub fn try_recv(&self) -> Result<OutpostToCore, TryRecvError> {
        if let Some(message) = self.take() {
            return Ok(message);
        }
        match self.reader.borrow().available() {
            Ok(0) => return Err(TryRecvError::Empty),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => {
                return Err(TryRecvError::Disconnected);
            }
            Err(error) => panic!("the outpost's pipe failed: {error}"),
        }
        // Something is there, and a frame is written whole, so the rest of
        // it is there or on its way.
        self.recv_timeout(super::WAIT_TIMEOUT)
            .map_err(|_| TryRecvError::Disconnected)
    }
}

/// The element the outpost under test reads as the one with the keyboard
/// focus, set by the test before it hands the outpost a UIA focus.
pub type FocusSlot = Arc<Mutex<Option<AgileReference<IUIAutomationElement>>>>;

/// Has `outpost` read `foreground` as the foreground window, counting its
/// reads, and as the window with the keyboard focus unless the test sets
/// another in the window returned.
fn read_the_tests_foreground(
    outpost: &Outpost,
    foreground: &Arc<AtomicIsize>,
) -> (Arc<ReadCount>, Arc<AtomicIsize>) {
    let reader = Arc::clone(foreground);
    let foreground_reads = Arc::new(ReadCount::default());
    let reads = Arc::clone(&foreground_reads);
    outpost.set_foreground_reader(Arc::new(move || {
        reads.count();
        reader.load(Ordering::Relaxed)
    }));
    let focus_window = Arc::new(AtomicIsize::new(0));
    let (apart, front) = (Arc::clone(&focus_window), Arc::clone(foreground));
    outpost.set_focus_window_reader(Arc::new(move || {
        Some(match apart.load(Ordering::Relaxed) {
            0 => front.load(Ordering::Relaxed),
            window => window,
        })
    }));
    (foreground_reads, focus_window)
}

/// How many times something was read, with the condition a test waits on
/// for it to reach a count.
#[derive(Default)]
struct ReadCount {
    count: Mutex<u64>,
    changed: std::sync::Condvar,
}

impl ReadCount {
    /// Counts one read.
    fn count(&self) {
        *self.count.lock().unwrap_or_else(PoisonError::into_inner) += 1;
        self.changed.notify_all();
    }

    /// The reads so far.
    fn read(&self) -> u64 {
        *self.count.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Waits until the reads reach `count`.
    fn wait_for(&self, count: u64) {
        let reads = self.count.lock().unwrap_or_else(PoisonError::into_inner);
        let (reads, timeout) = self
            .changed
            .wait_timeout_while(reads, super::WAIT_TIMEOUT, |reads| *reads < count)
            .unwrap_or_else(PoisonError::into_inner);
        assert!(
            !timeout.timed_out(),
            "the outpost read the foreground window {} times, never {count}",
            *reads
        );
    }
}

/// Which observation clock a test runs its outpost on, chosen once, as it
/// makes the outpost.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObservationTime {
    /// The run's shared clock moved on by every time the test has passed.
    Real,
    /// The run's shared clock when the outpost was made, moved on only by
    /// the time the test passes and by a microsecond each time it is read,
    /// so no real time is in it and no two reads are equal.
    Frozen,
}

/// The outpost's observation clock in a test ([`ObservationTime`]).
struct TestClock {
    /// Which clock it is.
    time: ObservationTime,
    /// The run's shared clock when the outpost was made, in microseconds.
    start_us: u64,
    /// How far the clock has moved on from `start_us`, in microseconds: the
    /// time passed and the reads.
    moved_us: AtomicU64,
    /// How much time the test has passed, in microseconds.
    passed_us: AtomicU64,
}

impl TestClock {
    /// Now on the clock: on the frozen clock, a microsecond after the last
    /// read.
    fn now(&self) -> u64 {
        match self.time {
            ObservationTime::Real => now_us().saturating_add(self.passed()),
            ObservationTime::Frozen => {
                self.start_us + self.moved_us.fetch_add(1, Ordering::SeqCst) + 1
            }
        }
    }

    /// How much time the test has passed, in microseconds.
    fn passed(&self) -> u64 {
        self.passed_us.load(Ordering::SeqCst)
    }

    /// Moves the clock on by `by_us`.
    fn pass(&self, by_us: u64) {
        self.passed_us.fetch_add(by_us, Ordering::SeqCst);
        self.moved_us.fetch_add(by_us, Ordering::SeqCst);
    }
}

/// Has `outpost` stamp its own events' observations, and the start of each
/// probe it reports, by the test's observation clock, `time`: the clock,
/// which the test moves on.
fn observe_on_the_tests_time(outpost: &Outpost, time: ObservationTime) -> Arc<TestClock> {
    let clock = Arc::new(TestClock {
        time,
        start_us: now_us(),
        moved_us: AtomicU64::new(0),
        passed_us: AtomicU64::new(0),
    });
    let observed = Arc::clone(&clock);
    outpost.set_observation_clock(Arc::new(move || observed.now()));
    clock
}

/// A real outpost in this process, watching mockapp, with its pipe to Core.
pub struct OutpostUnderTest {
    pub outpost: Outpost,
    pub messages: MessagePipe,
    /// Every event the outpost's own handlers took in from mockapp, in the
    /// order they were queued ([`Outpost::observe_heard`]).
    heard: Receiver<Heard>,
    pub focus: FocusSlot,
    /// The window the outpost reads as the foreground.
    foreground: Arc<AtomicIsize>,
    /// How many times the outpost has read the foreground window, which a
    /// test waits on as the evidence of the outpost's test of an entry.
    foreground_reads: Arc<ReadCount>,
    /// The window the outpost reads as the one with the keyboard focus,
    /// when the test set one apart from the foreground window; 0 when it
    /// is the foreground window.
    focus_window: Arc<AtomicIsize>,
    /// How the stand-in for the listener's reading thread judges whether a
    /// window can be read ([`Self::judge_hung`]).
    listener_readability: RefCell<verbatim_win32::window::WindowReadabilityReader>,
    /// Told each call the stand-in for the listener's reading thread is
    /// about to make reading an element Core names
    /// ([`Self::listener_observes_calls`]).
    read_observer: RefCell<Option<verbatim_listener::CallObserver>>,
    /// The properties a focus read reports, as Core's settings choose them
    /// ([`Self::read_with`]): [`focus_set`] until a test names others.
    reported_set: RefCell<Vec<PropertyId>>,
    /// The outpost's failed cached reads when last taken
    /// ([`Self::take_failed_reads`]).
    failed_reads_taken: Cell<u32>,
    /// The position of the last message read that carries nodes, as Core
    /// counts them, which Core's list of held nodes acknowledges.
    position: Cell<u64>,
    /// The outpost's observation clock, which the test moves on.
    observation: Arc<TestClock>,
    /// The window the keyboard focus moves to during the outpost's next
    /// read of the focused element, 0 for none.
    moves_to: Arc<AtomicIsize>,
    /// The number of the last request the request driver sent
    /// ([`Self::request`]).
    last_asked: Cell<RequestNumber>,
    /// The calls each request made, by its number, as the outpost tells
    /// them before it answers ([`Outpost::observe_request_calls`]).
    pub request_calls: Receiver<(RequestNumber, CallCounts)>,
    /// The calls told of requests whose answer the test has not taken.
    told_calls: RefCell<std::collections::HashMap<RequestNumber, CallCounts>>,
    /// The focus the stand-in for Core committed, then its ancestors
    /// nearest first, each by its name and the entries Core names it by in
    /// a request's list of the ancestors it already knows
    /// ([`Self::known`]).
    committed: RefCell<Vec<(Option<String>, Vec<KnownAncestor>)>>,
    /// mockapp's window, which a request about its focus names.
    app_window: HWND,
    /// mockapp's process.
    app_pid: u32,
}

impl OutpostUnderTest {
    /// An outpost watching `app` with the default options, on the real
    /// observation clock ([`ObservationTime::Real`]).
    pub fn new(app: &super::MockApp) -> Self {
        Self::with_options(app, OutpostOptions::default())
    }

    /// [`Self::new`], with a focus listener forwarding to it, as every
    /// outpost the supervisor starts has: its own hook's copies are held
    /// until the test says the listener's forwarding has ended
    /// ([`Self::forwarding_ended`]). Every other outpost but a spare is told
    /// no listener forwards to it, so its hook's copies are handled as they
    /// come.
    pub fn with_listener(app: &super::MockApp) -> Self {
        Self::build_ready(
            app,
            OutpostOptions::default(),
            MESSAGES_PIPE_BUFFER,
            ObservationTime::Real,
        )
    }

    /// An outpost watching `app` as `options` say, on the real observation
    /// clock, which has announced itself ready, and nothing else. It reads
    /// `app`'s window as the foreground window.
    pub fn with_options(app: &super::MockApp, options: OutpostOptions) -> Self {
        Self::with_pipe_buffer(app, options, MESSAGES_PIPE_BUFFER)
    }

    /// [`Self::new`] on the frozen observation clock
    /// ([`ObservationTime::Frozen`]), for a test whose assertions depend on
    /// a verdict's keep.
    pub fn on_frozen_time(app: &super::MockApp) -> Self {
        Self::on_frozen_time_with_options(app, OutpostOptions::default())
    }

    /// [`Self::with_options`] on the frozen observation clock.
    pub fn on_frozen_time_with_options(app: &super::MockApp, options: OutpostOptions) -> Self {
        Self::ready(app, options, MESSAGES_PIPE_BUFFER, ObservationTime::Frozen)
    }

    /// [`Self::with_options`], its pipe to Core holding `buffer` bytes
    /// unread: a test that leaves a message larger than that unread has
    /// its write in flight, and what the outpost sends after it waiting
    /// in its backlog.
    pub fn with_pipe_buffer(app: &super::MockApp, options: OutpostOptions, buffer: u32) -> Self {
        Self::ready(app, options, buffer, ObservationTime::Real)
    }

    /// An outpost watching `app`, on the observation clock `time`, which
    /// has announced itself ready.
    fn ready(
        app: &super::MockApp,
        options: OutpostOptions,
        buffer: u32,
        time: ObservationTime,
    ) -> Self {
        Self::build_ready(app, options, buffer, time)
    }

    /// An outpost watching `app`, on the observation clock `time`, which
    /// has announced itself ready.
    fn build_ready(
        app: &super::MockApp,
        options: OutpostOptions,
        buffer: u32,
        time: ObservationTime,
    ) -> Self {
        let under_test = Self::build(app, options, buffer, false, time);
        assert_eq!(
            under_test.next(),
            OutpostToCore::Ready {
                outpost_pid: Pid(std::process::id()),
                target_pid: Pid(app.pid()),
            }
        );
        under_test
    }

    /// A spare outpost (`books/dev/src/reference/adr/0001-focus.md`, "Spare outposts"), with
    /// no application yet, which has said so and nothing else;
    /// [`Self::assign`] gives it `app`. It reads `app`'s window as the
    /// foreground window.
    pub fn spare(app: &super::MockApp) -> Self {
        let under_test = Self::build(
            app,
            OutpostOptions::default(),
            MESSAGES_PIPE_BUFFER,
            true,
            ObservationTime::Real,
        );
        assert_eq!(
            under_test.next(),
            OutpostToCore::SpareReady {
                outpost_pid: Pid(std::process::id()),
            }
        );
        under_test
    }

    /// Gives this spare `app`, as the supervisor's `Assign` does, and
    /// waits for its `Ready`.
    pub fn assign(&self, app: &super::MockApp) {
        self.outpost
            .assign(app.pid())
            .expect("the spare's hook is installed");
        assert_eq!(
            self.next(),
            OutpostToCore::Ready {
                outpost_pid: Pid(std::process::id()),
                target_pid: Pid(app.pid()),
            }
        );
    }

    /// An outpost for `app`, watching it from the start, or a spare.
    fn build(
        app: &super::MockApp,
        options: OutpostOptions,
        buffer: u32,
        spare: bool,
        observation_time: ObservationTime,
    ) -> Self {
        let pid = app.pid();
        let (ours, theirs) =
            pipe(Flow::Inbound, buffer, End::Overlapped).expect("the outpost's pipe to Core");
        let messages = MessagePipe::new(BlockingReader::new(ours).expect("a reader"));
        let focus = FocusSlot::default();
        let slot = Arc::clone(&focus);
        // No window takes the foreground where these tests run: mockapp's
        // is the foreground unless the test says otherwise
        // ([`OutpostUnderTest::set_foreground`]).
        let foreground = Arc::new(AtomicIsize::new(super::find_window(&app.title).0 as isize));
        let moves_to = Arc::new(AtomicIsize::new(0));
        let (moved, moving) = (Arc::clone(&foreground), Arc::clone(&moves_to));
        // Stands in for `GetFocusedElementBuildCache`: one UIA call, as the
        // system read counts, the element's cache filled by the outpost's
        // request. A focus the test moves during the read moves as the
        // element is read ([`OutpostUnderTest::focus_moves_during_the_read`]).
        let read_focus: verbatim_outpost::FocusedElementReader = Arc::new(move |_, request| {
            let element = slot
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_ref()
                .ok_or_else(|| windows::core::Error::from(E_FAIL))?
                .resolve()?;
            let read = Live::new(element).refresh(request);
            let to = moving.swap(0, Ordering::Relaxed);
            if to != 0 {
                moved.store(to, Ordering::Relaxed);
            }
            read
        });
        let outpost = if spare {
            Outpost::spare(theirs, options, read_focus)
        } else {
            Outpost::with_focused_element_reader(theirs, pid, options, read_focus)
        };
        let observation = observe_on_the_tests_time(&outpost, observation_time);
        let (foreground_reads, focus_window) = read_the_tests_foreground(&outpost, &foreground);
        let (heard_tx, heard) = mpsc::channel();
        outpost.observe_heard(Arc::new(move |event| {
            // The test may have finished with what the outpost hears.
            let _ = heard_tx.send(event);
        }));
        let (calls_tx, request_calls) = mpsc::channel();
        outpost.observe_request_calls(Arc::new(move |number, calls| {
            // The test may have finished with the outpost's calls.
            let _ = calls_tx.send((number, calls));
        }));
        Self {
            outpost,
            messages,
            heard,
            focus,
            foreground,
            foreground_reads,
            focus_window,
            listener_readability: RefCell::new(Arc::new(verbatim_win32::window::readability)),
            read_observer: RefCell::new(None),
            reported_set: RefCell::new(focus_set()),
            failed_reads_taken: Cell::new(0),
            position: Cell::new(0),
            observation,
            moves_to,
            last_asked: Cell::new(RequestNumber(0)),
            request_calls,
            told_calls: RefCell::default(),
            committed: RefCell::new(Vec::new()),
            app_window: super::find_window(&app.title),
            app_pid: pid,
        }
    }

    /// Reads each later focus reporting `set`, as Core asks with settings
    /// that leave the other properties out: the listener's read takes
    /// `set` with the description, which the completion's layout judgment
    /// reads whatever the settings, as Core's walked read does, and the
    /// completion reports `set`.
    pub fn read_with(&self, set: Vec<PropertyId>) {
        *self.reported_set.borrow_mut() = set;
    }

    /// Moves the keyboard focus, as the outpost reads it, to `hwnd` while
    /// the outpost's next read of the focused element is made, between its
    /// check of the focus window and its return, as a focus that moves on
    /// during `GetFocusedElementBuildCache` does.
    pub fn focus_moves_during_the_read(&self, hwnd: HWND) {
        self.moves_to.store(hwnd.0 as isize, Ordering::Relaxed);
    }

    /// Tells `observer` the name of each call the listener's read of an
    /// element Core names ([`Self::listener_object`]) is about to make,
    /// before each is made, so a test can change mockapp's windows between
    /// two of them.
    pub fn listener_observes_calls(&self, observer: verbatim_listener::CallObserver) {
        *self.read_observer.borrow_mut() = Some(observer);
    }

    /// Has the outpost, and the listener's read of an element Core names,
    /// judge window `hung` as a hung application's,
    /// as Windows judges a window whose thread has not answered for five
    /// seconds, and every other window by the system's judgement.
    pub fn judge_hung(&self, hung: HWND) {
        let hung = hung.0 as isize;
        let reader: verbatim_win32::window::WindowReadabilityReader = Arc::new(move |hwnd| {
            if hwnd == hung {
                verbatim_win32::window::Readability::Hung
            } else {
                verbatim_win32::window::readability(hwnd)
            }
        });
        self.outpost
            .set_window_readability_reader(Arc::clone(&reader));
        *self.listener_readability.borrow_mut() = reader;
    }

    /// How the stand-in for the listener judges whether a window can be
    /// read.
    pub fn listener_readability(&self) -> verbatim_win32::window::WindowReadabilityReader {
        Arc::clone(&self.listener_readability.borrow())
    }

    /// Has the listener's read alone judge whether a window can be read by
    /// `reader`, as the listener's one window readability reader judges its
    /// reads' windows, the outpost judging by the system's: a test of the
    /// listener's own tests for a hung window.
    pub fn listener_judges_readability(
        &self,
        reader: verbatim_win32::window::WindowReadabilityReader,
    ) {
        *self.listener_readability.borrow_mut() = reader;
    }

    /// Now on the outpost's observation clock ([`TestClock`]), which a fact
    /// the test hands over is stamped by.
    pub fn observed_now(&self) -> u64 {
        self.observation.now()
    }

    /// Starts the idle barrier's round `round`, as verbatim.exe does: the
    /// listener has nothing waiting, and the outpost is asked.
    pub fn idle_round(&self, round: u64) {
        self.outpost
            .handle_command(&CoreToOutpost::IdleQuery { round });
    }

    /// Makes `hwnd` the window the outpost reads as the foreground from now
    /// on.
    pub fn set_foreground(&self, hwnd: HWND) {
        self.foreground.store(hwnd.0 as isize, Ordering::Relaxed);
    }

    /// Makes `hwnd` the window the outpost reads as the one with the
    /// keyboard focus from now on, apart from the foreground window, as a
    /// child window holds the focus inside a top-level window.
    pub fn set_focus_window(&self, hwnd: HWND) {
        self.focus_window.store(hwnd.0 as isize, Ordering::Relaxed);
    }

    /// How many times the outpost has read the foreground window so far.
    pub fn foreground_reads(&self) -> u64 {
        self.foreground_reads.read()
    }

    /// Waits until the outpost has read the foreground window `count`
    /// times in all: the evidence that it has tested an entry against the
    /// foreground, while nothing else reads it.
    pub fn wait_for_foreground_reads(&self, count: u64) {
        self.foreground_reads.wait_for(count);
    }

    /// Moves the outpost's observation clock on by `by`.
    pub fn pass_time(&self, by: Duration) {
        self.observation
            .pass(u64::try_from(by.as_micros()).unwrap_or(u64::MAX));
    }

    /// Sends Core's `request` about window `window` through the outpost's
    /// request driver (`verbatim_outpost::driver`), as Core's request table
    /// sends it, with no verdict in force for the window, and returns its
    /// number.
    pub fn request(
        &self,
        window: HWND,
        request: impl Into<verbatim_model::request::Question>,
    ) -> RequestNumber {
        self.request_with(window, WindowVerdict::ProbeFirst, request)
    }

    /// [`Self::request`] with Core's verdict for the window, `verdict`.
    pub fn request_with(
        &self,
        window: HWND,
        verdict: WindowVerdict,
        request: impl Into<verbatim_model::request::Question>,
    ) -> RequestNumber {
        let mut driver =
            verbatim_outpost::driver::Driver::after(&self.outpost, self.last_asked.get());
        let number = driver.ask(WindowHandle(window.0 as u64), verdict, request);
        self.last_asked.set(driver.last());
        number
    }

    /// Sends Core's batch `batch` about window `window`, with Core's
    /// verdict for it `verdict`, through the outpost's request driver, and
    /// returns its answer, the next message the outpost writes, with the
    /// calls it made into the application.
    pub fn batch(
        &self,
        window: HWND,
        verdict: WindowVerdict,
        batch: verbatim_model::batch::Batch,
    ) -> (Answer, CallCounts) {
        let number = RequestNumber(self.last_asked.get().0 + 1);
        self.last_asked.set(number);
        verbatim_outpost::driver::Driver::new(&self.outpost).send(verbatim_model::request::Ask {
            number,
            window: WindowHandle(window.0 as u64),
            verdict,
            request: batch.into(),
        });
        self.answer_with_calls(number)
    }

    /// Cancels request `number` for `reason`, as Core does.
    pub fn cancel_request(
        &self,
        number: RequestNumber,
        reason: verbatim_model::request::CancelReason,
    ) {
        verbatim_outpost::driver::Driver::after(&self.outpost, self.last_asked.get())
            .cancel(number, reason);
    }

    /// The answer to request `number`, which must be the next message the
    /// outpost writes.
    pub fn answer(&self, number: RequestNumber) -> Answer {
        self.answer_with_calls(number).0
    }

    /// [`Self::answer`], with the calls the request made into the
    /// application, which the outpost tells before it answers.
    pub fn answer_with_calls(&self, number: RequestNumber) -> (Answer, CallCounts) {
        let answer = match self.next() {
            OutpostToCore::Answer(answer) if answer.number == number => answer,
            other => panic!("the answer to {number}, not {other:?}"),
        };
        (answer, self.calls_of(number))
    }

    /// The calls request `number` made into the application, which the
    /// outpost tells before it answers: every telling for it, a watch's
    /// one per program, is in hand once its answer is, and a request ended
    /// with no program, as one cancelled while it waited, made none.
    /// Tellings of other requests are kept for theirs.
    pub fn calls_of(&self, number: RequestNumber) -> CallCounts {
        let mut told = self.told_calls.borrow_mut();
        while let Ok((request, calls)) = self.request_calls.try_recv() {
            *told.entry(request).or_default() += calls;
        }
        told.remove(&number).unwrap_or_default()
    }

    /// Hands the outpost `ask`, a request Core numbered itself, through
    /// the request driver, as Core's shell sends it, and returns at once.
    pub fn forward(&self, mut ask: verbatim_model::request::Ask) {
        ask.unstamp();
        verbatim_outpost::driver::Driver::new(&self.outpost).send(ask);
    }

    /// mockapp's window.
    pub fn app_window(&self) -> HWND {
        self.app_window
    }

    /// The window the outpost and the listener's reader read as the
    /// foreground.
    pub fn foreground(&self) -> isize {
        self.foreground.load(Ordering::Relaxed)
    }

    /// Sends `request` about `window` and returns its answer, the next
    /// message the outpost writes.
    pub fn requested(&self, window: HWND, request: Request) -> Answer {
        let number = self.request(window, request);
        self.answer(number)
    }

    /// The ancestors Core already knows, as a request names them: the
    /// focus the stand-in for Core committed last, then its ancestors
    /// nearest first, each by its node, its runtime id and its own window,
    /// as NVDA's walk meets its focus object as it meets the focus's
    /// ancestors.
    pub fn known(&self) -> Vec<KnownAncestor> {
        self.committed
            .borrow()
            .iter()
            .flat_map(|(_, known)| known.iter().cloned())
            .collect()
    }

    /// The names of the committed focus's ancestors, outermost first, as
    /// the stand-in for Core holds them: those its answer carried, then
    /// the earlier line's from the one its walk met.
    pub fn committed_ancestor_names(&self) -> Vec<Option<String>> {
        self.committed
            .borrow()
            .iter()
            .skip(1)
            .rev()
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Stands in for Core committing the focus `read` answers to a request
    /// that named `sent` as the ancestors it already knows: the focus,
    /// then the answer's ancestors, then the committed line from the one
    /// the walk met, the committed focus itself among them.
    pub fn commit_focus(&self, read: &FocusRead, sent: &[KnownAncestor]) {
        let mut committed = self.committed.borrow_mut();
        let entry = |node: &FocusNode| (name_of(node).map(str::to_owned), known_of(node));
        let mut line: Vec<_> = std::iter::once(entry(&read.focus))
            .chain(read.ancestors.iter().map(|ancestor| entry(&ancestor.node)))
            .collect();
        if let Some(met) = read.met {
            let met = &sent[usize::try_from(met).expect("a small place")];
            let place = committed
                .iter()
                .position(|(_, entries)| entries.contains(met))
                .expect("the ancestor met is one Core knows");
            line.extend(committed.drain(place..));
        }
        *committed = line;
    }

    /// Forgets the focus the stand-in for Core committed: Core's focus
    /// moved to another application.
    pub fn forget_focus(&self) {
        self.committed.borrow_mut().clear();
    }

    /// What the listener answers for Core's batch `batch` on `element`, the
    /// element a UI Automation event of mockapp named, which the test holds
    /// as the listener keeps it: run by the listener's own code
    /// (`verbatim_listener::read_object`) on this thread, with the calls
    /// it made.
    pub fn listener_object(
        &self,
        element: &Cached<ListenerEvent>,
        batch: &verbatim_model::batch::Batch,
    ) -> (Outcome, CallCounts) {
        self.listener_object_in(element, nearest_window(element), batch)
    }

    /// [`Self::listener_object`], Core's request naming `nearest`, the
    /// nearest window the check thread's walk found when the event was
    /// delivered, before the test changed how mockapp answers that walk.
    pub fn listener_object_in(
        &self,
        element: &Cached<ListenerEvent>,
        nearest: isize,
        batch: &verbatim_model::batch::Batch,
    ) -> (Outcome, CallCounts) {
        let uia = verbatim_uia::Uia::new().expect("a UIA client for the listener's reads");
        let _ = verbatim_win32::calls::take();
        let readability = self.listener_readability();
        let outcome = match self.read_observer.borrow().clone() {
            Some(observer) => verbatim_listener::read_object_observed(
                (&uia, &readability),
                element,
                (Pid(self.app_pid), nearest),
                batch,
                observer,
            ),
            None => verbatim_listener::read_object(
                (&uia, &readability),
                element,
                (Pid(self.app_pid), nearest),
                batch,
            ),
        };
        (outcome, verbatim_win32::calls::take())
    }

    /// The listener's read of a UI Automation focus on `element`, as Core
    /// asks it: the element and its ancestors up to the ones Core already
    /// knows, of which the stand-in for Core makes the route, with the
    /// name its event's cache had and the focused state, as Core does.
    /// With the calls it made.
    pub fn listener_focus(&self, element: &Cached<ListenerEvent>) -> (ListenerFocus, CallCounts) {
        self.listener_focus_in(element, nearest_window(element))
    }

    /// [`Self::listener_focus`], Core's request naming `nearest`, the
    /// nearest window the check thread's walk found at the event's
    /// delivery ([`nearest_window`]).
    pub fn listener_focus_in(
        &self,
        element: &Cached<ListenerEvent>,
        nearest: isize,
    ) -> (ListenerFocus, CallCounts) {
        use verbatim_model::batch::{Batch, Done, Op, Placement, Step, StepOutcome, Subject};
        let sent = self.known();
        let mut asked = self.reported_set.borrow().clone();
        for id in [
            PropertyId::Description,
            PropertyId::RuntimeId,
            PropertyId::NativeWindow,
            PropertyId::IsContent,
        ] {
            if !asked.contains(&id) {
                asked.push(id);
            }
        }
        let kept = Subject::Kept {
            listener: 1,
            token: verbatim_model::tell::ElementToken(0),
            keep: true,
        };
        let batch = Batch {
            steps: vec![
                Step {
                    op: Op::Read {
                        subject: kept.clone(),
                        set: asked.clone(),
                    },
                    when: None,
                },
                Step {
                    op: Op::Ancestors {
                        subject: kept,
                        until: sent.clone(),
                        depth: verbatim_model::batch::MAX_DEPTH,
                        set: asked,
                    },
                    when: None,
                },
            ],
            placement: Placement::Queue,
            spare: None,
        };
        let (outcome, calls) = self.listener_object_in(element, nearest, &batch);
        let Outcome::Result(Reply::Steps(steps)) = outcome else {
            panic!("the listener answered {outcome:?}, not the focus's read");
        };
        let [
            StepOutcome::Done(Done::Read(mut object)),
            StepOutcome::Done(Done::Ancestors(ancestry)),
        ] = <[StepOutcome; 2]>::try_from(steps).expect("two steps")
        else {
            panic!("the listener read no focus");
        };
        // As Core takes a focus's element: the name its event's cache had,
        // and the focused state.
        let cached = element.read().name.clone().filter(|name| !name.is_empty());
        for answer in &mut object.values.0 {
            match (answer.id, &mut answer.value) {
                (PropertyId::Name, value) => *value = cached.clone().map(PropertyValue::Text),
                (PropertyId::States, Some(PropertyValue::States(states))) => {
                    *states = states.with(verbatim_model::State::Focused);
                }
                _ => {}
            }
        }
        let route = route_of(&object, &ancestry, &sent);
        (
            ListenerFocus {
                object: ListenerObject {
                    object,
                    ancestors: ancestry.ancestors,
                    route: Some(route),
                },
                sent,
            },
            calls,
        )
    }

    /// The outpost's read of the focus whose route the listener's read
    /// `focus` gives, as Core asks it, which the stand-in for Core commits
    /// when it answers a focus; with its calls. Its window is read through
    /// UI Automation, as the listener's check found the focus's nearest
    /// window before Core asked: Core asks for it only then.
    pub fn complete_focus(&self, focus: ListenerFocus) -> (FocusOutcome, CallCounts) {
        let ListenerFocus { object, sent } = focus;
        let ListenerObject {
            object,
            ancestors,
            route,
        } = object;
        let mut route = route.expect("a focus read answers its route");
        route.read = std::iter::once(object.values)
            .chain(ancestors.into_iter().map(|ancestor| ancestor.values))
            .take(route.steps.len())
            .collect();
        let window = hwnd(route.window);
        let top = verbatim_win32::window::top_level_of(window.0 as isize);
        let top = WindowHandle(u64::try_from(top).expect("a window handle"));
        let batch = focus_batch(
            Some(verbatim_model::batch::WindowOf::TopLevelOf(top)),
            None,
            verbatim_model::batch::Subject::Route(Box::new(route)),
            (sent.clone(), self.reported_set.borrow().clone()),
        );
        let (answer, calls) = self.batch(window, WindowVerdict::Uia, batch.clone());
        let outcome = focus_outcome(&batch, answer.outcome);
        if let FocusOutcome::Result(FocusReply::Focus(read)) = &outcome {
            self.commit_focus(read, &sent);
        }
        (outcome, calls)
    }

    /// A UI Automation focus on `element`, which mockapp reports focused,
    /// read as Core reads it: the listener's read
    /// ([`Self::listener_focus`]), then the outpost's read of its route
    /// ([`Self::complete_focus`]). Returns the outpost's answer, with the
    /// calls of both reads.
    pub fn uia_focus_read(&self, element: &Cached<ListenerEvent>) -> (FocusOutcome, CallCounts) {
        self.uia_focus_read_in(element, nearest_window(element))
    }

    /// [`Self::uia_focus_read`], Core's request naming `nearest`, the
    /// nearest window found at the event's delivery ([`nearest_window`]).
    pub fn uia_focus_read_in(
        &self,
        element: &Cached<ListenerEvent>,
        nearest: isize,
    ) -> (FocusOutcome, CallCounts) {
        let (focus, listener_calls) = self.listener_focus_in(element, nearest);
        let (outcome, calls) = self.complete_focus(focus);
        (outcome, listener_calls + calls)
    }

    /// [`Self::uia_focus_read`], which must answer a focus.
    pub fn uia_focus_answer(&self, element: &Cached<ListenerEvent>) -> Focused {
        self.uia_focus_answer_in(element, nearest_window(element))
    }

    /// [`Self::uia_focus_answer`], Core's request naming `nearest`, the
    /// nearest window found at the event's delivery ([`nearest_window`]).
    pub fn uia_focus_answer_in(&self, element: &Cached<ListenerEvent>, nearest: isize) -> Focused {
        let _ = verbatim_uia::calls::take_failed_reads();
        let (outcome, calls) = self.uia_focus_read_in(element, nearest);
        let listener_failed = verbatim_uia::calls::take_failed_reads();
        match outcome {
            FocusOutcome::Result(FocusReply::Focus(read)) => Focused {
                read: *read,
                facts: None,
                calls,
                failed_reads: listener_failed + self.take_failed_reads(),
            },
            other => panic!("the completion answered {other:?}, not a focus"),
        }
    }

    /// One MSAA candidate of mockapp's, by the ids its event named and its
    /// kind, with no verdict in force for its window, read to `depth` as
    /// Core asks it with the ancestors it already knows; the focus it
    /// reads is committed. Returns the answer and its calls.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "its callers build the depth in place"
    )]
    pub fn msaa_run(
        &self,
        (ids, kind): (MsaaIds, CandidateKind),
        depth: Depth,
    ) -> (CandidateAnswer, CallCounts) {
        let sent = self.known();
        let (reply, calls) = self.candidate((ids, kind), &depth, (sent.clone(), focus_set()));
        let run = match reply {
            FocusReply::Candidate(run) => run,
            other => panic!("the run answered {other:?}"),
        };
        if let Some(read) = &run.read {
            self.commit_focus(read, &sent);
        }
        (run, calls)
    }

    /// One MSAA focus candidate of mockapp's at `ids`, read to `depth` as
    /// Core asks it, with no verdict in force: the reply as Core judges
    /// the outpost's answer, committing nothing, and its calls.
    pub fn msaa_run_reply(&self, ids: MsaaIds, depth: Depth) -> (FocusReply, CallCounts) {
        self.msaa_run_reply_with(ids, depth, focus_set())
    }

    /// [`Self::msaa_run_reply`] with the properties `set`.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "its callers build the depth in place"
    )]
    pub fn msaa_run_reply_with(
        &self,
        ids: MsaaIds,
        depth: Depth,
        set: Vec<PropertyId>,
    ) -> (FocusReply, CallCounts) {
        self.candidate((ids, CandidateKind::Focus), &depth, (self.known(), set))
    }

    /// Sends the candidate at `ids` of `kind`, asked to `depth` by Core's
    /// batch with no verdict in force, not waiting for its answer: its
    /// number and the batch, which [`candidate_outcome`] judges its answer
    /// against.
    pub fn ask_candidate(
        &self,
        (ids, kind): (MsaaIds, CandidateKind),
        depth: &Depth,
    ) -> (RequestNumber, verbatim_model::batch::Batch) {
        let batch = candidate_batch((ids, kind), depth, (self.known(), focus_set()));
        (self.request(hwnd(ids.window), batch.clone()), batch)
    }

    /// Sends the read of the application's current focus as Core asks it,
    /// naming `window`, not waiting for its answer: its number and the
    /// batch, which [`focus_outcome`] judges its answer against.
    pub fn ask_current_focus(&self, window: HWND) -> (RequestNumber, verbatim_model::batch::Batch) {
        let batch = focus_batch(
            Some(verbatim_model::batch::WindowOf::Focused),
            None,
            verbatim_model::batch::Subject::Focus,
            (self.known(), focus_set()),
        );
        (self.request(window, batch.clone()), batch)
    }

    /// The candidate at `ids` of `kind` asked to `depth` by Core's batch,
    /// the ancestors it knows `known`, by `set`: its answer as Core judges
    /// it, which must be a result, and its calls.
    fn candidate(
        &self,
        (ids, kind): (MsaaIds, CandidateKind),
        depth: &Depth,
        (known, set): (Vec<KnownAncestor>, Vec<PropertyId>),
    ) -> (FocusReply, CallCounts) {
        let batch = candidate_batch((ids, kind), depth, (known, set));
        let (answer, calls) =
            self.batch(hwnd(ids.window), WindowVerdict::ProbeFirst, batch.clone());
        let reply = match answer.outcome {
            Outcome::Refused(verbatim_model::request::Refusal::ReadThroughUia) => {
                FocusReply::Candidate(CandidateAnswer {
                    facts: CandidateFacts::Skipped,
                    read: None,
                })
            }
            Outcome::Result(Reply::Steps(steps)) => candidate_reply(&batch, (kind, depth), &steps),
            other => panic!("the run answered {other:?}"),
        };
        (reply, calls)
    }

    /// The focus an MSAA run of one candidate, the focus event at `ids`,
    /// finds, which it must: as [`Self::msaa_run`].
    pub fn msaa_focus_answer(&self, ids: MsaaIds) -> Focused {
        self.run_focus(ids, CandidateKind::Focus)
    }

    /// The focus an MSAA run of the one candidate `ids` of `kind` finds,
    /// which it must.
    pub fn run_focus(&self, ids: MsaaIds, kind: CandidateKind) -> Focused {
        let (run, calls) = self.msaa_run((ids, kind), Depth::Full);
        match run.read {
            Some(read) => Focused {
                read: *read,
                facts: Some(run.facts),
                calls,
                failed_reads: self.take_failed_reads(),
            },
            None => panic!(
                "the candidate's facts are {:?}, with no focus read",
                run.facts
            ),
        }
    }

    /// The application's current focus, asked as Core asks it with the
    /// ancestors it already knows, which it commits; with its calls.
    pub fn current_focus(&self) -> (FocusOutcome, CallCounts) {
        let sent = self.known();
        let batch = focus_batch(
            Some(verbatim_model::batch::WindowOf::Focused),
            None,
            verbatim_model::batch::Subject::Focus,
            (sent.clone(), focus_set()),
        );
        // Core names the window its foreground record holds.
        let (answer, calls) = self.batch(
            HWND(self.foreground() as _),
            WindowVerdict::ProbeFirst,
            batch.clone(),
        );
        let outcome = focus_outcome(&batch, answer.outcome);
        if let FocusOutcome::Result(FocusReply::Candidate(CandidateAnswer {
            read: Some(read),
            ..
        })) = &outcome
        {
            self.commit_focus(read, &sent);
        }
        (outcome, calls)
    }

    /// The outpost's next message, counted as Core counts the messages
    /// that carry nodes.
    pub fn next(&self) -> OutpostToCore {
        let message = self
            .messages
            .recv_timeout(super::WAIT_TIMEOUT)
            .expect("the outpost says something before the wait times out");
        if message.carries_nodes() {
            self.position.set(self.position.get() + 1);
        }
        message
    }

    /// Delivers Core's list of the nodes it holds in the outpost, `held`,
    /// acknowledging every message read so far, as verbatim.exe writes it
    /// after a focus is committed: the outpost releases every node it carried at
    /// or before that position that is not among them, and so stops reading
    /// changes for it.
    pub fn release(&self, held: &[NodeId]) {
        self.outpost.handle_command(&CoreToOutpost::NodesHeld {
            nodes: held.iter().map(|id| id.number()).collect(),
            anchors: Vec::new(),
            told: Vec::new(),
            acknowledged: self.position.get(),
        });
    }

    /// [`release`](Self::release) with the focus line the stand-in for
    /// Core committed ([`Self::commit_focus`]): what Core writes after it
    /// commits a focus.
    pub fn release_to_core_line(&self) {
        let line: Vec<NodeId> = self
            .committed
            .borrow()
            .iter()
            .flat_map(|(_, known)| known.iter())
            .filter_map(|known| match known {
                KnownAncestor::Node(id) => Some(*id),
                _ => None,
            })
            .collect();
        self.release(&line);
    }

    /// [`release`](Self::release) of every node: Core's focus moved to
    /// another application, and it holds none of the outpost's.
    pub fn release_all(&self) {
        self.forget_focus();
        self.release(&[]);
    }

    /// Waits for the outpost to take in mockapp's next events, which must
    /// be exactly `expected`, in any order: the evidence that the events
    /// mockapp raised have reached the outpost, whose handling
    /// [`settled`](Self::settled) then covers. mockapp acknowledges a
    /// command once its call that raised the event has returned, but a
    /// client receives the event later, on threads of its own. One UIA
    /// event can reach the outpost twice, by two routes in no fixed order:
    /// UIA's own, and the `WinEvent` UIA raises for MSAA clients alongside
    /// it.
    pub fn heard(&self, expected: &[Heard]) {
        let mut waiting = expected.to_vec();
        while !waiting.is_empty() {
            let heard = self
                .heard
                .recv_timeout(super::WAIT_TIMEOUT)
                .unwrap_or_else(|_| {
                    panic!(
                        "the outpost did not hear {waiting:?} within {:?}",
                        super::WAIT_TIMEOUT
                    )
                });
            let Some(at) = waiting.iter().position(|&event| event == heard) else {
                panic!("the outpost heard {heard:?} while it waited for {waiting:?}");
            };
            waiting.swap_remove(at);
        }
    }

    /// Waits for the outpost to settle and asserts it said nothing more.
    /// The failed cached reads the outpost has made since the last take,
    /// once it has handled everything handed to it
    /// ([`Outpost::failed_reads`]).
    pub fn take_failed_reads(&self) -> u32 {
        self.outpost.settle();
        let now = self.outpost.failed_reads();
        now - self.failed_reads_taken.replace(now)
    }

    /// Lets the outpost handle everything handed to it, without asserting
    /// what it said meanwhile.
    pub fn settle(&self) {
        self.outpost.settle();
    }

    pub fn settled(&self) {
        self.outpost.settle();
        match self.messages.try_recv() {
            Err(TryRecvError::Empty) => {}
            Ok(message) => panic!("the outpost said more: {message:?}"),
            Err(TryRecvError::Disconnected) => panic!("the outpost's pipe closed"),
        }
    }

    /// One object-navigation step from `node` in `window`, as Core asks it
    /// with the window's verdict in force since its focus: what the step
    /// reached, which it must, and the calls it made.
    pub fn navigate(
        &self,
        window: HWND,
        node: NodeId,
        direction: QueryKind,
    ) -> (NodeRead, CallCounts) {
        let number = self.request_with(
            window,
            WindowVerdict::Uia,
            Request::Navigate {
                node,
                direction,
                set: focus_set(),
            },
        );
        match self.answer_with_calls(number) {
            (
                Answer {
                    outcome: Outcome::Result(Reply::Navigated(Some(read))),
                    ..
                },
                calls,
            ) => (read, calls),
            (other, _) => panic!("the step answered {:?}, not a node", other.outcome),
        }
    }

    /// The outcome of Core's `request` about a node in `window`, with the
    /// window's verdict in force since its focus, and the calls it made.
    pub fn asked(&self, window: HWND, request: Request) -> (Outcome, CallCounts) {
        let number = self.request_with(window, WindowVerdict::Uia, request);
        let (answer, calls) = self.answer_with_calls(number);
        (answer.outcome, calls)
    }

    /// Makes the outpost's read of the focused element, for Core's request
    /// for the current focus, answer `element`.
    pub fn read_focus_as(&self, element: &IUIAutomationElement) {
        *self.focus.lock().unwrap_or_else(PoisonError::into_inner) =
            Some(AgileReference::new(element).expect("an agile reference"));
    }
}

/// Milliseconds since the Unix epoch on the run's shared clock, as the
/// hook stamps a caret key's press.
pub fn now_ms() -> u64 {
    verbatim_process::clock::now_us() / 1_000
}

/// Microseconds since the Unix epoch on the run's shared clock, as the
/// listener stamps an event's arrival.
pub fn now_us() -> u64 {
    verbatim_process::clock::now_us()
}

/// The properties Core reads of a focus and its ancestors.
pub fn focus_set() -> Vec<PropertyId> {
    vec![
        PropertyId::Name,
        PropertyId::Role,
        PropertyId::Value,
        PropertyId::States,
        PropertyId::Description,
        PropertyId::KeyboardShortcut,
        PropertyId::PositionInSet,
        PropertyId::SetSize,
        PropertyId::Level,
        PropertyId::Rect,
        PropertyId::ClassName,
        PropertyId::AutomationId,
        PropertyId::NativeWindow,
        PropertyId::Process,
        PropertyId::RuntimeId,
        PropertyId::HasTextPattern,
        PropertyId::RangeValue,
        PropertyId::Cell,
        PropertyId::TableCounts,
    ]
}

/// `window` as a window handle of this process's API.
pub fn hwnd(window: WindowHandle) -> HWND {
    HWND(isize::try_from(window.0).expect("a window handle") as _)
}

/// The MSAA ids mockapp raises an event of its node `index` with, in
/// `window`: the node at its own object id.
pub fn node_ids(window: HWND, index: usize) -> MsaaIds {
    MsaaIds {
        window: WindowHandle(window.0 as u64),
        object: i32::try_from(index + 1).expect("a small index"),
        child: 0,
    }
}

/// The entries Core names an ancestor by in its list of the ancestors it
/// already knows: its node, its runtime id and its own window.
fn known_of(node: &FocusNode) -> Vec<KnownAncestor> {
    let mut entries = Vec::new();
    if let Some(id) = node.read.node {
        entries.push(KnownAncestor::Node(id));
    }
    if let Some(runtime_id) = &node.runtime_id {
        entries.push(KnownAncestor::Uia(runtime_id.clone()));
    }
    if let Some(PropertyValue::Window(window)) = value(&node.read.values, PropertyId::NativeWindow)
        && window.0 != 0
    {
        entries.push(KnownAncestor::Window(*window));
    }
    entries
}

/// The value of property `id` among `values`, `None` when it has none or
/// was not read.
pub fn value(values: &PropertyValues, id: PropertyId) -> Option<&PropertyValue> {
    values.answer(id).and_then(|answer| answer.value.as_ref())
}

/// The listener's answer to Core's read of a UI Automation focus, with
/// the ancestors Core already knew when it asked, which its completion
/// names again.
pub struct ListenerFocus {
    /// The listener's answer, with its route.
    pub object: ListenerObject,
    /// The ancestors Core already knew.
    pub sent: Vec<KnownAncestor>,
}

/// A focus answer, as a test reads it.
pub struct Focused {
    /// The answer.
    pub read: FocusRead,
    /// The facts an MSAA candidate's or a current focus's answer carried.
    pub facts: Option<CandidateFacts>,
    /// The calls the reads made, the listener's and the outpost's.
    pub calls: CallCounts,
    /// The failed cached reads they made.
    pub failed_reads: u32,
}

impl Focused {
    /// The focus `outcome`, a completion's or a current focus's answer,
    /// reads, which it must, with `calls`.
    pub fn of(outcome: FocusOutcome, calls: CallCounts) -> Self {
        match outcome {
            FocusOutcome::Result(FocusReply::Focus(read)) => Self {
                read: *read,
                facts: None,
                calls,
                failed_reads: 0,
            },
            FocusOutcome::Result(FocusReply::Candidate(CandidateAnswer {
                facts,
                read: Some(read),
            })) => Self {
                read: *read,
                facts: Some(facts),
                calls,
                failed_reads: 0,
            },
            other => panic!("the outpost answered {other:?}, not a focus"),
        }
    }

    /// The names of the ancestors the answer carries, outermost first.
    pub fn ancestor_names(&self) -> Vec<Option<&str>> {
        self.read
            .ancestors
            .iter()
            .rev()
            .map(|ancestor| name_of(&ancestor.node))
            .collect()
    }

    /// The focus's name.
    pub fn name(&self) -> Option<&str> {
        name_of(&self.read.focus)
    }

    /// The focus's node.
    pub fn node(&self) -> NodeId {
        self.read
            .focus
            .read
            .node
            .expect("an outpost mints the focus's node")
    }

    /// The focus's role.
    pub fn role(&self) -> Role {
        role_of(&self.read.focus)
    }

    /// The focus's states.
    pub fn states(&self) -> StateSet {
        match value(&self.read.focus.read.values, PropertyId::States) {
            Some(PropertyValue::States(states)) => *states,
            _ => StateSet::default(),
        }
    }

    /// The names of the focus's chain, outermost first, ending with the
    /// focus: the ancestors the answer carries, up to the one Core already
    /// knows that its walk met.
    pub fn chain(&self) -> Vec<Option<&str>> {
        self.read
            .ancestors
            .iter()
            .rev()
            .map(|ancestor| name_of(&ancestor.node))
            .chain(std::iter::once(self.name()))
            .collect()
    }
}

/// The name a focus answer's node was read with.
pub fn name_of(node: &FocusNode) -> Option<&str> {
    name_in(&node.read.values)
}

/// The role a focus answer's node was read with.
pub fn role_of(node: &FocusNode) -> Role {
    role_in(&node.read.values)
}

/// The description a focus answer's node was read with: a dialog's text.
pub fn description_of(node: &FocusNode) -> Option<&str> {
    match value(&node.read.values, PropertyId::Description) {
        Some(PropertyValue::Text(text)) => Some(text),
        _ => None,
    }
}

/// The name among `values`.
pub fn name_in(values: &PropertyValues) -> Option<&str> {
    match value(values, PropertyId::Name) {
        Some(PropertyValue::Text(name)) => Some(name),
        _ => None,
    }
}

/// The role among `values`, unknown when it was not read.
pub fn role_in(values: &PropertyValues) -> Role {
    match value(values, PropertyId::Role) {
        Some(PropertyValue::Role(role)) => *role,
        _ => Role::Unknown,
    }
}

/// The nearest window of `element`, as the listener's check thread's walk
/// finds it when the element's event is delivered, which Core's request
/// names: a test that changes how mockapp answers the walk afterwards finds
/// it first.
pub fn nearest_window(element: &Cached<ListenerEvent>) -> isize {
    verbatim_uia::nearest_window_lookup(element.live().element())
        .expect("the check's walk to the nearest window")
        .expect("the element has a nearest window")
}

/// What the outpost found of an MSAA candidate, as Core judges it from its
/// batch's outcomes (`verbatim-core`'s `focus::validity::CandidateFacts`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CandidateFacts {
    /// Its window is read through UI Automation: the batch was refused.
    Skipped,
    /// The object is not there.
    NotThere,
    /// The object was found.
    Found {
        /// Whether it or an ancestor has the focused state, when read.
        focused: Option<bool>,
        /// Its role, read for a menu start.
        role: Option<Role>,
    },
}

/// A candidate's facts, and its full read when the batch read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CandidateAnswer {
    /// The facts.
    pub facts: CandidateFacts,
    /// The full read.
    pub read: Option<Box<FocusRead>>,
}

/// What kind of event named a candidate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CandidateKind {
    /// A focus event: its focused state is tested.
    Focus,
    /// A foreground event.
    Foreground,
    /// A menu start: its object taken with no redirect, its role read.
    MenuStart,
}

/// What an MSAA object reads as, which the identity check compares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    /// Its states.
    pub states: StateSet,
    /// Its role.
    pub role: Role,
    /// Its position in its set.
    pub position_in_set: Option<u32>,
    /// Its set's size.
    pub set_size: Option<u32>,
    /// Its name.
    pub name: Option<String>,
    /// Its value.
    pub value: Option<String>,
}

/// How far Core's batch reads a candidate (`verbatim-core`'s
/// `focus::msaa_run`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Depth {
    /// Its facts alone.
    Facts,
    /// In full only when focused.
    FullIfFocused,
    /// In full.
    Full,
    /// As [`Depth::FullIfFocused`], unless it is the object at `ids`
    /// reading as `identity`.
    FullUnlessSame {
        /// The ids of the readers' last focus.
        ids: MsaaIds,
        /// What it reads as.
        identity: Box<Identity>,
    },
}

/// A focus read's result as Core judges its batch's outcomes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FocusReply {
    /// A route's or the current focus's read.
    Focus(Box<FocusRead>),
    /// A candidate's facts and read.
    Candidate(CandidateAnswer),
    /// The candidate is the object the identity check named.
    CandidateSame,
    /// The window's top-level window is a hidden dialog.
    HiddenDialog(NodeWindow),
}

/// A focus read's outcome as Core judges it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FocusOutcome {
    /// Its result.
    Result(FocusReply),
    /// Refused.
    Refused(verbatim_model::request::Refusal),
    /// Not there.
    NotThere,
    /// Not responding.
    NotResponding,
    /// The reader ended.
    Ended,
}

/// The hidden-dialog check as Core asks it (`verbatim-core`'s
/// `focus::shown::check`).
pub fn hidden_check() -> verbatim_model::matcher::Matcher {
    use verbatim_model::matcher::Matcher;
    Matcher::All(vec![
        Matcher::Is(PropertyId::WindowShown, PropertyValue::Flag(false)),
        Matcher::IsAnyOf(
            PropertyId::Role,
            [Role::Dialog, Role::Alert, Role::PropertyPage]
                .into_iter()
                .map(PropertyValue::Role)
                .collect(),
        ),
    ])
}

/// The test of an object neither it nor an ancestor of which has the
/// focused state.
fn unfocused() -> verbatim_model::matcher::Matcher {
    verbatim_model::matcher::Matcher::Is(PropertyId::FocusedWithin, PropertyValue::Flag(false))
}

/// The identity check as Core writes it (`verbatim-core`'s
/// `focus::msaa_run::UnlessSame::matcher`), the object held by its ids'
/// child.
fn same(ids: MsaaIds, identity: &Identity) -> verbatim_model::matcher::Matcher {
    use verbatim_model::matcher::Matcher;
    let count = |id, count: Option<u32>| match count {
        Some(count) => Matcher::Any(vec![
            Matcher::Absent(id),
            Matcher::Is(id, PropertyValue::Count(count)),
        ]),
        None => Matcher::Absent(id),
    };
    let text = |id, text: &Option<String>| match text {
        Some(text) => Matcher::Is(id, PropertyValue::Text(text.clone())),
        None => Matcher::Absent(id),
    };
    Matcher::All(vec![
        Matcher::Is(
            PropertyId::MsaaAddress,
            PropertyValue::MsaaAddress(verbatim_model::request::HeldIds {
                event: ids,
                held_child: ids.child,
            }),
        ),
        Matcher::Is(PropertyId::States, PropertyValue::States(identity.states)),
        Matcher::Is(PropertyId::Role, PropertyValue::Role(identity.role)),
        count(PropertyId::PositionInSet, identity.position_in_set),
        count(PropertyId::SetSize, identity.set_size),
        text(PropertyId::Name, &identity.name),
        text(PropertyId::Value, &identity.value),
    ])
}

/// A step of a batch, run when the step at `on` did not match, if given.
fn step(op: verbatim_model::batch::Op, unless: Option<u8>) -> verbatim_model::batch::Step {
    verbatim_model::batch::Step {
        op,
        when: unless.map(|on| verbatim_model::batch::Condition {
            on: verbatim_model::batch::Place(on),
            check: verbatim_model::batch::Check::Matched,
            negated: true,
        }),
    }
}

/// A focus read of `subject` as Core asks it: the hidden-dialog check of
/// `hidden`'s window first when given, the test `facts` of the subject
/// next when given, then its read unless either matched.
fn focus_batch(
    hidden: Option<verbatim_model::batch::WindowOf>,
    facts: Option<verbatim_model::matcher::Matcher>,
    subject: verbatim_model::batch::Subject,
    (known, set): (Vec<KnownAncestor>, Vec<PropertyId>),
) -> verbatim_model::batch::Batch {
    use verbatim_model::batch::{Batch, Op, Placement, Subject};
    let mut steps = Vec::new();
    if let Some(which) = hidden {
        steps.push(step(
            Op::Test {
                subject: Subject::Window {
                    which,
                    backend: verbatim_model::Backend::Msaa,
                    visible: false,
                },
                matcher: hidden_check(),
            },
            None,
        ));
    }
    let last = |steps: &Vec<verbatim_model::batch::Step>| {
        steps
            .len()
            .checked_sub(1)
            .map(|place| u8::try_from(place).expect("a short batch"))
    };
    if let Some(matcher) = facts {
        let unless = last(&steps);
        steps.push(step(
            Op::Test {
                subject: subject.clone(),
                matcher,
            },
            unless,
        ));
    }
    let unless = last(&steps);
    steps.push(step(
        Op::FocusRead {
            subject,
            known,
            set,
            details: verbatim_model::TextDetails::NONE,
        },
        unless,
    ));
    Batch {
        steps,
        placement: Placement::Queue,
        spare: None,
    }
}

/// A candidate's batch at `ids` of `kind`, to `depth`, as Core writes it
/// (`verbatim-core`'s `focus::msaa_run::build`).
fn candidate_batch(
    (ids, kind): (MsaaIds, CandidateKind),
    depth: &Depth,
    (known, set): (Vec<KnownAncestor>, Vec<PropertyId>),
) -> verbatim_model::batch::Batch {
    use verbatim_model::batch::{Batch, Op, Placement, Subject, WindowOf};
    let subject = Subject::Msaa {
        ids,
        redirect: kind != CandidateKind::MenuStart,
    };
    let hidden = (!matches!(depth, Depth::Facts) && kind != CandidateKind::MenuStart)
        .then_some(WindowOf::TopLevelOf(ids.window));
    match (kind, depth) {
        (CandidateKind::MenuStart, _) | (CandidateKind::Foreground, Depth::Facts) => {
            let set_read = if kind == CandidateKind::MenuStart {
                vec![PropertyId::Role]
            } else {
                Vec::new()
            };
            let mut steps = vec![step(
                Op::Read {
                    subject: subject.clone(),
                    set: set_read,
                },
                None,
            )];
            if *depth != Depth::Facts {
                steps.push(step(
                    Op::FocusRead {
                        subject,
                        known,
                        set,
                        details: verbatim_model::TextDetails::NONE,
                    },
                    None,
                ));
            }
            Batch {
                steps,
                placement: Placement::Queue,
                spare: None,
            }
        }
        (CandidateKind::Foreground, _) => focus_batch(hidden, None, subject, (known, set)),
        // In full: the focused state read as a fact, the read not waiting
        // on it.
        (CandidateKind::Focus, Depth::Full) => Batch {
            steps: vec![
                step(
                    Op::Test {
                        subject: Subject::Window {
                            which: WindowOf::TopLevelOf(ids.window),
                            backend: verbatim_model::Backend::Msaa,
                            visible: false,
                        },
                        matcher: hidden_check(),
                    },
                    None,
                ),
                step(
                    Op::Test {
                        subject: subject.clone(),
                        matcher: unfocused(),
                    },
                    Some(0),
                ),
                step(
                    Op::FocusRead {
                        subject,
                        known,
                        set,
                        details: verbatim_model::TextDetails::NONE,
                    },
                    Some(0),
                ),
            ],
            placement: Placement::Queue,
            spare: None,
        },
        (CandidateKind::Focus, Depth::Facts) => Batch {
            steps: vec![step(
                Op::Test {
                    subject,
                    matcher: unfocused(),
                },
                None,
            )],
            placement: Placement::Queue,
            spare: None,
        },
        (
            CandidateKind::Focus,
            Depth::FullUnlessSame {
                ids: named,
                identity,
            },
        ) => focus_batch(
            hidden,
            Some(verbatim_model::matcher::Matcher::Any(vec![
                unfocused(),
                same(*named, identity),
            ])),
            subject,
            (known, set),
        ),
        (CandidateKind::Focus, _) => focus_batch(hidden, Some(unfocused()), subject, (known, set)),
    }
}

/// The `FocusedWithin` `values` read, when they read it.
fn focused_within(values: &PropertyValues) -> Option<bool> {
    match value(values, PropertyId::FocusedWithin) {
        Some(PropertyValue::Flag(focused)) => Some(*focused),
        _ => None,
    }
}

/// The outcome `outcome` of a candidate's batch `batch`, of `kind`, to
/// `depth`, as Core judges it.
pub fn candidate_outcome(
    batch: &verbatim_model::batch::Batch,
    (kind, depth): (CandidateKind, &Depth),
    outcome: Outcome,
) -> FocusOutcome {
    match outcome {
        Outcome::Result(Reply::Steps(steps)) => {
            FocusOutcome::Result(candidate_reply(batch, (kind, depth), &steps))
        }
        Outcome::Result(other) => panic!("a batch answered {other:?}"),
        Outcome::Refused(verbatim_model::request::Refusal::ReadThroughUia) => {
            FocusOutcome::Result(FocusReply::Candidate(CandidateAnswer {
                facts: CandidateFacts::Skipped,
                read: None,
            }))
        }
        Outcome::Refused(refusal) => FocusOutcome::Refused(refusal),
        Outcome::NotThere => FocusOutcome::NotThere,
        Outcome::NotResponding => FocusOutcome::NotResponding,
        Outcome::Ended => FocusOutcome::Ended,
    }
}

/// The outcomes `steps` of a candidate's batch `batch`, to `depth`, as Core
/// judges them (`verbatim-core`'s `focus::msaa_run::judge`).
fn candidate_reply(
    batch: &verbatim_model::batch::Batch,
    (kind, depth): (CandidateKind, &Depth),
    steps: &[verbatim_model::batch::StepOutcome],
) -> FocusReply {
    use verbatim_model::batch::{Done, Op, StepOutcome, Subject};
    let gone = FocusReply::Candidate(CandidateAnswer {
        facts: CandidateFacts::NotThere,
        read: None,
    });
    let mut focused = None;
    let mut role = None;
    let mut read = None;
    for (step, outcome) in batch.steps.iter().zip(steps) {
        match (&step.op, outcome) {
            (
                Op::Test {
                    subject: Subject::Window { .. },
                    ..
                },
                StepOutcome::Done(Done::Test(tested)),
            ) => {
                if tested.matched {
                    return FocusReply::HiddenDialog(tested.node.window);
                }
                if value(&tested.node.values, PropertyId::WindowShown)
                    == Some(&PropertyValue::Flag(false))
                    && tested
                        .node
                        .values
                        .answer(PropertyId::Role)
                        .is_some_and(|answer| answer.value.is_none())
                {
                    return gone;
                }
            }
            (Op::Test { .. }, StepOutcome::Done(Done::Test(tested))) => {
                focused = focused_within(&tested.node.values);
                if tested.matched && !matches!(depth, Depth::Full) {
                    if matches!(depth, Depth::Facts) || focused == Some(false) {
                        focused = Some(false);
                        break;
                    }
                    return FocusReply::CandidateSame;
                }
            }
            (Op::Read { .. }, StepOutcome::Done(Done::Read(node))) => {
                role = match value(&node.values, PropertyId::Role) {
                    Some(PropertyValue::Role(role)) if kind == CandidateKind::MenuStart => {
                        Some(*role)
                    }
                    _ => None,
                };
            }
            (Op::FocusRead { .. }, StepOutcome::Done(Done::FocusRead(focus))) => {
                read = Some(focus.clone());
            }
            // A window the application does not serve is no hidden dialog.
            (_, StepOutcome::Skipped)
            | (
                Op::Test {
                    subject: Subject::Window { .. },
                    ..
                },
                StepOutcome::NotThere(_),
            ) => {}
            (_, StepOutcome::NotThere(_) | StepOutcome::Failed) => return gone,
            (op, other) => panic!("a candidate's step {op:?} answered {other:?}"),
        }
    }
    FocusReply::Candidate(CandidateAnswer {
        facts: CandidateFacts::Found { focused, role },
        read,
    })
}

/// The outcome `outcome` of a focus read's batch `batch`, a route's or
/// the current focus's, as Core judges it.
pub fn focus_outcome(batch: &verbatim_model::batch::Batch, outcome: Outcome) -> FocusOutcome {
    use verbatim_model::batch::{Done, StepOutcome};
    let steps = match outcome {
        Outcome::Result(Reply::Steps(steps)) => steps,
        Outcome::Result(other) => panic!("a batch answered {other:?}"),
        Outcome::Refused(refusal) => return FocusOutcome::Refused(refusal),
        Outcome::NotThere => return FocusOutcome::NotThere,
        Outcome::NotResponding => return FocusOutcome::NotResponding,
        Outcome::Ended => return FocusOutcome::Ended,
    };
    let current = batch.reads_focus_first();
    for (step, outcome) in batch.steps.iter().zip(steps) {
        // A window the application does not serve is no hidden dialog.
        if matches!(
            (&step.op, &outcome),
            (
                verbatim_model::batch::Op::Test {
                    subject: verbatim_model::batch::Subject::Window { .. },
                    ..
                },
                StepOutcome::NotThere(_)
            )
        ) {
            continue;
        }
        match outcome {
            StepOutcome::Done(Done::Test(tested)) if tested.matched => {
                return FocusOutcome::Result(FocusReply::HiddenDialog(tested.node.window));
            }
            StepOutcome::Done(Done::Test(tested)) => {
                if value(&tested.node.values, PropertyId::WindowShown)
                    == Some(&PropertyValue::Flag(false))
                    && tested
                        .node
                        .values
                        .answer(PropertyId::Role)
                        .is_some_and(|answer| answer.value.is_none())
                {
                    return FocusOutcome::NotThere;
                }
            }
            StepOutcome::Done(Done::FocusRead(read)) if current => {
                return FocusOutcome::Result(FocusReply::Candidate(CandidateAnswer {
                    facts: CandidateFacts::Found {
                        focused: None,
                        role: None,
                    },
                    read: Some(read),
                }));
            }
            StepOutcome::Done(Done::FocusRead(read)) => {
                return FocusOutcome::Result(FocusReply::Focus(read));
            }
            StepOutcome::NotThere(_) | StepOutcome::Failed => return FocusOutcome::NotThere,
            other => panic!("a focus read's step answered {other:?}"),
        }
    }
    panic!("a focus read's batch answered no read")
}

/// The route of the element the listener read as `object`, its ancestors
/// as `ancestry` read them, up to one of `known`, as Core makes it.
fn route_of(
    object: &NodeRead,
    ancestry: &verbatim_model::batch::Ancestry,
    known: &[KnownAncestor],
) -> verbatim_model::request::Route {
    use verbatim_model::batch::AncestorsEnd;
    use verbatim_model::request::{RouteEnd, RouteStep};
    let step = |read: &NodeRead| RouteStep {
        runtime_id: match value(&read.values, PropertyId::RuntimeId) {
            Some(PropertyValue::RuntimeId(runtime_id)) => runtime_id.clone(),
            other => panic!("the listener read a runtime id: {other:?}"),
        },
        window: match value(&read.values, PropertyId::NativeWindow) {
            Some(PropertyValue::Window(window)) => Some(*window),
            _ => None,
        },
        content: value(&read.values, PropertyId::IsContent) == Some(&PropertyValue::Flag(true)),
    };
    let end = match ancestry.end {
        AncestorsEnd::Met(place) => {
            RouteEnd::Known(known[usize::try_from(place).expect("a place")].clone())
        }
        AncestorsEnd::ProcessBoundary => RouteEnd::ProcessBoundary,
        AncestorsEnd::Top => RouteEnd::Root,
        AncestorsEnd::ContinueFrom(window) => RouteEnd::ContinueFrom(window),
        AncestorsEnd::PlainFrom(window) => RouteEnd::PlainFrom(window),
        AncestorsEnd::DepthLimit => panic!("the listener's walk goes on by plain windows"),
    };
    let reads = || std::iter::once(object).chain(&ancestry.ancestors);
    verbatim_model::request::Route {
        window: object.window.handle,
        steps: reads().map(step).collect(),
        end,
        read: reads().map(|read| read.values.clone()).collect(),
    }
}

/// The listener's read of a kept element and its ancestors, as the
/// stand-in for Core holds it.
#[derive(Clone, Debug)]
pub struct ListenerObject {
    /// The element.
    pub object: NodeRead,
    /// Its ancestors, nearest first.
    pub ancestors: Vec<NodeRead>,
    /// Its route.
    pub route: Option<verbatim_model::request::Route>,
}
