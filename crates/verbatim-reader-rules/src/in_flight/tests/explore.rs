//! The work in flight's exploration test: the gate's generated test under
//! a seed and a case count read from `VERBATIM_EXPLORE_SEED` and
//! `VERBATIM_EXPLORE_CASES`, run only by the nightly workflow
//! (`.github/workflows/explore.yml`). The one file of the crate that reads
//! the environment (`docs/testing.md`, "Exploration tests"); a failure it finds is shrunk and added to the gate as a fixed
//! case, never kept as a seed.

use super::{config, run_generated};

/// The seed and case count the nightly workflow gives.
///
/// # Panics
///
/// When either is missing or not a number.
#[expect(
    clippy::disallowed_methods,
    reason = "the exploration test alone reads its seed and case count from the environment, and the gate never runs it"
)]
fn explored() -> (u64, u32) {
    let read = |name: &str| {
        std::env::var(name)
            .unwrap_or_else(|_| panic!("{name} is not set"))
            .parse::<u64>()
            .unwrap_or_else(|_| panic!("{name} is not a number"))
    };
    let seed = read("VERBATIM_EXPLORE_SEED");
    let cases = u32::try_from(read("VERBATIM_EXPLORE_CASES")).expect("a case count that fits");
    (seed, cases)
}

#[test]
#[ignore = "exploration: run by the nightly workflow with its own seed (docs/testing.md, Exploration tests)"]
fn explore_in_flight() {
    let (seed, cases) = explored();
    let again = format!(
        "$env:VERBATIM_EXPLORE_SEED='{seed}'; $env:VERBATIM_EXPLORE_CASES='{cases}'; cargo test -p verbatim-reader-rules --lib -- --ignored --exact in_flight::tests::explore::explore_in_flight"
    );
    let _ = run_generated(config(seed, cases), &again);
}
