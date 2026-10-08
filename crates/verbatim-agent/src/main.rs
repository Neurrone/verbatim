//! `verbatim-agent.exe` — the in-guest test agent for the M2 VM harness
//! (see the crate root doc for its role: process management plus a
//! control-plane tunnel, reachable over TCP).
//!
//! At startup, checks that its own window station is interactive and
//! exits with a one-line diagnosis if not: a screen reader test driven
//! from a non-interactive session (the classic "session 0" problem, the
//! way `WinRM` and PowerShell Direct start a process) can never work, so
//! failing fast with a clear reason beats a downstream timeout with no
//! explanation.
//!
//! Output is plain text, one fact per line, matching `verbatim-inspect`'s
//! convention.

use std::net::TcpListener;
use std::process::ExitCode;

use clap::Parser;
use verbatim_agent::protocol::DEFAULT_PORT;

/// In-guest test agent for the M2 VM harness.
#[derive(Parser)]
#[command(
    name = "verbatim-agent",
    about = "In-guest test agent for the M2 VM harness: process management and a control-plane tunnel, reachable over TCP"
)]
struct Cli {
    /// TCP port to listen on.
    #[arg(long, default_value_t = DEFAULT_PORT)]
    port: u16,
    /// Address to bind. Defaults to all interfaces: inside a Hyper-V
    /// guest, the host reaches this agent only through the Default
    /// Switch vNIC, whose guest-side address is not known ahead of time;
    /// on a CI runner, tests connect over loopback, which "all
    /// interfaces" also covers.
    #[arg(long, default_value = "0.0.0.0")]
    bind_address: String,
    /// Overrides the control-plane pipe name this agent tunnels
    /// `OpenControlTunnel` requests to; tests point this at a pipe name
    /// distinct from the well-known one a real, already-running Verbatim
    /// instance uses.
    #[arg(long, default_value_t = verbatim_control::protocol::PIPE_NAME.to_owned())]
    pipe_name: String,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let info = match verbatim_agent::session::current() {
        Ok(info) => info,
        Err(error) => {
            eprintln!("verbatim-agent: could not determine session info: {error}");
            return ExitCode::FAILURE;
        }
    };
    if !info.interactive_window_station {
        eprintln!(
            "verbatim-agent: this process's window station is not interactive (session {}): a screen reader test can never work here. This usually means the agent was started from a non-interactive context such as WinRM or PowerShell Direct (the \"session 0\" problem) rather than the guest's interactive logon session.",
            info.session_id
        );
        return ExitCode::FAILURE;
    }

    let bind = format!("{}:{}", cli.bind_address, cli.port);
    let listener = match TcpListener::bind(&bind) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!(
                "verbatim-agent: failed to bind {bind}: {error}. If nothing else on this machine is actually using port {}, some other policy (security software, a port reservation invisible to netstat and netsh's excluded-range queries) may be blocking it; retry with --port set to a different value.",
                cli.port
            );
            return ExitCode::FAILURE;
        }
    };

    println!("verbatim-agent: listening on {bind}");
    println!("verbatim-agent: session id {}", info.session_id);
    println!(
        "verbatim-agent: input desktop {}",
        info.input_desktop_name.as_deref().unwrap_or("(none)")
    );
    println!(
        "verbatim-agent: tunneling to control pipe {}",
        cli.pipe_name
    );

    verbatim_agent::server::serve(&listener, &cli.pipe_name);
    ExitCode::SUCCESS
}
