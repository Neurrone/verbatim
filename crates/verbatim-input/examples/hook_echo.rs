//! Manual sanity check for the keyboard hook: installs the real
//! `WH_KEYBOARD_LL` hook with a couple of bound gestures and prints each
//! gesture as it fires, then exits after a few seconds.
//!
//! This is a developer aid, not a test: it needs an interactive desktop and is
//! never run in CI. Run it with a timeout and press, for example, caps lock
//! plus V (bound below) to see the emitted gesture.
//!
//! ```text
//! cargo run -p verbatim-input --example hook_echo
//! ```

#[cfg(windows)]
fn main() {
    use std::time::{Duration, Instant};

    use verbatim_input::GestureMap;
    use verbatim_input::hook::InputHook;
    use verbatim_input::state::DecisionConfig;
    use verbatim_model::GestureId;

    let bindings = ["kb:v+verbatim", "kb:t+verbatim"]
        .into_iter()
        .map(|id| GestureId::parse(id).expect("valid gesture identifier"));
    let map = GestureMap::new(bindings).into_shared();

    let (tx, rx) = crossbeam_channel::bounded(64);
    let _hook = InputHook::start(DecisionConfig::default(), map, tx).expect("install hook");

    println!("Keyboard hook installed. Try caps lock + V or caps lock + T. Exiting in 8 seconds.");
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(emitted) => println!("gesture: {} (trace {})", emitted.gesture, emitted.trace_id),
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
    }
    println!("Done; unhooking.");
}

#[cfg(not(windows))]
fn main() {
    eprintln!("hook_echo only runs on Windows");
}
