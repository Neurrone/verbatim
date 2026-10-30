//! A real outpost in the test process, watching mockapp, as Core would
//! drive it: the tests hand it facts and queries over its command entry
//! point and read every message it writes to its pipe.
//!
//! The test stands in for the focus listener for UIA focus events: it
//! judges and reads each focus from the element mockapp reports focused
//! with the listener's own reader ([`FocusReader`], on the test's thread,
//! where its calls are counted apart from the outpost's), and hands the
//! outpost the read, as the listener sends it
//! (`docs/design/focus-pipeline.md`, section 12).
//!
//! A focus-now query reads the system's keyboard focus, which a test must
//! not take from the desktop it runs on, so this outpost reads it from the
//! test instead ([`Outpost::with_focused_element_reader`]): the test sets
//! the element mockapp reports focused, and the outpost's read of it
//! counts as the one call the system read is. The window with the keyboard
//! focus is the one the test reads as the foreground
//! ([`Outpost::set_focus_window_reader`]).
//!
//! The outpost and the listener's reader read the time from the test too
//! ([`Outpost::set_clock`], [`FocusReader::set_clock`]): the time stands
//! still unless the test moves it on ([`OutpostUnderTest::pass_time`], or
//! [`OutpostUnderTest::run_out_of_time_after`] partway through a step), so
//! a classic walk's or a focus read's time budget is spent only where the
//! test says, however long the test and its calls take. A test whose
//! budget a real wait must spend, as a stalled window's raw probe spends
//! it, makes the time the system's instead
//! ([`OutpostUnderTest::follow_the_system_clock`]). The outpost stamps its
//! own events' observations, and times its verdicts' keeps, by the test's
//! observation clock ([`Outpost::set_observation_clock`]), which a test
//! chooses once, as it makes the outpost ([`ObservationTime`]): the run's
//! shared clock moved on by every time the test has passed
//! ([`OutpostUnderTest::new`]), or that clock frozen when the outpost was
//! made, moved on by the time passed and by a microsecond each time it is
//! read ([`OutpostUnderTest::on_frozen_time`]). On the frozen clock a
//! window's verdict of no UIA provider runs out, and the gap record's
//! bound is reached, only once the test passes the time, however long its
//! steps take, so every test whose assertions depend on a verdict's keep,
//! the gap's bound or a restamp uses it. The others keep the real one,
//! because the outpost compares the stamps of caret, text and terminal
//! reads, read from the run's clock itself, with events' observations;
//! the facts the test hands over are stamped on the chosen clock
//! ([`OutpostUnderTest::observed_now`]).
//!
//! No window takes the foreground on the desktop a test runs on, so the
//! outpost and the listener's reader read the foreground window from the
//! test too ([`Outpost::set_foreground_reader`],
//! [`FocusReader::set_foreground_reader`]): mockapp's own window unless the
//! test says otherwise ([`OutpostUnderTest::set_foreground`]), as when a
//! user works in mockapp, so its events pass the acceptance test against
//! the foreground window, which the listener makes for a UIA focus and the
//! outpost for every other event.
//!
//! The outpost's own hook takes in mockapp's MSAA focus, foreground, menu,
//! alert, and tooltip events, as every outpost's does
//! (`docs/design/focus-pipeline.md`, section 12.4). A fact the test hands
//! over carries no `WinEvent` stamp, so the outpost never takes it for the
//! twin of an event its hook took in: a test hands over an MSAA fact for an
//! event mockapp never raised, or raised before the outpost was made. Only
//! [`OutpostUnderTest::deliver_win_event`] hands over a stamped one, the
//! listener's copy of an event mockapp raised.
//!
//! The outpost keeps its own gap record, and the test times its bound
//! ([`Outpost::set_gap_timer`]): once the test has passed the time left to
//! the bound when the outpost set it, rounded up to a tenth of a second so
//! that the observation clock's ticks between a copy's stamp and the
//! timer's setting never reach it, the test reaches it
//! ([`Outpost::gap_bound_reacher`]), as the outpost's timer does when it
//! fires, and a bound already past is reached at once from a thread of its
//! own, as the threadpool's callback reaches it, since the outpost sets the
//! timer under its intake's lock; no test waits for it. A drop the test hands over carries its
//! answer to the record ([`OutpostUnderTest::listener_outcome`] places it
//! as the listener does). An idle round the test starts
//! ([`OutpostUnderTest::idle_round`]) reaches the outpost's idle answer,
//! which waits while its record is open.
//!
//! The test stands in for Core's holding of the outpost's nodes too: as
//! each focus the outpost reports, or each focus-now answer with a control,
//! is read from its pipe, Core takes it, but for a foreground change of the
//! window its focus is already in, which Core refuses, and holds its focus
//! line, counting the position of every message that carries nodes. The
//! outpost reads changes for every node it has not released, so nothing is
//! released until a test delivers Core's list of the nodes it holds, as
//! verbatim.exe writes it after a focus report
//! ([`OutpostUnderTest::release`], [`OutpostUnderTest::release_all`]).
//!
//! The test stands in for the listener's read scheduler as well: each read
//! is the next event queued, and a test says when the user moved away from
//! mockapp's application since the last read, for a menu opening's
//! currentness ([`OutpostUnderTest::listener_moved_away`]).

use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use verbatim_model::protocol::{
    CoreToOutpost, DeliveredFact, EventTiming, FocusNow, FocusedControl, OutpostToCore, Query,
    QueryOutcome, QueryResult, WinEventStamp,
};
use verbatim_model::{
    CallCounts, FocusText, NodeId, NodeSnapshot, NormalizedEvent, Pid, QueryKind, TraceId,
    WindowFacts, WindowHandle,
};
use verbatim_outpost::focus_read::{Arrival, FocusEvent, FocusReader, Judged};
use verbatim_outpost::protocol::read_message;
use verbatim_outpost::{GapTimer, Heard, Outpost, OutpostOptions};
use verbatim_process::overlapped::{End, Flow, pipe};
use verbatim_process::transport::BlockingReader;
use verbatim_uia::{Cached, ListenerEvent, Live};
use windows::Win32::Foundation::{E_FAIL, HWND};
use windows::Win32::UI::Accessibility::IUIAutomationElement;
use windows::Win32::UI::WindowsAndMessaging::{GA_ROOT, GetAncestor};
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

/// Where the outpost's read of the foreground window waits while a test
/// holds its worker ([`OutpostUnderTest::hold_worker`]),
/// so that events the test raises meanwhile wait in the outpost's intake and
/// are planned as one batch once it lets the worker go on.
#[derive(Default)]
struct WorkerGate {
    state: Mutex<GateState>,
    changed: std::sync::Condvar,
}

#[derive(Default)]
struct GateState {
    /// Whether the next read waits.
    closed: bool,
    /// Whether a read is waiting.
    waiting: bool,
    /// How many times a read has come to wait at the gate.
    arrivals: u64,
    /// How many waiting reads may go on while the gate stays closed
    /// ([`OutpostUnderTest::step_worker`]).
    passes: u64,
}

impl WorkerGate {
    fn lock(&self) -> std::sync::MutexGuard<'_, GateState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// On the worker: waits while the gate is closed, saying so, unless
    /// the test lets it pass once.
    fn pass(&self) {
        let mut state = self.lock();
        if !state.closed {
            return;
        }
        state.waiting = true;
        state.arrivals += 1;
        self.changed.notify_all();
        let (mut state, timeout) = self
            .changed
            .wait_timeout_while(state, super::WAIT_TIMEOUT, |state| {
                state.closed && state.passes == 0
            })
            .unwrap_or_else(PoisonError::into_inner);
        if state.closed {
            state.passes = state.passes.saturating_sub(1);
        }
        state.waiting = false;
        self.changed.notify_all();
        assert!(!timeout.timed_out(), "the test never let the worker go on");
    }

    /// On the test: lets the read waiting at the closed gate and the
    /// `reads - 1` after it go on, and waits for the worker to wait at the
    /// gate with the read after those.
    fn step(&self, reads: u64) {
        let mut state = self.lock();
        assert!(state.waiting, "the worker is not waiting at the gate");
        let arrived = state.arrivals + reads;
        state.passes += reads;
        self.changed.notify_all();
        let (state, timeout) = self
            .changed
            .wait_timeout_while(state, super::WAIT_TIMEOUT, |state| {
                state.arrivals < arrived || !state.waiting
            })
            .unwrap_or_else(PoisonError::into_inner);
        drop(state);
        assert!(
            !timeout.timed_out(),
            "the worker never came back to the gate"
        );
    }

    /// On the test: waits for the worker to wait at the gate.
    fn wait_for_worker(&self) {
        let state = self.lock();
        let (state, timeout) = self
            .changed
            .wait_timeout_while(state, super::WAIT_TIMEOUT, |state| !state.waiting)
            .unwrap_or_else(PoisonError::into_inner);
        drop(state);
        assert!(!timeout.timed_out(), "the worker never reached the gate");
    }

    fn set_closed(&self, closed: bool) {
        self.lock().closed = closed;
        self.changed.notify_all();
    }
}

/// A focus the outpost reported, with the calls it made reading it.
pub struct Reported {
    pub node: NodeSnapshot,
    pub ancestors: Vec<NodeSnapshot>,
    pub selected_child: Option<NodeSnapshot>,
    /// What the outpost's caret read found, for a focus that may have text.
    pub text: Option<FocusText>,
    /// What the listener read of the element it held before under a UIA
    /// focus's runtime id.
    pub earlier: Option<verbatim_model::EarlierElement>,
    /// The top-level window of the event's window.
    pub window: Option<WindowHandle>,
    /// The event's window facts, as Core takes them.
    pub facts: Option<WindowFacts>,
    pub calls: CallCounts,
    /// The failed cached reads the outpost made for it, and the listener's
    /// reader on this thread for a UIA focus.
    pub failed_reads: u32,
}

impl Reported {
    /// The names of the focus's chain, outermost first, ending with the
    /// focus.
    pub fn chain(&self) -> Vec<Option<&str>> {
        self.ancestors
            .iter()
            .chain(std::iter::once(&self.node))
            .map(|node| node.name.as_deref())
            .collect()
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

/// The gap record's timer as the test times it: the time the test has to
/// pass, from how much it had passed, for the bound the outpost last set
/// to be reached, if it is set ([`OutpostUnderTest::pass_time`]). A bound
/// already past is reached at once, from a thread of its own.
struct TestGapTimer {
    /// The outpost's observation clock.
    clock: Arc<TestClock>,
    /// The passed time, in microseconds, at which the bound is reached.
    reached_at_passed_us: Mutex<Option<u64>>,
    /// What reaches the bound.
    reach: std::sync::OnceLock<Arc<dyn Fn() + Send + Sync>>,
}

/// The step the time left to a bound is rounded up to: more than the
/// frozen clock's ticks, a microsecond a read, between a copy's
/// observation and its handling in any test.
const BOUND_STEP_US: u64 = 100_000;

impl GapTimer for TestGapTimer {
    fn set(&self, due_us: u64) {
        let passed = self.clock.passed();
        let left = due_us.saturating_sub(self.clock.now());
        if left == 0 {
            *self
                .reached_at_passed_us
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = None;
            if let Some(reach) = self.reach.get() {
                let reach = Arc::clone(reach);
                std::thread::spawn(move || reach());
            }
            return;
        }
        let left = left.div_ceil(BOUND_STEP_US) * BOUND_STEP_US;
        *self
            .reached_at_passed_us
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(passed + left);
    }

    fn cancel(&self) {
        *self
            .reached_at_passed_us
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
    }
}

/// Has `outpost` stamp its own events' observations, and time its
/// verdicts' keeps, by the test's observation clock, `time`, and time its
/// gap record's bound by the test's time: the clock, which the test moves
/// on, and the timer.
fn observe_on_the_tests_time(
    outpost: &Outpost,
    time: ObservationTime,
) -> (Arc<TestClock>, Arc<TestGapTimer>) {
    let clock = Arc::new(TestClock {
        time,
        start_us: now_us(),
        moved_us: AtomicU64::new(0),
        passed_us: AtomicU64::new(0),
    });
    let observed = Arc::clone(&clock);
    outpost.set_observation_clock(Arc::new(move || observed.now()));
    let gap_timer = Arc::new(TestGapTimer {
        clock: Arc::clone(&clock),
        reached_at_passed_us: Mutex::new(None),
        reach: std::sync::OnceLock::new(),
    });
    let _ = gap_timer.reach.set(outpost.gap_bound_reacher());
    outpost.set_gap_timer(Arc::clone(&gap_timer) as Arc<dyn GapTimer>);
    (clock, gap_timer)
}

/// The focus line the stand-in for Core holds in the outpost: the trace of
/// the report it took it from, its nodes, outermost first, ending with the
/// focus, and its top-level window.
struct CoreHolds {
    trace: TraceId,
    line: Vec<NodeId>,
    top_level: Option<WindowHandle>,
}

/// A real outpost in this process, watching mockapp, with its pipe to Core.
pub struct OutpostUnderTest {
    pub outpost: Outpost,
    pub messages: MessagePipe,
    /// Every event the outpost's own handlers took in from mockapp, in the
    /// order they were queued ([`Outpost::observe_heard`]).
    heard: Receiver<Heard>,
    next_request: u64,
    pub focus: FocusSlot,
    /// Where the outpost's read of the foreground window waits while the
    /// test holds its worker.
    gate: Arc<WorkerGate>,
    /// The time the outpost and the listener's reader read.
    time: Arc<Mutex<Instant>>,
    /// How many more reads of the time give it unchanged before it jumps an
    /// hour on ([`Self::run_out_of_time_after`]), when set.
    jump_after: Arc<Mutex<Option<u32>>>,
    /// Whether the time read is the system's
    /// ([`Self::follow_the_system_clock`]).
    system_time: Arc<AtomicBool>,
    /// The window the outpost reads as the foreground.
    foreground: Arc<AtomicIsize>,
    /// The focus listener's reader, standing in for the listener's thread
    /// for mockapp's application.
    reader: RefCell<FocusReader>,
    /// The outpost's failed cached reads when last taken
    /// ([`Self::take_failed_reads`]).
    failed_reads_taken: Cell<u32>,
    /// How many events the stand-in for the listener's scheduler has
    /// queued: each read is the next.
    listener_queued: Cell<u64>,
    /// The place of the newest event that moved away from mockapp's
    /// application, as the listener's scheduler records it.
    listener_left_at: Cell<u64>,
    /// The focus line the stand-in for Core holds ([`Self::core_judges`]).
    core: RefCell<Option<CoreHolds>>,
    /// The position of the last message read that carries nodes, as Core
    /// counts them, which Core's list of held nodes acknowledges.
    position: Cell<u64>,
    /// The trace of the UIA focus event the listener's reader last read,
    /// which its read is delivered under, as the listener sends it.
    last_read: Cell<Option<TraceId>>,
    /// The outpost's observation clock, which the test moves on.
    observation: Arc<TestClock>,
    /// The gap record's timer.
    gap_timer: Arc<TestGapTimer>,
    /// The window the keyboard focus moves to during the outpost's next
    /// read of the focused element, 0 for none.
    moves_to: Arc<AtomicIsize>,
}

impl OutpostUnderTest {
    /// An outpost watching `app` with the default options, on the real
    /// observation clock ([`ObservationTime::Real`]).
    pub fn new(app: &super::MockApp) -> Self {
        Self::with_options(app, OutpostOptions::default())
    }

    /// An outpost watching `app` as `options` say, on the real observation
    /// clock, which has announced itself ready, and nothing else. It reads
    /// `app`'s window as the foreground window.
    pub fn with_options(app: &super::MockApp, options: OutpostOptions) -> Self {
        Self::with_pipe_buffer(app, options, MESSAGES_PIPE_BUFFER)
    }

    /// [`Self::new`] on the frozen observation clock
    /// ([`ObservationTime::Frozen`]), for a test whose assertions depend on
    /// a verdict's keep, the gap record's bound or a restamp.
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

    /// A spare outpost (`docs/design/focus-pipeline.md`, section 19), with
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
        let time = Arc::new(Mutex::new(Instant::now()));
        let jump_after = Arc::new(Mutex::new(None));
        let system_time = Arc::new(AtomicBool::new(false));
        let (clock, jump, system) = (
            Arc::clone(&time),
            Arc::clone(&jump_after),
            Arc::clone(&system_time),
        );
        let clock: verbatim_outpost::arbitration::Clock = Arc::new(move || {
            if system.load(Ordering::Relaxed) {
                return Instant::now();
            }
            let mut jump = jump.lock().unwrap_or_else(PoisonError::into_inner);
            let mut time = clock.lock().unwrap_or_else(PoisonError::into_inner);
            match *jump {
                Some(0) => {
                    *time += Duration::from_secs(3600);
                    *jump = None;
                }
                Some(reads) => *jump = Some(reads - 1),
                None => {}
            }
            *time
        });
        outpost.set_clock(Arc::clone(&clock));
        let (observation, gap_timer) = observe_on_the_tests_time(&outpost, observation_time);
        let reader = Arc::clone(&foreground);
        let gate = Arc::new(WorkerGate::default());
        let worker_gate = Arc::clone(&gate);
        outpost.set_foreground_reader(Arc::new(move || {
            worker_gate.pass();
            reader.load(Ordering::Relaxed)
        }));
        // And the window with the keyboard focus is that window too.
        let focus_window = Arc::clone(&foreground);
        outpost
            .set_focus_window_reader(Arc::new(move || Some(focus_window.load(Ordering::Relaxed))));
        let (heard_tx, heard) = mpsc::channel();
        outpost.observe_heard(Arc::new(move |event| {
            // The test may have finished with what the outpost hears.
            let _ = heard_tx.send(event);
        }));
        let mut reader = FocusReader::new().expect("a UIA client for the listener's reads");
        // The listener's acceptance test reads the same foreground window.
        let listener_foreground = Arc::clone(&foreground);
        reader.set_foreground_reader(Arc::new(move || {
            listener_foreground.load(Ordering::Relaxed)
        }));
        // And the same time, and the same observation clock, by which how
        // long an event waited before its read is measured.
        reader.set_clock(clock);
        let observed = Arc::clone(&observation);
        reader.set_observation_clock(Arc::new(move || observed.now()));
        Self {
            outpost,
            messages,
            heard,
            next_request: 1,
            focus,
            gate,
            time,
            jump_after,
            system_time,
            foreground,
            reader: RefCell::new(reader),
            listener_queued: Cell::new(0),
            listener_left_at: Cell::new(0),
            failed_reads_taken: Cell::new(0),
            core: RefCell::new(None),
            position: Cell::new(0),
            last_read: Cell::new(None),
            observation,
            gap_timer,
            moves_to,
        }
    }

    /// Moves the keyboard focus, as the outpost reads it, to `hwnd` while
    /// the outpost's next read of the focused element is made, between its
    /// check of the focus window and its return, as a focus that moves on
    /// during `GetFocusedElementBuildCache` does.
    pub fn focus_moves_during_the_read(&self, hwnd: HWND) {
        self.moves_to.store(hwnd.0 as isize, Ordering::Relaxed);
    }

    /// Tells the listener's reader whether a backend override forcing UIA
    /// holds for mockapp's application, as the listener is told
    /// (`CoreToOutpost::SetBackendOverride`).
    pub fn listener_forces_uia(&self, forced: bool) {
        self.reader.borrow().forces_uia(forced);
    }

    /// Tells `observer` the name of each call the listener's reader's
    /// classic walk is about to make, before it makes it, so a test can
    /// change mockapp's windows between two of them.
    pub fn listener_observes_calls(&self, observer: verbatim_outpost::focus_read::CallObserver) {
        self.reader.borrow_mut().observe_calls(observer);
    }

    /// Has the outpost, and the listener's reader in the walk it reads a
    /// focus's ancestors with, judge window `hung` as a hung application's,
    /// as Windows judges a window whose thread has not answered for five
    /// seconds, and every other window by the system's judgement.
    pub fn judge_hung(&self, hung: HWND) {
        let hung = hung.0 as isize;
        let reader: verbatim_outpost::WindowReadabilityReader = Arc::new(move |hwnd| {
            if hwnd == hung {
                Some("its application is not responding")
            } else {
                verbatim_outpost::window_unreadable(hwnd)
            }
        });
        self.outpost
            .set_window_readability_reader(Arc::clone(&reader));
        self.reader
            .borrow_mut()
            .set_window_readability_reader(reader);
    }

    /// Now on the outpost's observation clock ([`TestClock`]), which a fact
    /// the test hands over is stamped by.
    pub fn observed_now(&self) -> u64 {
        self.observation.now()
    }

    /// The time the outpost and the listener's reader read now.
    fn now(&self) -> Instant {
        if self.system_time.load(Ordering::Relaxed) {
            Instant::now()
        } else {
            *self.time.lock().unwrap_or_else(PoisonError::into_inner)
        }
    }

    /// Starts the idle barrier's round `round`, as verbatim.exe does: the
    /// listener has nothing waiting, and the outpost is asked.
    pub fn idle_round(&self, round: u64) {
        self.outpost
            .handle_command(&CoreToOutpost::IdleQuery { round });
    }

    /// Hands the outpost what the listener sends for the UIA focus event
    /// `marker` names when its outcome is `judged`, observed at
    /// `observed_at_us`, a drop placed, as the listener places it, in the
    /// top-level window of `window`, the sender's own window or its nearest
    /// window its read found (`None` for neither).
    pub fn listener_outcome(
        &self,
        marker: u64,
        judged: Judged,
        (observed_at_us, window): (u64, Option<isize>),
    ) {
        self.hand_over(
            judged.into_fact(marker, FocusEvent::Focus, top_level(window)),
            observed_at_us,
        );
    }

    /// Hands the outpost `fact`, observed at `observed_at_us`, as the
    /// supervisor relays the listener's fact.
    fn hand_over(&self, fact: DeliveredFact, observed_at_us: u64) {
        let command = CoreToOutpost::DeliverFact {
            trace_id: self.trace_for(&fact),
            timing: EventTiming {
                observed_at_us,
                ..EventTiming::default()
            },
            fact,
            win_event: None,
        };
        self.outpost.handle_command(&command);
    }

    /// Makes `hwnd` the window the outpost reads as the foreground from now
    /// on.
    pub fn set_foreground(&self, hwnd: HWND) {
        self.foreground.store(hwnd.0 as isize, Ordering::Relaxed);
    }

    /// Moves the time the outpost and the listener's reader read on by `by`.
    ///
    /// # Panics
    ///
    /// Panics once the time is the system's
    /// ([`follow_the_system_clock`](Self::follow_the_system_clock)), which
    /// this cannot move.
    pub fn pass_time(&self, by: Duration) {
        self.assert_the_time_is_the_tests();
        *self.time.lock().unwrap_or_else(PoisonError::into_inner) += by;
        self.observation
            .pass(u64::try_from(by.as_micros()).unwrap_or(u64::MAX));
        let passed = self.observation.passed();
        let mut reached = self
            .gap_timer
            .reached_at_passed_us
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if reached.is_some_and(|at| passed >= at) {
            *reached = None;
            drop(reached);
            if let Some(reach) = self.gap_timer.reach.get() {
                reach();
            }
        }
    }

    /// Whether the outpost has the gap record's timer set, for a bound not
    /// yet reached.
    pub fn gap_timer_set(&self) -> bool {
        self.gap_timer
            .reached_at_passed_us
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    /// Fails the test once the time is the system's, which the test can no
    /// longer move.
    fn assert_the_time_is_the_tests(&self) {
        assert!(
            !self.system_time.load(Ordering::Relaxed),
            "the time follows the system's clock, which a test cannot move"
        );
    }

    /// Lets the time the outpost and the listener's reader read be read
    /// `reads` more times unchanged, and then jump an hour on, past every
    /// time budget, for a step whose budget runs out at a chosen call: a
    /// focus read reads the time first to set its budget's end.
    ///
    /// # Panics
    ///
    /// Panics once the time is the system's
    /// ([`follow_the_system_clock`](Self::follow_the_system_clock)), which
    /// cannot be made to jump.
    pub fn run_out_of_time_after(&self, reads: u32) {
        self.assert_the_time_is_the_tests();
        *self
            .jump_after
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(reads);
    }

    /// Makes the time the outpost and the listener's reader read the
    /// system's from now on, for a step whose time budget a real wait must
    /// spend, as a stalled window's raw probe spends it. The test can no
    /// longer move the time ([`pass_time`](Self::pass_time),
    /// [`run_out_of_time_after`](Self::run_out_of_time_after)).
    pub fn follow_the_system_clock(&self) {
        self.system_time.store(true, Ordering::Relaxed);
    }

    /// The outpost's next message, a focus in it judged as Core judges it
    /// ([`Self::core_judges`]).
    pub fn next(&self) -> OutpostToCore {
        let message = self
            .messages
            .recv_timeout(super::WAIT_TIMEOUT)
            .expect("the outpost says something before the wait times out");
        if message.carries_nodes() {
            self.position.set(self.position.get() + 1);
        }
        self.core_judges(&message);
        message
    }

    /// Stands in for Core judging `message`: a focus the outpost reported,
    /// or a focus-now answer's control, becomes the focus Core holds, with
    /// the line it was reported with, but for a foreground change of the
    /// window its focus is already in, which Core refuses.
    fn core_judges(&self, message: &OutpostToCore) {
        let (trace, node, foreground, ancestors, top_level) = match message {
            OutpostToCore::Event {
                trace_id,
                window,
                event:
                    NormalizedEvent::FocusChanged {
                        node,
                        foreground,
                        ancestors,
                        ..
                    },
                ..
            } => (
                *trace_id,
                node,
                *foreground,
                ancestors,
                window.as_ref().map(|window| window.top_level),
            ),
            OutpostToCore::Reply {
                trace_id,
                outcome: QueryOutcome::Done(QueryResult::Focus(now)),
                ..
            } => {
                let Some(control) = &now.focus else {
                    return;
                };
                (
                    *trace_id,
                    &control.node,
                    false,
                    &control.ancestors,
                    control
                        .window
                        .as_ref()
                        .or(now.window.as_ref().map(|(_, facts)| facts))
                        .map(|window| window.top_level),
                )
            }
            _ => return,
        };
        let mut core = self.core.borrow_mut();
        let refused = foreground
            && core
                .as_ref()
                .is_some_and(|focus| focus.top_level.is_some() && focus.top_level == top_level);
        if !refused {
            *core = Some(CoreHolds {
                trace,
                line: ancestors
                    .iter()
                    .map(|ancestor| ancestor.id)
                    .chain(std::iter::once(node.id))
                    .collect(),
                top_level,
            });
        }
    }

    /// Delivers Core's list of the nodes it holds in the outpost, `held`,
    /// acknowledging every message read so far, as verbatim.exe writes it
    /// after a focus report: the outpost releases every node it carried at
    /// or before that position that is not among them, and so stops reading
    /// changes for it.
    pub fn release(&self, held: &[NodeId]) {
        self.outpost.handle_command(&CoreToOutpost::NodesHeld {
            nodes: held.iter().map(|id| id.number()).collect(),
            anchors: Vec::new(),
            acknowledged: self.position.get(),
        });
    }

    /// [`release`](Self::release) with the focus line Core holds: what
    /// Core writes after it judged the outpost's focus reports read so far.
    pub fn release_to_core_line(&self) {
        let line = self
            .core
            .borrow()
            .as_ref()
            .map(|core| core.line.clone())
            .unwrap_or_default();
        self.release(&line);
    }

    /// [`release`](Self::release) of every node: Core's focus moved to
    /// another application, and it holds none of the outpost's.
    pub fn release_all(&self) {
        *self.core.borrow_mut() = None;
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

    /// Hands the outpost `fact`, as the supervisor relays the listener's
    /// fact for a UIA event its callback received at `observed_at_us` on
    /// the run's shared clock. Returns at once.
    pub fn deliver_stamped(&self, fact: DeliveredFact, observed_at_us: u64) {
        self.hand_over(fact, observed_at_us);
    }

    /// The trace `fact` is delivered under: a UIA focus read's is the
    /// trace of the event the listener's reader last read, as the listener
    /// sends it; any other fact's a new one.
    fn trace_for(&self, fact: &DeliveredFact) -> TraceId {
        match fact {
            DeliveredFact::UiaFocusRead { .. } => {
                self.last_read.take().unwrap_or_else(TraceId::mint)
            }
            _ => TraceId::mint(),
        }
    }

    /// Hands the outpost `fact`, as the listener would, observed now on the
    /// outpost's observation clock, and returns at once.
    pub fn deliver(&self, fact: DeliveredFact) {
        self.hand_over(fact, self.observed_now());
    }

    /// Hands the outpost the fact the listener forwards for an MSAA
    /// `WinEvent`, with the thread that raised the event and when, which
    /// the outpost matches with its own hook's copy, and returns at once.
    pub fn deliver_win_event(&self, fact: DeliveredFact, stamp: WinEventStamp) {
        self.deliver_win_event_observed(fact, stamp, self.observed_now());
    }

    /// [`deliver_win_event`](Self::deliver_win_event), the event observed
    /// at `observed_at_us`, in microseconds on the outpost's observation
    /// clock ([`observed_now`](Self::observed_now)).
    pub fn deliver_win_event_observed(
        &self,
        fact: DeliveredFact,
        stamp: WinEventStamp,
        observed_at_us: u64,
    ) {
        let command = CoreToOutpost::DeliverFact {
            trace_id: TraceId::mint(),
            timing: EventTiming {
                observed_at_us,
                ..EventTiming::default()
            },
            fact,
            win_event: Some(stamp),
        };
        self.outpost.handle_command(&command);
    }

    /// Tells the outpost, as the supervisor relays it, that the focus
    /// listener forwards none of its application's `WinEvent`s raised after
    /// `through`, an event clock time: the MSAA hand-over's end.
    pub fn forwarding_ended(&self, through: u32) {
        self.outpost
            .handle_command(&CoreToOutpost::ForwardingEnded { through });
    }

    /// Hands the outpost `fact`, as the listener would, stamped on the
    /// run's shared clock with the time it is handed over as the time it
    /// was observed, and returns at once.
    pub fn deliver_observed_now(&self, fact: DeliveredFact) {
        self.hand_over(fact, self.observed_now());
    }

    /// Asks the outpost `query`, as Core would, and returns its request id
    /// at once.
    pub fn ask(&mut self, query: Query) -> u64 {
        let request_id = self.next_request;
        self.next_request += 1;
        self.outpost.handle_command(&CoreToOutpost::Query {
            trace_id: TraceId::mint(),
            request_id,
            query,
            timing: EventTiming::default(),
        });
        request_id
    }

    /// Judges and reads a UIA focus event whose element is `element`, as
    /// the focus listener's thread for mockapp does, on this thread, and
    /// returns what became of it and the calls the read made. `element`
    /// is filled as the listener's registration delivers it.
    pub fn listener_read(&self, element: &Cached<ListenerEvent>) -> (Judged, CallCounts) {
        self.listener_read_as(element, FocusEvent::Focus)
    }

    /// [`Self::listener_read`] for an event of kind `kind`: a UIA menu
    /// opening is judged and read as a focus on the menu.
    pub fn listener_read_as(
        &self,
        element: &Cached<ListenerEvent>,
        kind: FocusEvent,
    ) -> (Judged, CallCounts) {
        let _ = verbatim_uia::calls::take();
        let _ = verbatim_uia::calls::take_failed_reads();
        let arrival = self.listener_arrival();
        self.last_read.set(Some(arrival.trace));
        let judged = self.reader.borrow_mut().read(element, kind, arrival);
        (judged, verbatim_uia::calls::take())
    }

    /// [`Self::listener_read`] for a UIA focus event observed `waited`
    /// before its read begins, as one that waits behind its application's
    /// earlier reads is: the test's time passes between its observation and
    /// its read, so its walk's bound, measured from its observation, comes
    /// that much sooner.
    pub fn listener_read_after(&self, element: &Cached<ListenerEvent>, waited: Duration) -> Judged {
        let arrival = self.listener_arrival();
        self.last_read.set(Some(arrival.trace));
        self.pass_time(waited);
        self.reader
            .borrow_mut()
            .read(element, FocusEvent::Focus, arrival)
    }

    /// The next event the stand-in for the listener's scheduler queues,
    /// observed now: its place, and where mockapp's application was last
    /// moved away from.
    fn listener_arrival(&self) -> Arrival {
        let place = self.listener_queued.get() + 1;
        self.listener_queued.set(place);
        Arrival {
            trace: TraceId::mint(),
            observed_at_us: self.observed_now(),
            place,
            left_at: self.listener_left_at.get(),
        }
    }

    /// Has the user moved away from mockapp's application since the
    /// listener's last read, as another application's focus queued does:
    /// that read is no longer current for a later menu opening
    /// (`QueuePlace::current_since` in `verbatim_outpost`'s read schedule).
    pub fn listener_moved_away(&self) {
        self.listener_left_at.set(self.listener_queued.get());
    }

    /// The fact the focus listener sends for a UIA focus event on `element`
    /// once it has read it, which must not be dropped.
    pub fn listener_fact(&self, element: &Cached<ListenerEvent>) -> DeliveredFact {
        match self.listener_read(element).0 {
            read @ Judged::Read(_) => read.into_fact(0, FocusEvent::Focus, None),
            dropped @ Judged::Dropped(_) => {
                panic!("the listener dropped the focus: {dropped:?}")
            }
            Judged::Passed(fact) => panic!("a focus read passed a fact on: {fact:?}"),
        }
    }

    /// What the focus listener's reading thread makes of `fact`, a
    /// selection or a notification captured from `element`, as its
    /// application's thread checks it (`FocusReader::check_fact`).
    pub fn listener_check(&self, element: &Cached<ListenerEvent>, fact: DeliveredFact) -> Judged {
        self.reader.borrow_mut().check_fact(element, fact)
    }

    /// Holds the outpost's idle worker: hands it a foreground change of
    /// mockapp's window, whose acceptance test's read of the foreground
    /// window waits until [`Self::release_worker`], and returns once the
    /// worker is waiting there. Events the test then has the application
    /// raise wait in the outpost's intake, and are planned as one batch
    /// when the worker goes on. The change itself says nothing: UIA owns
    /// mockapp's window in its UIA backend, the only one this is for. The
    /// worker waits under the change's deadline, so a step that holds it
    /// must be brief.
    pub fn hold_worker(&self) {
        self.hold_worker_on(self.foreground.load(Ordering::Relaxed));
    }

    /// [`Self::hold_worker`] on a foreground change of window `hwnd`, for
    /// any backend: the worker waits in that change's first read of the
    /// foreground window. A change of a window that is not the foreground
    /// window then says nothing.
    pub fn hold_worker_on(&self, hwnd: isize) {
        self.gate.set_closed(true);
        self.deliver(DeliveredFact::Foreground { hwnd });
        self.gate.wait_for_worker();
    }

    /// Lets the worker [`Self::hold_worker`] held go on through `reads`
    /// reads of the foreground window, the one it waits in included, and
    /// returns once it waits in the read after them. A foreground change
    /// of mockapp's window reads it twice, its acceptance test and its
    /// check that the window is in front, and reads nothing more, since
    /// UIA owns the window: stepping two reads from one held foreground
    /// change to the next has the worker handle everything handed to it
    /// between the two and hold it again.
    pub fn step_worker(&self, reads: u64) {
        self.gate.step(reads);
    }

    /// Lets the worker [`Self::hold_worker`] held go on.
    pub fn release_worker(&self) {
        self.gate.set_closed(false);
    }

    /// Makes the outpost's read of the focused element for a focus-now
    /// query answer `element`.
    pub fn read_focus_as(&self, element: &IUIAutomationElement) {
        *self.focus.lock().unwrap_or_else(PoisonError::into_inner) =
            Some(AgileReference::new(element).expect("an agile reference"));
    }

    /// Hands the outpost `fact` and returns the focus it reports, which
    /// must be the next thing it says, and the only thing.
    pub fn focus(&self, fact: DeliveredFact) -> Reported {
        self.deliver(fact);
        self.reported_focus()
    }

    /// The focus the outpost reports next, which must be the next thing it
    /// says, and the only thing but a terminal's first read: one handed
    /// over, or one its own hook took in from a focus event mockapp's
    /// control raised.
    pub fn reported_focus(&self) -> Reported {
        let reported = match self.next() {
            OutpostToCore::Event {
                event:
                    NormalizedEvent::FocusChanged {
                        node,
                        foreground: false,
                        ancestors,
                        selected_child,
                        text,
                        earlier,
                        ..
                    },
                window,
                timing,
                ..
            } => Reported {
                node,
                ancestors,
                selected_child,
                text: text.map(|text| *text),
                earlier,
                window: window.as_ref().map(|window| window.top_level),
                facts: window,
                calls: timing.calls,
                failed_reads: 0,
            },
            other => panic!("the outpost said {other:?}, not a focus with its ancestors"),
        };
        // A terminal is read once it has the focus, and the read sent.
        if reported.node.role == verbatim_model::Role::Terminal {
            self.first_terminal_read(reported.node.id);
        }
        self.settled();
        Reported {
            failed_reads: self.take_failed_reads(),
            ..reported
        }
    }

    /// Takes the read of terminal `node` the outpost sends once the
    /// terminal has the focus, which must be the next thing it says.
    pub fn first_terminal_read(&self, node: NodeId) {
        match self.next() {
            OutpostToCore::Event {
                event: NormalizedEvent::TerminalSnapshot { node_id, .. },
                ..
            } if node_id == node => {}
            other => panic!("the outpost said {other:?}, not the terminal's first read"),
        }
    }

    /// Hands the outpost an MSAA focus on mockapp's node `index`, as the
    /// listener would, and returns the focus it reports.
    pub fn msaa_focus(&self, hwnd: HWND, index: usize) -> Reported {
        self.focus(DeliveredFact::MsaaFocus {
            hwnd: hwnd.0 as isize,
            id_object: i32::try_from(index + 1).expect("a small index"),
            id_child: 0,
        })
    }

    /// Hands the outpost a UIA focus on `element`, which mockapp reports
    /// focused, as the listener reads and sends it, and returns the focus
    /// it reports, with the listener's calls added to the outpost's.
    /// `element` carries the listener's cache request.
    pub fn uia_focus(&self, element: &Cached<ListenerEvent>) -> Reported {
        let (judged, calls) = self.listener_read(element);
        let Judged::Read(_) = &judged else {
            panic!("the listener dropped the focus: {judged:?}");
        };
        let listener_failed_reads = verbatim_uia::calls::take_failed_reads();
        let mut reported = self.focus(judged.into_fact(0, FocusEvent::Focus, None));
        reported.calls += calls;
        reported.failed_reads += listener_failed_reads;
        reported
    }

    /// Asks the outpost for one object-navigation step, as Core would, and
    /// returns the neighbor and the calls it made; the reply must be the
    /// next thing the outpost says, and the only thing.
    pub fn navigate(&mut self, node_id: NodeId, kind: QueryKind) -> (NodeSnapshot, CallCounts) {
        let request_id = self.ask(Query::Navigate { node_id, kind });
        let answer = match self.next() {
            OutpostToCore::Reply {
                request_id: answered,
                outcome: QueryOutcome::Done(QueryResult::Navigated(Some(neighbor))),
                timing,
                ..
            } if answered == request_id => (neighbor, timing.calls),
            other => panic!("the outpost said {other:?}, not the step's neighbor"),
        };
        self.settled();
        answer
    }

    /// Asks the outpost for the focus now, as Core would at its start, and
    /// returns its focused control; the reply must be the next thing the
    /// outpost says, and the only thing.
    pub fn focus_now(&mut self) -> Option<FocusedControl> {
        self.focus_now_with_calls().0
    }

    /// [`focus_now`](Self::focus_now) with the calls its read made.
    pub fn focus_now_with_calls(&mut self) -> (Option<FocusedControl>, CallCounts) {
        let (now, calls) = self.focus_now_answer();
        (now.focus, calls)
    }

    /// [`focus_now`](Self::focus_now)'s whole answer, with the calls its
    /// read made.
    pub fn focus_now_answer(&mut self) -> (FocusNow, CallCounts) {
        let request_id = self.ask(Query::FocusNow);
        let answer = match self.next() {
            OutpostToCore::Reply {
                request_id: answered,
                outcome: QueryOutcome::Done(QueryResult::Focus(now)),
                timing,
                ..
            } if answered == request_id => (now, timing.calls),
            other => panic!("the outpost said {other:?}, not the focus now"),
        };
        self.settled();
        answer
    }

    /// Asks the outpost `query`, as Core would, and returns the calls it
    /// made answering it gone; the reply must be the next thing the outpost
    /// says, and the only thing.
    pub fn gone(&mut self, query: Query) -> CallCounts {
        let request_id = self.ask(query);
        let calls = match self.next() {
            OutpostToCore::Reply {
                request_id: answered,
                outcome: QueryOutcome::Gone,
                timing,
                ..
            } if answered == request_id => timing.calls,
            other => panic!("the outpost said {other:?}, not that the node is gone"),
        };
        self.settled();
        calls
    }

    /// Asks the outpost for a node's ancestors, as Core would, and returns
    /// them, outermost first, and the calls it made; the reply must be the
    /// next thing the outpost says, and the only thing.
    pub fn ancestors(&mut self, node_id: NodeId) -> (Vec<NodeSnapshot>, CallCounts) {
        let request_id = self.ask(Query::Ancestors { node_id });
        let answer = match self.next() {
            OutpostToCore::Reply {
                request_id: answered,
                outcome: QueryOutcome::Done(QueryResult::Ancestors(chain)),
                timing,
                ..
            } if answered == request_id => (chain, timing.calls),
            other => panic!("the outpost said {other:?}, not the node's ancestors"),
        };
        self.settled();
        answer
    }
}

/// Milliseconds since the Unix epoch on the run's shared clock, as the
/// hook stamps a caret key's press.
pub fn now_ms() -> u64 {
    verbatim_process::clock::now_us() / 1_000
}

/// `window`'s top-level window (`GetAncestor` with `GA_ROOT`), `None` for
/// none.
fn top_level(window: Option<isize>) -> Option<isize> {
    let window = window.filter(|&window| window != 0)?;
    // SAFETY: GetAncestor tolerates any handle, returning null for an
    // invalid one.
    let top = unsafe { GetAncestor(HWND(window as *mut _), GA_ROOT) }.0 as isize;
    (top != 0).then_some(top)
}

/// Microseconds since the Unix epoch on the run's shared clock, as the
/// listener stamps an event's arrival.
pub fn now_us() -> u64 {
    verbatim_process::clock::now_us()
}
