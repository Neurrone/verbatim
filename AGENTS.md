# Verbatim

This repository contains the source code for Verbatim, a screen reader in Rust inspired by NVDA. It attempts to rewrite NVDA in Rust. The NVDA source code is available in the `nvda` submodule.

## Coding Standards

We use Rust 2024 edition.

Arm64 is a first-class target, but since this machine is not an Arm machine, just verify that arm builds work without running them.

Use Clippy with the pedantic lint group from the start. Public Rust APIs should have doc comments so the workspace can be browsed with Rustdoc.

We are using GitHub actions for CI.

## Project Documentation

Before changing architecture or phase scope, start with:

- `docs/superpowers/specs/2026-06-27-verbatim-rewrite-design.md` - project charter, goals, non-goals, and documentation map
- `docs/architecture/README.md` - architecture overview and source-of-truth links
- `docs/phases/README.md` - phase roadmap, gates, and acceptance criteria
- `docs/tooling/README.md` - CI, benchmarks, query tools, VM/container tooling
- `docs/observability/trace-schema.md` - trace schema and latency metrics
- `docs/parity/README.md` - NVDA/Verbatim parity strategy

For detailed subsystem design, follow the links from `docs/architecture/README.md`

## Useful Commands

The repository is pinned with `rust-toolchain.toml`

- Format check: `cargo fmt --all -- --check`
- Lint: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- Test: `cargo test --workspace --all-features`
- Build Windows x64: `cargo build --workspace --target x86_64-pc-windows-msvc`
- Build Windows arm64: `cargo build --workspace --target aarch64-pc-windows-msvc`
