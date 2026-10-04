# Contributing to Verbatim

Contributions are welcome. Three things to read before the first one:

- `CLA.md`, the contributor licence agreement. Every commit you submit
  carries a `Signed-off-by` line as your acceptance of it, and your first
  pull request adds your name to `CONTRIBUTORS`.
- The "NVDA provenance" section of `CLAUDE.md`. The platform-neutral
  crates listed there must stay original work; code ported from NVDA
  belongs only in the Windows-specific crates, with its source named.
- `docs/readme.md`, the documentation index, starting with
  `docs/architecture.md` for the decisions of record.

After cloning, fetch eSpeak NG's source, which Verbatim builds as its
default synthesizer, with
`git submodule update --init third_party/espeak-ng`. Building it needs
CMake; the `nvda` submodule is optional reference material.
`docs/tooling.md` has the full first-time setup.

Before opening a pull request, run `cargo xtask ci`. It is exactly what
continuous integration runs, and it includes the check that the
platform-neutral crates pull in no Windows bindings.

All documentation, comments, and commit messages must read well with a
screen reader in both rendered and source form: no ASCII-art diagrams, box
drawings, or arrow chains, and prose and lists in preference to pipe
tables. `CLAUDE.md` has the rest of the writing and coding conventions.
