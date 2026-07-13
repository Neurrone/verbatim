# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this repository is

Verbatim: a screen reader for Windows 11 (x64 and ARM64, both first-class), written in Rust. The project is currently in the planning stage — there is no code yet, only design documents. The authoritative sources are:

- `docs/architecture.md` — decisions of record (D1–D9), process/thread model, crate map, testing strategy, and top risks (R1–R6). Read this before proposing any design or implementation work.
- `docs/roadmap.md` — milestones M0–M12 scoped by risk retired, with explicit exit criteria, plus the NVDA app-module porting track.

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

No build system exists yet. Per the roadmap, M0 establishes a Cargo workspace with an `xtask` crate as the automation entry point; once that lands, `cargo xtask ci` (build for x64 and ARM64, clippy, unit tests) is the standard check, and `cargo xtask vm <cmd>` drives the Hyper-V E2E harness. Update this section when the workspace exists.

The wxDragon GUI dependency uses bindgen. If `libclang.dll` is not already on `PATH`, set `LIBCLANG_PATH` before running workspace build, test, or lint commands. On this machine, use:

`$env:LIBCLANG_PATH='C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Tools\Llvm\x64\bin'`

Use release profile for the ARM64 workspace build until upstream wxDragon fixes debug-profile ARM64 MSVC builds: https://github.com/AllenDang/wxDragon/issues/162
