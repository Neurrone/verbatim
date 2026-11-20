//! `verbatim-agent`: the end-to-end suite's test agent.
//!
//! The suite drives the machine Verbatim runs on, this one over loopback
//! or another one (`books/dev/src/explanation/architecture.md`, section
//! 14), only through this agent. It runs inside the interactive user
//! session and does jobs a host-side E2E test cannot do any other way:
//!
//! - Manages processes on the suite's behalf — launching and killing
//!   Verbatim and target applications such as Notepad — which must happen
//!   inside the interactive session. `WinRM` and PowerShell Direct hand a
//!   process a non-interactive window station (the "session 0" problem),
//!   which can never host a screen reader test.
//! - Lets a launched application take the foreground as a user's launch
//!   would, without injecting anything, and waits for evidence on the
//!   desktop (windows appearing, closing, taking the foreground; processes
//!   exiting; files appearing; events set) rather than for time.
//! - Records the desktop for the scenarios' videos, as `verbatim-agent
//!   capture-desktop` ([`capture`]), which the agent launches as it launches
//!   any program.
//! - Numbers every key it injects, so Verbatim can say when it has handled
//!   it (`verbatim_input::harness`).
//! - Tunnels a connection through to Verbatim's own control-plane named
//!   pipe, which deliberately never listens on the network itself
//!   (architecture section 10, decision D8): a Verbatim on another
//!   machine is driven by a client on that machine, and this agent is that
//!   client's only way in from the suite.
//!
//! [`protocol`] is the wire vocabulary; [`server::serve`] runs the TCP
//! accept loop. The crate is a lib, so tests (here and in the M2 E2E
//! suite) can drive an agent in-process, plus a thin `verbatim-agent.exe`
//! binary that the suite's runs start.

pub mod capture;
mod clock;
pub mod desktop;
mod diagnostics;
mod events;
mod files;
mod focus;
mod jobs;
mod keys;
mod pace;
mod process;
pub mod protocol;
mod scenario_windows;
pub mod server;
pub mod session;
mod terminals;
mod tunnel;
mod typing;
mod wait;
mod wait_chain;
