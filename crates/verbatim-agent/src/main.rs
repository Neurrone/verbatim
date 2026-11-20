//! `verbatim-agent.exe` — the end-to-end suite's test agent (see the
//! crate root doc for its role: process management plus a control-plane
//! tunnel, reachable over TCP).
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

/// The end-to-end suite's test agent.
#[derive(Parser)]
#[command(
    name = "verbatim-agent",
    about = "The end-to-end suite's test agent: process management and a control-plane tunnel, reachable over TCP"
)]
struct Cli {
    /// TCP port to listen on.
    #[arg(long, default_value_t = DEFAULT_PORT)]
    port: u16,
    /// Address to bind, loopback by default. Another address exposes an
    /// agent with no authentication, which injects real input and launches
    /// programs, to that network, so it is given only on a network no one
    /// else shares (`books/dev/src/explanation/architecture.md`, section
    /// 14).
    #[arg(long, default_value = "127.0.0.1")]
    bind_address: String,
    /// Overrides the control-plane pipe name this agent tunnels
    /// `OpenControlTunnel` requests to; tests point this at a pipe name
    /// distinct from the well-known one a real, already-running Verbatim
    /// instance uses.
    #[arg(long, default_value_t = verbatim_control::protocol::PIPE_NAME.to_owned())]
    pipe_name: String,
    /// What to run instead of the agent.
    #[command(subcommand)]
    mode: Option<Mode>,
}

/// What `verbatim-agent.exe` runs other than the agent.
#[derive(clap::Subcommand)]
enum Mode {
    /// Records the desktop into ffmpeg until `q` on standard input
    /// (`verbatim_agent::capture`), exiting with ffmpeg's code.
    CaptureDesktop {
        /// Frames a second.
        #[arg(long)]
        framerate: u32,
        /// The ffmpeg executable.
        #[arg(long)]
        ffmpeg: String,
        /// ffmpeg's arguments for its output, after its raw input.
        #[arg(last = true)]
        output: Vec<String>,
    },
}

fn main() -> ExitCode {
    init_tracing();
    let cli = Cli::parse();
    if let Some(Mode::CaptureDesktop {
        framerate,
        ffmpeg,
        output,
    }) = &cli.mode
    {
        return match verbatim_agent::capture::run(*framerate, ffmpeg, output) {
            Ok(0) => ExitCode::SUCCESS,
            Ok(code) => {
                eprintln!("verbatim-agent capture-desktop: ffmpeg exited with code {code}");
                ExitCode::FAILURE
            }
            Err(error) => {
                eprintln!("verbatim-agent capture-desktop: {error}");
                ExitCode::FAILURE
            }
        };
    }

    let info = match verbatim_agent::session::current() {
        Ok(info) => info,
        Err(error) => {
            eprintln!("verbatim-agent: could not determine session info: {error}");
            return ExitCode::FAILURE;
        }
    };
    if !info.interactive_window_station {
        eprintln!(
            "verbatim-agent: this process's window station is not interactive (session {}): a screen reader test can never work here. This usually means the agent was started from a non-interactive context such as WinRM or PowerShell Direct (the \"session 0\" problem) rather than the machine's interactive logon session.",
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

/// Installs a tracing subscriber writing to standard error, so the agent's
/// own diagnostics (a window the taskbar's Minimize All left restored, a
/// failed request) are kept wherever its standard error goes. Respects
/// `RUST_LOG`, defaulting to `info`. A failed write is dropped rather than
/// reported, which would panic when standard error fails too.
fn init_tracing() {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .log_internal_errors(false)
        .with_writer(std::io::stderr)
        .try_init();
}
