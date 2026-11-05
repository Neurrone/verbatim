# A remote operation stops after exactly 10000 instructions

## Fact

UIA stops a remote operation after exactly 10,000 executed instructions with `InstructionLimitExceeded` (`books/dev/src/explanation/windows/uia.md`, "What one request is").

## What Verbatim relies on

`books/dev/src/reference/performance.md`, "The instruction limit", and `books/dev/src/reference/crates/verbatim-uia-rops.md`: every program is sized under half the limit so its instructions can be counted, the caret read reaching about 7,700, and a run past the limit falls back to classic calls, hundreds of them. A lower limit would push programs to the fallback and break the counting tests.

## Method

A kept CI test: `the_instruction_limit_is_measured` in `crates/mockapp/tests/instruction_limit.rs` runs remote operations of growing length against mockapp and finds the count at which UIA stops them. `cargo xtask ci` runs it, so it is not copied here.

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| The limit: exactly 10,000 instructions | Each `cargo xtask ci` run | Every machine `cargo xtask ci` runs on, x64 and ARM64 | `the_instruction_limit_is_measured` | Equal |

## How to rerun

Rerun: `cargo test -p mockapp --test instruction_limit -- --exact the_instruction_limit_is_measured`. It needs nothing beyond a build of the main workspace and takes seconds.

## Evidence kept

The test itself.
