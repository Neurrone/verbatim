//! Wasm extension host (architecture section 7, decision D6).
//!
//! Runs extensions as Wasm components under wasmtime, in a contained host
//! process of its own, so no Wasm runtime is in Core. Capabilities are
//! deny-by-default and derived from a component's imports, never declared.
//! The runtime stays behind this crate's seam so it remains replaceable.
//!
//! Skeleton only in M0.

#![forbid(unsafe_code)]
