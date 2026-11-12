# ADR 0002: No screen reader per outpost

Status: decided, 2026-11-10; amended 2026-11-13.

This record keeps why the keys, the focus and everything that depends on them are judged in Core and not in each application's outpost: the problem, the options weighed, what was chosen and by whom, and the limits accepted. How the processes are laid out is in [the architecture](../../explanation/architecture.md), section 1, and how a focus travels is in [How focus works](../../explanation/design/focus.md).

## The problem

Verbatim reads each application from an outpost process of its own, and decides what is said in Core. An interaction with an application therefore crosses two processes: the calls are made in the outpost, and the decision is made in Core. Where one interaction keeps state on both sides, the two have to be kept in step by messages, and the defects of October and early November 2026 that involved more than one thread or process came mostly from that: a caret key's progress was kept by the keyboard hook, Core's shell, Core's reducer and the outpost, and a focus report could wait in Core for a second read by its reader.

NVDA has no such split. It decides on the thread that reads the application, so a rule can consult NVDA's own state, its focus and its queue of keys, while it reads. The question was whether Verbatim could give each application the same: the screen reader's logic for one application running in that application's outpost, next to the calls, with Core reduced to what spans applications.

## Decisions

### Core judges keys and focus (Dickson, 2026-11-10)

Chosen: Core stays the one judge of every key and every focus, and of what depends on them: typed echo, a held caret key's handling and words, the terminal's memory and rules, the review cursor, object navigation and say-all. An outpost makes the calls and reports what it read.

Two facts decide it.

- Focus across applications has one order only where every application's reports and the focus listener's foreground changes meet, and that place is Core's input. NVDA's focus rules are defined on that one order: a focus is a repeat unless a focus elsewhere came between, a late focus from an application just left is dropped, and a focus in another top-level window cuts all speech.
- A key is ordered with an application's reads only because the keyboard hook posts it to Core before it returns, so Core has the key before the application has received it. Any other process learns of the key after a hop, and the console host shows a typed character within about a millisecond.

Core also needs the focus in any case, since the keys are judged there: typed echo and the rule for protected fields, the hook's decision to hold a caret key, the mode a key is in under browse mode, the review cursor's start, braille's tether and a profile's switch all read it. Judging focus elsewhere would leave each of them reading a copy.

Weighed and rejected:

- Each application's outpost runs the whole screen reader for its application, and Core keeps the hook, the speech queue and attention. Rejected, because an outpost would then judge facts that are Core's, such as whether the focus has been elsewhere, from notices that reach it by another route than its own application's events, and nothing orders the two: a return to an application could be judged a repeat of its last focus and stay silent.
- Only what needs no state of Core's is judged in the outpost. Rejected, because nearly every rule reads Core's state, so little moves, and what does move splits a record that is whole today: a terminal's memory in the outpost with its review position, its focus and its record of keys in Core.
- Core forwards each key to the outpost of the focused application, and the listener tells an outpost when the focus goes elsewhere. Rejected for keys, because a forwarded key can arrive after the read of its own echo, which no comparison of times repairs, since the key is not yet there to compare; and for focus, because the listener learns that a focus elsewhere counts only after its checks, on another thread than the one that sees the event, so the notice is either early and sometimes false, or late.
- Focus alone judged in the outposts, with the relay telling each outpost when another application's focus passes. Rejected, because the keys stay in Core and need the focus there, so nothing that reads the focus would become simpler, and a third place would hold which application's focus is current.
- A keyboard hook in each application's outpost, installed while that application is in front and removed when it is not, so that a key and the application's reads meet beside the application (Dickson, 2026-11-12). Rejected, because a key pressed in the gap at a switch between applications reaches the old outpost's hook or none, so Verbatim's modifier or Caps Lock would act on the application; a held modifier or held caret key would have to be carried across the switch; no outpost is in front on the desktop, in an ignored process or while one is starting; a stuck outpost is killed and its hook goes with it, at the moment stop-speech and Alt+Tab matter most; and Windows removes a hook that answers too slowly without telling the process. A permanent fallback hook in Core would mean two hooks judging one key.

### What an outpost keeps for Core (Dickson, 2026-11-10; amended 2026-11-13)

A reader keeps nothing for Core between requests but held objects, the requests waiting in an outpost's table and a terminal's "changed" flag, and judges nothing about them: each request carries what it needs and gets one answer ([ADR 0003](0003-readers-tell-core-asks.md), "Readers tell, Core asks"). Reason: an interaction's progress then lives in one place, Core's, and an outpost's ending or its worker's abandonment loses nothing Core has to reconcile. A design that needs the outpost to remember something for Core between two requests says why one request cannot carry it.

A reader decides nothing about what to read: it drops an event only by NVDA's local checks that cannot hang, and every call into an application is made for a request from Core ([ADR 0003](0003-readers-tell-core-asks.md)), so that Core never has to ask a reader again in order to judge a report.

## Limits accepted

- Core's reducer is on the path of every key and every focus, so a slow step in it delays them all, and each later feature that reads the keys or the focus adds work there. The delay between the keyboard hook and the reducer is measured and bounded as a performance budget.
- A held caret key's handling stays on two sides at least, the hook's line and Core's, because Windows waits on the hook's return and the read is made in the outpost.
- Keys, commands and every application's events wait in Core's one event queue behind its front entry, so a read of one slow application holds what is queued behind it until it answers or Core gives it up ([ADR 0003](0003-readers-tell-core-asks.md), "Limits accepted").

## What would reopen this

A measured fact that removes one of the two deciding facts: a way for an outpost to receive what Core knows in one order with its own application's events, or a way for a key to reach an outpost before the application's reaction to it can be read. A planned feature that needs an application's focus judged without Core would also be such a fact; none is on the roadmap. [ADR 0003](0003-readers-tell-core-asks.md) rests on this record, so such a fact reopens it too.
