//! Core's reducer driving a real outpost watching mockapp, as verbatim.exe
//! runs them (`books/dev/src/reference/crates/mockapp.md`, "The test
//! harness"): the test causes events in mockapp and hands each to
//! [`CoreDriver`] as the tell the listener would send, Core's requests go
//! to the outpost through its request driver ([`OutpostUnderTest::forward`])
//! or, for the listener, are answered on this thread with the listener's
//! own read of the element it keeps (`verbatim_listener::read_object`),
//! and every answer is fed back to Core, so that a test asserts what Core
//! says.
//!
//! The stand-in for the listener makes its check thread's calls on this
//! thread with the listener's own code (`verbatim_listener::listener`'s
//! `check_event`, `CheckEvent::into_tell`, `msaa_tell` and
//! `written_together`), and reads a tell's window facts with the
//! foreground the test sets
//! ([`OutpostUnderTest::set_foreground`]), since nothing takes the
//! foreground on the desktop a test runs on. It keeps the elements it told
//! of as the listener keeps them (`verbatim_listener_logic::kept`), each
//! tell numbered as it is given to Core, and lets them go by Core's held
//! views, which the driver sends after each input as verbatim.exe's shell
//! does (`verbatim_core::ListenerView`): written at once, or left for the
//! next request to the listener.
//!
//! Core's time is the test's ([`CoreTime`]): the outpost's observation
//! clock when the driver was made, moved on only by the time the test
//! passes ([`CoreDriver::pass_time`]), which moves the outpost's clocks on
//! with it, and by a wake it has Core take
//! ([`CoreDriver::due`]), so that no verdict in Core's backend cache, no
//! verdict in the check thread's record of its probes and no timer lapses
//! with the wall clock; or, for a test whose outpost watches a caret key's
//! effect on the run's shared clock, as the outpost does, that clock moved
//! on by the time the test passes.

use std::collections::VecDeque;
use std::sync::mpsc::TryRecvError;
use std::sync::{Arc, Mutex, PoisonError};

use verbatim_core::{ListenerView, ViewSend};
use verbatim_core::{Runtime, SrState, reduce};
use verbatim_ia2::{WinEvent, WinEventHook, WinEventKind};
use verbatim_listener::listener::{
    Applications, CheckEvent, EventReads, check_event, msaa_tell, written_together,
};
use verbatim_listener_logic::check::{Call, ProbeRecord};
use verbatim_listener_logic::kept::{ListenerKept, Reported, Start, View};
use verbatim_model::event_kind::EventKind;
use verbatim_model::property::RuntimeId;
use verbatim_model::protocol::OutpostToCore;
use verbatim_model::request::{
    Answer, Ask, ForegroundReport, FrontWindow, Instance, LiveFront, ProcessFact, QueueEffect,
    QueueInput, Reader, RequestNumber,
};
use verbatim_model::stroke::{HeldStroke, Stroke};
use verbatim_model::tell::{ElementToken, Event, EventObject, Payload};
use verbatim_model::{
    CallCounts, CaretKey, Earcon, Effect, Input, OutpostId, Pid, SegmentContent, StateSet, TraceId,
    WindowHandle,
};
use verbatim_process::held_slot::HeldSlot;
use verbatim_uia::{Cached, ListenerEvent, Uia};
use verbatim_win32::window::Readability;
use verbatim_win32::window_facts::{SystemReads, WindowReads};
use windows::Win32::Foundation::HWND;

use super::outpost::OutpostUnderTest;

/// The listener's instance.
const LISTENER: Instance = Instance {
    reader: Reader::Listener,
    generation: 1,
};

/// The generation of mockapp's outpost.
const GENERATION: u64 = 1;

/// What one utterance said: its segments' contents, marks left out.
pub type Utterance = Vec<SegmentContent>;

/// What Core did that a user notices, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Said {
    /// An utterance.
    Speech(Utterance),
    /// Speech was stopped.
    Stop,
    /// An earcon played.
    Earcon(Earcon),
}

/// What one step of a test brought from Core: what it said, stopped and
/// played, in order, and the calls its requests made into mockapp.
#[derive(Debug, Default)]
pub struct Step {
    /// What Core said, stopped and played.
    pub said: Vec<Said>,
    /// The calls of the requests answered, the listener's and the
    /// outpost's.
    pub calls: CallCounts,
    /// The calls the stand-in for the listener's check thread made
    /// judging the UI Automation events told, before Core asked anything.
    pub checked: CallCounts,
    /// Say-all's utterances whole, their marks included.
    pub say_all: Vec<Vec<SegmentContent>>,
    /// Whether Core let the display turn off again, as say-all's end does.
    pub display_released: bool,
}

impl Step {
    /// The utterances alone.
    pub fn spoken(&self) -> Vec<Utterance> {
        self.said
            .iter()
            .filter_map(|said| match said {
                Said::Speech(utterance) => Some(utterance.clone()),
                Said::Stop | Said::Earcon(_) => None,
            })
            .collect()
    }
}

/// Core and the real outpost, with the stand-in for the listener.
pub struct CoreDriver {
    /// The outpost Core asks.
    pub outpost: OutpostUnderTest,
    state: SrState,
    runtime: Runtime,
    uia: Uia,
    app: Pid,
    /// The elements the stand-in for the listener keeps, by token.
    kept: ListenerKept<Cached<ListenerEvent>>,
    /// The listener's numbering of its tells, as written to Core.
    reported: Reported,
    /// The shell's record of the views it sends the listener.
    view: ListenerView,
    /// A view left for the next request to the listener, in the
    /// supervisor's writer's slot.
    left: HeldSlot<verbatim_core::View>,

    next_token: u64,
    /// The last key's number.
    next_key: u64,
    /// Whether Core has started mockapp's outpost.
    started: bool,
    /// The check thread's record of its probes, kept across events.
    probes: ProbeRecord,
    /// The outpost's requests not yet answered.
    open: Vec<RequestNumber>,
    /// How many tells the outpost has sent.
    tells: usize,
    /// The wake Core last asked for.
    wake: Option<u64>,
    /// Core's time.
    clock: Clock,
    /// The listener's record of the applications, mockapp's, which names
    /// it and keeps its marks of hung threads.
    applications: Mutex<Applications>,
    /// What Core said and the calls its requests made since the last take.
    said: Vec<Said>,
    calls: CallCounts,
    checked: CallCounts,
    /// Say-all's utterances whole, and whether the display was let go,
    /// since the last take.
    say_all: Vec<Vec<SegmentContent>>,
    display_released: bool,
    /// What Core said at its start.
    pub startup: Vec<Utterance>,
    /// Every input Core took and effect it gave, printed when a test
    /// fails.
    log: Vec<String>,
    /// What the test does around the check's calls ([`Self::observe_check`]).
    check_observer: Option<CheckObserver>,
    /// What the test does as Core sends the outpost a request
    /// ([`Self::observe_sends`]).
    send_observer: Option<SendObserver>,
}

/// A test's observer of each request Core sends the outpost, called with
/// the outpost before it is handed the request.
pub type SendObserver = Box<dyn FnMut(&Ask, &OutpostUnderTest)>;

/// A moment of the stand-in for the listener's check thread, which a
/// test's observer acts at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckMoment {
    /// The check is about to make `Call`.
    Calling(Call),
    /// The check has made its last call.
    Checked,
}

/// A test's observer of the check's moments.
pub type CheckObserver = Box<dyn FnMut(CheckMoment)>;

/// Which clock Core reads, chosen once, as the test makes the driver.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoreTime {
    /// The outpost's observation clock when the driver was made, on which
    /// it stamps its probes, moved on only by the time the test passes and
    /// the wakes it has Core take: nothing lapses while the test runs,
    /// however slowly.
    Frozen,
    /// The run's shared clock moved on by the time the test passes, for a
    /// test whose caret key's watch the outpost times by the run's shared
    /// clock from the pass Core stamps
    /// (`books/dev/src/reference/crates/verbatim-outpost-logic.md`, "A caret key's wait").
    Real,
}

/// Core's time in a test ([`CoreTime`]).
#[derive(Clone, Copy, Debug)]
struct Clock {
    time: CoreTime,
    /// The outpost's observation clock when the driver was made, in
    /// microseconds.
    start_us: u64,
    /// How much time the test has passed, in microseconds.
    passed_us: u64,
}

impl Clock {
    fn new(time: CoreTime, start_us: u64) -> Self {
        Self {
            time,
            start_us,
            passed_us: 0,
        }
    }

    /// Now on the clock.
    fn now(self) -> u64 {
        match self.time {
            CoreTime::Frozen => self.start_us + self.passed_us,
            CoreTime::Real => verbatim_process::clock::now_us() + self.passed_us,
        }
    }

    /// Moves the clock on to `at_us` at least.
    fn reach(&mut self, at_us: u64) {
        let now = self.now();
        if at_us > now {
            self.passed_us += at_us - now;
        }
    }
}

impl Drop for CoreDriver {
    fn drop(&mut self) {
        if std::thread::panicking() {
            for line in &self.log {
                eprintln!("{line:.3000}");
            }
        }
    }
}

impl CoreDriver {
    /// Core started with the listener, the window the outpost reads as the
    /// foreground reported in front, when there is one, as the listener
    /// reports it at its start: `outpost` is the outpost Core starts for
    /// mockapp when it asks for one, and Core's startup read of the
    /// foreground application's focus is made. What Core said at its start
    /// is kept in [`Self::startup`].
    pub fn new(app: &super::MockApp, outpost: OutpostUnderTest) -> Self {
        Self::start(app, outpost, CoreTime::Frozen)
    }

    /// [`Self::new`], with Core's time on `time`.
    pub fn on_time(app: &super::MockApp, outpost: OutpostUnderTest, time: CoreTime) -> Self {
        Self::start(app, outpost, time)
    }

    /// [`Self::new`] with another application's window in front at Core's
    /// start, whose outpost could not be started, and then mockapp's window
    /// reported in front: the user switched to mockapp, whose first focus
    /// enters its window.
    pub fn switched_to(app: &super::MockApp, outpost: OutpostUnderTest) -> Self {
        let window = outpost.app_window();
        outpost.set_foreground(super::another_applications_window());
        let mut driver = Self::start(app, outpost, CoreTime::Frozen);
        driver.outpost.set_foreground(window);
        let switched = driver.foreground_report().spoken();
        driver.startup.extend(switched);
        driver
    }

    fn start(app: &super::MockApp, outpost: OutpostUnderTest, time: CoreTime) -> Self {
        let start_us = outpost.observed_now();
        let mut driver = Self {
            outpost,
            state: SrState::default(),
            runtime: Runtime::default(),
            uia: Uia::new().expect("a UIA client for the listener's reads"),
            app: Pid(app.pid()),
            kept: ListenerKept::default(),
            reported: Reported::default(),
            view: ListenerView::default(),
            left: HeldSlot::default(),
            next_token: 0,
            next_key: 0,
            started: false,
            probes: ProbeRecord::default(),
            open: Vec::new(),
            tells: 0,
            wake: None,
            clock: Clock::new(time, start_us),
            applications: Mutex::new(Applications::default()),
            said: Vec::new(),
            say_all: Vec::new(),
            display_released: false,
            calls: CallCounts::default(),
            checked: CallCounts::default(),
            startup: Vec::new(),
            log: Vec::new(),
            check_observer: None,
            send_observer: None,
        };
        driver.view.started();
        driver.feed(QueueInput::Process(ProcessFact::ListenerStarted(1)));
        driver.feed(QueueInput::ReaderReady(LISTENER));
        let started = if driver.outpost.foreground() == 0 {
            driver.take()
        } else {
            driver.foreground_report()
        };
        driver.startup = started.spoken();
        driver
    }

    fn outpost_instance(&self) -> Instance {
        Instance {
            reader: Reader::Outpost(self.app),
            generation: GENERATION,
        }
    }

    /// The listener's foreground report of the window the outpost reads as
    /// the foreground, as it sends one when that window comes to the front.
    pub fn foreground_report(&mut self) -> Step {
        let window = self.outpost.foreground();
        let (thread, process) = verbatim_win32::window::window_owner(window);
        self.feed(QueueInput::Foreground(ForegroundReport {
            window: WindowHandle::from(window),
            root_owner: WindowHandle::from(verbatim_win32::window::root_owner_of(window)),
            process: Pid(process),
            thread,
            class: verbatim_win32::window::window_class_name(window),
            ghost: None,
            console: false,
            integrity: None,
            own_integrity: None,
        }));
        self.take()
    }

    /// What Core said, and the calls its requests made, since the last
    /// take.
    pub fn take(&mut self) -> Step {
        Step {
            said: std::mem::take(&mut self.said),
            calls: std::mem::take(&mut self.calls),
            checked: std::mem::take(&mut self.checked),
            say_all: std::mem::take(&mut self.say_all),
            display_released: std::mem::take(&mut self.display_released),
        }
    }

    /// The listener tells Core that `window` of mockapp's was destroyed,
    /// raised by a thread of mockapp's process, as it sends a destroy:
    /// what Core said once the outpost handled everything it was handed.
    pub fn destroyed(&mut self, window: HWND) -> Step {
        self.feed(QueueInput::WindowDestroyed {
            from: LISTENER,
            window: WindowHandle::from(window.0 as isize),
            process: Some(self.app),
        });
        self.take()
    }

    /// The listener tells Core of a UI Automation focus on `element`, which
    /// mockapp raised; what Core said once every answer came, and the
    /// calls of its reads.
    pub fn uia_focus(&mut self, element: &Cached<ListenerEvent>) -> Step {
        self.uia_event(EventKind::UiaFocus, element, None, Payload::None)
    }

    /// The listener tells Core of a UI Automation event of `kind` on
    /// `element`, with `states` for a kind that caches them and `payload`.
    pub fn uia_event(
        &mut self,
        kind: EventKind,
        element: &Cached<ListenerEvent>,
        states: Option<StateSet>,
        payload: Payload,
    ) -> Step {
        let tell = self.checked_tell(kind, element, states, payload);
        self.tell(tell)
    }

    /// The listener tells Core of an MSAA focus on mockapp's node `index`
    /// in `window`, at its own object id, as for a focus mockapp's focus
    /// moved to with no event (`set-focus`).
    pub fn msaa_focus(&mut self, window: HWND, index: usize) -> Step {
        let object = i32::try_from(index + 1).expect("a small index");
        self.msaa_event(EventKind::MsaaFocus, window, (object, 0))
    }

    /// The listener tells Core of a foreground event of `window`, after its
    /// foreground report when `window` is the one the outpost reads as the
    /// foreground, as the listener's order holds a foreground's tells
    /// until its window is in front.
    pub fn foreground(&mut self, window: HWND) -> Step {
        let mut step = Step::default();
        if window.0 as isize == self.outpost.foreground() {
            step = self.foreground_report();
        }
        // The listener's hook names a window object's event by its client
        // area, as NVDA's hook does (`verbatim_ia2`'s hook).
        let told = self.msaa_event(
            EventKind::Foreground,
            window,
            (windows::Win32::UI::WindowsAndMessaging::OBJID_CLIENT.0, 0),
        );
        step.said.extend(told.said);
        step.calls += told.calls;
        step.checked += told.checked;
        step
    }

    /// Has mockapp run each of `lines`, and tells Core of the `count`
    /// `WinEvent`s of `kinds` it raised meanwhile, in the order a hook on
    /// this thread heard them, as the listener's hook hears them.
    pub fn raise(
        &mut self,
        app: &mut super::MockApp,
        lines: &[&str],
        kinds: &[WinEventKind],
        count: usize,
    ) -> Step {
        let pid = app.pid();
        let events = win_events(
            pid,
            || {
                for line in lines {
                    app.send(line);
                }
            },
            kinds,
            count,
        );
        let mut step = Step::default();
        for event in events {
            let told = self.win_event(&event);
            step.said.extend(told.said);
            step.calls += told.calls;
            step.checked += told.checked;
        }
        step
    }

    /// The listener tells Core of `event`, a `WinEvent` its hook heard.
    pub fn win_event(&mut self, event: &WinEvent) -> Step {
        let kind = verbatim_ia2::tell_kind(event).expect("a WinEvent the listener tells");
        self.msaa_event(
            kind,
            HWND(event.hwnd as _),
            (event.id_object, event.id_child),
        )
    }

    /// The listener tells Core of an MSAA `WinEvent` of `kind` for the
    /// object `object`, child `child`, of `window`.
    pub fn msaa_event(
        &mut self,
        kind: EventKind,
        window: HWND,
        (object, child): (i32, i32),
    ) -> Step {
        match self.win_event_tell(kind, window, (object, child)) {
            Some(tell) => self.tell(tell),
            None => Step::default(),
        }
    }

    /// Has mockapp run each of `lines`, and tells Core of the `count`
    /// `WinEvent`s of `kinds` it raised meanwhile all at once, in the order
    /// a hook on this thread heard them, before Core hears any answer to
    /// the requests the first makes: the listener's pipe brought them
    /// together, as it brings a burst an application raises within a
    /// millisecond, its limiter keeping one waiting event per object, kind
    /// and raising thread, the newest in its place
    /// (`books/dev/src/reference/crates/verbatim-core.md`, "The waiting
    /// entries").
    pub fn raise_together(
        &mut self,
        app: &mut super::MockApp,
        lines: &[&str],
        kinds: &[WinEventKind],
        count: usize,
    ) -> Step {
        let pid = app.pid();
        let events = win_events(
            pid,
            || {
                for line in lines {
                    app.send(line);
                }
            },
            kinds,
            count,
        );
        let reads = TestReads {
            foreground: self.outpost.foreground(),
        };
        let tells = events
            .iter()
            .filter_map(|event| {
                let kind = verbatim_ia2::tell_kind(event).expect("a WinEvent the listener tells");
                let address = (event.hwnd, event.id_object, event.id_child);
                msaa_tell(
                    &self.applications,
                    (kind, address, event.thread),
                    self.clock.now(),
                    &reads,
                )
            })
            .collect();
        let inputs = written_together(tells)
            .into_iter()
            .map(|tell| {
                self.queued(QueueInput::Tell {
                    from: LISTENER,
                    tell: Box::new(tell),
                })
            })
            .collect();
        self.run(inputs);
        self.take()
    }

    /// The tell the listener sends for an MSAA `WinEvent` of `kind` for the
    /// object `object`, child `child`, of `window`, raised by the window's
    /// thread, unless a local drop takes it.
    fn win_event_tell(
        &mut self,
        kind: EventKind,
        window: HWND,
        (object, child): (i32, i32),
    ) -> Option<Event> {
        let window = window.0 as isize;
        let (thread, _) = verbatim_win32::window::window_owner(window);
        let reads = TestReads {
            foreground: self.outpost.foreground(),
        };
        msaa_tell(
            &self.applications,
            (kind, (window, object, child), thread),
            self.clock.now(),
            &reads,
        )
        .map(|(_, tell)| tell)
    }

    /// The listener tells Core `tell`; what Core said once every answer
    /// came.
    pub fn tell(&mut self, tell: Event) -> Step {
        // Numbered as the listener writes it, and counted as the shell
        // hands it to Core.
        if let EventObject::Uia { token, .. } = &tell.object {
            self.reported.written(true, [*token]);
            self.view.handled(*token);
        }
        self.feed(QueueInput::Tell {
            from: LISTENER,
            tell: Box::new(tell),
        });
        self.take()
    }

    /// The user types `text`, which reaches the application at once, as a
    /// typed key does; what Core said once every answer came.
    pub fn typed(&mut self, text: &str) -> Step {
        self.next_key += 1;
        self.feed(QueueInput::Press(Box::new(
            verbatim_model::request::Press {
                key_seq: verbatim_model::KeySeq::new(self.next_key),
                pressed_at_us: self.now(),
                action: verbatim_model::request::PressAction::Typed {
                    text: text.to_owned(),
                },
            },
        )));
        self.take()
    }

    /// The user presses the key of review `command`, its first press; what
    /// Core said once every answer came.
    pub fn command(&mut self, command: verbatim_model::ReviewCommand) -> Step {
        self.next_key += 1;
        self.feed(QueueInput::Press(Box::new(
            verbatim_model::request::Press {
                key_seq: verbatim_model::KeySeq::new(self.next_key),
                pressed_at_us: self.now(),
                action: verbatim_model::request::PressAction::Command { command, repeat: 0 },
            },
        )));
        self.take()
    }

    /// Playback reached say-all's `mark`; what Core said once every answer
    /// came.
    pub fn mark_reached(&mut self, mark: verbatim_model::SpeechMark) -> Step {
        self.feed(QueueInput::MarkReached(mark));
        self.take()
    }

    /// The user presses caret key `key` with the committed focus's record
    /// bound to it at its press, as the hook's view publishes it
    /// ([`SrState::caret_binding`]), held back from the application when
    /// the view holds it (a key bound to text that does not select), and
    /// `application` handles the key when it reaches the application: at
    /// once for a key passed at once, else when Core sends its stroke. What
    /// Core said once every answer it waits on without the outpost's watch
    /// came; [`Self::answered`] waits for the watch.
    pub fn caret_key(&mut self, key: CaretKey, mut application: impl FnMut()) -> Step {
        self.next_key += 1;
        let bound = self.state.caret_binding();
        let held = (bound.is_some() && !key.select).then(|| HeldStroke {
            stroke: Stroke {
                vk: 0,
                scan_code: 0,
                extended: false,
                sequence: u32::try_from(self.next_key).expect("a small key number"),
            },
            modifiers: Vec::new(),
        });
        if held.is_none() {
            application();
        }
        let press = QueueInput::Press(Box::new(verbatim_model::request::Press {
            key_seq: verbatim_model::KeySeq::new(self.next_key),
            pressed_at_us: self.now(),
            action: verbatim_model::request::PressAction::CaretKey {
                key,
                held,
                bound: bound.map(Box::new),
            },
        }));
        let input = self.queued(press);
        self.run_handling(VecDeque::from([input]), &mut application);
        self.take()
    }

    /// Waits for the outpost to answer every request Core has out to it,
    /// such as a caret key's watch, feeding Core each of its messages as it
    /// arrives; what Core said meanwhile.
    pub fn answered(&mut self) -> Step {
        while !self.open.is_empty() {
            let message = self
                .outpost
                .messages
                .recv_timeout(super::WAIT_TIMEOUT)
                .expect("the outpost answers Core's requests");
            let mut inputs = VecDeque::new();
            self.message(message, &mut inputs);
            self.run(inputs);
        }
        self.take()
    }

    /// Feeds Core what the outpost has answered since, once it has handled
    /// everything handed to it: the answer to a `Follow` waiting for a
    /// change mockapp raised.
    pub fn pump(&mut self) -> Step {
        let mut inputs = VecDeque::new();
        self.collect(&mut inputs);
        self.run(inputs);
        self.take()
    }

    /// Waits for the outpost to tell Core of `count` more events its
    /// `Follow` registration delivered, such as a change mockapp raised,
    /// feeding Core each of its messages as it arrives; what Core said
    /// meanwhile.
    pub fn told(&mut self, count: usize) -> Step {
        let target = self.tells + count;
        self.until_told(target)
    }

    /// How many tells the outpost has sent Core so far.
    pub fn tells(&self) -> usize {
        self.tells
    }

    /// Waits until the outpost has told Core of `total` events in all
    /// since the driver was made ([`Self::tells`]), feeding Core each of its
    /// messages as it arrives; what Core said meanwhile. For an event the
    /// test caused during a step that may or may not have taken in its tell
    /// by its end, as a caret event mockapp raises while it handles a key:
    /// UI Automation delivers it on a thread of its own, so the step's last
    /// settle may come before or after the outpost hears it.
    pub fn until_told(&mut self, total: usize) -> Step {
        while self.tells < total {
            let message = self
                .outpost
                .messages
                .recv_timeout(super::WAIT_TIMEOUT)
                .expect("the outpost tells Core of the change");
            let mut inputs = VecDeque::new();
            self.message(message, &mut inputs);
            self.run(inputs);
        }
        self.take()
    }

    /// Wakes Core at the time it last asked to be woken at, when it asked,
    /// moving its clock on to that time. While a check after a menu, popup
    /// or switcher end waits, the wake carries the window in front as the
    /// shell's does, here the window the outpost reads as the foreground,
    /// since a test's window is not Windows' foreground window.
    pub fn due(&mut self) -> Step {
        if let Some(wake) = self.wake.take() {
            self.clock.reach(wake);
            let in_front = self.state.menu_check_waits().then(|| {
                let window = self.outpost.foreground();
                let (thread, process) = verbatim_win32::window::window_owner(window);
                LiveFront::Window(FrontWindow {
                    window: WindowHandle::from(window),
                    root_owner: WindowHandle::from(verbatim_win32::window::root_owner_of(window)),
                    process: Pid(process),
                    thread,
                    class: verbatim_win32::window::window_class_name(window),
                })
            });
            self.feed(QueueInput::Due { in_front });
        }
        self.take()
    }

    /// Moves Core's clock and the outpost's on by `by`, waking Core at
    /// none of its timers.
    pub fn pass_time(&mut self, by: std::time::Duration) {
        self.clock.passed_us += u64::try_from(by.as_micros()).expect("a short time");
        self.outpost.pass_time(by);
    }

    /// Whether Core has asked to be woken.
    pub fn wake(&self) -> Option<u64> {
        self.wake
    }

    /// Has `observer` told of each call the stand-in for the listener's
    /// check is about to make, and of the check's end, from now on, as a
    /// test acts on mockapp at a moment of the check; `None` stops it.
    pub fn observe_check(&mut self, observer: Option<CheckObserver>) {
        self.check_observer = observer;
    }

    /// Has `observer` called with each request Core sends the outpost from
    /// now on, before the outpost is handed it, as a test acts on mockapp
    /// while a request is on its way; `None` stops it.
    pub fn observe_sends(&mut self, observer: Option<SendObserver>) {
        self.send_observer = observer;
    }

    /// Gives Core the details the theme reports, as the shell does when a
    /// theme becomes active.
    pub fn reported(&mut self, reported: verbatim_model::ReportedDetails) {
        self.run(VecDeque::from([Input::Reported(reported)]));
    }

    /// Core's state, for a test that reads what it committed.
    pub fn state(&self) -> &SrState {
        &self.state
    }

    /// Core's time ([`CoreTime`]).
    fn now(&self) -> u64 {
        self.clock.now()
    }

    /// Feeds Core `input`, and everything its effects bring back, in
    /// order.
    fn feed(&mut self, input: QueueInput) {
        self.run(VecDeque::from([Input::Queue {
            trace_id: TraceId::mint(),
            at_us: self.now(),
            input,
        }]));
    }

    fn run(&mut self, inputs: VecDeque<Input>) {
        self.run_handling(inputs, &mut || {});
    }

    /// [`Self::run`], with `application` handling each key Core sends it.
    fn run_handling(&mut self, mut inputs: VecDeque<Input>, application: &mut dyn FnMut()) {
        loop {
            while let Some(input) = inputs.pop_front() {
                let effects = reduce(&mut self.state, &mut self.runtime, &input);
                self.log.push(format!("Core takes {input:?}"));
                for effect in &effects {
                    self.log.push(format!("Core gives {effect:?}"));
                }
                for effect in effects {
                    if matches!(effect, Effect::Queue(QueueEffect::SendStrokes(_))) {
                        application();
                    }
                    self.effect(effect, &mut inputs);
                }
                self.send_views();
            }
            self.collect(&mut inputs);
            if inputs.is_empty() {
                break;
            }
        }
    }

    /// The outpost's messages since, once it has handled everything handed
    /// to it, as Core's inputs.
    fn collect(&mut self, inputs: &mut VecDeque<Input>) {
        self.outpost.settle();
        loop {
            match self.outpost.messages.try_recv() {
                Ok(message) => self.message(message, inputs),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => panic!("the outpost's pipe closed"),
            }
        }
    }

    /// The outpost's `message` as Core's input.
    fn message(&mut self, message: OutpostToCore, inputs: &mut VecDeque<Input>) {
        match message {
            OutpostToCore::Answer(mut answer) => {
                self.open.retain(|open| *open != answer.number);
                let calls = self.outpost.calls_of(answer.number);
                self.log
                    .push(format!("The calls of {}: {calls:?}", answer.number));
                self.calls += calls;
                answer.assign_outpost(OutpostId(GENERATION));
                inputs.push_back(self.queued(QueueInput::Answer {
                    from: self.outpost_instance(),
                    answer,
                }));
            }
            OutpostToCore::Tell(mut tell) => {
                self.tells += 1;
                tell.assign_outpost(OutpostId(GENERATION));
                inputs.push_back(self.queued(QueueInput::Tell {
                    from: self.outpost_instance(),
                    tell,
                }));
            }
            other => panic!("the outpost said {other:?}, which Core does not take here"),
        }
    }

    fn queued(&self, input: QueueInput) -> Input {
        Input::Queue {
            trace_id: TraceId::mint(),
            at_us: self.now(),
            input,
        }
    }

    /// Carries out one of Core's effects, queuing what it brings back.
    fn effect(&mut self, effect: Effect, inputs: &mut VecDeque<Input>) {
        match effect {
            Effect::Speak(utterance, key) if utterance.say_all => {
                self.say_all.push(
                    utterance
                        .segments
                        .iter()
                        .map(|segment| segment.content.clone())
                        .collect(),
                );
                self.effect(
                    Effect::Speak(
                        verbatim_model::Utterance {
                            say_all: false,
                            ..utterance
                        },
                        key,
                    ),
                    inputs,
                );
            }
            Effect::KeepDisplayOn(false) => self.display_released = true,
            Effect::Speak(utterance, _) => self.said.push(Said::Speech(
                utterance
                    .segments
                    .into_iter()
                    .map(|segment| segment.content)
                    .filter(|content| !matches!(content, SegmentContent::Mark(_)))
                    .collect(),
            )),
            Effect::StopSpeech => self.said.push(Said::Stop),
            Effect::PlayEarcon(earcon) => self.said.push(Said::Earcon(earcon)),
            // A key's speech, fenced by its number: said as any other.
            Effect::ForKey(_, effect) => self.effect(*effect, inputs),
            Effect::Queue(queue) => self.queue_effect(queue, inputs),
            _ => {}
        }
    }

    fn queue_effect(&mut self, effect: QueueEffect, inputs: &mut VecDeque<Input>) {
        match effect {
            QueueEffect::Send { to, ask } => match to.reader {
                Reader::Listener => {
                    let answer = self.listener_answer(&ask);
                    inputs.push_back(self.queued(QueueInput::Answer {
                        from: LISTENER,
                        answer,
                    }));
                }
                Reader::Outpost(_) => {
                    if let Some(observer) = &mut self.send_observer {
                        observer(&ask, &self.outpost);
                    }
                    self.open.push(ask.number);
                    self.outpost.forward(ask);
                }
            },
            QueueEffect::Cancel { to, cancel } => {
                if matches!(to.reader, Reader::Outpost(_)) {
                    self.outpost.cancel_request(cancel.number, cancel.reason);
                }
            }
            QueueEffect::WakeAt(wake) => self.wake = wake,
            QueueEffect::CheckHung(windows) => {
                for window in windows {
                    inputs.push_back(
                        self.queued(QueueInput::HangCheck {
                            window,
                            hung: verbatim_win32::window::readability(
                                isize::try_from(window.0).expect("a window handle"),
                            )
                            .is_hung(),
                        }),
                    );
                }
            }
            // Another application's outpost cannot be started here.
            QueueEffect::StartOutpost(process) if process != self.app => {
                inputs.push_back(self.queued(QueueInput::Process(ProcessFact::LaunchFailed {
                    process,
                    application_gone: false,
                })));
            }
            QueueEffect::StartOutpost(_) => {
                assert!(!self.started, "Core started mockapp's outpost again");
                self.started = true;
                inputs.push_back(
                    self.queued(QueueInput::Process(ProcessFact::OutpostStarted {
                        process: self.app,
                        generation: GENERATION,
                    })),
                );
                inputs.push_back(self.queued(QueueInput::ReaderReady(self.outpost_instance())));
            }
            // As Core's shell sends it, ahead of any request after it.
            QueueEffect::WindowDestroyed { to, window } => {
                assert_eq!(to, self.outpost_instance(), "a destroy for another outpost");
                self.outpost.outpost.handle_command(
                    &verbatim_model::protocol::CoreToOutpost::WindowDestroyed(window),
                );
            }
            QueueEffect::Answered { .. }
            | QueueEffect::SendStrokes(_)
            | QueueEffect::ReleaseAtExit(_)
            | QueueEffect::AnswerDropped { .. }
            | QueueEffect::EndOutpost(_)
            | QueueEffect::StaleFrame(_) => {}
            QueueEffect::Undecodable(number) => {
                panic!("Core could not decode the answer to {number}")
            }
            QueueEffect::Mismatch { replayed, original } => {
                panic!("Core's checkpoint did not replay: {replayed} against {original}")
            }
            QueueEffect::ForeignHandle { batch, handle } => {
                panic!("a handle of batch {handle:?} was given to batch {batch:?}")
            }
        }
    }

    /// Core's held view after an input, as the shell sends it: taken by
    /// the stand-in for the listener at once, or left for the next request
    /// to it.
    fn send_views(&mut self) {
        match self.view.after_pass(self.state.held_tokens().as_ref()) {
            ViewSend::Nothing => {}
            ViewSend::Now(view) => {
                self.left.superseded();
                self.take_view(view);
            }
            ViewSend::Leave(view) => self.left.leave(view, 0),
        }
    }

    /// The stand-in for the listener takes Core's held view `view`: every
    /// element it rules out is let go, as the listener lets go of it.
    fn take_view(&mut self, view: verbatim_core::View) {
        assert_eq!(
            view.generation, LISTENER.generation,
            "a view of another listener"
        );
        let view = View {
            tokens: view.tokens.into_iter().collect(),
            acknowledged: view.acknowledged,
        };
        for (_, released) in self.kept.view(view, &mut self.reported) {
            self.log.push(format!(
                "The listener lets go of {} elements",
                released.len()
            ));
        }
    }

    /// The listener's answer to Core's `ask`: its read of the element it
    /// keeps, or of an MSAA object at an event's address, on this thread;
    /// a view left for it goes with the request, after it.
    fn listener_answer(&mut self, ask: &Ask) -> Answer {
        let answer = self.listener_read(ask);
        if let Some(view) = self.left.carried(false) {
            self.take_view(view);
        }
        answer
    }

    /// The listener's read for Core's `ask` of the element it keeps: a
    /// batch on it, or a generic read of it.
    fn listener_read(&mut self, ask: &Ask) -> Answer {
        let answered = |outcome| Answer {
            number: ask.number,
            outcome,
            probes: Vec::new(),
            detail: String::new(),
            began_us: 0,
            ended_us: 0,
        };
        let (listener, token, keep) = match &ask.request {
            verbatim_model::request::Question::Batch(batch)
                if matches!(
                    batch.steps.first().and_then(|step| step.op.subject()),
                    Some(verbatim_model::batch::Subject::Msaa { .. })
                ) =>
            {
                return answered(verbatim_listener::read_msaa_batch(batch));
            }
            verbatim_model::request::Question::Batch(batch) => {
                let Some(verbatim_model::batch::Subject::Kept {
                    listener,
                    token,
                    keep,
                }) = batch.steps.first().and_then(|step| step.op.subject())
                else {
                    panic!(
                        "Core asked the listener {:?}, which it reads no other way",
                        ask.request
                    );
                };
                (*listener, *token, *keep)
            }
        };
        assert_eq!(listener, LISTENER.generation, "a token of another listener");
        let element = match self
            .kept
            .start_read((self.app, token), (ask.number.0, keep))
        {
            Start::Read(element) => element,
            Start::NotThere => return answered(verbatim_model::request::Outcome::NotThere),
            Start::NotAsked => panic!("Core asked of {token:?} twice"),
        };
        let _ = verbatim_win32::calls::take();
        let readability = self.outpost.listener_readability();
        let nearest = (
            self.app,
            isize::try_from(ask.window.0).expect("a window handle"),
        );
        let outcome = match &ask.request {
            verbatim_model::request::Question::Batch(batch) => {
                verbatim_listener::read_object((&self.uia, &readability), &element, nearest, batch)
            }
        };
        self.calls += verbatim_win32::calls::take();
        let _ = self
            .kept
            .read_ended((self.app, token), element, true, &mut self.reported);
        answered(outcome)
    }

    /// The tell the listener sends for a UI Automation event of `kind` on
    /// `element`, kept behind a new token, after its check thread's calls,
    /// made now with the listener's own code: [`Self::tell`] gives it to
    /// Core.
    pub fn checked_tell(
        &mut self,
        kind: EventKind,
        element: &Cached<ListenerEvent>,
        states: Option<StateSet>,
        payload: Payload,
    ) -> Event {
        let fields = element.read();
        let event = CheckEvent::new(
            kind,
            (element, &fields),
            self.app,
            self.now(),
            (states, payload),
        )
        .expect("the element kept for its check");
        let kept: Vec<(ElementToken, Option<Cached<ListenerEvent>>)> = self
            .kept
            .with_runtime_id(self.app, event.runtime_id())
            .into_iter()
            .map(|(token, kept)| (token, Some(kept)))
            .collect();
        let parent_request = self
            .uia
            .request_fixed::<ListenerEvent>()
            .expect("the parent's cache request");
        let walker = self.uia.raw_view_walker().ok();
        let clock = self.clock;
        let now = move || clock.now();
        let mut observer = self.check_observer.take();
        let reads = TestReads {
            foreground: self.outpost.foreground(),
        };
        let _ = verbatim_win32::calls::take();
        let told = check_event(
            &event,
            (Some(element), &kept),
            (walker.as_ref(), Some(&parent_request)),
            (&mut self.probes, &now),
            &reads,
            &mut |call| {
                if let Some(observer) = observer.as_mut() {
                    observer(CheckMoment::Calling(call));
                }
            },
        );
        self.checked += verbatim_win32::calls::take();
        if let Some(observer) = observer.as_mut() {
            observer(CheckMoment::Checked);
        }
        self.check_observer = observer;
        self.next_token += 1;
        let token = ElementToken(self.next_token);
        let mut kept_runtime_id = None;
        let tell = event.into_tell(
            told,
            |_, runtime_id| {
                kept_runtime_id = Some(runtime_id.clone());
                EventObject::Uia { token, runtime_id }
            },
            "mockapp".to_owned(),
        );
        let runtime_id: RuntimeId = kept_runtime_id.expect("the tell's runtime id");
        self.kept
            .keep((self.app, token), (element.clone(), runtime_id), kind);
        self.reported.queued(token);
        tell.finish(&reads).1
    }
}

/// The `count` `WinEvent`s of `kinds` mockapp, process `pid`, raised while
/// `cause` ran, in the order a hook on this thread heard them, as the
/// listener's hook hears them.
pub fn win_events(
    pid: u32,
    cause: impl FnOnce(),
    kinds: &[WinEventKind],
    count: usize,
) -> Vec<WinEvent> {
    let seen = Arc::new(Mutex::new(Vec::<WinEvent>::new()));
    let sink = Arc::clone(&seen);
    let hook = WinEventHook::install(
        pid,
        kinds,
        Box::new(move |event: WinEvent, _class: Option<&str>| {
            sink.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(event);
        }),
    )
    .expect("a hook of mockapp's events");
    cause();
    let events = super::pump_until(|| {
        let seen = seen.lock().unwrap_or_else(PoisonError::into_inner);
        (seen.len() >= count).then(|| seen.clone())
    });
    drop(hook);
    assert_eq!(events.len(), count, "mockapp raised {events:?}");
    events
}

/// Windows' own state, as the listener reads it, but for the foreground
/// and active window, which are the test's.
struct TestReads {
    foreground: isize,
}

impl WindowReads for TestReads {
    fn class(&self, window: isize) -> String {
        verbatim_win32::window::window_class_name(window)
    }

    fn owner(&self, window: isize) -> (u32, u32) {
        verbatim_win32::window::window_owner(window)
    }

    fn console_host(&self, window: isize) -> Option<u32> {
        verbatim_win32::provider::provider_process(window)
    }

    fn parent(&self, window: isize) -> isize {
        verbatim_win32::window::parent(window)
    }

    fn root(&self, window: isize) -> isize {
        verbatim_win32::window::top_level_of(window)
    }

    fn root_owner(&self, window: isize) -> isize {
        verbatim_win32::window::root_owner_of(window)
    }

    fn is_child(&self, parent: isize, child: isize) -> bool {
        verbatim_win32::window::is_child(parent, child)
    }

    fn topmost(&self, window: isize) -> bool {
        verbatim_win32::window::is_topmost(window)
    }

    fn desktop(&self) -> isize {
        verbatim_win32::window::desktop_window()
    }

    fn foreground(&self) -> isize {
        self.foreground
    }

    fn active_window(&self) -> isize {
        self.foreground
    }

    fn hidden_frame(&self, window: isize) -> bool {
        verbatim_win32::window::window_is_hidden_frame(window)
    }
}

impl EventReads for TestReads {
    fn exists(&self, window: isize) -> bool {
        SystemReads.exists(window)
    }

    fn hang_state(&self, window: isize) -> Readability {
        SystemReads.hang_state(window)
    }

    fn stood_for(&self, window: isize) -> Option<isize> {
        SystemReads.stood_for(window)
    }

    fn process_of_thread(&self, thread: u32) -> Option<u32> {
        SystemReads.process_of_thread(thread)
    }

    fn input_desktop_opens(&self) -> bool {
        SystemReads.input_desktop_opens()
    }
}
