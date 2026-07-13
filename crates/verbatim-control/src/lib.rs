//! Control plane (architecture section 10).
//!
//! One authenticated protocol fronting Core for dev tooling and, later,
//! remote support: event and speech streams, tree queries, gesture and input
//! injection, config, lifecycle. Local transport is a named pipe restricted
//! to the owning interactive user; remote is a separate opt-in TLS
//! WebSocket. The E2E harness and verbatim-inspect are the first clients
//! (D8), so the protocol is battle-tested long before the Remote feature.
//!
//! Skeleton only in M0; v0 of the protocol lands with milestone M2.
