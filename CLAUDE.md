# CLAUDE.md

## What this repository is

Verbatim: a screen reader for Windows 11 (x64 and ARM64, both first-class), written in Rust. Milestones M0 (foundations) and M1 (the self-voicing prototype) are implemented: `verbatim.exe` reads its own GUI through a real out-of-process outpost over UIA and MSAA, speaks through OneCore and WASAPI, and is drivable and inspectable live through the control plane (`verbatim-inspect`). The authoritative sources are:

- `docs/architecture.md` — decisions of record (D1–D10), process/thread model, crate map, testing strategy, and top risks (R1–R6). Read this before proposing any design or implementation work.
- `docs/roadmap.md` — milestones M0–M12 scoped by risk retired, with explicit exit criteria, plus the NVDA app-module porting track.
- `docs/overview.md` — the reviewer's guide to the implemented crates: what each does, its public API, and how the intricate parts work. Keep it current when public APIs change.

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

`cargo xtask vm <cmd>` is a stub until the M2 Hyper-V harness lands.

The wxDragon GUI dependency uses bindgen. `cargo xtask ci` probes known Visual Studio and LLVM install paths for `libclang.dll` automatically; when invoking cargo directly on targets that build `verbatim-gui`, set `LIBCLANG_PATH` yourself if `libclang.dll` is not on `PATH`. On this machine, use:

`$env:LIBCLANG_PATH='C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Tools\Llvm\x64\bin'`

Use release profile for the ARM64 workspace build until upstream wxDragon fixes debug-profile ARM64 MSVC builds: https://github.com/AllenDang/wxDragon/issues/162 (`cargo xtask ci` already does this).
