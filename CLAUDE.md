# CLAUDE.md

## What this repository is

Verbatim: a screen reader for Windows 11 (x64 and ARM64, both first-class), written in Rust. Milestones M0 (foundations), M1 (the self-voicing prototype), M2 (the test harness and VM), and M3 (desktop usability core) are implemented: `verbatim.exe` reads its own GUI through a real out-of-process outpost over UIA and MSAA, speaks through OneCore and WASAPI, and is drivable and inspectable live through the control plane (`verbatim-inspect`); `mockapp` exercises both client stacks cross-process against scripted providers, and an end-to-end suite drives a real Verbatim through an in-guest agent, either on the local machine or in a Hyper-V VM. The authoritative sources are:

- `docs/architecture.md` — decisions of record (D1–D16), process/thread model, crate map, testing strategy, and top risks (R1–R6). Read this before proposing any design or implementation work.
- `docs/roadmap.md` — the milestones ahead (M4 onward), scoped by risk retired with explicit exit criteria, plus the NVDA app-module porting track; completed milestones are archived in `docs/roadmap-done.md`.
- `docs/readme.md` — the index of all documentation, with reading paths; `docs/glossary.md` defines Verbatim's invented vocabulary.
- `docs/crates/` — the reviewer's guide to the implemented crates, one file per crate (`docs/crates/readme.md` is the index): what each does, its public API, and how the intricate parts work. Keep the file for a crate current when its public API changes.
- `docs/tooling.md` — how to actually drive this project: `verbatim-inspect` against a running instance, `mockapp`, the end-to-end suite, and the traps that cost us time (the interactive-session rule above all). `docs/vm.md` covers every `cargo xtask vm` verb and rebuilding the golden image.

`nvda/` is the NVDA screen reader vendored as a git submodule **for reference only** (IA2 IDL under `nvda/include/ia2`, app modules under `nvda/source/appModules`, design docs under `nvda/projectDocs`). Never modify anything under `nvda/`; Verbatim is informed by NVDA but not constrained by its architecture.

## Writing conventions (mandatory)

All documentation, comments, and commit messages in this repo must read well with a screen reader, in both rendered and source form:

- No ASCII-art diagrams, box drawings, or arrow chains.
- Prefer prose and lists over pipe tables.

## NVDA provenance

The crates fall into two tiers. The platform-neutral crates, which are `verbatim-model`, `verbatim-core`, `verbatim-speech`, `verbatim-config`, `verbatim-i18n`, `verbatim-input`, `verbatim-audio`, and `verbatim-text`, contain only original code, so that tier's licensing does not depend on NVDA's. Implement their behavior from `docs/parity.md` and the prose under `docs/nvda/`, never by translating NVDA source, and cite those docs in comments rather than NVDA file paths, function names, or line numbers. They never depend on a Windows-specific crate or on the Windows bindings; the hook thread and the WASAPI sink live in `verbatim-input-windows` and `verbatim-audio-wasapi` for that reason, and `cargo xtask ci` fails if the rule is broken. NVDA's spoken vocabulary, messages, and key layouts are deliberately matched for user familiarity and are fine to use; NVDA's non-English translations are never imported.

The Windows-specific crates are GPL like NVDA and may port NVDA code freely. Ported code never moves from a Windows crate into a platform-neutral one, and NVDA-core behavior destined for the reducer, such as browse mode, review modes, or symbol processing, is written from the parity docs rather than ported.

## Coding Standards

We use Rust 2024 edition.

Arm64 is a first-class target. Builds and tests always use the host's own architecture (no command names a target), so ARM64 is built and tested on an ARM64 machine, which GitHub Actions provides (the `ci-arm64` job); this machine is x64.

Use Clippy with the pedantic lint group from the start. Public Rust APIs should have doc comments so the workspace can be browsed with Rustdoc.

We are using GitHub actions for CI.

## Commands

`cargo xtask ci` is the standard check, and exactly what GitHub Actions runs: the platform-neutral dependency check (the crates listed under NVDA provenance must not pull in the Windows bindings), a check that the generated `crates/workspace-hack` crate is current (it needs `cargo install cargo-hakari --locked`; after adding a package or changing a third-party dependency, run `cargo hakari generate` and `cargo hakari manage-deps`, as `docs/tooling.md` explains), rustfmt, clippy (pedantic via workspace lints, warnings denied), and unit tests, all for the host's architecture and into `target/debug`, the same output the end-to-end suite uses.

`cargo xtask vm <cmd>` drives the Hyper-V harness: `create` (Packer-built golden image, imported, deployed to, checkpointed), `start`, `stop`, `restart`, `restore`, `deploy`, `test` (the end-to-end suite against the VM), `logs`, and `delete`. Guest credentials come from a `.env` at the repo root, which is never committed. See `docs/tooling.md`.

`cargo xtask park` moves this Remote Desktop session onto the console, unlocked, so a local end-to-end run works with no RDP client connected. It needs a one-time, elevated `vm\scripts\Register-VerbatimParkTask.ps1`, and disconnects any connected RDP client, so run it only when Dickson is away or has agreed.

The end-to-end suite also runs without a VM, against this machine, by pointing it at a locally running `verbatim-agent`. It launches a real Verbatim and injects real keystrokes, so it takes over the desktop while it runs and does nothing useful on a locked one; `docs/tooling.md` has the details.

The GUI's wxWidgets is built from source by `verbatim-gui`'s build script the first time anything builds that crate: it downloads the pinned release, builds static libraries with CMake (Ninja on x64), and keeps them in `target/wxwidgets`, or under `VERBATIM_WX_DIR` when that is set, so later builds reuse them. Nothing has to be set up by hand beyond CMake and the Visual Studio C++ tools.
