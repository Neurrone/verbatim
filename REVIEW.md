# Code review in this repository

A review checks a finished change, or a design, against the code, NVDA's pinned source and Dickson's rulings. It finds defects. It does not add requirements, ask for more evidence than the change needs, or propose features. `CLAUDE.md`, "How work is done", says when a review happens.

## What a review looks for

- A claim about NVDA that its pinned source or a capture contradicts.
- A spoken expectation with no capture or cited source line behind it.
- A test that accepts two outcomes, waits on time rather than evidence, runs a copy of Verbatim's own code, or passes when the behaviour it names is removed (`books/dev/src/reference/testing.md`).
- A race hidden by a retry, a sleep or a bound added for one failure.
- A wake-up, call into another process, lock or timer added on the event or speech path that the change does not account for.
- State in the code that its design's state table does not list, or a flag or optional field that stands in for a state.
- An addition to shared machinery (a kind of request or reply, a queue hook, a copied fact) that the change does not need.
- A document that describes removed code, or a decision attributed to Dickson that he did not take.
- A removal that something still depends on.
- A fact the change writes, copies or judges whose other writers and readers are not accounted for. The reviewer lists them with a grep, inside the diff and outside it, and gives one sequence through each. A holder of the same fact that the change does not update is a must-fix, because 15 of 18 recent defects in the 2026-11-27 audit came from a reader or writer outside the diff.
- A default (`unwrap_or`, `unwrap_or_default`, 0, the first index, an empty string) on a value that can be missing for a reason outside Verbatim's code: another process, the system, a file or the user.
- A rule that decides a fact another function already decides. The change calls that function or replaces both.

## What a finding carries

Each finding gives:

- the file and line;
- the defect in one sentence;
- a concrete counterexample: a sequence, a failing test, or a source line.

Anything without one is left out. Each finding is tagged **must-fix**, meaning wrong behaviour, a wrong claim, or a test that proves nothing, or **nit**. Style remarks are left out. The report opens with the count of must-fixes.

## The loop

1. The fixing agent reproduces each must-fix before fixing it.
2. The fixes go in, `cargo xtask ci` passes, and the loop ends. A large fix gets one look at its own diff.
3. Nits are fixed only where the fix already touches the lines.
4. A must-fix that contradicts one of Dickson's rulings goes in the report instead of being fixed.
