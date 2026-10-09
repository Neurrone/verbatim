//! Agent protocol v0.
//!
//! Newline-delimited compact JSON over TCP, framed with
//! [`verbatim_control::protocol::write_message`] and
//! [`verbatim_control::protocol::read_message`] — the same wire convention
//! the control plane uses, reused rather than reinvented. The envelope
//! style mirrors the control protocol too: every [`Request`] carries a
//! client-chosen correlation id in a [`RequestEnvelope`], and every
//! [`Frame`] answering one echoes that id.
//!
//! This is a deliberately separate vocabulary from
//! [`verbatim_control::protocol`]: the agent's pids are raw OS process ids
//! for test process management (Notepad, `verbatim.exe` itself), not
//! [`verbatim_model::Pid`]s naming an *observed* application in the
//! accessibility domain the control plane speaks about. Keeping the two
//! protocols and their pid types apart avoids conflating "the process this
//! test is driving" with "the process Verbatim is watching".

use serde::{Deserialize, Serialize};

/// The protocol version this vocabulary defines. Version 2 added
/// [`Request::ListFiles`]; version 3 added [`Request::ForegroundInfo`],
/// [`Request::CloseWindows`], [`Request::WriteFile`], and
/// `BringToForeground`'s title filter; version 4 added
/// [`Request::ReadFileChunk`]; version 5 added [`Request::SendKeys`];
/// version 6 added [`Request::TypeText`]; version 7 added
/// [`Request::DeleteFile`]; version 8 added [`Request::ListFolders`] and
/// [`Request::DeleteFolder`]; version 9 removed killing by image name and
/// forcing a window to the foreground, and added the evidence waits
/// ([`Request::WaitForWindow`], [`Request::WaitForExit`],
/// [`Request::WaitForFile`], [`Request::WaitForEvent`]),
/// [`Request::SetForeground`], [`Request::ChildProcesses`],
/// [`Request::EndLaunched`], and [`Request::CreateEvent`]; version 11
/// added `ignore_foreign_terminals` to [`Request::LaunchProcess`] and
/// `ignored` to its answer, so the owner's own Windows Terminal is never
/// read by the Verbatim under test. A test run against an older agent is
/// refused at `Hello` instead of losing its connection mid-run, or running
/// without the exclusion.
pub const AGENT_PROTOCOL_VERSION: u32 = 11;

/// The environment variable that names, to the Verbatim under test, the
/// processes it ignores entirely (`ignore_foreign_terminals` in
/// [`Request::LaunchProcess`]): their pids, separated by commas. The same
/// name as `verbatim_outpost::supervisor::IGNORE_PIDS_ENV`, which this crate
/// does not depend on.
pub const IGNORE_PIDS_ENV: &str = "VERBATIM_IGNORE_PIDS";

/// The default TCP port the agent listens on.
///
/// Deliberately not a port in the 47000s: on a real development machine, a
/// whole band around 47600 (47600, 47601, 47650, 47712, 47800, 47900 all
/// tried) refused every bind attempt with "address already in use", while
/// nothing owned any of those ports per `Get-NetTCPConnection` and none of
/// them appear in `netsh interface ipv4/ipv6 show excludedportrange` — an
/// invisible reservation, almost certainly security software, that
/// standard tooling cannot see or explain. Since this protocol's whole
/// purpose is talking to unpredictable Hyper-V guests and CI runners,
/// picking a default outside that band (44001, empirically clear on the
/// same machine) is cheaper than debugging invisible port policy on every
/// machine this ever runs on. Override with `--port` when even this one is
/// unavailable.
pub const DEFAULT_PORT: u16 = 44001;

/// One client request with its correlation id.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RequestEnvelope {
    /// Client-chosen id echoed in the matching reply or error.
    pub id: u64,
    /// The request.
    pub request: Request,
}

/// A client request.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Request {
    /// Must be the first request on a connection; agrees on a protocol
    /// version. Refused on mismatch, mirroring the control server's rule.
    Hello {
        /// The highest protocol version the client speaks.
        protocol_version: u32,
    },
    /// Spawns a process via `std::process::Command`, inheriting the
    /// agent's own interactive session — the reason this request exists at
    /// all rather than something `WinRM` or PowerShell Direct could do.
    /// Stdio is inherited (not captured) unless `stderr_to` is set.
    LaunchProcess {
        /// The executable to run.
        command: String,
        /// Arguments, in order.
        args: Vec<String>,
        /// Working directory; `None` inherits the agent's.
        working_dir: Option<String>,
        /// Additional environment variables, added to (not replacing) the
        /// agent's own environment.
        env: Vec<(String, String)>,
        /// When set, a path the agent creates (truncating any existing
        /// file) and redirects the child's stdout and stderr into, so a
        /// panic message that would otherwise vanish with the process is
        /// captured for a later `ReadFile` (or `cargo xtask vm logs`) pull.
        /// `None` (the default on the wire, via `serde(default)`, so an
        /// older client omitting this field still deserializes) inherits
        /// the agent's own stdio exactly as before.
        #[serde(default)]
        stderr_to: Option<String>,
        /// When set, the title of the console window a console program
        /// opens, from its first frame: a console host started directly
        /// otherwise shows its own path until the shell sets a title. When
        /// not set, a console program is started with no console window
        /// (`CREATE_NO_WINDOW`), so Windows never hands its console to the
        /// default terminal application; only a console host started
        /// explicitly, for a scenario that drives its window, is given one.
        #[serde(default)]
        console_title: Option<String>,
        /// Whether the program's first window opens minimized and
        /// inactive, for a caller that brings it forward once it is ready.
        #[serde(default)]
        minimized: bool,
        /// For a launch of Verbatim: whether to name, in its
        /// `VERBATIM_IGNORE_PIDS`, every `WindowsTerminal.exe` process the
        /// agent did not launch, and the `OpenConsole.exe` processes they
        /// host, so the Verbatim under test never reads the owner's own
        /// Windows Terminal. The agent holds those processes open for as
        /// long as it keeps the launch, so their pids name no other
        /// process meanwhile. The portable Windows Terminal a scenario
        /// launches is in the agent's jobs, and is not named.
        #[serde(default)]
        ignore_foreign_terminals: bool,
    },
    /// Terminates a process by pid.
    KillProcess {
        /// The OS process id, as returned by a prior
        /// [`ReplyPayload::Launched`].
        pid: u32,
    },
    /// Terminates every process this agent launched that is still running,
    /// with everything each one started (its job object), and reports how
    /// many were running: the pre-launch sweep a test runs, so a process an
    /// aborted earlier run left behind is ended by its own handle, never by
    /// its image name. Answered by [`ReplyPayload::EndedLaunched`].
    EndLaunched,
    /// Lists the processes whose parent is `pid`, such as the synthesizer
    /// host a Verbatim started, so a test can act on one process of its own
    /// rather than every process of that name. Answered by
    /// [`ReplyPayload::Processes`].
    ChildProcesses {
        /// The parent's OS process id.
        pid: u32,
    },
    /// Brings the window `window` names to the foreground as clicking its
    /// taskbar button does, injecting no input: a window not already in
    /// front is restored from minimized, after being minimized first when
    /// it was not, and then set as the foreground with
    /// `SetForegroundWindow`, which Windows allows for a window it has just
    /// restored whatever input came last. Answered by
    /// [`ReplyPayload::Foreground`] once Windows reports the window in
    /// front, or once a bound well within the client's read timeout passes
    /// without it.
    SetForeground {
        /// The window's handle, as [`WindowInfo::window`] reports it.
        window: u64,
    },
    /// Waits up to `timeout_ms` for `condition` to hold, checking it again
    /// each time Windows reports a top-level window shown, hidden, created,
    /// destroyed, renamed, cloaked, or brought to the foreground: evidence,
    /// never a poll. Answered by [`ReplyPayload::WindowState`].
    WaitForWindow {
        /// What to wait for.
        condition: WindowCondition,
        /// How long to wait, in milliseconds.
        timeout_ms: u64,
    },
    /// Minimizes every window as the taskbar's Show Desktop command does,
    /// the state every end-to-end scenario starts from, and waits up to
    /// `timeout_ms` for every window that can be minimized to be. Answered
    /// by [`ReplyPayload::WindowState`].
    MinimizeAll {
        /// How long to wait, in milliseconds.
        timeout_ms: u64,
    },
    /// Reports the focused element as UI Automation sees it, read
    /// independently of any screen reader: state a test fixes its
    /// expectation from, such as the desktop's focused item. Answered by
    /// [`ReplyPayload::Focused`].
    FocusedElement,
    /// Focuses the first element of the foreground window whose UI
    /// Automation identifier is `automation_id`, with UI Automation's
    /// `SetFocus`, injecting no input, as a script would, and reports how
    /// many children it has. Answered by [`ReplyPayload::Children`].
    FocusByAutomationId {
        /// The element's UI Automation identifier.
        automation_id: String,
    },
    /// Reports the words of the focused element's text that its
    /// application marks as misspelt, with UI Automation's spelling-error
    /// annotation, in order, read independently of any screen reader.
    /// Answered by [`ReplyPayload::Words`].
    MisspeltWords,
    /// Reports whether the lock key `key` (a key name of the control
    /// plane's vocabulary, such as `scrolllock`) is on, read independently
    /// of any screen reader. Answered by [`ReplyPayload::KeyToggled`].
    KeyToggled {
        /// The key's name.
        key: String,
    },
    /// Lists the processes that have exited in the job of the process `pid`
    /// the agent launched (every process it started, and they started), in
    /// the order they exited: how a test finds that one of Verbatim's own
    /// processes ended while it ran. Answered by [`ReplyPayload::Exits`].
    JobExits {
        /// The launched process's OS pid.
        pid: u32,
    },
    /// Waits up to `timeout_ms` for every process in the job of the process
    /// `pid` the agent launched to have exited, on the job's own messages,
    /// and lists every exit in the job in the order they happened: how a
    /// test checks that Verbatim left none of its processes behind, and how
    /// each one ended. Answered by [`ReplyPayload::Exits`]; fails, naming
    /// the processes still running, when the wait runs out.
    WaitForJobEmpty {
        /// The launched process's OS pid.
        pid: u32,
        /// How long to wait, in milliseconds.
        timeout_ms: u64,
    },
    /// Reports, and forgets, every Windows Terminal window shown since the
    /// last such request by a process outside the jobs of every process the
    /// agent launched: a console handed to the default terminal
    /// application, or anything else reaching the user's own Windows
    /// Terminal. The first request starts the watch and reports nothing.
    /// Answered by [`ReplyPayload::TerminalWindows`].
    TakeForeignTerminalWindows,
    /// Waits up to `timeout_ms` for process `pid` to exit, on its process
    /// handle. Answered by [`ReplyPayload::ProcessStatus`]: still running
    /// when the wait ran out.
    WaitForExit {
        /// The OS process id.
        pid: u32,
        /// How long to wait, in milliseconds.
        timeout_ms: u64,
    },
    /// Waits up to `timeout_ms` for a file to exist, checking again each
    /// time Windows reports a change in its folder. Answered by
    /// [`ReplyPayload::FileExists`].
    WaitForFile {
        /// Path to the file, agent-local.
        path: String,
        /// How long to wait, in milliseconds.
        timeout_ms: u64,
    },
    /// Creates a named, manual-reset event, not yet set, which a process
    /// the agent launches later can open by name and set when it has
    /// reached a point a test waits for, such as Verbatim being ready for
    /// input. The agent keeps it until [`Request::WaitForEvent`] has waited
    /// on it. Answered by [`ReplyPayload::EventCreated`].
    CreateEvent {
        /// The event's name, such as `Local\verbatim-e2e-ready-1`.
        name: String,
    },
    /// Waits up to `timeout_ms` for the event [`Request::CreateEvent`]
    /// made to be set, or for process `pid` to exit first, and then
    /// forgets the event. Answered by [`ReplyPayload::EventWait`].
    WaitForEvent {
        /// The event's name.
        name: String,
        /// The process expected to set it.
        pid: u32,
        /// How long to wait, in milliseconds.
        timeout_ms: u64,
    },
    /// Reports the foreground window and the visible top-level windows, so
    /// a test can check the state it starts from and say what held the
    /// foreground when an assertion fails. Answered by
    /// [`ReplyPayload::ForegroundInfo`].
    ForegroundInfo,
    /// Sends a close request to every visible top-level window whose title
    /// contains `title_contains`, and waits up to `timeout_ms` for them to
    /// go. Answered by [`ReplyPayload::WindowsClosed`].
    CloseWindows {
        /// Text the windows' titles contain.
        title_contains: String,
        /// How long to wait for them to close, in milliseconds.
        timeout_ms: u64,
    },
    /// Writes a small file, creating or replacing it and any missing parent
    /// directories: a test's own document for an application to open, or a
    /// folder of files, named so its window can be told apart.
    /// Answered by [`ReplyPayload::FileWritten`].
    WriteFile {
        /// Path to the file, agent-local.
        path: String,
        /// The contents, base64 encoded.
        data_base64: String,
    },
    /// Deletes a file a test wrote with [`Request::WriteFile`], such as a
    /// harness document once its window has closed. A file that does not
    /// exist is not an error. Answered by [`ReplyPayload::FileDeleted`].
    DeleteFile {
        /// Path to the file, agent-local.
        path: String,
    },
    /// Deletes a folder a test laid out with [`Request::WriteFile`], with
    /// everything in it, such as a harness folder once the window that used
    /// it has closed. A folder that does not exist is not an error.
    /// Answered by [`ReplyPayload::FolderDeleted`].
    DeleteFolder {
        /// Path to the folder, agent-local.
        path: String,
    },
    /// Asks whether a process is still running.
    ProcessStatus {
        /// The OS process id.
        pid: u32,
    },
    /// Asks for the agent's own session diagnostics: session id, whether
    /// its window station is interactive, and the input desktop's name
    /// when it can be opened. Exists so a screen reader test that can
    /// never work in a non-interactive session (the classic session 0
    /// problem) fails with a diagnosis instead of a mystery.
    SessionInfo,
    /// Reads a small file's contents, base64-encoded — logs, dumps. Refused
    /// above a size limit documented on [`ReplyPayload::FileContents`].
    ReadFile {
        /// Path to the file, agent-local.
        path: String,
    },
    /// Reads part of a file of any size, base64-encoded: at most
    /// `MAX_READ_FILE_BYTES` (see the agent's `files` module) from `offset`,
    /// answered by [`ReplyPayload::FileContents`], empty past the end. For
    /// files too large for [`Request::ReadFile`], such as recordings.
    ReadFileChunk {
        /// Path to the file, agent-local.
        path: String,
        /// Where in the file to start reading.
        offset: u64,
    },
    /// Lists the names of the files directly inside a directory, so a test
    /// can fetch logs whose names it cannot know in advance. Answered by
    /// [`ReplyPayload::FileNames`].
    ListFiles {
        /// Path to the directory, agent-local.
        path: String,
    },
    /// Lists the names of the folders directly inside a directory, so a
    /// test can find the folders an earlier run left behind. Answered by
    /// [`ReplyPayload::FileNames`], holding the folders' names.
    ListFolders {
        /// Path to the directory, agent-local.
        path: String,
    },
    /// Synthesizes real OS keyboard input with `SendInput`, in the key-name
    /// vocabulary of the control plane's `SendKeys`, whose parser and
    /// injection this reuses. Exists so a screen reader other than Verbatim
    /// (NVDA, for a transcript) can be driven with no Verbatim running.
    /// Every name is validated before any key is sent. Answered by
    /// [`ReplyPayload::KeysSent`].
    SendKeys {
        /// Key strokes in order, each a plus-joined combination such as
        /// `shift+tab`.
        keys: Vec<String>,
    },
    /// Types `text` as real key presses with `SendInput`: each character is
    /// mapped to its virtual key and Shift, Control, and Alt state in the
    /// keyboard layout of the foreground window's thread (`VkKeyScanEx`),
    /// so a keyboard hook sees ordinary typing. A character that layout
    /// cannot type, or a control character such as a line break (named
    /// keys go through [`Request::SendKeys`]), fails the request before any
    /// key is sent. Answered by [`ReplyPayload::TextTyped`].
    TypeText {
        /// The text to type.
        text: String,
    },
    /// Asks the agent to stop speaking this protocol on this connection and
    /// instead relay raw bytes to and from Verbatim's control-plane named
    /// pipe. After the reply to this request, the connection is a raw
    /// tunnel: the caller must switch to the control protocol's own
    /// framing immediately, starting with its own `Hello`.
    OpenControlTunnel,
}

/// One server-to-client frame.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Frame {
    /// Successful answer to a request.
    Reply {
        /// The request id this answers.
        to: u64,
        /// The payload.
        payload: ReplyPayload,
    },
    /// Failed answer to a request.
    Error {
        /// The request id this answers.
        to: u64,
        /// Human-readable reason.
        message: String,
    },
}

/// Payload of a [`Frame::Reply`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ReplyPayload {
    /// Answer to [`Request::Hello`]: the version the agent will speak.
    Hello {
        /// Agreed protocol version.
        protocol_version: u32,
    },
    /// Answer to [`Request::LaunchProcess`].
    Launched {
        /// The spawned process's OS pid.
        pid: u32,
        /// Whether Windows let the agent allow the process to take the
        /// foreground with its first window, as a program a user starts
        /// may (`AllowSetForegroundWindow`). It does when the agent may set
        /// the foreground itself, such as when it injected the last input.
        foreground_allowed: bool,
        /// The pids named to the process as ones to ignore
        /// (`ignore_foreign_terminals`), empty otherwise.
        #[serde(default)]
        ignored: Vec<u32>,
    },
    /// Answer to [`Request::KillProcess`].
    Killed(KillOutcome),
    /// Answer to [`Request::EndLaunched`]: how many launched processes were
    /// still running and were ended.
    EndedLaunched {
        /// Count of processes ended.
        ended: u32,
    },
    /// Answer to [`Request::ChildProcesses`].
    Processes {
        /// The processes, in the order Windows lists them.
        processes: Vec<ProcessInfo>,
    },
    /// Answer to [`Request::WaitForWindow`]: whether the condition held when
    /// the wait ended, and the desktop as it was then.
    WindowState {
        /// Whether the condition held.
        met: bool,
        /// The foreground window and the visible windows.
        desktop: ForegroundInfo,
    },
    /// Answer to [`Request::WaitForFile`]: whether the file exists.
    FileExists {
        /// Whether the file existed when the wait ended.
        exists: bool,
    },
    /// Answer to [`Request::CreateEvent`].
    EventCreated,
    /// Answer to [`Request::FocusedElement`].
    Focused(FocusedElement),
    /// Answer to [`Request::FocusByAutomationId`].
    Children {
        /// How many children the focused element has.
        count: u32,
    },
    /// Answer to [`Request::KeyToggled`].
    KeyToggled {
        /// Whether the key is on.
        on: bool,
    },
    /// Answer to [`Request::MisspeltWords`].
    Words {
        /// The words, as the text range of each reads, white space after
        /// it trimmed.
        words: Vec<String>,
    },
    /// Answer to [`Request::TakeForeignTerminalWindows`].
    TerminalWindows {
        /// The windows shown, in the order they were shown.
        windows: Vec<WindowInfo>,
    },
    /// Answer to [`Request::JobExits`] and [`Request::WaitForJobEmpty`].
    Exits {
        /// The processes that exited, oldest first.
        exits: Vec<ProcessExit>,
    },
    /// Answer to [`Request::WaitForEvent`].
    EventWait(EventOutcome),
    /// Answer to [`Request::ProcessStatus`].
    ProcessStatus(ProcessState),
    /// Answer to [`Request::SetForeground`]: whether the window is the
    /// foreground window.
    Foreground {
        /// `false` when Windows refused.
        taken: bool,
    },
    /// Answer to [`Request::SessionInfo`].
    SessionInfo(SessionInfo),
    /// Answer to [`Request::ReadFile`]: the file's raw bytes, base64
    /// encoded. Files larger than a few megabytes are refused as a
    /// [`Frame::Error`] instead (see the agent's `files` module for the
    /// exact cap) — this request is for logs and small dumps, not bulk
    /// transfer.
    FileContents {
        /// The file's contents, base64 encoded (standard alphabet, with
        /// padding).
        data_base64: String,
    },
    /// Answer to [`Request::ForegroundInfo`].
    ForegroundInfo(ForegroundInfo),
    /// Answer to [`Request::CloseWindows`]: how many matching windows were
    /// still open when the wait ended; zero means every one closed.
    WindowsClosed {
        /// Windows still open.
        remaining: u32,
    },
    /// Answer to [`Request::WriteFile`].
    FileWritten,
    /// Answer to [`Request::DeleteFile`].
    FileDeleted,
    /// Answer to [`Request::DeleteFolder`].
    FolderDeleted,
    /// Answer to [`Request::ListFiles`]: the names of the files directly
    /// inside the directory, sorted; subdirectories are left out. Answer to
    /// [`Request::ListFolders`] too, holding the subdirectories' names
    /// instead, sorted.
    FileNames {
        /// The file names, without the directory.
        names: Vec<String>,
    },
    /// Answer to [`Request::SendKeys`]: every key was injected.
    KeysSent {
        /// The number the last key stroke carried
        /// (`verbatim_input::harness`), which Verbatim reports once it has
        /// handled it.
        input: u64,
    },
    /// Answer to [`Request::TypeText`]: every character was typed.
    TextTyped {
        /// The number the last character's key events carried.
        input: u64,
    },
    /// Answer to [`Request::OpenControlTunnel`]: the agent successfully
    /// opened Verbatim's control-plane pipe and is ready to relay bytes.
    /// A failure to open that pipe is reported as a [`Frame::Error`]
    /// instead, before any tunneling begins.
    TunnelReady,
}

/// Outcome of [`Request::KillProcess`], distinguishing "terminated it" from
/// "it was already gone" — both success, deterministically reported rather
/// than folding the second case into an error.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum KillOutcome {
    /// The process was running and was terminated.
    Terminated,
    /// The process had already exited; there was nothing to terminate.
    AlreadyExited,
}

/// Answer to [`Request::ProcessStatus`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProcessState {
    /// The process is still running.
    Running,
    /// The process has exited, with its exit code when it could be read.
    Exited {
        /// The process's exit code, when the OS reported one.
        exit_code: Option<i32>,
    },
}

/// Answer to [`Request::SessionInfo`]: diagnostics about the agent's own
/// process, since a screen reader driven from a non-interactive session
/// (session 0, or a non-interactive window station) can never work.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionInfo {
    /// The Windows session id the agent process is running in
    /// (`ProcessIdToSessionId`).
    pub session_id: u32,
    /// Whether the agent's process window station is interactive
    /// (`GetProcessWindowStation` plus the `WSF_VISIBLE` flag from
    /// `GetUserObjectInformationW`).
    pub interactive_window_station: bool,
    /// The name of the current input desktop, when `OpenInputDesktop`
    /// succeeds. `None` when it cannot be opened, which itself is
    /// diagnostic: a non-interactive window station has no input desktop.
    pub input_desktop_name: Option<String>,
}

/// A top-level window, as [`ForegroundInfo`] reports it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowInfo {
    /// The window's handle, for [`Request::SetForeground`].
    pub window: u64,
    /// The OS process id of the process that owns it.
    pub pid: u32,
    /// The window's title.
    pub title: String,
    /// The window's class name.
    pub class: String,
    /// The executable file name of the process that owns it.
    pub image: String,
    /// Whether the window is cloaked (DWM hides it although it may hold the
    /// foreground, as the Start menu's search window can after it closes).
    pub cloaked: bool,
    /// Whether the window is minimized.
    pub minimized: bool,
    /// Whether Windows judges the window not responding: its thread has
    /// not taken a message for five seconds (`IsHungAppWindow`). Keys sent
    /// to it wait unread, and go to whichever window is in front when they
    /// are finally read, or when it closes.
    #[serde(default)]
    pub hung: bool,
}

/// What [`Request::WaitForWindow`] waits for. Titles are matched as
/// containing the given text, the way a test names its own windows with a
/// marker of its run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WindowCondition {
    /// The foreground window is titled with `title_contains`, is not
    /// cloaked, and, when `unsaved` is set, marks unsaved changes (a title
    /// starting with `*`) or not, as it says.
    Foreground {
        /// Text the title contains.
        title_contains: String,
        /// Whether the title must mark unsaved changes, or must not.
        unsaved: Option<bool>,
    },
    /// The foreground window is not titled with `title_contains`, such as
    /// once a dialog has closed.
    NotForeground {
        /// Text the title contains.
        title_contains: String,
    },
    /// A visible top-level window, minimized or not but not cloaked (kept
    /// by the window manager but not shown, as the Settings app keeps its
    /// closed window), is titled with `title_contains`.
    Present {
        /// Text the title contains.
        title_contains: String,
    },
    /// No visible top-level window is titled with `title_contains`.
    Absent {
        /// Text the title contains.
        title_contains: String,
    },
    /// Every visible top-level window that can be minimized is, or is
    /// cloaked, kept by the window manager but not shown.
    AllMinimized,
}

/// The focused element as UI Automation reports it, for
/// [`Request::FocusedElement`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FocusedElement {
    /// Its name.
    pub name: String,
    /// Its position among its siblings and how many there are, counting
    /// from one, when UI Automation reports both.
    pub position: Option<(i32, i32)>,
    /// Whether it is selected, when it can be.
    pub selected: Option<bool>,
}

/// A process, as [`Request::ChildProcesses`] reports it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessInfo {
    /// The OS process id.
    pub pid: u32,
    /// The executable file name, such as `verbatim-synth-host.exe`.
    pub image: String,
}

/// A process that exited in a launched process's job, as
/// [`Request::JobExits`] reports it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessExit {
    /// The OS process id it had.
    pub pid: u32,
    /// Its executable file name.
    pub image: String,
    /// Its exit code, when it could be read.
    pub exit_code: Option<i32>,
    /// Whether Windows reported the exit as abnormal: the process ended on
    /// an unhandled exception, as a crash does.
    pub abnormal: bool,
}

/// How a [`Request::WaitForEvent`] ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventOutcome {
    /// The event was set.
    Signalled,
    /// The process exited first, with its exit code when it could be read.
    Exited {
        /// The exit code.
        exit_code: Option<i32>,
    },
    /// Neither happened within the timeout.
    TimedOut,
}

/// The answer to [`Request::ForegroundInfo`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForegroundInfo {
    /// The foreground window, `None` when there is none.
    pub foreground: Option<WindowInfo>,
    /// The visible, titled, unowned top-level windows, in Z order.
    pub windows: Vec<WindowInfo>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use verbatim_control::protocol::{read_message, write_message};

    #[test]
    fn requests_and_frames_round_trip() {
        let request = RequestEnvelope {
            id: 7,
            request: Request::LaunchProcess {
                command: "notepad.exe".to_owned(),
                args: vec![],
                working_dir: None,
                env: vec![("VERBATIM_TEST_AUDIO".to_owned(), "null".to_owned())],
                stderr_to: Some(r"C:\VerbatimLab\verbatim\stderr-e2e.log".to_owned()),
                console_title: Some("A console".to_owned()),
                minimized: true,
                ignore_foreign_terminals: true,
            },
        };
        let frame = Frame::Reply {
            to: 7,
            payload: ReplyPayload::Launched {
                pid: 4242,
                foreground_allowed: true,
                ignored: vec![29864, 31000],
            },
        };

        let mut buffer = Vec::new();
        write_message(&mut buffer, &request).expect("writes");
        write_message(&mut buffer, &frame).expect("writes");

        let mut reader = buffer.as_slice();
        let read_request: RequestEnvelope = read_message(&mut reader)
            .expect("reads")
            .expect("not end of stream");
        let read_frame: Frame = read_message(&mut reader)
            .expect("reads")
            .expect("not end of stream");
        assert_eq!(read_request, request);
        assert_eq!(read_frame, frame);
    }

    /// A `LaunchProcess` request written by an older client that predates
    /// `stderr_to` — the field must be optional on the wire so such a
    /// client stays compatible with a newer agent.
    #[test]
    fn launch_process_without_stderr_to_deserializes_as_none() {
        let json = r#"{"id":1,"request":{"LaunchProcess":{"command":"notepad.exe","args":[],"working_dir":null,"env":[]}}}"#;
        let mut buffer = json.as_bytes().to_vec();
        buffer.push(b'\n');
        let mut reader = buffer.as_slice();
        let envelope: RequestEnvelope = read_message(&mut reader)
            .expect("reads")
            .expect("not end of stream");
        match envelope.request {
            Request::LaunchProcess { stderr_to, .. } => {
                assert_eq!(stderr_to, None);
            }
            other => panic!("expected LaunchProcess, got {other:?}"),
        }
    }

    #[test]
    fn kill_and_status_and_session_info_round_trip() {
        let messages = [
            Frame::Reply {
                to: 1,
                payload: ReplyPayload::Killed(KillOutcome::AlreadyExited),
            },
            Frame::Reply {
                to: 2,
                payload: ReplyPayload::ProcessStatus(ProcessState::Exited { exit_code: Some(0) }),
            },
            Frame::Reply {
                to: 3,
                payload: ReplyPayload::SessionInfo(SessionInfo {
                    session_id: 1,
                    interactive_window_station: true,
                    input_desktop_name: Some("Default".to_owned()),
                }),
            },
        ];

        let mut buffer = Vec::new();
        for message in &messages {
            write_message(&mut buffer, message).expect("writes");
        }
        let mut reader = buffer.as_slice();
        for expected in &messages {
            let read: Frame = read_message(&mut reader)
                .expect("reads")
                .expect("not end of stream");
            assert_eq!(&read, expected);
        }
    }

    #[test]
    fn type_text_round_trips() {
        let request = RequestEnvelope {
            id: 11,
            request: Request::TypeText {
                text: "echo hello".to_owned(),
            },
        };
        let frame = Frame::Reply {
            to: 11,
            payload: ReplyPayload::TextTyped { input: 4 },
        };

        let mut buffer = Vec::new();
        write_message(&mut buffer, &request).expect("writes");
        write_message(&mut buffer, &frame).expect("writes");

        let mut reader = buffer.as_slice();
        let read_request: RequestEnvelope = read_message(&mut reader)
            .expect("reads")
            .expect("not end of stream");
        let read_frame: Frame = read_message(&mut reader)
            .expect("reads")
            .expect("not end of stream");
        assert_eq!(read_request, request);
        assert_eq!(read_frame, frame);
    }

    #[test]
    fn a_window_wait_round_trips() {
        let request = RequestEnvelope {
            id: 9,
            request: Request::WaitForWindow {
                condition: WindowCondition::Foreground {
                    title_contains: "verbatim-e2e-notes".to_owned(),
                    unsaved: Some(false),
                },
                timeout_ms: 15_000,
            },
        };
        let frame = Frame::Reply {
            to: 9,
            payload: ReplyPayload::WindowState {
                met: true,
                desktop: ForegroundInfo {
                    foreground: None,
                    windows: Vec::new(),
                },
            },
        };

        let mut buffer = Vec::new();
        write_message(&mut buffer, &request).expect("writes");
        write_message(&mut buffer, &frame).expect("writes");

        let mut reader = buffer.as_slice();
        let read_request: RequestEnvelope = read_message(&mut reader)
            .expect("reads")
            .expect("not end of stream");
        let read_frame: Frame = read_message(&mut reader)
            .expect("reads")
            .expect("not end of stream");
        assert_eq!(read_request, request);
        assert_eq!(read_frame, frame);
    }
}
