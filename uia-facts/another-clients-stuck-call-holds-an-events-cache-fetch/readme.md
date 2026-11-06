# Another client's stuck call holds an event's cache fetch

## Fact

Before calling a handler, a client process's event worker fetches the event's cache request from the application with requests of its own. So another client's call stuck in that application holds the fetch, and with it every later event of the client process; the explainer infers this, and does not know whether the transaction timeout bounds the fetch (`books/dev/src/explanation/windows/uia.md`, "Events into the client" and "Not known").

## What Verbatim relies on

Nothing recorded: the explainer's model of the worker.

## Method

Provider program P holds two UIA windows on UI threads of their own, and provider program Q one. Client X, a listening process with no registration, reads the name of P's window 0, and P holds that read in its provider (`hold GetPropertyValue w=0`), so X's call is stuck in P's one request reader. Client D, a listening process of its own, has name-change registrations on P's window 1 and on Q's window, each caching the sender's name. Once P has signalled that X's read is held, P's window 1 raises a name change, and 100 ms later Q's window raises one. The hold's length is deliberate, since the question is how long a stuck call holds the fetch.

- `holds`: X's read held 1 s; D at UIA's default transaction timeout, 20 s.
- `timeout`: X's read held 6 s; D's transaction timeout 1 s, then 5 s.

Every claim is an open question, printed as new: whether D hears each change only after X's release, whether it hears P's change at all, and whether it hears Q's before the release, with the time.

## Client settings

Neither connection recovery nor event coalescing changes these outcomes (checked on 2026-11-06 by running every case by default and once each with `--recovery`, `--coalescing` and both).

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| The worker fetches the event's cache with 8 requests for one selection event | 2026-11-02 | Build 26200.9711 | the parallel-calls experiment's readme, in git at fdfb63ae, "Events and releases" | Cited by `uia.md`; reproduced by the requests guide |
| First uia-facts run, `holds`: both changes heard only after X's release | 2026-11-06 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | 132b6af4 | The answer |
| First uia-facts run, `timeout`: at 1 s and 5 s, P's change never heard; Q's heard 2,199 and 2,203 ms after P's raise, before X's release at 6 s | 2026-11-06 | As above | As above | The answer: the transaction timeout does not bound the hold |

## How to rerun

`uia-facts/build.ps1`, then `target\debug\another-clients-stuck-call-holds-an-events-cache-fetch.exe`, or one case with `--case`. No desktop or application.

## Evidence kept

None: the claim was an inference with no record.
