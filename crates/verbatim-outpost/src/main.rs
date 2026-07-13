//! `verbatim-outpost.exe` — the per-application outpost process
//! (architecture section 1, decision D9).
//!
//! One outpost per target application: it owns that app's accessibility
//! subscriptions (UIA event registrations, out-of-context `WinEvent` hook),
//! maintains the cached normalized tree fragment, and answers queries, all
//! from its own process so a hung or crashed app never touches Core. This
//! crate also grows the Core-side supervisor (job objects with
//! kill-on-job-close, inherited-handle IPC, the recovery ladder).
//!
//! Stub in M0; the first real outpost lands with milestone M1.

fn main() {
    // Developer-facing stub, not a user-visible string: outposts are spawned
    // by the Core supervisor and have no console of their own.
    eprintln!(
        "verbatim-outpost is a stub until milestone M1; it is spawned by verbatim.exe, not run directly."
    );
    std::process::exit(1);
}
