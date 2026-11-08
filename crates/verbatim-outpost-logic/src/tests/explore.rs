//! The outpost's generated tests' configuration, and their exploration
//! tests: the gate's generated tests under a seed and a case count read
//! from `VERBATIM_EXPLORE_SEED` and `VERBATIM_EXPLORE_CASES`, run only by
//! the nightly workflow (`.github/workflows/explore.yml`). The one file of
//! the crate that reads the environment (`books/dev/src/reference/testing.md`, "Exploration tests"); a failure an exploration finds is
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

/// The configuration the nightly workflow gives: its seed and case count.
///
/// # Panics
///
/// When either is missing or not a number.
#[expect(
    clippy::disallowed_methods,
    reason = "the exploration tests alone read their seed and case count from the environment, and the gate never runs them"
)]
fn explored(test: &str) -> (Config, String) {
    let read = |name: &str| {
        std::env::var(name)
            .unwrap_or_else(|_| panic!("{name} is not set"))
            .parse::<u64>()
            .unwrap_or_else(|_| panic!("{name} is not a number"))
    };
    let seed = read("VERBATIM_EXPLORE_SEED");
    let cases = u32::try_from(read("VERBATIM_EXPLORE_CASES")).expect("a case count that fits");
    let again = format!(
        "$env:VERBATIM_EXPLORE_SEED='{seed}'; $env:VERBATIM_EXPLORE_CASES='{cases}'; cargo test -p verbatim-outpost-logic --lib -- --ignored --exact tests::explore::{test}"
    );
    (config(seed, cases), again)
}

#[test]
#[ignore = "exploration: run by the nightly workflow with its own seed (books/dev/src/reference/testing.md, Exploration tests)"]
fn explore_reducer() {
    let (config, again) = explored("explore_reducer");
    let _ = super::reducer::run_generated(config, &again);
}

#[test]
#[ignore = "exploration: run by the nightly workflow with its own seed (books/dev/src/reference/testing.md, Exploration tests)"]
fn explore_followed() {
    let (config, again) = explored("explore_followed");
    super::followed::run_generated(config, &again);
}

#[test]
#[ignore = "exploration: run by the nightly workflow with its own seed (books/dev/src/reference/testing.md, Exploration tests)"]
fn explore_generated() {
    let (config, again) = explored("explore_generated");
    super::generated::run_generated(config, &again);
}

#[test]
#[ignore = "exploration: run by the nightly workflow with its own seed (books/dev/src/reference/testing.md, Exploration tests)"]
fn explore_intake() {
    let (config, again) = explored("explore_intake");
    let _ = super::intake_order::run_generated(config, &again);
}
