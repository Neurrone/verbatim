# Standing rules

These are rules Dickson has given repeatedly in past sessions. They live in
Claude's private memory, which subagents never see, so this skill carries
them until `CLAUDE.md` does. When `CLAUDE.md` gains a standing-rules
section, this file becomes a pointer to it.

Paste or point to this file in every subagent brief.

- Fix root causes; do not add retries, sleeps, skipped cleanup, or
  self-termination to hide a race or failure, because the outpost code once
  became a pile of independent mechanisms each added for one live failure and
  had to be redesigned. Use a workaround only with evidence that the proper
  fix is not enough, and document it as a workaround.
- Verify any claim about NVDA's behavior from the pinned source under `nvda/`
  or a live capture, never from memory; take spoken expectations only from
  captures, because an expectation taken from anywhere else has encoded
  Verbatim's old behavior as NVDA's.
- Follow `docs/testing.md`: one fixed target and code path per test, exact
  assertions, waits only for evidence, no branching on environment, no
  alternative expectations, and no substring matches, because a test that
  accepts two outcomes proves neither.
- Run the end-to-end scenarios related to the change, and the full suite only
  for wide-ranging changes or when the brief says so, because the full suite
  takes a long time and takes over the desktop.
- Work on main; open no pull requests or feature branches unless asked, so
  that every session and subagent builds on the same history.
- Push rarely, and only after `cargo xtask ci` and the related scenarios pass
  locally; never push to see what GitHub CI does, because a push is not a
  test run.
- Use UIA remote operations wherever a sequence of UIA calls would benefit,
  and audit this whenever UIA code changes, because each separate call is a
  cross-process round trip.
- Judge every change on wake-ups and CPU as well as latency; a change on the
  event or speech path must not raise them unless that is its purpose,
  because a latency figure alone hides a cost paid on every event.
- Clean up: stop processes you started, remove worktrees once merged, delete
  large traces once read, and release held keys, because leftovers break the
  next run and fill the disk.
- Do not stop while work in the slice remains; when a question needs
  Dickson, record it and continue with everything it does not block,
  because he is often away while a session runs.
- Platform-neutral crates (listed in `CLAUDE.md`) contain only original code
  written from the parity docs; ported NVDA code lives in the
  Windows-specific crates only, so that the platform-neutral tier's licensing
  does not depend on NVDA's.
