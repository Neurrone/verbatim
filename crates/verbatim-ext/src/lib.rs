//! Wasm extension host (architecture section 7, decision D6).
//!
//! Runs extensions as Wasm components under wasmtime with epoch preemption,
//! on dedicated in-process threads: a runaway extension is interrupted, never
//! hangs Core. Capabilities are deny-by-default and manifest-declared. The
//! runtime stays behind this crate's seam so it remains replaceable; the
//! choice is ratified in milestone M5.
//!
//! Skeleton only in M0.
