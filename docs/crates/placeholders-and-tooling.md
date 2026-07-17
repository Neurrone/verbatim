# Placeholders and tooling

- `verbatim-uia-rops` — UIA remote operations, lands in M4.
- `verbatim-ext` and `verbatim-ext-api` — the Wasm extension host and WIT
  contract, land in M5.
- `xtask` — workspace automation. `cargo xtask ci` is the standard check
  and exactly what GitHub Actions runs: rustfmt, pedantic clippy with
  warnings denied, unit tests on x64, then a release-profile ARM64
  cross-build (build-verified only; never run on this x64 machine). It
  probes known Visual Studio and LLVM locations for `libclang.dll` so
  wxDragon's bindgen works without manual environment setup. `cargo xtask
  vm` is the milestone M2 Hyper-V harness (build, deploy, and E2E-test a
  real VM) — see this document's "xtask VM harness" section above and
  `docs/tooling.md` for the full verb reference.
