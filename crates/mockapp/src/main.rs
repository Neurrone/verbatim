//! `mockapp` — the provider-level test host (architecture section 13,
//! layer 2).
//!
//! Implements real UIA provider interfaces (and, in a sibling mode, the same
//! scripted trees as MSAA/IA2 providers) in a separate process, so
//! Verbatim's genuine client stacks and arbitration logic are exercised
//! cross-process — COM marshaling, cache requests, event plumbing — with no
//! real applications and no VM, on plain CI runners.
//!
//! Stub in M0; lands with the test harness in milestone M2.

fn main() {
    // Developer-facing stub, not a user-visible string.
    eprintln!("mockapp is a stub until milestone M2, when the provider test harness lands.");
    std::process::exit(1);
}
