//! The read scheduler's generated test's configuration, and its
//! exploration test: the gate's generated test under a seed and a case count read
//! from `VERBATIM_EXPLORE_SEED` and `VERBATIM_EXPLORE_CASES`, run only by
//! the nightly workflow (`.github/workflows/explore.yml`). The one file of
//! the crate that reads the environment (`docs/testing.md`, "Exploration tests"); a failure an exploration finds is
//! shrunk and added to the gate as a fixed case, never kept as a seed.

use proptest::test_runner::{Config, RngAlgorithm, RngSeed};

/// A generated test's configuration: the seed and case count given, no
/// failure persistence, no fork and no timeout, every field written out.
pub(super) fn config(seed: u64, cases: u32) -> Config {
    Config {
        cases,
        max_local_rejects: 65_536,
        max_global_rejects: 1024,
        max_flat_map_regens: 1_000_000,
        failure_persistence: None,
        source_file: None,
        test_name: None,
        fork: false,
        timeout: 0,
        max_shrink_time: 0,
        max_shrink_iters: u32::MAX,
        max_default_size_range: 100,
        result_cache: proptest::test_runner::noop_result_cache,
        verbose: 0,
        rng_algorithm: RngAlgorithm::ChaCha,
        rng_seed: RngSeed::Fixed(seed),
        _non_exhaustive: (),
    }
}

/// The configuration the nightly workflow gives, its seed and case count,
/// with the command that runs the exploration with them again.
///
/// # Panics
///
/// When either is missing or not a number.
#[expect(
    clippy::disallowed_methods,
    reason = "the exploration tests alone read their seed and case count from the environment, and the gate never runs them"
)]
fn explored() -> (Config, String) {
    let read = |name: &str| {
        std::env::var(name)
            .unwrap_or_else(|_| panic!("{name} is not set"))
            .parse::<u64>()
            .unwrap_or_else(|_| panic!("{name} is not a number"))
    };
    let seed = read("VERBATIM_EXPLORE_SEED");
    let cases = u32::try_from(read("VERBATIM_EXPLORE_CASES")).expect("a case count that fits");
    let again = format!(
        "$env:VERBATIM_EXPLORE_SEED='{seed}'; $env:VERBATIM_EXPLORE_CASES='{cases}'; cargo test -p verbatim-listener-logic --lib -- --ignored --exact schedule::explore::explore_schedule"
    );
    (config(seed, cases), again)
}

#[test]
#[ignore = "exploration: run by the nightly workflow with its own seed (docs/testing.md, Exploration tests)"]
fn explore_schedule() {
    let (config, again) = explored();
    let _ = super::generated::run_generated(config, &again);
}
