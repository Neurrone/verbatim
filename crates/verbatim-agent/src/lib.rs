//! `verbatim-agent`: the in-guest test agent for the M2 VM harness.
//!
//! A Hyper-V guest (or a CI runner reached over loopback) exposes nothing
//! to the host except this agent. It runs inside the interactive user
//! session and does two jobs a host-side E2E test cannot do any other way:
//!
//! - Manages processes on the guest's behalf — launching and killing
//!   Verbatim and target applications such as Notepad — which must happen
//!   inside the interactive session. `WinRM` and PowerShell Direct hand a
//!   process a non-interactive window station (the "session 0" problem),
//!   which can never host a screen reader test.
//! - Tunnels a connection through to Verbatim's own control-plane named
//!   pipe, which deliberately never listens on the network itself
//!   (architecture section 10, decision D8): a Verbatim inside a VM is
//!   driven by a client inside that VM, and this agent is that client's
//!   only way in from the host.
//!
//! [`protocol`] is the wire vocabulary; [`server::serve`] runs the TCP
//! accept loop. The crate is a lib, so tests (here and in the M2 E2E
//! suite) can drive an agent in-process, plus a thin `verbatim-agent.exe`
//! binary for real guest deployment.

mod files;
mod process;
pub mod protocol;
pub mod server;
pub mod session;
mod tunnel;
