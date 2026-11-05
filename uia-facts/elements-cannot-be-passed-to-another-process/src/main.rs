//! Elements cannot be passed to another process: a sender kept in its own
//! process is read live after 31 s and 152 s, P3's pauses
//! (`uia-facts/elements-cannot-be-passed-to-another-process/readme.md`).
//!
//! This process registers the focus event in an event handler group on the
//! provider program's window A, subtree scope, as P3's prototype did, and
//! keeps the sender of A's focus event on node 5. It reads the sender live,
//! its name, keyboard focus and process id, 0.6 s, 31 s and 152 s after
//! the event.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use uia_facts_support::args::{Args, trials};
use uia_facts_support::events::{self, Handler, OnEvent};
use uia_facts_support::provider::Provider;
use uia_facts_support::report::{Report, deterministic, runs};
use uia_facts_support::{note, uia};
use windows::Win32::UI::Accessibility::IUIAutomationElement;

fn main() {
    let args = Args::parse();
    uia::mta();
    let mut report = Report::start("elements-cannot-be-passed-to-another-process");
    if args.wants("kept") {
        report.case("kept", "keeping the sender of a focus event on A's node 5; reading its name, keyboard focus and process id live 0.6, 31 and 152 s later");
        kept(&args, &mut report);
    }
    report.finish();
}

/// The three live reads of P3, their outcomes.
fn reads(sender: &IUIAutomationElement) -> Vec<String> {
    vec![
        uia::outcome(&uia::name(sender)),
        // SAFETY: a live element.
        uia::outcome(&unsafe { sender.CurrentHasKeyboardFocus() }),
        // SAFETY: as above.
        uia::outcome(&unsafe { sender.CurrentProcessId() }),
    ]
}

fn kept(args: &Args, report: &mut Report) {
    let marks = [600u64, 31_000, 152_000];
    let mut outcomes: Vec<Vec<Vec<String>>> = vec![Vec::new(); marks.len()];
    for _ in 0..trials() {
        let mut provider = Provider::launch(&["--windows", "uia:0"]);
        args.pause_for_debugger(&[("provider", provider.pid), ("client", std::process::id())]);
        let client = uia::client();
        let kept: Arc<Mutex<Option<uia::Shared>>> = Arc::new(Mutex::new(None));
        let keep = kept.clone();
        let on_event: OnEvent = Box::new(move |sender: &IUIAutomationElement| {
            let mut slot = keep.lock().expect("a lock no one poisons");
            if slot.is_none() {
                *slot = Some(uia::share(sender));
            }
        });
        let (handler, reports) = Handler::calling(Some(on_event));
        let group = events::focus_group(&client, &handler).expect("a focus group");
        let window = uia::element(&client, provider.hwnd(0)).expect("A's element");
        events::add_group(&client, &window, &group).expect("the group registered");
        provider.request("focus 0 5");
        provider.request("raise focus 0 5");
        let heard = reports.recv_timeout(Duration::from_secs(5));
        assert!(heard.is_ok(), "the focus event is heard");
        let event = Instant::now();
        let sender = uia::resolve(
            kept.lock()
                .expect("a lock no one poisons")
                .as_ref()
                .expect("the sender is kept"),
        );
        for (i, mark) in marks.iter().enumerate() {
            uia::pause(
                Duration::from_millis(*mark).saturating_sub(event.elapsed()),
                "the sender kept, before its next live read",
            );
            let read = reads(&sender);
            note::evidence(&format!(
                "{:.1} s after the event: {read:?}",
                event.elapsed().as_secs_f64()
            ));
            outcomes[i].push(read);
        }
    }
    let ok = vec!["ok".to_owned(); 3];
    for (i, mark) in marks.iter().enumerate() {
        report.claim(
            &format!("the kept sender's name, keyboard focus and process id read live {:.1} s after its event", f64::from(u32::try_from(*mark).expect("a short time")) / 1_000.0),
            &runs(&outcomes[i]),
            "ok, each in 0.2 to 1.3 ms (P3, at 0.6, 31 and 152 s)",
            deterministic(&outcomes[i], &ok),
        );
    }
}
