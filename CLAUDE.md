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

## Coding Standards

We use Rust 2024 edition.

Arm64 is a first-class target, but since this machine is not an Arm machine, just verify that arm builds work without running them.

Use Clippy with the pedantic lint group from the start. Public Rust APIs should have doc comments so the workspace can be browsed with Rustdoc.

We are using GitHub actions for CI.

## Commands

`cargo xtask ci` is the standard check, and exactly what GitHub Actions runs: rustfmt, clippy (pedantic via workspace lints, warnings denied) and unit tests on x64, then a release-profile ARM64 cross-build. ARM64 artifacts are build-verified only, never run on this x64 machine.

`cargo xtask vm <cmd>` drives the Hyper-V harness: `create` (Packer-built golden image, imported, deployed to, checkpointed), `start`, `stop`, `restart`, `restore`, `deploy`, `test` (the end-to-end suite against the VM), `logs`, and `delete`. Guest credentials come from a `.env` at the repo root, which is never committed. See `docs/tooling.md`.

The end-to-end suite also runs without a VM, against this machine, by pointing it at a locally running `verbatim-agent`. It launches a real Verbatim and injects real keystrokes, so it takes over the desktop while it runs and does nothing useful on a locked one; `docs/tooling.md` has the details.

The wxDragon GUI dependency uses bindgen. `cargo xtask ci` probes known Visual Studio and LLVM install paths for `libclang.dll` automatically; when invoking cargo directly on targets that build `verbatim-gui`, set `LIBCLANG_PATH` yourself if `libclang.dll` is not on `PATH`. On this machine, use:

`$env:LIBCLANG_PATH='C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Tools\Llvm\x64\bin'`

Use release profile for the ARM64 workspace build until upstream wxDragon fixes debug-profile ARM64 MSVC builds: https://github.com/AllenDang/wxDragon/issues/162 (`cargo xtask ci` already does this).
