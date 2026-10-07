//! A real outpost in the test process, watching mockapp, as Core would
//! drive it: the tests hand it facts and queries over its command entry
//! point and read every message it writes to its pipe.
//!
//! The outpost reads a UIA focus's element from the system's keyboard
//! focus, which a test must not take from the desktop it runs on, so this
//! outpost reads it from the test instead
//! ([`Outpost::with_focused_element_reader`]): the test sets the element
//! mockapp reports focused, and the outpost's read of it counts as the one
//! call the system read is.

use std::io::Write;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError};

use verbatim_model::{
    CallCounts, CallKind, NodeId, NodeSnapshot, NormalizedEvent, Pid, QueryKind, TraceId,
    WindowHandle,
};
use verbatim_outpost::listener::uia_focus_fact;
use verbatim_outpost::protocol::{
    DeliveredFact, EventTiming, ListenerFact, OutpostToSupervisor, Query, QueryOutcome,
    QueryResult, SupervisorToOutpost, read_message,
};
use verbatim_outpost::{Outpost, OutpostOptions};
use windows::Win32::Foundation::{E_FAIL, HWND};
use windows::Win32::UI::Accessibility::IUIAutomationElement;
use windows::core::AgileReference;

/// The outpost's pipe to Core, as a test's: every message the outpost
/// writes is decoded and handed on as soon as it is written, so a message
/// is in the channel once [`Outpost::settle`] returns.
struct MessageSink {
    pending: Vec<u8>,
    messages: Sender<OutpostToSupervisor>,
}

impl Write for MessageSink {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.pending.extend_from_slice(bytes);
        while let Some(end) = self.pending.iter().position(|&byte| byte == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=end).collect();
            let message = read_message::<_, OutpostToSupervisor>(&mut line.as_slice())?
                .expect("a whole line holds a message");
            // The test may have finished with the outpost's messages.
            let _ = self.messages.send(message);
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The element the outpost under test reads as the one with the keyboard
/// focus, set by the test before it hands the outpost a UIA focus.
pub type FocusSlot = Arc<Mutex<Option<AgileReference<IUIAutomationElement>>>>;

/// A focus the outpost reported, with the calls it made reading it.
pub struct Reported {
    pub node: NodeSnapshot,
    pub ancestors: Vec<NodeSnapshot>,
    pub selected_child: Option<NodeSnapshot>,
    /// The top-level window of the event's window.
    pub window: Option<WindowHandle>,
    pub calls: CallCounts,
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

/// A real outpost in this process, watching mockapp, with its messages.
pub struct OutpostUnderTest {
    pub outpost: Outpost,
    pub messages: Receiver<OutpostToSupervisor>,
    next_request: u64,
    pub focus: FocusSlot,
}

impl OutpostUnderTest {
    /// An outpost watching `pid` with the default options.
    pub fn new(pid: u32) -> Self {
        Self::with_options(pid, OutpostOptions::default())
    }

    /// An outpost watching `pid` as `options` say, which has announced
    /// itself ready, and nothing else.
    pub fn with_options(pid: u32, options: OutpostOptions) -> Self {
        let (messages_tx, messages) = mpsc::channel();
        let sink = MessageSink {
            pending: Vec::new(),
            messages: messages_tx,
        };
        let focus = FocusSlot::default();
        let slot = Arc::clone(&focus);
        // Stands in for `GetFocusedElementBuildCache`: one UIA call, as the
        // system read counts.
        let read_focus: verbatim_outpost::FocusedElementReader = Arc::new(move |_, _| {
            verbatim_uia::calls::count(CallKind::Uia);
            slot.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_ref()
                .ok_or_else(|| windows::core::Error::from(E_FAIL))?
                .resolve()
        });
        let outpost =
            Outpost::with_focused_element_reader(Box::new(sink), pid, options, read_focus);
        let under_test = Self {
            outpost,
            messages,
            next_request: 1,
            focus,
        };
        assert_eq!(
            under_test.next(),
            OutpostToSupervisor::Ready {
                outpost_pid: Pid(std::process::id()),
                target_pid: Pid(pid),
            }
        );
        under_test
    }

    /// The outpost's next message.
    pub fn next(&self) -> OutpostToSupervisor {
        self.messages
            .recv_timeout(super::WAIT_TIMEOUT)
            .expect("the outpost says something before the wait times out")
    }

    /// Waits for the outpost to settle and asserts it said nothing more.
    pub fn settled(&self) {
        self.outpost.settle();
        match self.messages.try_recv() {
            Err(TryRecvError::Empty) => {}
            Ok(message) => panic!("the outpost said more: {message:?}"),
            Err(TryRecvError::Disconnected) => panic!("the outpost's pipe closed"),
        }
    }

    /// Hands the outpost `fact` and returns the focus it reports, which
    /// must be the next thing it says, and the only thing.
    pub fn focus(&self, fact: DeliveredFact) -> Reported {
        self.outpost
            .handle_command(&SupervisorToOutpost::DeliverFact {
                trace_id: TraceId::mint(),
                observed_at_ms: 0,
                timing: EventTiming::default(),
                fact,
            });
        let reported = match self.next() {
            OutpostToSupervisor::Event {
                event:
                    NormalizedEvent::FocusChanged {
                        node,
                        foreground: false,
                        ancestors,
                        ancestors_unknown: false,
                        selected_child,
                    },
                window,
                timing,
                ..
            } => Reported {
                node,
                ancestors,
                selected_child,
                window: window.map(|window| window.top_level),
                calls: timing.calls,
            },
            other => panic!("the outpost said {other:?}, not a focus with its ancestors"),
        };
        self.settled();
        reported
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
    /// focused, as the listener would, and returns the focus it reports.
    /// `element` carries the listener's cache request, and is what the
    /// outpost's read of the focused element answers.
    pub fn uia_focus(&self, element: &IUIAutomationElement) -> Reported {
        let ListenerFact { pid: _, fact } =
            uia_focus_fact(element).expect("mockapp's element has its process");
        *self.focus.lock().unwrap_or_else(PoisonError::into_inner) =
            Some(AgileReference::new(element).expect("an agile reference"));
        self.focus(fact)
    }

    /// Asks the outpost for one object-navigation step, as Core would, and
    /// returns the neighbor and the calls it made; the reply must be the
    /// next thing the outpost says, and the only thing.
    pub fn navigate(&mut self, node_id: NodeId, kind: QueryKind) -> (NodeSnapshot, CallCounts) {
        let request_id = self.next_request;
        self.next_request += 1;
        self.outpost.handle_command(&SupervisorToOutpost::Query {
            trace_id: TraceId::mint(),
            request_id,
            query: Query::Navigate { node_id, kind },
        });
        let answer = match self.next() {
            OutpostToSupervisor::Reply {
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
}
