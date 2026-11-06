# A focus change in a window busy past both of UIA's waits is dropped

## Fact

In a process holding a UIA focus registration, a focus change in a window that stays busy past both of UIA's waits for it, each one connection timeout, is dropped, never delivered; a stall shorter than both depends on the application (`books/dev/src/reference/uia-bugs.md`, "A hung window named in a system focus event holds a process's UIA events", "Workarounds that worked"). In `h6`, an MSAA mockapp raised its focus from inside a 3-second stall: at a 10,000 ms connection timeout it arrived 3,003 ms into the stall, and at 500 ms and the default 2,000 ms it had not arrived 8 seconds after the stall began. In the provider program's MSAA window, a stall of 1.5 connection timeouts, 1.5 seconds at 1,000 ms, delivers the focus as the stall ends.

## What Verbatim relies on

The cost of the 2-second `CONNECTION_TIMEOUT` that bounds the hold (`books/dev/src/reference/uia-bugs.md`, the same entry): a shorter timeout drops a focus change in a window that stays busy past both waits. If the drop stopped, a shorter timeout would cost less; if it began at a stall shorter than both waits, the 2-second timeout would drop focus changes Verbatim now hears.

## Method

On a desktop of the program's own (`uia_facts_support::desktop`), as `h6` ran, with the hung-window scene of support's `uia_facts_support::hung`: M, the provider program's MSAA window, moves its focus to its first child while it answers, then to its second at the start of a 1.5 s stall of its UI thread, from inside the stall (`busy 2 1500 focus-at 0 2`). A client, a listening process of its own, holds a desktop focus handler caching `h6`'s properties, at a connection timeout of 10 s, 1 s and 250 ms, each in a trial of its own, and listens 3 s from the stall's start where `h6` listened 8 s from a 3 s stall. At 1 s the stall is 1.5 waits long, as `h6`'s 3 s stall was at 2 s. The stall's length is deliberate, since the fact is about a timed stall.

- `dropped`: judged: the focus raised while M answers heard within 100 ms at each timeout (a relation); at 10 s and at 1 s the focus from inside the stall heard once the stall ends; at 250 ms not heard within 3 s of the stall's start (relations).

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| `h6`: at 10,000 ms the focus arrived 3,003 ms into the stall; at 500 and 2,000 ms never within 8 s; the control 10 to 12 ms. Rerun at 01254292 against mockapp on 2026-11-06: the same at 2,000 ms | 2026-10-18 | Build 26200 | `recorded/h6_*.out` | Heard at the stall's end; not heard within 8 s; the 2,000 ms outcome is the provider program's |
| First uia-facts run, `dropped`: the control 7.5 to 9.0 ms; at 10 s heard 1,012.1 ms into the 1 s stall, at 1 s 1,015.2 ms, at 250 ms never within 2 s | 2026-11-06 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | 132b6af4 | Pass |
| `dropped` with its 1.5 s stall: at 1 s heard 1,504.7 ms into the stall, at 250 ms never within 3 s | 2026-11-06 | As above | 93fed1a0 | Pass |
| This program's first run: the control 7.9 to 9.3 ms, and once with `--coalescing` 101.5 ms at 1 s; at 10 s heard 1,509.0 ms into the 1.5 s stall, at 1 s 1,503.4 ms, at 250 ms never within 3 s | 2026-11-06 | As above | ff23f81a | Pass but the one control, 1.5 ms past the 100 ms bound |

## How to rerun

`uia-facts/build.ps1`, then `target\debug\a-focus-change-in-a-window-busy-past-both-of-uias-waits-is-dropped.exe`. It needs no desktop or application; its processes run on a desktop of their own.

## Evidence kept

`recorded/h6_cto10000.out`, `h6_cto500.out` and `h6_default.out`: `h6`'s raw outputs, moved here from the hold's folder. Cited: when, or whether, each run heard the focus raised inside the stall, and the control's delay.
