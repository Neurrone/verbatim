# Recording what NVDA says

How to capture what NVDA says from this repository, and the rules the capture follows; the NVDA transcript guide has the rest.

Install `nvda-addon/verbatimTranscript.nvda-addon` into NVDA and run
`cargo xtask nvda capture` with the keys to press;
[the NVDA transcript guide](../nvda-transcript.md) has the details. To
run a whole end-to-end scenario against NVDA and see its expectations
beside what NVDA said, step by step, run `cargo xtask nvda compare
<scenario>` ("Comparing a scenario" in the same guide).

The capture presses its keys only in the window it means to, known by
its handle: the one in front when it starts, then the one each `--front`
brings forward. It stops with an error, sending nothing more, when a
`--front` finds no window to bring forward, when a `--launch` has not
been followed by a `--front`, and, checked just before every single key
of a step and every character typed, when that window is not in front or
Windows judges it not responding. It also fails at its end when its
window is no longer in front and responding: keys it has not read yet
go to whichever window is in front when they are read, or when it
closes, so such a window must not be closed. A key sent to a window that
is not reading its input waits; the check cannot see keys already
waiting, so a capture should leave its window open until its program has
caught up (its typing echoed), and close it only then.

Why: keys a window has not yet read go to whatever window is in front
when they are read, so a capture whose window was closed with keys
waiting once had them open a game from the desktop (2026-10-09).
