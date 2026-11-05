//! What a run prints besides its claims (`uia-facts/readme.md`, "Output
//! and verdicts"): the line naming each deliberate wait of a second or
//! more, the evidence a claim judged, printed when its verdict is differ or
//! inconclusive, and under `--verbose` each traced UIA call, the provider's
//! logs and each trial's figures as they come.
//!
//! A second client process ([`crate::role`]) writes these to its standard
//! error, which it shares with the run, since its standard output carries
//! its results.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

/// `--verbose`.
static VERBOSE: AtomicBool = AtomicBool::new(false);
/// Whether this process is a second client process.
static ROLE: AtomicBool = AtomicBool::new(false);
/// The current case's evidence, oldest first.
static EVIDENCE: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// The most evidence lines printed under one claim; the oldest beyond it
/// are counted, not printed.
const MOST_PRINTED: usize = 200;

/// Turns `--verbose` on.
pub fn set_verbose() {
    VERBOSE.store(true, Ordering::SeqCst);
}

/// Whether `--verbose` is on.
#[must_use]
pub fn verbose() -> bool {
    VERBOSE.load(Ordering::SeqCst)
}

/// Marks this process as a second client process.
pub fn set_role() {
    ROLE.store(true, Ordering::SeqCst);
}

/// Whether this process is a second client process.
#[must_use]
pub fn in_role() -> bool {
    ROLE.load(Ordering::SeqCst)
}

/// Prints one line where this process's notes go.
pub fn say(line: &str) {
    if ROLE.load(Ordering::SeqCst) {
        eprintln!("{line}");
    } else {
        println!("{line}");
    }
}

/// Prints the line naming a deliberate wait, before it starts.
pub fn waiting(what: &str) {
    say(&format!("waiting: {what}"));
}

/// Prints a progress line within a long case.
pub fn progress(what: &str) {
    say(&format!("progress: {what}"));
}

/// Prints the line naming a wait on a timeout of `ms` milliseconds, when
/// it is a second or more: `what` says which.
pub fn waiting_ms(ms: u32, what: &str) {
    if ms >= 1_000 {
        waiting(&format!("up to {:.1} s, {what}", f64::from(ms) / 1_000.0));
    }
}

/// Keeps one line of the current case's evidence, a trial's figures or a
/// call's outcome, and prints it at once under `--verbose`.
pub fn evidence(line: &str) {
    if verbose() {
        say(&format!("  trial: {line}"));
    }
    EVIDENCE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(line.to_owned());
}

/// Prints `line` under `--verbose` only.
pub fn trace(line: &str) {
    if verbose() {
        say(&format!("  {line}"));
    }
}

/// Forgets the evidence kept so far, at a case's start.
pub fn clear() {
    EVIDENCE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clear();
}

/// Prints the evidence kept since the case's start, under a claim whose
/// verdict is differ or inconclusive.
pub fn print_evidence() {
    let evidence = EVIDENCE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    if evidence.is_empty() {
        say("  evidence: none kept");
        return;
    }
    let skipped = evidence.len().saturating_sub(MOST_PRINTED);
    if skipped > 0 {
        say(&format!("  evidence: {skipped} earlier lines left out"));
    }
    for line in &evidence[skipped..] {
        say(&format!("  evidence: {line}"));
    }
}
