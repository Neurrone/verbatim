//! Capture synth (architecture section 13, layer 3).
//!
//! A synth driver that records utterances and timestamps instead of
//! producing audio, so pipeline tests and the E2E harness can assert on
//! what would have been spoken and on latency against the budget.
//!
//! Skeleton only in M0; lands with the pipeline tests in milestone M2.
