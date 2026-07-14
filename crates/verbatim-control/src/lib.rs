//! Control plane (architecture section 10).
//!
//! One authenticated protocol fronting Core for dev tooling and, later,
//! remote support: event and speech streams, gesture and key injection,
//! status, latency timelines, lifecycle. Local transport is a named pipe
//! restricted to the owning interactive user with remote SMB clients
//! rejected (a Verbatim inside a VM is reached by a client running inside
//! that VM; remote support is a separate opt-in TLS transport). The E2E
//! harness and `verbatim-inspect` are the first clients (D8).
//!
//! Milestone M1 pulls a minimal protocol v0 forward from M2 so every M1
//! change is verifiable live: this module freezes the vocabulary; the
//! server lands with workstream WS-E.

pub mod client;
pub mod protocol;
mod send_keys;
pub mod server;
