# Focus events cannot be limited to one application

## Fact

The documented call to limit focus events to one window is refused: `AddAutomationEventHandler` for `UIA_AutomationFocusChangedEventId` on one window fails with `E_INVALIDARG`. Two undocumented routes are accepted, the focus event in an event handler group on a window's element and the core API's `UiaAddEvent` on a window's node, and each delivers that window's provider-raised focus and no other application's, but neither any focus of a window UIA reads through its proxy. Each process registered on a window adds about 47 to 49 provider calls per focus event on the application's UI thread (`books/dev/src/reference/uia-bugs.md`, the limitation of that name).

## What Verbatim relies on

D13's listener, which exists to hold the one desktop-wide focus registration while outposts hold none (`books/dev/src/explanation/architecture.md`). A change breaks nothing; it would only reopen D13.

## Method

The kept CI tests, in `crates/mockapp/tests/uia_bugs/focus_events_cannot_be_limited_to_one_application.rs`, cover E-scope, P1 and P5: `a_focus_handler_on_one_window_is_refused`, `a_focus_event_in_a_group_on_one_window_hears_that_applications_focus_alone`, `a_focus_event_in_a_group_on_a_proxied_window_hears_none_of_its_focus` and `a_core_api_focus_registration_on_one_window_hears_that_applications_focus_alone`. The program covers only what they do not count: P2's and E-cost's provider calls per focus event, which vary from one event to the next.

- `cost`: A is the provider program's UIA window. Each registered process is a listening process holding the focus event in an event handler group on A's window, subtree scope, with one of P2's cache requests: none, the process id, or NVDA's base cache request (thirteen properties and the text pattern). A moves its focus between nodes 5 and 6 and raises the focus event 10 times, each once every registered process has heard the one before. A's log counts every provider call and `WM_GETOBJECT` from the first raise to the last delivery; over 10, that is one event's cost. With 1 and then 4 processes registered, the rise over 3 is the cost per registered process. P2 counted 0, 1, 2, 4 and 8 processes; 1 and 4 give the same slope with less running.

Judged: the cost per registered process against 40 to 60 for no cache and the process id (a count that varies by event); the base cache's, which adds its own property reads, for trends. The provider program's tree is not mockapp's, so its count is recorded again, with mockapp's figures kept as history.

## Client settings

Neither connection recovery nor event coalescing changes these outcomes (checked on 2026-11-06 by running every case by default and once each with `--recovery`, `--coalescing` and both).

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| E-cost: 90 calls for one focus event with one scoped registration, 185 with three | 2026-10-10 | Build 26200 | the mockapp experiments, in git at b0a1989 | History |
| P2 on mockapp: per focus event 1, 70, 117, 210 and 397 calls with 0, 1, 2, 4 and 8 processes and no cache; 1, 71, 119, 214 and 405 with the process id; 1, 86, 149, 274 and 525 with the base cache: about 47 to 49 a process, and about 16 more `GetPropertyValue` with the base cache | 2026-10-18 | Build 26200 | `recorded/p2_cost.out`, `recorded/p2_cost.summary.txt` | 40 to 60 |

## How to rerun

`uia-facts/build.ps1`, then `target\debug\focus-events-cannot-be-limited-to-one-application.exe`. No desktop or application. The tests: `cargo test -p mockapp --test uia_bugs -- --exact a_focus_handler_on_one_window_is_refused`, and the same with the other three names.

## Evidence kept

- `recorded/p2_cost.out` and `recorded/p2_cost.summary.txt`: P2's raw counts and their summary. Cited: `provider_calls_per_event` of the `group` lines, by cache and number of processes.
- `recorded/p1_scope.run1.out` to `run3.out` and `recorded/p5_core1.out`, `p5_core2.out`: P1's and P5's outputs, the CI tests' evidence. Cited: the refusal's HRESULT and message, and the senders each registration heard.
