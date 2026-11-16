//! Core's reducer driving a real outpost watching mockapp, as verbatim.exe
//! runs them (`docs/design/readers-tell-core-asks-build1.md`, "The test
//! harness's idle"): the test causes events in mockapp and hands each to
//! [`CoreDriver`] as the tell the listener would send, Core's requests go
//! to the outpost through its request driver ([`OutpostUnderTest::forward`])
//! or, for the listener, are answered on this thread with the listener's
//! own read of the element it keeps (`verbatim_listener::focus_read::read_object`),
//! and every answer is fed back to Core, so that a test asserts what Core
//! says.
//!
//! The stand-in for the listener makes its check thread's calls on this
//! thread, as `verbatim-listener`'s `checking` makes them, through the
//! same decisions (`verbatim_listener_logic::check`), and reads a tell's
//! window facts with the foreground the test sets
//! ([`OutpostUnderTest::set_foreground`]), since nothing takes the
//! foreground on the desktop a test runs on.
//!
//! Core's time is the run's shared clock, never behind the last wake it
//! was given ([`CoreDriver::due`]).

use std::collections::VecDeque;
use std::sync::mpsc::TryRecvError;
use std::sync::{Arc, Mutex, PoisonError};

use verbatim_core::{Runtime, SrState, reduce};
use verbatim_ia2::{WinEvent, WinEventHook, WinEventKind};
use verbatim_listener_logic::check::{Call, Check, ProbeRecord, Walked};
use verbatim_listener_logic::focus::{BarObject, BarVerdict, ClassicBarRead, WindowAnswer};
use verbatim_model::event_kind::EventKind;
use verbatim_model::property::{
    PropertyAnswer, PropertyId, PropertyValue, PropertyValues, RuntimeId,
};
use verbatim_model::protocol::OutpostToCore;
use verbatim_model::request::{
    Answer, Ask, ForegroundReport, Instance, ObjectRef, Probe, ProcessFact, QueueEffect,
    QueueInput, Reader, Request, RequestNumber,
};
use verbatim_model::tell::{ElementToken, Event, EventObject, Payload, Read, TellWindow};
use verbatim_model::{
    BarChange, CallCounts, Earcon, Effect, Input, OutpostId, Pid, SegmentContent, StateSet,
    TraceId, WindowHandle,
};
use verbatim_reader_rules::acceptance::WindowQueries;
use verbatim_uia::{Cached, Direction, ElementExt, ListenerEvent, Uia};
use verbatim_win32::window_facts::{WindowPart, WindowReads};
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
    kept: Vec<(ElementToken, Cached<ListenerEvent>)>,
    next_token: u64,
    /// The last key's number.
    next_key: u64,
    /// Whether Core has started mockapp's outpost.
    started: bool,
    /// The check thread's record of its probes, kept across events.
    probes: ProbeRecord,
    /// The progress path's record of the objects a value change came
    /// from, as a reading thread found them.
    bars: Vec<(BarObject, BarVerdict)>,
    /// The windows the progress path has judged.
    bar_windows: Vec<isize>,
    /// The outpost's requests not yet answered.
    open: Vec<RequestNumber>,
    /// How many tells the outpost has sent.
    tells: usize,
    /// The wake Core last asked for.
    wake: Option<u64>,
    /// Core's time is never behind this.
    floor_us: u64,
    /// What Core said and the calls its requests made since the last take.
    said: Vec<Said>,
    calls: CallCounts,
    /// What Core said at its start.
    pub startup: Vec<Utterance>,
    /// Every input Core took and effect it gave, printed when a test
    /// fails.
    log: Vec<String>,
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
        Self::start(app, outpost)
    }

    /// [`Self::new`] with another application's window in front at Core's
    /// start, whose outpost could not be started, and then mockapp's window
    /// reported in front: the user switched to mockapp, whose first focus
    /// enters its window.
    pub fn switched_to(app: &super::MockApp, outpost: OutpostUnderTest) -> Self {
        let window = outpost.app_window();
        outpost.set_foreground(super::another_applications_window());
        let mut driver = Self::start(app, outpost);
        driver.outpost.set_foreground(window);
        let switched = driver.foreground_report().spoken();
        driver.startup.extend(switched);
        driver
    }

    fn start(app: &super::MockApp, outpost: OutpostUnderTest) -> Self {
        let mut driver = Self {
            outpost,
            state: SrState::default(),
            runtime: Runtime::default(),
            uia: Uia::new().expect("a UIA client for the listener's reads"),
            app: Pid(app.pid()),
            kept: Vec::new(),
            next_token: 0,
            next_key: 0,
            started: false,
            probes: ProbeRecord::default(),
            bars: Vec::new(),
            bar_windows: Vec::new(),
            open: Vec::new(),
            tells: 0,
            wake: None,
            floor_us: 0,
            said: Vec::new(),
            calls: CallCounts::default(),
            startup: Vec::new(),
            log: Vec::new(),
        };
        driver.feed(QueueInput::Process(ProcessFact::ListenerStarted(1)));
        driver.feed(QueueInput::ReaderReady(LISTENER));
        if driver.outpost.foreground() != 0 {
            driver.foreground_report();
        }
        driver.startup = driver.take().spoken();
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
            window: handle(window),
            root_owner: handle(verbatim_win32::window::root_owner_of(window)),
            process: Pid(process),
            thread,
            class: verbatim_win32::window::window_class_name(window),
        }));
        self.take()
    }

    /// What Core said, and the calls its requests made, since the last
    /// take.
    pub fn take(&mut self) -> Step {
        Step {
            said: std::mem::take(&mut self.said),
            calls: std::mem::take(&mut self.calls),
        }
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
        let tell = self.uia_tell(kind, element, states, payload);
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
        }
        step
    }

    /// The listener tells Core of `event`, a `WinEvent` its hook heard; a
    /// value change takes the listener's progress path instead
    /// ([`Self::bar_change`]).
    pub fn win_event(&mut self, event: &WinEvent) -> Step {
        if event.kind == WinEventKind::ValueChange {
            return self.bar_change(event);
        }
        let kind = verbatim_ia2::tell_kind(event).expect("a WinEvent the listener tells");
        self.msaa_event(
            kind,
            HWND(event.hwnd as _),
            (event.id_object, event.id_child),
        )
    }

    /// The listener's progress path for the MSAA value change `event`, as
    /// its hook and the application's reading thread take one
    /// (`docs/design/progress-bars.md`): the object's class and the
    /// path's record decide the read, which
    /// `verbatim_listener::listener::read_classic_bar` makes on this
    /// thread, and a bar's change goes to Core's inbox, related to the
    /// foreground the test sets rather than the desktop's.
    pub fn bar_change(&mut self, event: &WinEvent) -> Step {
        use std::sync::mpsc;
        use verbatim_model::protocol::Responder;
        let object = BarObject {
            window: event.hwnd,
            object: event.id_object,
            child: event.id_child,
        };
        let listed = verbatim_reader_rules::class::is_progress_bar_class(
            &verbatim_reader_rules::class::normalize_class_name(
                &verbatim_win32::window::window_class_name(event.hwnd),
            ),
        );
        let verdict = self
            .bars
            .iter()
            .find(|(known, _)| *known == object)
            .map(|(_, verdict)| *verdict);
        let Some(read) = verbatim_listener_logic::focus::classic_value_change(
            listed,
            verbatim_win32::window::window_is_visible(event.hwnd),
            verdict,
        ) else {
            return Step::default();
        };
        // A window the path has not judged is judged first, as its record
        // asks.
        let read = if read == ClassicBarRead::Unknown && !self.bar_windows.contains(&event.hwnd) {
            ClassicBarRead::Probe
        } else {
            read
        };
        let (reports, changed) = mpsc::channel();
        let (verdicts, learned) = mpsc::channel();
        let (windows, judged) = mpsc::channel();
        verbatim_listener::listener::read_classic_bar(
            object,
            read,
            Responder::new(move |change: Option<BarChange>| {
                if let Some(change) = change {
                    let _ = reports.send(change);
                }
            }),
            Responder::new(move |verdict: Option<BarVerdict>| {
                if let Some(verdict) = verdict {
                    let _ = verdicts.send(verdict);
                }
            }),
            Responder::new(move |answer: Option<WindowAnswer>| {
                if let Some(answer) = answer {
                    let _ = windows.send(answer);
                }
            }),
        );
        if judged.try_recv().is_ok() {
            self.bar_windows.push(event.hwnd);
        }
        if let Ok(verdict) = learned.try_recv() {
            self.bars.push((object, verdict));
        }
        let Ok(mut change) = changed.try_recv() else {
            return Step::default();
        };
        let front = self.outpost.foreground();
        change.in_foreground = front != 0
            && (event.hwnd == front || verbatim_win32::window::is_child(front, event.hwnd));
        self.run(VecDeque::from([Input::Progress {
            trace_id: TraceId::mint(),
            change,
        }]));
        self.take()
    }

    /// The listener tells Core of an MSAA `WinEvent` of `kind` for the
    /// object `object`, child `child`, of `window`.
    pub fn msaa_event(
        &mut self,
        kind: EventKind,
        window: HWND,
        (object, child): (i32, i32),
    ) -> Step {
        let window = window.0 as isize;
        let (thread, process) = verbatim_win32::window::window_owner(window);
        let reads = TestReads {
            foreground: self.outpost.foreground(),
        };
        let tell = Event {
            trace_id: TraceId::mint(),
            kind,
            process: Pid(process),
            application: "mockapp".to_owned(),
            thread,
            at_us: self.now(),
            object: EventObject::Msaa {
                window: handle(window),
                object,
                child,
            },
            cached: PropertyValues(Vec::new()),
            payload: Payload::None,
            window: TellWindow::Found(Box::new(
                WindowPart::read(&reads, window, None).with_foreground(&reads),
            )),
            checked: None,
        };
        self.tell(tell)
    }

    /// The listener tells Core `tell`; what Core said once every answer
    /// came.
    pub fn tell(&mut self, tell: Event) -> Step {
        self.feed(QueueInput::Tell {
            from: LISTENER,
            tell: Box::new(tell),
        });
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
        while self.tells < target {
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

    /// Wakes Core at the time it last asked to be woken at, when it asked.
    pub fn due(&mut self) -> Step {
        if let Some(wake) = self.wake.take() {
            self.floor_us = self.floor_us.max(wake);
            self.feed(QueueInput::Due);
        }
        self.take()
    }

    /// Whether Core has asked to be woken.
    pub fn wake(&self) -> Option<u64> {
        self.wake
    }

    /// Core's state, for a test that reads what it committed.
    pub fn state(&self) -> &SrState {
        &self.state
    }

    /// Core's time: the run's shared clock, never behind its floor.
    fn now(&self) -> u64 {
        verbatim_process::clock::now_us().max(self.floor_us)
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

    fn run(&mut self, mut inputs: VecDeque<Input>) {
        loop {
            while let Some(input) = inputs.pop_front() {
                let effects = reduce(&mut self.state, &mut self.runtime, &input);
                self.log.push(format!("Core takes {input:?}"));
                for effect in &effects {
                    self.log.push(format!("Core gives {effect:?}"));
                }
                for effect in effects {
                    self.effect(effect, &mut inputs);
                }
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
                self.calls += self.outpost.calls_of(answer.number);
                answer.assign_outpost(OutpostId(GENERATION));
                inputs.push_back(self.queued(QueueInput::Answer {
                    from: self.outpost_instance(),
                    answer,
                }));
            }
            OutpostToCore::Tell(tell) => {
                self.tells += 1;
                inputs.push_back(self.queued(QueueInput::Tell {
                    from: self.outpost_instance(),
                    tell,
                }));
            }
            mut message @ OutpostToCore::Event { .. } => {
                message.assign_outpost(OutpostId(GENERATION));
                inputs.extend(verbatim_model::protocol::event_input(self.app, message));
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
            QueueEffect::Answered { .. }
            | QueueEffect::SendStrokes(_)
            | QueueEffect::ReleaseAtExit(_)
            | QueueEffect::WindowDestroyed { .. }
            | QueueEffect::AnswerDropped { .. }
            | QueueEffect::EndOutpost(_)
            | QueueEffect::StaleFrame(_) => {}
            QueueEffect::Undecodable(number) => {
                panic!("Core could not decode the answer to {number}")
            }
            QueueEffect::Mismatch { replayed, original } => {
                panic!("Core's checkpoint did not replay: {replayed} against {original}")
            }
        }
    }

    /// The listener's answer to Core's `ask`: its read of the element it
    /// keeps, on this thread.
    fn listener_answer(&mut self, ask: &Ask) -> Answer {
        let Request::ReadObject(read) = &ask.request else {
            panic!(
                "Core asked the listener {:?}, which it reads no other way",
                ask.request
            );
        };
        let ObjectRef::Kept(token) = read.object else {
            panic!(
                "Core asked the listener of {:?}, which it does not keep",
                read.object
            );
        };
        let element = &self
            .kept
            .iter()
            .find(|(kept, _)| *kept == token)
            .expect("Core names an element the listener keeps")
            .1;
        let _ = verbatim_uia::calls::take();
        let readability = self.outpost.listener_readability();
        let outcome = verbatim_listener::focus_read::read_object(
            (&self.uia, &readability),
            element,
            (
                self.app,
                isize::try_from(ask.window.0).expect("a window handle"),
            ),
            read,
        );
        self.calls += verbatim_uia::calls::take();
        Answer {
            number: ask.number,
            outcome,
            probes: Vec::new(),
            detail: String::new(),
        }
    }

    /// The tell the listener sends for a UI Automation event of `kind` on
    /// `element`, kept behind a new token, after its check thread's calls,
    /// made now: [`Self::tell`] gives it to Core.
    #[expect(
        clippy::too_many_lines,
        reason = "the check thread's calls, one arm each, as the listener makes them"
    )]
    pub fn uia_tell(
        &mut self,
        kind: EventKind,
        element: &Cached<ListenerEvent>,
        states: Option<StateSet>,
        payload: Payload,
    ) -> Event {
        let fields = element.read();
        let runtime_id = RuntimeId(element.runtime_id());
        let kept: Vec<_> = self
            .kept
            .iter()
            .filter(|(_, kept)| kept.runtime_id() == runtime_id.0)
            .cloned()
            .collect();
        let name = fields.name.as_deref().unwrap_or_default();
        let placeholder = fields.role.class_name.as_deref() == Some("NetUITWMenuItem")
            && fields.role.control_type
                == Some(windows::Win32::UI::Accessibility::UIA_MenuItemControlTypeId.0)
            && name.is_empty();
        let switcher_item = fields.role.class_name.as_deref()
            == Some(verbatim_listener_logic::check::SWITCHER_ITEM_CLASS)
            && fields.role.control_type
                == Some(windows::Win32::UI::Accessibility::UIA_ListItemControlTypeId.0);
        let mut check = Check::new(
            kind,
            placeholder,
            kept.iter().map(|(token, _)| *token).collect(),
        )
        .reading_parent(switcher_item);
        let parent_request = self
            .uia
            .request_fixed::<ListenerEvent>()
            .expect("the parent's cache request");
        let record = &mut self.probes;
        let walker = self.uia.raw_view_walker().ok();
        let keyboard_focus = |element: &Cached<ListenerEvent>| {
            element
                .live()
                .element()
                .has_keyboard_focus()
                .map_or(Read::Failed, Read::Answered)
        };
        let now = verbatim_process::clock::now_us;
        while let Some(call) = check.next(record, now()) {
            match call {
                Call::NearestWindow => check.walked(
                    verbatim_uia::nearest_window_lookup(element.live().element())
                        .map_or(Read::Failed, Read::Answered),
                ),
                Call::Classes(window) => {
                    check.classed(verbatim_reader_rules::class::class_verdict(
                        &verbatim_win32::window::window_classes(window),
                    ));
                }
                Call::Probe(window) => {
                    let began_us = now();
                    let (thread, _) = verbatim_win32::window::window_owner(window);
                    let provider = verbatim_uia::raw_probe(window).has_provider();
                    check.probed(
                        Probe {
                            window: handle(window),
                            thread,
                            provider,
                            began_us,
                        },
                        record,
                    );
                }
                Call::KeptFocus(token) => {
                    let focused = kept
                        .iter()
                        .find(|(kept, _)| *kept == token)
                        .map_or(Read::Failed, |(_, kept)| keyboard_focus(kept));
                    check.kept_focus(token, focused);
                }
                Call::PreviousSibling => check.previous_sibling(match &walker {
                    Some(walker) => match element.live().step(walker, Direction::PreviousSibling) {
                        Ok(_) => Read::Answered(true),
                        Err(error) if error.code().is_ok() => Read::Answered(false),
                        Err(_) => Read::Failed,
                    },
                    None => Read::Failed,
                }),
                Call::ParentAutomationId => check.parent_automation_id(match &walker {
                    Some(walker) => {
                        match element
                            .live()
                            .walk(walker, Direction::Parent, &parent_request)
                        {
                            Ok(parent) => Read::Answered(parent.read().automation_id),
                            Err(error) if error.code().is_ok() => Read::Answered(None),
                            Err(_) => Read::Failed,
                        }
                    }
                    None => Read::Failed,
                }),
                Call::KeyboardFocus => check.keyboard_focus(keyboard_focus(element)),
            }
        }
        let (found, checked, parent) = check.finish();
        let window = match found {
            Walked::Found(window) => Some(window),
            Walked::NotWalked => (fields.role.hwnd != 0).then_some(fields.role.hwnd),
            Walked::NoWindow | Walked::Failed => None,
        };
        let reads = TestReads {
            foreground: self.outpost.foreground(),
        };
        self.next_token += 1;
        let token = ElementToken(self.next_token);
        self.kept.push((token, element.clone()));
        Event {
            trace_id: TraceId::mint(),
            kind,
            process: self.app,
            application: "mockapp".to_owned(),
            thread: window.map_or(0, |window| verbatim_win32::window::window_owner(window).0),
            at_us: self.now(),
            object: EventObject::Uia { token, runtime_id },
            cached: {
                let mut cached = cached_of(kind, &fields, self.app, states);
                if let Some(parent) = parent {
                    match cached.0.iter_mut().find(|answer| answer.id == parent.id) {
                        Some(place) => *place = parent,
                        None => cached.0.push(parent),
                    }
                }
                cached
            },
            payload,
            window: match (window, found) {
                (Some(window), _) => TellWindow::Found(Box::new(
                    WindowPart::read(&reads, window, None).with_foreground(&reads),
                )),
                (None, Walked::Failed) => TellWindow::Failed,
                (None, _) => TellWindow::NoWindow,
            },
            checked: Some(checked),
        }
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
        Box::new(move |event: WinEvent| {
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

/// The properties `kind` caches, read from the event's cache `fields` as
/// the listener's check thread reads them.
fn cached_of(
    kind: EventKind,
    fields: &ListenerEvent,
    process: Pid,
    states: Option<StateSet>,
) -> PropertyValues {
    let text = |value: &Option<String>| {
        value
            .as_ref()
            .filter(|text| !text.is_empty())
            .map(|text| PropertyValue::Text(text.clone()))
    };
    PropertyValues(
        kind.entry()
            .cached
            .iter()
            .map(|&id| PropertyAnswer {
                id,
                value: match id {
                    PropertyId::Name => text(&fields.name),
                    PropertyId::Role => Some(PropertyValue::Role(fields.role.role())),
                    PropertyId::States => states.map(PropertyValue::States),
                    PropertyId::ClassName => text(&fields.role.class_name),
                    PropertyId::AutomationId => text(&fields.automation_id),
                    PropertyId::NativeWindow => fields.role.window().map(PropertyValue::Window),
                    PropertyId::Process => Some(PropertyValue::Process(process)),
                    PropertyId::HasTextPattern => Some(PropertyValue::Flag(fields.text_available)),
                    _ => None,
                },
            })
            .collect(),
    )
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
        verbatim_reader_rules::Window::new(window).is_some_and(|window| {
            verbatim_win32::window::SystemWindows { foreground: 0 }.is_topmost(window)
        })
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

/// `window` as the model names a window.
fn handle(window: isize) -> WindowHandle {
    WindowHandle(i64::try_from(window).unwrap_or(0).cast_unsigned())
}
