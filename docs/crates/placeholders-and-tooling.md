# Placeholders and tooling

- `verbatim-uia-rops` — UIA remote operations, lands in M4.
- `verbatim-ext` and `verbatim-ext-api` — the Wasm extension host and WIT
  contract, land in M5.
- `xtask` — workspace automation. `cargo xtask ci` is the standard check
  and exactly what GitHub Actions runs: the platform-neutral dependency
  check (each crate the `CLAUDE.md` NVDA provenance section lists as
  neutral has its `cargo tree` inspected, and the step fails naming every
  crate that pulls in the `windows` or `windows-core` bindings), rustfmt,
  pedantic clippy with warnings denied, unit tests on x64, then a
  release-profile ARM64 cross-build (build-verified only; never run on
  this x64 machine). It probes known Visual Studio and LLVM locations for
  `libclang.dll` so wxDragon's bindgen works without manual environment
  setup. `cargo xtask
  vm` is the milestone M2 Hyper-V harness (build, deploy, and E2E-test a
  real VM) — see this document's "xtask VM harness" section above and
  `docs/tooling.md` for the full verb reference. `cargo xtask park`
  (`xtask/src/park.rs`) moves the caller's Remote Desktop session onto the
  machine's console through a scheduled task that
  `vm/scripts/Register-VerbatimParkTask.ps1` registers once, then checks
  that the console desktop is unlocked with an uncloaked foreground
  window, so a local end-to-end run works with no RDP client connected.
