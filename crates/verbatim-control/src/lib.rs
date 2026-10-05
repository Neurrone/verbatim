//! Control plane (architecture section 10).
//!
//! One protocol fronting Core for dev tooling and, later, remote support:
//! event and speech streams, gesture and key injection, status, latency
//! timelines, lifecycle. The only transport is a named
//! pipe whose owner-only security descriptor, restricting it to the owning
//! interactive user, is the sole access control; remote SMB clients are
//! rejected, and the protocol itself carries no authentication. A Verbatim
//! inside a VM is reached by a client running inside that VM, or through
//! the in-guest agent's control tunnel. Remote support, with its own
//! transport, is not implemented. The E2E harness and `verbatim-inspect`
//! are the first clients (D8).
//!
//! Milestone M1 pulled a minimal protocol v0 forward from M2 so every M1
//! change is verifiable live: [`protocol`] holds the vocabulary,
//! [`server`] the named-pipe server, and [`client`] the client.

pub mod client;
pub mod protocol;
mod send_keys;
pub mod server;
