# A remote operation's keyboard focus read can differ from a plain read

## Fact

Windows Terminal raised a focus event for a second window opened in its running process from a second identity of its terminal; inside the listener's remote operation the sender answered that it had the keyboard focus, and a plain property read of the same element answered that it had not (`books/dev/src/reference/uia-bugs.md`, the fault of that name). That the imported element inside the operation is the provider's own and answers for itself, where the plain read is answered otherwise, is inferred. On a provider that answers for itself, the provider program's tree, the two reads agree (new, 2026-11-06).

## What Verbatim relies on

The listener reading the sender's keyboard focus with a plain property read after the acceptance test, and dropping the focus when it has none (ab18245e; `books/dev/src/reference/performance.md`).

## Method

Each read is of `HasKeyboardFocus`, plainly (`CurrentHasKeyboardFocus`) and inside a remote operation built with Verbatim's `verbatim-uia-rops` (`uia_facts_support::rop::property`).

- `provider`: the provider program's hidden window 0, its tree's focus moved to node 3 with no event; node 3, node 4 and the window's own element read both ways. New: the pairs.
- `windows-terminal`, on the unlocked desktop, run only when named: Windows Terminal from the folder `UIA_FACTS_WT` names (the portable 1.24.12741.0 copy), a second window opened in its running process with `wt.exe -w new`, and every focus a desktop-wide focus handler in this program hears from Windows Terminal's process from the second window's request until 5 s after it appeared, a deliberate listening time; each sender read both ways after the 5 s. Three launches, as the end-to-end runs met it in each of three. Judged: launches in which a sender answered true inside the operation and false plainly, a rate against 3 of 3.

The provider program cannot stand for Windows Terminal's second identity, whose cause is not known, so the fault itself is tried only against Windows Terminal, the application its claim is about.

## Client settings

Neither connection recovery nor event coalescing changes these outcomes (checked on 2026-11-06 by running the default run once each by default and with `--recovery`, `--coalescing` and both).

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| `windows_terminal_two_windows` in each of three runs and `windows_terminal_leave_flood` in one: the second identity true inside the operation, false plainly; the usual identity the same both ways | 2026-10-12 | Build 26200 | `books/dev/src/explanation/design/focus-pipeline.md`, sections 16.8 and 16.9; commit ab18245e | A rate |
| First uia-facts run, `provider`: node 3 true both ways, node 4 false both ways, the window false both ways | 2026-11-06 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | fdfb63ae | New: the reads agree |

## How to rerun

`uia-facts/build.ps1`, then `target\debug\a-remote-operations-keyboard-focus-read-can-differ-from-a-plain-read.exe`, which runs `provider` and takes over nothing. `--case windows-terminal` needs the unlocked desktop, `UIA_FACTS_WT` set to the portable Windows Terminal's folder, and no other Windows Terminal running; it opens and closes Windows Terminal three times.

## Evidence kept

None: the original runs are the end-to-end logs the focus pipeline's sections cite.
