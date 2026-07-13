//! `verbatim-inspect` — developer tooling over the control plane
//! (architecture section 9).
//!
//! Live event stream, tree dumps, gesture injection, speech capture, and
//! latency histograms against a running Verbatim instance. Deliberately not
//! a child of Core and not in any job object; it attaches through the named
//! pipe like any control-plane client.
//!
//! Stub in M0; lands with the control plane in milestone M2.

fn main() {
    // Developer-facing stub, not a user-visible string.
    eprintln!("verbatim-inspect is a stub until milestone M2, when the control plane lands.");
    std::process::exit(1);
}
