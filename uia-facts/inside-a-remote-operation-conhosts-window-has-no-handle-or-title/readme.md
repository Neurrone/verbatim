# Inside a remote operation, conhost's window has no handle or title

## Fact

Read inside a remote operation, the window element above a focus in conhost's text area has no native window handle and no name; only the client side's window proxy supplies them (`books/dev/src/reference/uia-bugs.md`, the fault of that name). Whether the fault is conhost's provider's or UIA's is not established.

## What Verbatim relies on

The outpost reading again through the window's proxy any ancestor that is a window's own element read without its handle, one `ElementFromHandleBuildCache` (`crates/verbatim-outpost/src/outpost/read.rs`). The kept CI test `a_window_element_read_without_its_handle_is_named_by_its_runtime_id` in that file tests the workaround against a stand-in element; this program shows the fault in the real console host, which no stand-in can.

## Method

- `parent`: on a desktop of the program's own, a client there starts a console host directly (`conhost.exe cmd.exe`), finds its text area (the window element's descendant of the document control type), and reads the text area's raw-view parent's native window handle and name inside a remote operation that navigates to the parent there, and through the client side's raw view walker. Judged: inside, the handle 0 and the name empty; through the walker, the console window's handle and a name (deterministic).

The console host is the target because the claim is about its windows.

## Client settings

Neither connection recovery nor event coalescing changes these outcomes (checked on 2026-11-06 by running the default run once each by default and with `--recovery`, `--coalescing` and both).

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| Inside the outpost's operation conhost's window element had no handle (3f4dbb44), then with the handle supplied no name (1b2e5420); a probe against a real console host showed the operation's window element nameless with handle 0 and the proxy's read named, with its handle | 2026-10-06 and 2026-10-11 | Build 26200 | Commits 3f4dbb44 and 1b2e5420; the probe was not kept | Deterministic |
| First uia-facts run: inside, no handle and no name; through the walker, the window's handle and title | 2026-11-06 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | fdfb63ae | Pass |

## How to rerun

`uia-facts/build.ps1`, then `target\debug\inside-a-remote-operation-conhosts-window-has-no-handle-or-title.exe`. It starts a console host on a desktop of its own, so nothing reaches the user's screen. The test: `cargo test -p verbatim-outpost -- --exact outpost::read::tests::a_window_element_read_without_its_handle_is_named_by_its_runtime_id`.

## Evidence kept

None: the program's output is the evidence.
