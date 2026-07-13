//! `verbatim.exe` — the composition root (architecture section 1).
//!
//! From M1 this wires the supervisor, reducer, speech pipeline, GUI thread,
//! and input hook together. In M0 it proves the two foundations that must
//! exist before any feature work: structured tracing with trace IDs, and
//! localization for the first user-visible string.

use verbatim_model::TraceId;

fn main() {
    init_tracing();
    let trace_id = TraceId::mint();
    tracing::info!(%trace_id, "verbatim starting");
    println!("{}", verbatim_i18n::startup_message());
}

/// Installs the process-wide tracing subscriber.
///
/// Verbosity follows `RUST_LOG` when set and defaults to `info`. The M1
/// pipeline extends this with the flight-recorder layer so recent spans are
/// captured alongside reducer inputs.
fn init_tracing() {
    use tracing_subscriber::EnvFilter;

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
}
