# Debugging tools of the parallel UIA calls experiment

The experiment's facts, its programs' cases and its outputs moved to `uia-facts/` (`uia-facts/readme.md` lists the folders), where each fact is reproduced against uia-facts' own provider program. What stays here are the tools `books/dev/src/how-to/debugging.md` uses as worked examples:

- `dbg.psm1`: attaches cdb to a process from a script, breaks in with `DebugBreakProcess`, sends the empty line a redirected standard input needs, and detaches with `qd`.
- `stacks.ps1`: a non-invasive `~*k` of a process while calls wait on a suspended thread.
- `blocked-summary.py`: one line per wait from `cargo xtask trace waits --blocked`.
- `src/` with `Cargo.toml.txt`: the standalone program `puia`, whose `puia threads pid=<pid>` lists a process's threads with their start addresses and processor cycles, with no attach. Build it outside the workspace: copy `Cargo.toml.txt` to `Cargo.toml` in a scratch folder, make both of its paths absolute, and run `cargo build --release`. The scripts read `PUIA_SCRATCH`, the scratch folder holding `build\target\release\puia.exe`.

The experiment's full readme, scripts and outputs before the move are in git, in the commit before the one that trimmed this folder.
