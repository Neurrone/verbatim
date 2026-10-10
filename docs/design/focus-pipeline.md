# How events reach Verbatim and are processed: a design aligned with NVDA

Design draft, 2026-10-10, for the redo of phase 6's step 6 (and steps 5 and the related open items). Written against main at 2cadb35. Verbatim references are relative to the repository root; NVDA references are under `nvda/source/` unless they start with `nvda/nvdaHelper/`. Prior work this builds on, all in the scratchpad: `event-processing-audit.md` (the audit, cited as "audit D1" and so on), `nvda-answers.md`, `focus-read-timeout.md`, `verify-focus.md`, `event-filtering-plan.md`, and the unmerged step 6 branch `worktree-agent-a27b534fac627b1ae` with its runs `runs-step6-full` and `runs-step6-fix`.

Claims about Microsoft's UIA behaviour that come from my own knowledge, not from a test or a source in this repository, are marked "knowledge". Section 11 records the mockapp experiments that check the ones the design depends on, run on 2026-10-10, and what they could not show.

## Status, 2026-10-10 (later)

Dickson rejected option (c): its standby spare discards the buffered events of every application it is not given to, and discarding events is a correctness bug. The root problem, in his words, is that a UIA element cannot be "transmitted" from the process that received the event to the outpost that needs it. Experiment E-marshal (section 11) confirms that: COM marshaling of an `IUIAutomationElement` to another process fails. Option (b) is worked through in section 12 and is the plan of record, with Core's guards (Dickson, 2026-10-10, later); the stale-focus window it leaves is analysed in section 13 and measured by section 14's scenario. (b2), the per-window hand-over, is an experiment only (section 13.3 gives the research behind that). Sections 0 to 10 are kept as written for option (c), which is rejected; the parts that do not depend on where focus is judged (sections 4 to 8: Core's single channel and duplicate rule, the MSAA side, time limits, background speech, the probe) carry over to (b) unchanged except where section 12 says otherwise.

## 0. Summary (of the rejected option (c))

- Recommended at the time: option (c). Each application's outpost receives every event for its own application itself, UIA focus included, so it holds the focus event's sender element as NVDA does. The focus listener keeps only out-of-context WinEvent hooks, which make no cross-process call, and its one job becomes noticing an application that has no outpost yet (plus the menu and switcher ends that go to Core).
- Inside each outpost, two stages, as NVDA has: a UIA receiving thread that does NVDA's flusher-thread work for every UIA event of the application in arrival order (the hung-window check, the duplicate evidence, acceptance, and for a focus the live `HasKeyboardFocus` read on the sender), then the worker, which handles every event, MSAA and UIA, in the order it reached it. There is no focus lane and no reordering: a focus is judged as early as NVDA judges it, and then waits its turn.
- A new application's first focus comes from one of two reserved spare outposts. One of them, the standby, holds the desktop's only desktop-wide UIA focus registration, with a three-property cache, and keeps the recent focus events of applications that have no outpost; given to an application, it takes that application's buffered events, sender elements included, as its first events. The listener's WinEvent tells the supervisor which application to give it to. Served outposts register focus only on their own windows if UIA allows it (experiment E-scope), so no application's event is received by every outpost (section 2.7 counts the cost per focus change for each option; section 2.8 times the first focus of a new application).
- Core receives outpost events and key commands on one channel, in arrival order, as NVDA queues gestures with events. Core applies NVDA's duplicate rule using the outpost's live evidence, and drops background speech observed before a foreground change it has applied.
- MSAA keeps NVDA's limiter for MSAA only, keyed by the event's own thread, with focus collapsing only within consecutive runs of up to four, and without `msaa_focus_waiting`'s drop.
- Every outpost sets UIA's transaction and connection timeouts once, for the whole process; `Uia::within` is retired; MSAA calls and window messages get bounds or a cancel; every thread that others wait on has a watchdog.
- The UIA probe asks once, trusting a "no" for 0.5 seconds, as NVDA does.
- This removes the moved-on rules, the name-and-position lookup, the focus-window following, `overtaken` and the take-back, the console special cases, `GetFocusedElement` on the focus path, and the UIA side of the listener.

## 1. What is wrong today, in one paragraph

The UIA focus event's sender lives in the listener process, so the outpost never has it: it has to find the element again, which is why it calls `GetFocusedElementBuildCache` (`crates/verbatim-uia/src/client.rs` 189 to 197, from `live_focus_element`, `crates/verbatim-outpost/src/outpost/worker.rs` 3214), searches for the element by runtime id and then by name and position (`element_of_moved_focus`, worker.rs 3092; `Uia::element_by_name_and_position`, client.rs 324), and follows the focus's whole window until the element turns up (`follow_focus_window`, worker.rs 3282; `adopt_focus_element`, 3322). It judges the focus when the worker reaches it, often after it has moved on, which is why the moved-on rules exist (`uia_focus_element`, worker.rs 2933). The focus arrives by a different route (listener, Core, supervisor, outpost reader) from the application's other events (the outpost's own hooks), so an event raised after a focus can be handled before it (audit D3). Each of these was patched where it showed up. The design below removes the cause: the outpost receives its own application's events, focus included, with the sender in hand.

## 2. Where focus is judged and read: three options

Dickson's constraints for any option:

- something must still notice a new application, so its outpost is started;
- the same event must not be processed twice, by the listener and by an outpost;
- a slow application must never stall the listener or another application.

### 2.1 Option (a): the listener reads the sender's `HasKeyboardFocus` on per-application threads, then the outpost finds the element

How it would work: the listener's UIA focus callback only queues the sender to a thread for its process (one thread per application, created on demand). That thread reads `CurrentHasKeyboardFocus` on the sender, live, as NVDA's `shouldAllowUIAFocusEvent` does (`NVDAObjects/UIA/__init__.py` 1632 to 1637), and forwards the fact marked accepted or drops it. The outpost then has to find the element, as today.

What it fixes: the judgement is made on the sender, early, as NVDA makes it.

What it does not fix, and what it costs:

- The outpost still has no sender. It still needs the runtime-id search and the name-and-position lookup, because Windows Terminal raises its tabs' focus events from elements that are not in its tree (`phase6-design.md` 2835). The "tab control" difference stays (the tab found in the tree has the tab control above its list; the sender has the terminal's container, `docs/parity.md` 2092 to 2095). This is what made `windows_terminal_tabs` fail on the step 6 branch in both runs: the focus was reported with only the window as its ancestor and no position (`runs-step6-fix/windows_terminal_tabs-1.log` 27 to 35).
- Two processes call into the application for one event: the listener's read and the outpost's search. That is the same event processed twice.
- Two routes per application remain: focus through the listener, everything else through the outpost's hooks (audit D3).
- The listener now makes cross-process calls, which D13 exists to forbid. Its per-application threads can each be stuck in a hung application; the listener has no worker abandonment and no way to reclaim them short of being restarted, which loses focus for the whole desktop for the restart.
- The listener's desktop-wide registration itself already makes cross-process calls inside UIA: before each callback, UIA fills the registration's cache request from the application (knowledge, and audit section 1.1 records the same). With a 34-property cache, a hung application can hold UIA's delivery inside the listener; whether that delays other applications' focus events is experiment E-delivery (section 11).

### 2.2 Option (b): the listener does the whole focus read in one remote operation, on per-application threads, and sends the result

How it would work: as (a), but the per-application thread runs the focus's whole remote program (the live check, the states, the ancestors) and sends a finished snapshot.

What it fixes: the judgement and the read are made on the sender; the ancestors are the sender's, so Windows Terminal's tabs are read correctly.

What it costs:

- The outpost never holds the focus element at all. It needs it for the focus-following property and text subscriptions (`register_focus_properties`, `crates/verbatim-outpost/src/outpost/mod.rs` 823; `register_text_events`, 864), for the caret and typed text, for object navigation from the focus, and for review. It would have to find it again, which is the problem we started from, now for every use rather than once.
- Everything listed under (a) about two routes, the listener's calls and hung threads, and the listener's own UIA delivery.
- D9's isolation is lost for focus reads: an IA2 or UIA failure while reading one application's focus would be in the process that serves every application.

### 2.3 Option (c): each outpost subscribes to focus events for its own application

How it would work: every outpost holds its own UIA focus subscription and adds focus, foreground, menu popup start, alert and show to its own process-scoped WinEvent hook (`APP_SUBSCRIPTIONS`, `crates/verbatim-ia2/src/hook.rs` 157). Every event for the application, focus included, reaches its outpost by one route. The sender element is in the outpost, so the judgement, the read, the ancestors and the subscriptions all start from it, as in NVDA.

What UIA supports for scoping focus events:

- `IUIAutomation::AddFocusChangedEventHandler` takes a cache request and a handler, and no element and no scope: it is desktop-wide. Knowledge, consistent with `crates/verbatim-uia/src/focus.rs` 8 to 10 and with NVDA's registration (`UIAHandler/__init__.py` 593).
- `IUIAutomationEventHandlerGroup` has methods for property changes, automation events, notifications, active text position, structure changes, text edit text changes and changes events, and none for focus (knowledge; NVDA's group, built at lines 594 to 640, also leaves focus out and registers it separately at 593). So groups do not help with focus directly.
- Whether `UIA_AutomationFocusChangedEventId` can be registered through `AddAutomationEventHandler`, or a group's `AddAutomationEventHandler`, on one window's element with `TreeScope_Subtree`: Microsoft's pages, as I know them, direct clients to `AddFocusChangedEventHandler` for focus. Measured (E-scope, section 11): the direct call is refused with `E_INVALIDARG` and the message "Focus events must be registered using IUIAutomation::AddFocusChangedEventHandler"; the same event id added to an event handler group, registered on one window, is accepted and delivers that window's focus events, with the sender and its cache, and no other application's. That route is undocumented, so it needs a live check with real applications before it is relied on.
- Even if it works, a subscription scoped to an application's windows has to be added on each new top-level window after it appears, which takes an `ElementFromHandle` and an `AddEventHandlerGroup`, both calls into the application while it is building the window. A focus event raised in the new window before the registration lands is lost. A new File Explorer folder window in an existing `explorer.exe` is exactly that case. So a scoped focus subscription is not safe on its own; it would need the desktop-wide one beside it.

So (c) has two variants, chosen by the experiments:

- (c1), preferred, since E-scope found a focus registration can be scoped to a window's subtree through an event handler group: each served outpost registers focus only on its own application's top-level windows, so it receives only its own events, and the one desktop-wide registration on the desktop is the standby spare's (section 2.4). The new-window race above remains for a served application; the outpost closes it with its own WinEvent hook, which reports `EVENT_OBJECT_FOCUS` on the new window from Windows: when that window has no registration yet, the outpost registers on it and, if the registration landed after the focus event, makes the one recovery read of section 2.4's last paragraph for that window. E-scope also measures how long a registration on a new window takes, which is the race's width.
- (c2), if scoping is not possible: each outpost holds a desktop-wide focus registration with a minimal cache request (process id, runtime id and native window handle), and its callback drops at once every event whose cached process id is not its application's. That was Verbatim's arrangement before D13, which D13 consolidated because N outposts meant N callbacks per focus change. Every outpost then receives every application's focus events, which is the redundant processing Dickson wants to avoid; section 2.7 counts it.

The costs of (c2), to be measured by E-cost and E-delivery:

- Each focus change in the desktop asks the focused application for the minimal cache once per outpost (knowledge: UIA fills each registration's cache request before the callback). The cache request is three properties instead of today's 34 (`CACHED_PROPERTIES`, `crates/verbatim-uia/src/cache.rs` 63); E-cost counts the provider calls each extra registration adds.
- If a hung application holds UIA's delivery in each client process while it fills the cache, every outpost's focus delivery waits on the hung application for as long as UIA waits (bounded by the transaction timeout, section 6). E-delivery measures whether a held application delays another application's events to a handler in the same process, and whether it delays only focus delivery or also the outpost's own property events. If it does delay them, the per-outpost registration has to move off the outposts that serve applications (section 2.5 has the fallback).
- The outpost's own property, text, selection, menu and notification subscriptions move from the listener to the outpost and are scoped to the outpost's application: the focus-following ones stay on the focus element (they already are), and selection, menu opened and notifications are registered as one group on each of the application's top-level windows. A menu opened in a brand-new popup window before its registration is the same race as above; NVDA's focus event into the menu item follows and is caught by the desktop-wide focus registration, so what can be lost is the "menu" announcement that a UIA `MenuOpened` brings (NVDA turns it into a fake focus). That is a decision for Dickson (section 10, item 13); the alternative is desktop-wide registrations for those three events too, which multiplies their cost by the number of outposts.

How (c) meets the three constraints:

- Noticing a new application: the listener keeps desktop-wide out-of-context WinEvent hooks (focus, foreground, menu popup start, alert and show, plus menu end and switch end for Core), which make no cross-process call. Windows itself raises `EVENT_OBJECT_FOCUS` whenever the keyboard focus window changes and `EVENT_SYSTEM_FOREGROUND` whenever the foreground window changes (knowledge), so any application that takes the focus or the foreground is seen, including UIA-only applications whose providers raise no MSAA focus event for their inner elements (the Start menu's search box, `docs/architecture.md` 528 to 533): the event is on the window, from the system. The reserved spares (section 2.4) are a second detector for the one case without a window change: a UIA focus in a process that owns no window of its own in the focus chain.
- Not processing an event twice: the listener only looks up the window's process (`GetWindowThreadProcessId`, local) and drops the event if that process has an outpost; it forwards events only for an application without one, and only until that application's outpost has its own hooks in (section 2.4). UIA's cache fill for the spares' and outposts' desktop-wide registrations is the one repeated cost, measured by E-cost.
- No stall across applications: the listener makes no call and holds no UIA registration. Each outpost's explicit calls go only into its own application. UIA's own cache fills are the exception, covered above and by E-delivery.

### 2.4 How a new or reserved outpost learns the focus that caused its start

The order of events when an application without an outpost takes the focus:

1. Windows raises the foreground and focus WinEvents for the application's window; its UIA provider may raise a UIA focus event for an inner element before or after them.
2. The listener's out-of-context hook delivers the WinEvents. The listener finds the owning process (for a console window, section 5.4), sees that it has no outpost, and sends the supervisor an `ApplicationSeen` message carrying the event as a trigger fact (window, object id, child id, event thread, time observed). It keeps forwarding that application's focus-shaped WinEvents, and only those, until the supervisor tells it the application's outpost has its own hooks in.
3. The supervisor gives the application one of the two reserved spares (Dickson's decision of 2026-10-10: two spares kept started and initialized, replenished whenever fewer than two remain, never returned once given). The assignment message names the application's process and carries the trigger facts held so far, in order.
4. The spare becomes the application's outpost: it installs its process-scoped WinEvent hook for the application (a local call, about a millisecond) and stamps the time it took effect; forwarded facts observed before that time are taken from the supervisor, and its own hook covers everything after. The listener is told and stops forwarding for that process. It registers its scoped UIA subscriptions on the application's windows in the background (calls into the application, tens of milliseconds).
5. The first focus:
   - an MSAA focus: the trigger fact carries the event's address, and the outpost acquires the object with `AccessibleObjectFromEvent` exactly as it would have from its own hook. No extra cost.
   - a foreground change: the trigger fact carries the window; the outpost checks it against the system's foreground window when it handles it, as today.
   - a UIA focus: only one of the two spares, the standby (the one the supervisor will give out next), holds a desktop-wide focus registration, with the minimal cache, and keeps the focus events of applications that have no outpost: per application, coalesced one per element as NVDA's limiter keeps them, in arrival order, the newest four. The other spare holds no registration, so the spares do not both receive every application's events. On assignment the standby takes the application's events from its buffer, sender elements included, and puts them into its UIA receiving thread first, ahead of anything its new subscriptions bring; the rest of the buffer is dropped. The second spare then becomes the standby and registers (`AddFocusChangedEventHandler` is a registration in the client process; its cost is measured in E-cost), and a new spare is started. The supervisor tells the standby which applications have outposts, so its buffer holds only the others; an event for a served application is dropped in the callback by comparing its cached process id, a local read.

What it costs:

- Two spares running at all times: two outpost processes, each with a UIA client and a WinEvent thread, measured at about 40 ms each to start in a debug build (`docs/roadmap.md` 186 to 193). Working set to be measured (risk R2).
- The standby's desktop-wide registration: one minimal cache fill per focus change on the desktop. It replaces the listener's registration, which today makes one 34-property fill per focus change, so the desktop's cost per focus event does not rise; it moves and shrinks (section 2.7).
- A standby that a hung application holds in UIA's delivery misses the events behind it until UIA gives up (bounded by the transaction timeout). E-delivery says whether this happens. Today the listener has the same exposure, for every application at once.

When no spare is ready (two applications appear within the 40 ms a replacement takes, or a spare has failed), the outpost started fresh has no buffer. Its MSAA and foreground trigger facts are handled as above; a UIA-only first focus is not known. The options for that rare case are in section 10, item 12: announce only the window and wait for the next focus event (no read at all), or ask once for the focus through the existing focus-now query (`Query::FocusNow`, worker.rs 741), which uses `GetFocusedElement` after a local `GetGUIThreadInfo` check that the focus window belongs to the application. The second is a recovery read, not the focus path, but it is the call Dickson ruled out there, so it is his decision.

### 2.5 Fallbacks if the experiments say no

- If E-delivery shows that one held application delays another application's events to a handler in the same process, the outposts that serve applications must not hold a desktop-wide registration. Then only the spares hold one, and the served outpost relies on scoped focus registrations on its windows (only if E-scope says they work). A spare cannot help with the new-window race, since an element cannot be handed across processes; the race can only be closed by a single read of the new window's focus when the outpost's own WinEvent hook reports focus on a window it has not registered yet. That read is again `GetFocusedElement` or a subtree search, so this fallback is worse and goes to Dickson with the measurements.
- If E-scope shows scoped focus registration is not possible and E-delivery shows cross-application delays, option (c) as recommended cannot meet the third constraint for UIA focus, and the choice is between (c) with that recorded exposure, bounded by the transaction timeout, and (a). I expect neither "no" (knowledge: UIA's client side handles each event on its own work item), but this is what the experiments are for.

### 2.6 Recommendation (rejected; see section 12)

Rejected by Dickson on 2026-10-10: the standby spare discards the buffered events of applications it is not given to. What follows was the recommendation. Option (c), variant (c1): each served outpost's focus registration scoped to its own windows through an event handler group (E-scope: it works, undocumented, live check first), one desktop-wide minimal-cache registration held by the standby spare, everything else scoped to the application, and the listener reduced to WinEvent hooks. Variant (c2) is ruled out: E-cost measured each focus registration adding 47 to 90 provider calls on the application's UI thread per focus event, so a registration in every outpost multiplies that by the number of outposts. If the live check finds the group route does not deliver real applications' focus events, the choice falls back to (a) with the listener's per-application threads (E-concurrency shows those do not stall each other), and goes back to Dickson. It is the only option that gives the outpost the sender, which is what NVDA judges and reads from; it makes one route per application; it keeps every explicit call in the application's own outpost; and it makes the standby spare the source of a new application's first UIA focus.

### 2.7 What one focus change costs the desktop, per option

Counted for one UIA focus change in an application that already has an outpost, with N outposts running. A "cache fill" is UIA asking the focused application for a registration's cached properties before its callback runs (knowledge; E-cost counts the provider calls each fill makes). A "local discard" is a callback that reads its cached process id and returns, microseconds and no call. "Calls" are explicit calls into the focused application.

- Today: one 34-property cache fill (the listener); no discards; in the outpost, `GetFocusedElementBuildCache` and one remote program in the steady state, plus up to two subtree searches when the focus has moved on (audit section 3.4: the focused-element read had a median of 574 ms on a first File Explorer focus in release; the searches up to 633 ms).
- (a): one cache fill (the listener's, which could shrink to the minimal cache); in the listener, one live `HasKeyboardFocus` read; in the outpost, the element found again (`ElementFromHandle` and a subtree search, or `GetFocusedElement`) and one remote program. The same event is handled in two processes, and two of its calls are made from the listener.
- (b): one cache fill; in the listener, one remote program; in the outpost, the element found again whenever it is needed for subscriptions, text or navigation, at least once per focus. The same event is handled in two processes.
- (c1): two minimal cache fills, the owning outpost's scoped one (measured at 47 to 90 provider calls inside the application per focus event in mockapp, E-cost, most of them UIA testing the element against the registration's scope) and the standby's desktop-wide one (not measurable in mockapp); one local discard (the standby's); in the owning outpost, one live `HasKeyboardFocus` read (stage 1) and one remote program (the worker). The listener's WinEvent callback is one local `GetWindowThreadProcessId` and a set lookup, then a discard. No event is handled in two processes; the standby's fill and the listener's lookup are the only repeated work, both smaller than today's listener fill.
- (c2): N plus one minimal cache fills (every outpost and the standby); N local discards; in the owning outpost, the same two calls as (c1). With ten applications open, eleven fills per focus change, each tens of provider calls on the focused application's UI thread at the moment it is busiest (E-cost: three registrations cost 185 provider calls for one focus event against 90 for one). This is the redundancy D13 removed, and it rules (c2) out.

For MSAA events nothing repeats in any option: Windows delivers each WinEvent to each hook, the listener discards a served application's event after one local lookup, and only the owning outpost's process-scoped hook acts on it.

### 2.8 The first focus in a brand-new application under (c), step by step

The case: the user starts or switches to an application that has no outpost. The figures are measured where a source is given and estimates otherwise; phase 2 measures the ones marked "to measure" that mockapp can show.

1. Windows raises the foreground and focus WinEvents; the provider raises its UIA focus event. No cost to Verbatim.
2. The listener's out-of-context hook receives the WinEvents. Delivery waits for the listener's message loop: sub-millisecond normally (each fact carries `raised_ms_ago`). Its own work is one local lookup.
3. In parallel, the standby spare's desktop-wide focus callback receives the UIA event after UIA's minimal cache fill from the new application: a round trip to its UI thread, to measure (E-cost); longer if the application is busy starting. The event is buffered.
4. The listener's `ApplicationSeen` reaches the supervisor: one pipe hop, median 0.05 to 0.15 ms (audit D3's routing measurement).
5. The supervisor gives the application the standby: one more pipe hop, about 0.1 ms. Starting the replacement spare (about 40 ms in debug) happens in the background and is not on this path.
6. The standby installs its process-scoped WinEvent hook for the application: a local call, about a millisecond (to measure), and takes the forwarded trigger facts and its buffered UIA focus events, in order.
7. Stage 1 reads the sender's `HasKeyboardFocus` live: one call into the application, a few milliseconds when it is idle, as long as the application takes when it is busy starting, bounded by the transaction timeout.
8. The worker handles the focus: the provider probe for its window if the window's verdict is unknown (median 83 ms for unnamed windows, up to 3 s, audit section 3.4; the same in NVDA), then the focus's remote program (27 to 31 ms on File Explorer's first focus, audit section 3.4).
9. The report reaches Core: one pipe hop, under a millisecond.

On this path Verbatim adds about 1 to 2 ms of its own (steps 2, 4, 5, 6 and 9); everything else is the application answering, the probe and the program, which NVDA pays too. Today the same focus pays the outpost's start-up, about 40 ms in debug, then `GetFocusedElementBuildCache` (median 574 ms on a first File Explorer focus in release) before the program.

When no spare is ready, step 5 becomes "start an outpost", about 40 ms in debug, and step 3's buffered UIA event does not exist: the MSAA and foreground trigger facts are handled as in step 6, and a UIA-only first focus needs section 2.4's recovery read or waits for the next focus event (section 10.1, item 12).

## 3. One route per application, and two stages inside the outpost

### 3.1 The route

After the change, every event for an application reaches its outpost through the outpost's own hook or handlers:

- MSAA: one process-scoped out-of-context WinEvent hook, on one thread (`EventThread`, `mod.rs` 536), for `APP_SUBSCRIPTIONS` plus focus, foreground, menu popup start, alert and show. WinEvents for one hook arrive on that thread's message queue in the order Windows raised them (knowledge), so MSAA events are in arrival order among themselves.
- UIA: the desktop-wide focus registration (filtered to the application), the focus-following property group, the text group, and one group with element selected, menu opened and notifications on the application's top-level windows. Each callback only puts the event, with its sender, on the UIA receiving thread's queue.
- From the listener, only trigger facts for the gap before the hook is in (section 2.4).

### 3.2 Two stages, as NVDA has

NVDA's UIA events pass through two stages: the flusher thread, which judges and builds objects for every UIA event in arrival order and then queues it (`UIAHandler/__init__.py` 874 to 958 for focus; `nvda/nvdaHelper/local/UIAEventLimiter/rateLimitedEventHandler.cpp` 65 to 101 and 245 to 262), and the main thread, which executes queued events in order. Its MSAA events are judged in `IAccessibleHandler.pumpAll` on the main thread just before the queue is drained (`IAccessibleHandler/__init__.py` 1019 to 1103; `core.py` 1015 to 1031). So NVDA judges a UIA focus a few milliseconds after it arrives, behind only the flusher's work on earlier UIA events, not behind the speech reads of earlier events (`nvda-answers.md` section 1; `verify-focus.md` section 1).

Verbatim's worker today does both stages for every event in one pass, so a focus is judged only after every earlier event has been fully read (up to 2.6 seconds measured in File Explorer, `verify-focus.md` chain E). Matching NVDA:

- Stage 1, the UIA receiving thread, one per outpost, in COM's multithreaded apartment with the outpost's UIA client. It takes the UIA events in arrival order and does NVDA's flusher work for each:
  - every event: the hung-window check on the cached window (`IsHungAppWindow`, local; NVDA's `_shouldSkipEventForHungWindow`, `UIAHandler/utils.py` 438 to 459), and step 3's acceptance test with local reads (`crates/verbatim-outpost/src/outpost/acceptance.rs`);
  - a property, text or automation event: whether its sender is the focus (runtime ids compared locally, from the cache);
  - a focus: NVDA's duplicate evidence (section 4.2), then the live `CurrentHasKeyboardFocus` read on the sender, one call; a sender without the keyboard focus is dropped, as NVDA drops it. The console host's "Console Window" element is refused, NVDA's one console rule (`NVDAObjects/UIA/winConsoleUIA.py` 356 to 358).
  - the UIA provider probe for the event's window when its verdict is not known, since NVDA makes that call on the flusher thread too (`isNativeUIAElement`, `UIAHandler/__init__.py` 1516 to 1578). A window that takes 3 seconds to answer makes its own application's UIA events wait, as Dickson decided (`docs/roadmap.md` 286 to 289), and no other application's.
  - It then puts the accepted event, sender included, on the worker's queue.
- Stage 2, the worker, unchanged in role: it takes everything on its queue in order, MSAA events straight from the hook and UIA events from stage 1, applies the MSAA limiter (section 5), reads what will be spoken (for a UIA focus, the one remote program from the sender: states, value, details and ancestors), and publishes. A focus accepted in stage 1 is reported whatever has happened since; Core culls its speech once a newer focus is applied (`FocusValidity`, `crates/verbatim-core/src/reduce.rs` 791 to 856), as NVDA culls it in `doPreGainFocus` (`eventHandler.py` 386 to 397).

This is not a focus lane: stage 1 handles every UIA event in arrival order, and stage 2 handles every event in the order it reached it. A UIA event and an MSAA event raised close together can reach stage 2 in either order, because the UIA one passes through stage 1 first. NVDA has the same property: its UIA events reach the queue when the flusher finishes them and its MSAA events when the main thread pumps them.

Compared with the step 6 branch's judge (`crates/verbatim-outpost/src/outpost/judge.rs` on `worktree-agent-a27b534fac627b1ae`):

- it judged only focus facts and pushed accepted ones behind events that arrived during its read (audit section 3.1, item 6); stage 1 takes every UIA event in order;
- it judged with `GetFocusedElementBuildCache` and a stand-in comparison; stage 1 reads the sender;
- it compared the stand-in's selection (8be2223) and had to drop that comparison when the tab moved to was dropped (2059792); stage 1 has no tab rule;
- it had no watchdog; stage 1 gets one (section 6.4);
- `Judge::superseded` dropped an accepted focus when a newer one was accepted; here every accepted focus is reported and Core culls.

The step 6 runs' remaining failures map onto this design as follows. `windows_terminal_tabs` (both runs): the stand-in's ancestors and position; gone with the sender. `windows_terminal_typing` and `windows_terminal_up_typing` (first run): a second focus event on the same terminal announced again; NVDA's duplicate rule, section 4.2. `conhost_two_windows` (second run: "terminal" never queued after the window, `runs-step6-fix/conhost_two_windows-1.log` 25 to 30): not diagnosed on the branch; the console split is the likeliest cause, and section 5.4 removes it. That attribution is a guess until the trace is read.

## 4. One ordered input to Core, and NVDA's duplicate rule

### 4.1 One channel

Today the reducer thread selects between `outpost_rx` and `command_rx` (`crates/verbatim-app/src/main.rs` 1602 to 1611); when both have a message, crossbeam picks at random (audit D10). NVDA queues gestures, scripts and the speech cancel for a key on the same `eventQueue` as events (`inputCore.py` 583 to 637; `scriptHandler.py` 242 to 253).

Change: one channel, `ReducerInput`, with two kinds of message, and every sender (each outpost's reader thread in the supervisor, the listener's reader, the router thread for gestures, `InputHandled` and idle barriers) sends on it. A crossbeam channel is first in, first out across all its senders, so the reducer sees inputs in the order they reached Core. The idle barrier's two lengths (`answer_idle_waiters`, main.rs 1613) become one.

Order by arrival, not by observation time: ordering by the time each event was observed would need a reorder buffer that waits for slower outposts, which is a delay on every input, and NVDA does not do it either. Arrival order already puts a key before the application's reaction to it, because the key reaches Core in about a millisecond and the reaction has to be raised, received and read first.

### 4.2 NVDA's duplicate rule, in Core with the outpost's evidence

NVDA ignores a UIA focus event whose sender compares equal to `eventHandler.lastQueuedFocusObject` while that object's element still has the keyboard focus, read live (`UIAHandler/__init__.py` 902 to 924); `lastQueuedFocusObject` is set when a focus is queued (`eventHandler.py` 48 to 67 and 173 to 183), for any application. For MSAA, `processFocusNVDAEvent` makes the same comparison (`IAccessibleHandler/__init__.py` 762 to 783).

Verbatim splits it, since only the outpost can read live and only Core knows the last focus of any application:

- The outpost, in stage 1, compares the sender with the last focus it reported (runtime ids, local). When they match, it reads the earlier element's `HasKeyboardFocus` live, one call, and marks the fact "same element as this outpost's last focus, still focused". For MSAA the worker's existing address and identity comparison (worker.rs 2701 to 2721) supplies the same mark.
- Core drops a focus so marked when Core's last applied focus is that same node. If another application's focus was applied in between, Core's last focus is different and the focus is announced, as in NVDA.
- The marked fact still goes through the worker, so its node and subscriptions stay current; only Core's announcement is skipped.

This replaces `reissue_unless_focused` (worker.rs 3175) and the branch's judge-only rule (2059792), and fixes the open item "A Windows Terminal terminal was once announced twice" (`docs/roadmap.md` 237 to 239).

## 5. The MSAA side

### 5.1 Limits for MSAA only

NVDA's limiter keeps the newest 10 events per thread and the newest 4 focus events per flush, a foreground counted in both (`IAccessibleHandler/orderedWinEventLimiter.py` 10, 59 to 67, 115 to 116). It applies only to WinEvents; UIA events are only coalesced one per element and kind.

- Verbatim's `admit` (`crates/verbatim-outpost/src/outpost/intake.rs` 702 to 764) counts UIA focus, UIA menu opened, UIA selection and UIA notifications against those limits (`classify`, 616 to 689; audit D5). Change: only MSAA entries are counted. UIA entries are only coalesced (`push`, 402 to 430, already does that).
- The per-thread count is keyed by the window's thread today (`window_thread`, `crates/verbatim-outpost/src/outpost/window.rs` 264), and the hook drops the event's own thread id (`win_event_proc`, hook.rs 343 to 374, the `_thread` parameter). Change, as Dickson decided (`docs/roadmap.md` 217 to 218): the hook passes the event's thread id and the limiter counts by it. It differs from the window's thread for console windows, whose events come from the console host's thread (NVDA records it, `IAccessibleHandler/internalWinEventHandler.py` 151 to 153).
- `FOCUS_CANDIDATES` (intake.rs 261) and `uia_focus_plan` (905) go: every UIA focus accepted in stage 1 is handled, in order.

### 5.2 Collapsing consecutive focus runs

NVDA's `pumpAll` walks the flushed events in order, collects each consecutive run of focus and foreground events, and when a non-focus event comes processes the run newest first, stopping at the first valid one, before that event (`IAccessibleHandler/__init__.py` 1061 to 1075). A run keeps its place among the events around it. Menu start, menu end and switch end become a fake focus tried last (1077 to 1079 and 1093 to 1103).

Verbatim collapses all of a batch's MSAA focus events into one group of the newest 3, placed where the newest was (`plan`, intake.rs 775 to 887; `msaa_focus_plan`, 892; audit D4). Change: `plan` walks the batch in order; each consecutive run of MSAA focus and foreground entries becomes one group, tried newest first until one is reported, at the run's place; the limiter's 4 is the cap. Only the newest foreground per batch is kept, as today (it matches the run collapse).

### 5.3 `msaa_focus_waiting`

`msaa_focus` drops a focus when a newer MSAA focus in the same window is waiting for the next batch (worker.rs 2740; `Intake::msaa_focus_waiting`, intake.rs 526 to 530). NVDA has no such drop: while its main thread processes a pump's events, no newer WinEvent is visible. Change: removed. Its only purpose was to shorten long batches, which stage 1 and arrival order make shorter anyway.

### 5.4 Console windows

Decided by Dickson (`docs/roadmap.md` 276 to 280): every event for a console window goes to the console host's outpost, with the client process as the application's identity, as NVDA's app module does (`NVDAObjects/UIA/__init__.py` 2336 to 2346; `appModuleHandler.py` 227 to 238).

- The listener finds the console host from the WinEvent's own thread (`GetProcessIdOfThread` on the event's thread id, local), for a window of class `ConsoleWindowClass`, as NVDA's `consoleWindowsToThreadIDs` does. That is the process it checks for an outpost and the one given a spare.
- The console host's outpost already receives the console's UIA events (its provider is in the console host). Whether its process-scoped WinEvent hook receives the console window's focus and foreground events (Windows raises them from the console host's thread, knowledge) needs a live check before this step; mockapp cannot show it.
- Each event the outpost reports for a console window carries the client's process id (`GetWindowThreadProcessId` on the window) as its application identity for Core.
- Then `report_foreign_window` for consoles (54f9a26, worker.rs 2632 to 2656), 3f4dbb4's console rule, and the worker's and judge's console rules go (audit section 4). `report_foreign_window` stays for `ApplicationFrameHost` and Settings. The elevated console's two names (`docs/roadmap.md` 267 to 272) should become one name, read by one outpost.

## 6. Time limits

### 6.1 UIA: one transaction timeout and one connection timeout per outpost

`Uia::within` (client.rs 208 to 219) sets only `ConnectionTimeout`, restores a fixed 10 seconds rather than the earlier value, and does not bound a call on an element already fetched or a remote operation; those run under UIA's `TransactionTimeout`, 20 seconds by default and never changed outside tests (`focus-read-timeout.md` sections 1 and 2; `crates/mockapp/tests/remote_ops.rs` 416 to 476).

Change:

- Each outpost process sets both timeouts once, at start, on its first client, and `create_client` (client.rs 910 to 923) stops setting the connection timeout on every new client: E1 measured both timeouts as process-wide, so today any thread creating a client resets the whole process to 10 seconds. Because each outpost serves one application, a process-wide setting is a per-application setting, which is what we want.
- The transaction timeout bounds each call, not an operation (E-TT): a `FindFirst` held 13.2 seconds under a 1-second timeout, and a registration timed out but reported success. Operations made of many calls get a deadline of their own or become one remote operation; a registration that took longer than the timeout is treated as failed and made again.
- `within` and `FOCUS_READ_WAIT` are removed; with `GetFocusedElement` off the focus path, no caller needs a shorter wait than the process's.
- The value is Dickson's choice. NVDA leaves 2 and 20 seconds and relies on its watchdog, which cannot cancel a UIA call (knowledge; `focus-read-timeout.md` section 5). Recommendation: 5 seconds for both, under the worker's 10-second deadline (`HANDLING_DEADLINE`, worker.rs 84), so a slow call fails and the worker carries on instead of being abandoned, and over the 2 to 3 seconds Notepad and the Start menu's search window took while starting (`crates/mockapp/tests/slow_application.rs` 5 to 9). E3 and E-TT check that the transaction timeout bounds a held provider's `GetFocusedElement`, property reads and `HasKeyboardFocus`.
- The listener has no UIA client after the change; the spares set the same timeouts.

### 6.2 MSAA calls

`AccessibleObjectFromEvent` and every `IAccessible` call have no bound except the worker's watchdog, which abandons the worker without cancelling the call (audit section 3.5). NVDA's watchdog calls `CoCancelCall` on its main thread (`watchdog.py` 237) after enabling cancellation on it.

Change, proposed: the worker calls `CoEnableCallCancellation` once, and the watchdog first calls `CoCancelCall` on the worker's thread when an entry passes its deadline; only if the call does not return within a short grace is the worker abandoned, as today. Whether a cancel reaches a call through oleacc's marshalling is knowledge I am unsure of; it needs a mockapp test with the MSAA backend and `stall` before it is relied on, and abandonment stays as the backstop either way.

### 6.3 Window messages

Tree view messages use plain `SendMessageW` (`crates/verbatim-ia2/src/window.rs` 165 to 180), unbounded. List view and edit messages already use `SendMessageTimeoutW` with `SMTO_ABORTIFHUNG` and 500 ms (`list_view.rs` 28, `edit.rs` 63). Change: tree view the same.

### 6.4 Watchdogs for every thread others wait on

- The worker: as today (worker.rs 555 to 607), including the 500 ms abandonment when the foreground moves to another thread of the same application (`MOVED_ON_GRACE`, 103), which is NVDA's `MIN_CORE_ALIVE_TIMEOUT` rule. For an entry with no window, the check should use the foreground window's process (`focus-read-timeout.md` fix 3).
- Stage 1, the UIA receiving thread: new, with the same watchdog: past its deadline it is abandoned and replaced, the entry in hand dropped (as NVDA loses a cancelled call's event), and later events continue.
- The subscription threads: `Registration::settle` and `close` wait on them (`crates/verbatim-uia/src/subscribe.rs` 212 to 241), and their moves call into the application. With the transaction timeout set, those calls are bounded if UIA applies it to registration calls; E-TT checks `AddEventHandlerGroup` too. If it does not, `settle` gets a deadline after which the registration is abandoned and a new one made.
- The listener, Core's reducer and the supervisor's owner make no calls and wait on nothing that does.

## 7. Background speech when the foreground changes

NVDA executes events in queue order, and a foreground change's `event_foreground` calls `cancelSpeech` (`docs/parity.md` 296 to 302). So speech for a background event queued before the foreground change, such as a toast, a help balloon or a snap result from the application being left, is cut. Verbatim already cancels speech when it applies a foreground change (reduce.rs 639 to 641 and 766). The gap is a background event from another outpost that reaches Core after the foreground change was applied, though it was observed before it (audit D9): Verbatim speaks it, NVDA would not.

Change: Core keeps the observation time of the last applied foreground change; a background event (`Acceptance::Background`, reduce.rs 142 and 373) observed before it is dropped, the same comparison `is_stale_focus` makes for focus (reduce.rs 237 to 250). Background events observed after the change, and attended events, are unaffected.

## 8. The UIA probe asks once

`probe` (`crates/verbatim-uia/src/probe.rs` 60 to 91) asks `UiaHasServerSideProvider`; when the answer is "no" and took longer than a second, it waits for the window to answer `WM_NULL` and asks again, up to 8 seconds in all (`PROBE_BUDGET`, 22). NVDA asks once (`UIAHandler/__init__.py` 1280) and keeps a "no" for 0.5 seconds (1368 to 1381); Verbatim already keeps a "no" for 0.5 seconds (D15 amendment of 2026-10-02, `docs/architecture.md` 257 to 262).

Change: one ask; its answer is the answer; `responds`, `ANSWERED_WITHIN` and the second ask go, and `PROBE_BUDGET` with them. This was added for Windows 11 Notepad's text control, which answered "no" after 3 to 5 seconds while Notepad was starting and "yes" 18 ms later (probe.rs 11 to 16). With one ask, such a window is read through MSAA for the next half second, as NVDA reads it. That is a possible audible change, item 11 in section 10.

## 9. What becomes removable, and what stays

Removable once the steps in section 10.2 are in:

- Intake: `overtaken`, `take_back_overtaken` and the ancestor-state exception (42d9caa, ce53dbc; intake.rs 317 to 352, 371 to 394, 843 to 849, 868 to 877); `FOCUS_CANDIDATES` and `uia_focus_plan`; whole-batch MSAA collapsing; UIA's counting in MSAA's limits; `msaa_focus_waiting`.
- Worker: `live_focus_element`, `LiveFocus`, `own_element_focused` (3214 to 3280); the moved-on rules in `uia_focus_element` (2933 to 2992): 2818fd4's report "as the event said", the gone element ("Working on it..."), `holds_element` (5076b5a), 8be2223's tab rule; `element_of_moved_focus`; `reissue_unless_focused`; `follow_focus_window` and `adopt_focus_element` (dd9114f); `FocusQuery::require_focus` and its `NotFocused` answers (the program no longer judges; stage 1 has); the console rules (2944 and 3257 at the audit's commit) and console `report_foreign_window`; `FOCUS_READ_WAIT`.
- `verbatim-uia`: `Uia::within`, `element_by_name_and_position`, the probe's second ask and `PROBE_BUDGET`. `focused_element` stays only for the focus-now query after a restart.
- Listener: the UIA focus registration, the desktop-wide selection, menu and notification group, `uia_focus_fact`, `focus_window_for` (moves to the outpost for windowless senders), and routing by a UIA element's process.
- Supervisor: routing of UIA facts; `HeldFacts` stays for trigger facts.
- Core: the random `select!`.
- Tests that pin the removed behaviour: in `crates/mockapp/tests/focus_reports.rs`, the moved-on and stand-in tests and the follow-the-window test; in `remote_ops.rs`, `an_element_that_lost_the_focus_returns_early` if `require_focus` goes.

What stays:

- One outpost per application (D9), its worker, the watchdog and abandonment, the kill-and-respawn ladder.
- Step 3's acceptance filter and step 4's cheaper reads.
- Core's cross-outpost stale-focus drop (`is_stale_focus`), which gives NVDA's final outcome across parallel applications, and `FocusValidity` culling.
- The outbound merging and the supervisor writer's cap (audit D11), recorded.
- The foreground checks, the shell's staging windows (1a1bc32), and `report_foreign_window` for `ApplicationFrameHost` and Settings. edd0352 (a window announced once shown) is decided in step 7.
- The focus-now query after a listener or outpost restart.

## 10. Audible changes, decisions, and the migration

### 10.1 What a user will hear differently, with NVDA's behaviour and a recommendation

1. A UIA focus whose element lost the keyboard focus before stage 1 read it is dropped. Today it is reported "as the event said" unless a moved-on rule drops it. NVDA drops it (`NVDAObjects/UIA/__init__.py` 1632 to 1637). Cases: File Explorer's file list pane before "Items View" (now dropped like NVDA in every run, not only when the worker was late), File Explorer's "Working on it..." placeholder (dropped or kept as NVDA would; unknown until captured). Recommend: adopt, as decided.
2. Control+Tab: the tab left is announced, then cut when the focus moves on. NVDA queues it and culls it. Decided by Dickson (`docs/roadmap.md` 274 to 275).
3. Windows Terminal's tabs: "tab control" is no longer said before "list"; the tab's position is read from the sender. NVDA says "list", "<title> 1 of 2". Recommend: adopt (it was accepted as a difference only for now).
4. A repeated focus on an element that still has the keyboard focus is not announced again, unless another application's focus came in between. NVDA does the same. Recommend: adopt, as decided.
5. Several MSAA focus events in a row: the newest valid one of each consecutive run is processed, up to 4 per run, instead of the newest of the whole batch (up to 3); an intermediate focus is no longer dropped because a newer one is waiting. Heard only in which containers are announced on the way in. NVDA as described. Recommend: adopt.
6. Bursts of UIA events (selections in a list, notifications, rapid Tab): no longer cut at 10 per thread or 4 focus per batch. NVDA speaks or culls them. Recommend: adopt.
7. Arrival order: a change raised before a focus is handled before it (NVDA says "unavailable" and then the new focus within a window); a menu opened before a focus is handled before it; a change raised just after a focus is no longer handled before it. NVDA as described. Recommend: adopt, as decided (steps 5 and 6).
8. Background speech (a toast, a balloon, a snap result) observed before a switch to another application is not spoken after the switch. NVDA cancels it. Recommend: adopt.
9. Key commands and events reach Core in arrival order: a typed character's echo always before the output it causes; a review command answered after a focus that arrived first. NVDA queues them together. Recommend: adopt.
10. Console windows: announced once, with one name; the application is the shell (for example PowerShell). NVDA as described. Decided.
11. The probe asks once: a window busy for more than about a second when first asked is read through MSAA for half a second (Windows 11 Notepad while starting was the case the second ask was added for). NVDA behaves the same. Recommend: adopt, and check `notepad_` scenarios ten times; if Notepad's first focus is read through MSAA, bring the capture to Dickson rather than restoring the second ask.
12. A new application's first UIA focus when no spare is ready: either only the window is announced until the next focus event, or one focus-now read (`GetFocusedElement` after a local check that the focus window is the application's). NVDA reads the event it was given, so it has no such case. Recommend: the focus-now read, since it is the existing restart path and is bounded by the new timeouts; Dickson's decision because it is `GetFocusedElement`.
13. A UIA menu opened in a brand-new popup window before the outpost's scoped registration reaches it is not announced as a menu (the focus into its item still is). NVDA, with one desktop-wide registration, announces it. Recommend: scoped registration, and measure how often `menu_and_settings_dialog` and the context menu scenarios miss it; if they do, register menu opened desktop-wide in each outpost and accept its cost.
14. Time limits: a hung application's reads fail after 5 seconds instead of 10 to 20; its focus is reported from the event, with its containers unknown, sooner. NVDA waits up to 20 seconds and is unresponsive meanwhile. Recommend: 5 seconds; Dickson to choose the value.

### 10.2 Decisions for Dickson, as a list

- Option (c), the two-stage outpost, and the listener reduced to WinEvent hooks (sections 2 and 3).
- Variant (c1), focus registered on the application's own windows plus one standby registration, or (c2), a desktop-wide registration in every outpost, given E-scope, E-cost and E-delivery's numbers (sections 2.3 and 2.7).
- Selection, menu opened and notifications scoped to the application's windows, or desktop-wide per outpost (item 13).
- The spares buffering applications' focus events, and the no-spare case (item 12).
- The timeout value (section 6.1, item 14), and trying `CoCancelCall` for MSAA calls (section 6.2).
- Core's background drop on foreground change (section 7, item 8).
- The probe's single ask (section 8, item 11).
- Arrival order, not observation order, for Core's single channel (section 4.1).

### 10.3 Migration in small steps

Each step is one change, checked with `cargo xtask ci` and the related scenarios run ten times, as the roadmap requires (`docs/roadmap.md` 112 to 121). Tests are named here by what they assert; each follows `docs/testing.md` (one fixed target, exact assertions, waits for evidence).

1. Time limits (not audible except item 14). Set the transaction and connection timeouts once per outpost process; remove `within` and `FOCUS_READ_WAIT`; bound tree view messages. Tests: mockapp, a held provider's `HasKeyboardFocus`, property read and remote program each return `UIA_E_TIMEOUT` within the timeout and before `release` (E3 and E-TT kept as tests); a tree view message to a stalled window returns within its bound. Scenarios: `explorer_folder_window`, `notepad_editing`, `system_information_tree`.
2. The probe asks once (item 11). Tests: the existing probe tests, with the second-ask test replaced by "a slow no is the answer for 0.5 seconds". Scenarios: the `notepad_` scenarios, `explorer_folder_window`.
3. One channel into Core (item 9). Tests: in `verbatim-app`, a command sent after an outpost message is reduced after it, and the other way round, repeated over many interleavings with no randomness left. Scenarios: `windows_terminal_typing`, `conhost_typing`, `notepad_typed_words`.
4. MSAA limiter for MSAA only, the event's own thread, consecutive runs of up to 4, no `msaa_focus_waiting` (items 5 and 6). Tests: intake unit tests asserting exact plans for runs split by a non-focus event, five focus events in one run, UIA events beyond 10 per thread kept, and a console update counted by its event thread; the `msaa_events` mockapp tests. Scenarios: `menu_and_settings_dialog`, `system_information_tree`, `rapid_tabbing_in_settings`, `explorer_folder_window`.
5. The outpost receives its own focus (option c), stage 1, and Core's duplicate rule (items 1, 3, 4). The outpost adds its focus registration ((c1) or (c2)), the scoped selection, menu and notification group, and focus, foreground, menu popup, alert and show to its own hook; the supervisor stops routing listener facts for a process that has an outpost, so nothing is handled twice; stage 1 judges from the sender; the worker reads from the sender; Core drops marked duplicates. The listener still forwards UIA focus facts for a process with no outpost, handled as today, until step 6. The moved-on rules, name-and-position, focus-window following and 8be2223 go. Tests: mockapp, a focus whose sender lost the focus before stage 1 is dropped; a focus accepted and then moved on is reported; a repeated focus on a still-focused element is marked and Core drops it; one with another application's focus between is announced; a focus raised from an element outside the tree is read with that element's own ancestors (mockapp needs a fixture node not reachable from the root, as Windows Terminal's tabs are); stage 1 judges a focus while the worker is held in a call (the branch's `a_focus_queued_behind_slow_reads_is_judged_on_arrival`, kept). Scenarios: the list in the step 6 section of `phase6-design.md` (on the branch), all `windows_terminal_` and `conhost_` scenarios, `explorer_folder_window` ten times.
6. Reserved spares and the WinEvent-only listener (item 12). Two spares, replenished, never reused; the standby holds the desktop-wide focus registration and buffers unserved applications' focus events; the listener loses its UIA registrations and sends `ApplicationSeen` with trigger facts. Tests: supervisor unit tests for assignment, replenishment and never reusing a spare; mockapp, an outpost given an application takes that application's UIA focus raised before the assignment, with its sender; an MSAA trigger fact handed over before the hook is in is handled once, and an event after it comes from the outpost's own hook. Scenarios: `explorer_folder_window`, `second_application_and_verbatim_menu`, `settings_system_page`, and the start-up of every scenario.
7. Arrival order in the intake (item 7): remove `overtaken`, the take-back and ce53dbc's exception. Tests: intake unit tests with exact plans in arrival order; the `msaa_events` ancestor-state tests keep their expectations, now from order rather than the exception. Scenarios: `menu_and_settings_dialog`, `settings_dialog_keys`, `explorer_folder_window`, `theme_panel`.
8. Console windows in one outpost (item 10). Needs the live check of section 5.4 first. Tests: listener unit test routing a `ConsoleWindowClass` event by its thread's process; Core reducer test that a console focus carries the client as its application. Scenarios: every `conhost_` scenario, `conhost_raised_flood`.
9. Background speech dropped across a foreground change (item 8). Tests: reducer tests, a background event observed before an applied foreground change is not spoken, one observed after it is. Scenarios: `explorer_folder_window` (snap results), `second_application_and_verbatim_menu`.
10. Watchdogs for stage 1 and the subscription threads, and `CoCancelCall` for MSAA if its mockapp test shows it works. Tests: mockapp, stage 1 abandoned on a held provider and the next UIA event handled.
11. Documentation: D13 and D14 amended in `docs/architecture.md`, `docs/parity.md` corrected where the audit found it wrong (its lines 1407 to 1427 and 2075 to 2095), `docs/crates/verbatim-outpost.md` and `docs/crates/verbatim-uia.md` for the API changes.

Steps 1 to 4 are independent of each other and of the option chosen. Step 5 is the largest; it can be split into the outpost's own MSAA focus hooks first (one route for MSAA) and the UIA half second, if Dickson prefers smaller pieces.

## 11. Experiments (phase 2)

Run on 2026-10-10 in a debug build, from `crates/mockapp/tests/experiments.rs` on branch `worktree-agent-a29a3c28fd0a090dd` (with three small helpers added to `tests/common/mod.rs`), each experiment in a process of its own on a desktop of its own (`crates/mockapp/tests/common/harness.rs`), against mockapp's server-side UIA provider with the `small.json` fixture. Each was run once; the numbers are single runs. The file prints measurements rather than asserting them and is not meant to be merged; E3, E-TT and E-scope's group case are worth keeping as tests (section 10.3, steps 1 and 5).

Two limits of the setup, which apply to several results below:

- On a test's own desktop nothing has the system focus: `GetFocusedElement` answered the shell's "Desktop 1" element (process 4124), not mockapp. So no experiment could aim `GetFocusedElement` at a held mockapp, and desktop-wide focus handlers did not receive mockapp's focus events at all (E-scope). Those cases need a live check.
- `hold` holds a call inside mockapp's process, whichever UIA component makes it. mockapp's provider is server-side, so UIA fills caches inside mockapp; a client-side proxy, as for plain Win32 windows, would make those calls from the client process instead. E-delivery covers only the server-side case.

### E-concurrency: one held application does not stall a read of another

Thread A read mockapp A's window name and was held (`GetPropertyValue`). While it was held, a second client on another thread connected to mockapp B in 16 ms and read its name in 0 ms; a third thread with a new client connected and read B in 1 ms. A's read returned 23 ms after A was released. So two threads in one process call two applications independently; nothing in UIA's client serializes them. A listener or outpost that calls into one application per thread is not stalled by another application.

### E-delivery: one held application does not delay another's events in the same process

A group-scoped focus registration on both windows and a name-change registration on B's window, in one test process. With mockapp A held while raising its own focus event (the hold caught UIA's `ProviderOptions` call inside A, in A's raise), B's focus event reached the handler 1 ms after B raised it and B's name change 0 ms after; A's focus event arrived once A was released. So UIA delivers events per providing process, and a held provider holds only its own events. This was measured with a server-side provider only (see above). Whether a client-side proxy's cache fill, which runs in the client, holds the client's delivery of other applications' events is not shown; Win32 controls proxied by UIA are that case, and it needs a live check with a busy classic application.

### E-scope: a focus registration scoped to one window works only through an event handler group

- `IUIAutomation::AddAutomationEventHandler(UIA_AutomationFocusChangedEventId, A's window, TreeScope_Subtree)` is refused: `E_INVALIDARG`, with UIA's own message "Focus events must be registered using IUIAutomation::AddFocusChangedEventHandler".
- The same event id added to an event handler group (`IUIAutomationEventHandlerGroup::AddAutomationEventHandler`) and the group registered on A's window (`AddEventHandlerGroup`, which is what `verbatim_uia::Registration` does) is accepted. It delivered A's two focus events, with the sender and its cache (process id and name), and not B's.
- A desktop-wide `AddFocusChangedEventHandler` in the same process, and a group with the focus event registered on the desktop's root element, received none of mockapp's focus events. On this desktop nothing has the system focus, so my reading (knowledge, unverified) is that UIA's desktop-wide focus handler reports only focus changes it confirms with its own focus tracking, while a group scoped to a window passes on what the provider raises.

What this means for the design:

- Variant (c1) is possible, but only through a behaviour Microsoft does not document and the direct call explicitly refuses. It could change in a Windows update. Before step 5 is built on it, a live check must show that a group-scoped focus registration receives focus events from real applications: a UIA application (Windows 11 Notepad, Windows Terminal, File Explorer's item view) and a classic Win32 application whose UIA comes from UIA's proxy (the Windows Forms text box), whose focus events UIA derives from WinEvents rather than from a provider.
- A scoped registration passes on focus events the desktop handler would not have reported. Stage 1's live `HasKeyboardFocus` read is NVDA's check for exactly that and drops them.
- The standby spare's desktop-wide registration could not be exercised here; its behaviour is the listener's today, which works live.

### E-cost: what a focus registration costs the application

Provider calls counted inside mockapp for one focus event:

- no registration: 1 call (`ProviderOptions`, from raising the event);
- one group-scoped focus registration with the four-property minimal cache: 90 calls (4 `WM_GETOBJECT`, 39 `ProviderOptions`, 16 `HostRawElementProvider`, 10 `Navigate`, 8 `FragmentRoot`, 7 `GetPropertyValue`, 6 `GetRuntimeId`), the callback running 1.9 ms after the command;
- three such registrations, each its own client: 185 calls, the callbacks after 2.4, 1.7 and 0.9 ms.

So each registration adds about 47 to 90 provider calls per focus event, most of them UIA finding where the element is in the tree to test it against the registration's scope (`Navigate`, `FragmentRoot`, `HostRawElementProvider`), not the four cached properties. These are calls inside the provider's process, not cross-process round trips each, but they run on the application's UI thread. Under (c1) only the owning outpost's scoped registration and the standby's desktop one see an application's focus event; under (c2) every outpost's would, so (c2) multiplies this cost by the number of outposts and is ruled out. The desktop-wide registration's cost could not be measured here (it received nothing).

Registration times: the first group-scoped registration, including its thread and client, 22 to 25 ms; later ones on the same window 1 to 3 ms. That is the width of the new-window race for (c1) when the application answers at once.

### E1: both timeouts are process-wide

A client on one thread set `ConnectionTimeout` and `TransactionTimeout` to 1000 ms; a client created earlier on another thread then read 1000 and 1000. Before that it read 10,000 and 20,000. So both settings belong to the process, not the client. `create_client` (client.rs 910 to 923) sets the connection timeout to 10 seconds whenever any thread creates a client, which overwrites any shorter value `within` has set on another thread. This confirms hypothesis H1's mechanism in `focus-read-timeout.md`: the 10-second `GetFocusedElement` in File Explorer ran under the 10-second connection timeout that another thread's client had set.

### E2: a stalled window and `within(1 s)`

- `within(1 s, focused_element)` with mockapp stalled for 4 seconds returned in 1 ms with the shell's desktop element, because mockapp did not have the focus here (see the limits above). Inconclusive for `GetFocusedElement`.
- A fresh client's `within(1 s, ElementFromHandle)` on the stalled window returned after 1,007 ms with `UIA_E_TIMEOUT` (0x80131505), an error, not a stand-in. So `ConnectionTimeout` does bound the connection step.

### E3: a held transaction, never released

The element was fetched first, so the held call was a transaction (`GetPropertyValue` for `HasKeyboardFocus`), run inside `within(1 s)`:

- with the default transaction timeout it returned after 22,111 ms with `UIA_E_TIMEOUT`;
- with `TransactionTimeout` at 1 second it returned after 1,092 ms with `UIA_E_TIMEOUT`.

`within` does nothing for a transaction; the transaction timeout bounds it. (A first version of this experiment fetched the element after the hold, so the held call was the connection, `GetObject`; it returned after about 1 second under `within`, which agrees with E2.)

### E4: the provider killed during a held call

- Held in the connection step and killed, with the connection timeout at 1 or 10 seconds and connection recovery on or off (four runs): each read returned at once after the kill (0 ms), with an empty name rather than an error.
- Held in a transaction and killed: returned at once with `E_UNEXPECTED` ("Catastrophic failure").

So a provider that exits does not keep a client waiting. Hypothesis H3 in `focus-read-timeout.md` (an exiting server ending on an internal 10-second wait) does not hold for mockapp; H1 is left as the explanation of the 10-second incident.

### E5: an outpost whose focused-element read lands in a held application

An outpost for mockapp A was given A's UIA focus fact, with its focused-element read replaced by a read of mockapp B's window (nothing can give B the system focus here), and B held. The read was held in the connection step (`GetObject`), `within(1 s)` ended it, and the outpost reported A's focus 1,001 ms after the fact. Had the read been held in a transaction, which is where `GetFocusedElement` asks the focused element's provider, E3 shows it would have waited 20 seconds, and the worker's watchdog would have abandoned it at 10. Either way, the outpost for A waits on B, which is the per-application isolation `GetFocusedElement` breaks.

### E-TT: what a 1-second transaction timeout bounds

Each call on an element fetched before the hold, with the transaction timeout at 1 second:

- `CurrentName`: 1,095 ms, `UIA_E_TIMEOUT`;
- `CurrentHasKeyboardFocus`: 1,092 ms, `UIA_E_TIMEOUT`;
- `AddEventHandlerGroup` on the window, held in `FragmentRoot`: returned success after 1,089 ms. The registration call is bounded, but it reports success though the provider never answered, so whether that registration delivers anything afterwards is unknown;
- `FindFirst` over the window's descendants: 13,208 ms, `UIA_E_TIMEOUT`. The timeout bounds each call UIA makes into the provider, not the operation: a search that makes many calls waits a full timeout for each one that the still-held thread does not answer.
- `GetFocusedElement` could not be aimed at mockapp (it answered the desktop at once).

What this means for section 6.1:

- Set both timeouts once, at the outpost's start, and stop `create_client` from setting the connection timeout for every new client, since any client's setting applies to the whole process (E1).
- The transaction timeout bounds single reads, the live `HasKeyboardFocus` read included, and remote operations (`crates/mockapp/tests/remote_ops.rs` 416 to 476), but not operations made of many calls: `FindFirst`, the classic ancestor walk, and a registration on a window. Those need either a remote operation (one transaction) or a deadline of their own checked by the watchdog, as `ENRICHMENT_BUDGET` is checked between hops today. Removing `GetFocusedElement` and the `FindFirst` searches from the focus path (section 9) removes the worst of them.
- A timed-out registration reports success; the outpost should treat a registration that took longer than the timeout as failed and make it again.

### E-marshal: a UIA element cannot be handed to another process

Run on 2026-10-10 (`marshal` module of `crates/mockapp/tests/experiments.rs`, commit f86da7f). An owner process (the test binary started again as a helper, on the same desktop) read mockapp's window element with the base cache request and marshaled it; the test process tried to use it.

- `CoMarshalInterface` with `MSHCTX_LOCAL` and `MSHLFLAGS_NORMAL`, into a stream from `CreateStreamOnHGlobal`: succeeded in 0.7 to 1.3 ms, writing a 68-byte standard OBJREF (flags 1). `IUIAutomationElement` is registered for standard marshaling through the type library marshaler (`ProxyStubClsid32` {00020424-0000-0000-C000-000000000046}, type library UIAutomationClient in `UIAutomationCore.dll`).
- `CoUnmarshalInterface` of those bytes in the other process: `E_FAIL` (0x80004005, "Unspecified error"), in three runs.
- `CoMarshalInterThreadInterfaceInStream`: succeeded, writing a 256-byte custom OBJREF whose unmarshal class is {0000033A-0000-0000-C000-000000000046}, the free-threaded marshaler, which the element aggregates, so within a process it travels as a raw pointer. Unmarshaled in the other process: `E_UNEXPECTED` (0x8000FFFF), as expected for an in-process OBJREF.
- Within the owner process, the `MSHCTX_LOCAL` bytes unmarshaled on a single-threaded apartment thread gave a working proxy (`CurrentName` answered). So the standard marshaling path works; it fails only across processes.
- Control: an event handler object of our own (`IUIAutomationEventHandler`, same type library), marshaled the same way, unmarshaled in the other process, and a call through it with no element argument succeeded; the same call with an element argument failed with `E_FAIL`. So COM between the two processes works, and what fails is unmarshaling an `IUIAutomationElement` in a process other than the one that created it, whichever way it travels.
- (c), the element's own `IMarshal`: the `windows` crate's `Com_Marshal` feature is not enabled in the workspace, so `GetUnmarshalClass` was not called directly; the OBJREFs above say what it reports: the standard marshaler for `MSHCTX_LOCAL`, the free-threaded marshaler in-process.
- Not applicable, since no proxy was obtained: whether calls round-trip through the owner, their latency, the owner exiting or leaving its apartment, and events on a proxy. For reference, the same reads made directly in the test process: `CurrentName` 85 to 118 us, `HasKeyboardFocus` 76 to 95 us, a cached read 2 to 3 us, `FindFirst` 2.5 to 4.3 ms, the focus remote operation 9 to 11 ms (debug build).

Conclusion: no route was found, documented or not, for giving another process the event's sender. The process that receives a UIA event is the only one that can read from that element, so the marshal variant of option (b) is not described.

## 12. Option (b): the listener judges and reads each UIA focus, on a thread per application

### 12.1 The shape

- The listener keeps the one desktop-wide UIA focus registration it has today (`install_focus_registration`, `crates/verbatim-outpost/src/listener.rs` 362), with a smaller cache request (process id, runtime id, native window handle, class name, `HasKeyboardFocus`). Its callback only puts the sender on the queue of the thread for the sender's application and returns. Nothing is discarded: every UIA focus event goes to its application's thread.
- One reading thread per application in the listener process, created when that application's first UIA focus event arrives. It takes its application's events in arrival order and, for each, does NVDA's flusher-thread work (`UIAHandler/__init__.py` 874 to 958):
  - `IsHungAppWindow` on the cached window (local); a hung window's event is dropped, as NVDA's `_shouldSkipEventForHungWindow` drops it;
  - the shell's staging windows and the console host's "Console Window" element are refused (local, from the cache);
  - step 3's acceptance test against the foreground window (local reads);
  - NVDA's duplicate evidence: the runtime id compared with the last focus this thread accepted, and if equal, that element's `HasKeyboardFocus` read live;
  - one remote operation from the sender (`focus_ancestry_remote`, `crates/verbatim-uia-rops/src/focus.rs` 253): `HasKeyboardFocus` read live first (`require_focus`, which stops with `NotFocused` and drops the event, NVDA's `shouldAllowUIAFocusEvent`), then the states, value and details, and the ancestors up to the first one whose runtime id the thread already knows, each with its runtime id and native window handle. About 9 to 11 ms against mockapp in a debug build (E-marshal's reference figures).
  - It sends the result as a fact: the snapshot, the ancestors, the runtime ids of the whole chain, the window facts, the duplicate mark, and the time the event was observed.
- The UIA provider probe that NVDA makes in `isNativeUIAElement` stays in the outpost, which keeps each window's verdict; the fact carries the nearest window handle.

### 12.2 Threads and their lifecycle

- A reading thread is created on demand, joins COM's multithreaded apartment and creates its own UIA client. E-concurrency showed separate clients on separate threads read different applications independently.
- It ends when its application exits: the listener holds the process handle (`OpenProcess` with synchronize rights, local) and one waiting thread watches them all, as the supervisor does for outposts. It also ends after a period with no events; the only state it keeps is the last accepted focus element and its ancestors' runtime ids, and losing them costs one fuller ancestor walk next time.
- Microsoft's threading page (knowledge: "Understanding Threading Issues", learn.microsoft.com/windows/win32/winauto/uiauto-threading) asks clients to add and remove event handlers from one thread, and to make UIA calls from handlers only off the UI thread. The reading threads never add or remove handlers; they only read. The registrations stay on the one thread that owns them (`crates/verbatim-uia/src/focus.rs` 125 to 162), so the rule is kept.
- Time limits: the listener process sets `TransactionTimeout` and `ConnectionTimeout` once (E1: both are process-wide), at 5 seconds as section 6.1 recommends. The remote operation is one transaction, so the timeout bounds it (`crates/mockapp/tests/remote_ops.rs` 416 to 476; E-TT). Each reading thread has a watchdog: past a deadline a little longer than the timeout it is abandoned (it returns when UIA gives up), a new thread takes the application's remaining events, and the event in hand is reported dropped. An application whose abandoned threads reach a small cap gets no new thread until one returns, and its events are dropped with a log line, NVDA's hung-window outcome.

### 12.3 How a slow application is contained, and D13

- Explicit calls: each application's reads are on its own thread. E-concurrency: while one mockapp held a read, a read of another from another thread took 0 to 16 ms.
- UIA's own work for the desktop-wide registration: E-delivery showed that, for a server-side provider, a held application holds only its own events; another application's focus event reached a handler in the same process 1 ms after it was raised. For a client-side proxy (plain Win32 windows) UIA fills the registration's cache inside the listener; whether a hung proxied window delays other applications' events there is not measured, and today's listener has the same exposure. The smaller cache request reduces the work. A live check against a hung Win32 window is needed before relying on it.
- The listener's callback, writer, main and registration threads make no calls, so no application can stall them, and the supervisor's ping keeps reaching the listener.
- D13's amendment, proposed: "The listener receives the desktop-wide UIA focus events and judges and reads each one from its sender, on a thread for the sender's application, under UIA's timeouts set for the process and a watchdog on each thread; its callbacks, writer and registration thread make no cross-process call, and a slow or hung application holds only its own reading thread. MSAA events are forwarded with no call, as before."

### 12.4 One route per application

The outpost still receives two streams: the listener's facts and its own hooks and handlers (property, text, selection and MSAA state events). Ordering them:

- MSAA focus, foreground, menu popup, alert and show move into the outpost's own process-scoped hook, so all of an application's MSAA events are one stream in the order Windows raised them. The listener keeps its desktop-wide WinEvent hooks only for an application with no outpost, and forwards that application's events until the outpost reports its hook installed, with the time it took effect; facts observed before that time are taken from the listener, everything after from the outpost's hook. Every event is handled exactly once; a copy the listener forwarded after the hook was in duplicates one the outpost has, so no event is lost.
- For a UIA focus, the listener sends a marker the moment its callback queues the event (no call; the relay's median is 0.05 to 0.15 ms, audit D3), and the result once the reading thread has it. The outpost puts the marker in its intake in arrival order; entries behind it wait until the result or a drop arrives, and the focus is handled in the marker's place. This is NVDA's behaviour: its UIA events pass through the flusher thread in order, so later UIA events wait for a focus's judgement. The wait is the reading thread's time, normally one remote operation.
- The residual race: an event the outpost's own handlers receive before the marker reaches it is handled before the focus. Step 1's measurements put the relay's 90th percentile at up to 36 ms and its maximum at 146 ms, both in the supervisor's owner thread. Every message from the listener to an outpost, markers and the MSAA "forwarding ended" message included, keeps travelling through the supervisor in verbatim.exe; no second route is added (Dickson, 2026-10-10). The relay's latency budget is 1 to 2 ms in the worst case, not only the median: verbatim.exe must never be a source of latency, and the relay's present tail is being investigated separately (scratchpad `supervisor-latency.md`). This design assumes that budget is met. Section 13 shows that even within it, two routes leave a race; a fast relay narrows it but does not close it.

#### The MSAA hand-over, exactly

While an application's outpost installs its process-scoped hook, both the listener's desktop-wide hook and the outpost's hook can receive the same WinEvent. Each copy carries the same identity: event id, `hwnd`, `idObject`, `idChild`, `idEventThread` and `dwmsEventTime` (the callback's arguments apart from the hook handle). Today's hook drops the thread and time (`crates/verbatim-ia2/src/hook.rs` 343 to 374); both are kept from now on.

- `dwmsEventTime` is a `GetTickCount` value, which advances in ticks of about 15.6 ms. Two different events with the same event id, object and thread raised within one tick have identical identities, so identity alone cannot tell "one event seen twice" from "two events seen once each". Matching by time windows would lose one of the two.
- The rule instead counts occurrences: during the overlap the outpost keeps a multiset of identities from each source. A copy that arrives when the other source has an unmatched copy of the same identity is matched with it and dropped; otherwise it is handled and recorded as unmatched. Because both hooks receive every event of the application raised after the outpost's hook was installed, every such event arrives once from each source and is matched once; an event raised before the hook arrives only from the listener and stays unmatched (handled once); an event the listener no longer forwards arrives only from the outpost's hook (handled once). Two identical events in one tick arrive twice from each source and are matched pairwise, so both are handled. No time window, no discard of an unduplicated event.
- The overlap's ends: it starts when the outpost installs its hook (stamped), and ends when the listener, having been told through the supervisor that the hook is in, sends "forwarding ended for this application" after its last forwarded fact, through the supervisor like every fact. Then the outpost clears both multisets. The supervisor relays one listener's messages to one outpost in order, so nothing the listener forwarded can arrive after that message; this ordering is a requirement on the supervisor's relay, to be kept in its tests.
- Whichever copy arrives first is handled at its own place in the outpost's intake; the later copy is only matched.
- Tests (mockapp, MSAA backend, real hooks, the listener's forwarding run in the test process):
  - an event raised before the outpost's hook is installed is handled once, from the listener's copy;
  - an event raised after the listener's "forwarding ended" is handled once, from the outpost's hook;
  - an event raised during the overlap, received by both, is handled once;
  - a unit test of the matcher with two identical identities from each source, in both interleavings, handles two events; and with two from the listener and one from the hook (one raised before the hook), handles two.

### 12.5 The outpost's own element, off the focus path

The outpost needs an element of its own for the focus-following property and text subscriptions, the caret watch and text reads, review, and object navigation from the focus. It cannot receive the listener's (E-marshal). So:

- The outpost reports the focus to Core from the listener's result at once, with node ids it assigns; nothing on the way to speech waits for its own element.
- Then, as the next entry of its queue, it finds its own element by the path the listener sent: from the nearest ancestor with a native window handle (`ElementFromHandle`, one call), a remote operation walks down the chain's runtime ids one level at a time, comparing each child's runtime id. One connection call and one transaction, bounded by the timeout; its cost, the depth below that window times the children compared, is to be measured. It replaces today's `GetFocusedElementBuildCache` with a read that stays inside the application.
- Until the element is found, the property and text subscriptions follow the focus's top-level window (dd9114f's `follow_focus_window`, which stays), so a change on the focus in the gap is still heard.
- An element that cannot be reached from its window, such as Windows Terminal's tab focus elements, which are not in the tree (`phase6-design.md` 2835), has no outpost element. Its snapshot and ancestors are still right, because the listener read them from the sender, and the focus moves to the terminal a few milliseconds later. Review and navigation from such a focus start from its nearest ancestor that was found.
- Elements for the ancestors are found the same way, only when navigation or review needs them.

### 12.6 The first focus in a new application

1. The application raises its focus events; the listener's UIA callback queues the sender for a new reading thread, and its WinEvent hook sees the application for the first time.
2. The reading thread is created (a thread and a UIA client, a few milliseconds, to measure) and judges and reads the focus (one remote operation, about 10 ms in debug against mockapp, longer for a busy application). In parallel, the supervisor gives the application a reserved spare or starts an outpost (about 40 ms in debug).
3. The result reaches the outpost, which reports the focus to Core, then finds its own element (12.5).

With a spare, the first focus is spoken after about the remote operation's time; without one, after the outpost's start. Nothing is buffered and dropped: the reading thread handles every event of its application whether or not an outpost exists yet, and the supervisor holds the results for an outpost that is starting (`HeldFacts`, `crates/verbatim-outpost/src/supervisor/policy.rs` 131). The spares need no desktop-wide registration.

### 12.7 Cost per focus change

- One cache fill for the listener's registration, with the smaller cache request (today's is 34 properties).
- In the listener: one remote operation, plus one live `HasKeyboardFocus` read on the previous element for a repeated runtime id.
- In the outpost, after reporting: one `ElementFromHandle` and one remote operation to find its own element.
- Today the outpost makes `GetFocusedElementBuildCache` (which reads whichever application has the focus; 574 ms median on a first File Explorer focus in release) and one remote operation, plus up to two subtree searches when the focus has moved on. Option (b) makes about the same number of calls, all inside the application, and none of the outpost's is on the way to speech.
- No other process handles the event: the outposts and the spares register no focus handler.

### 12.8 What becomes removable under (b)

- As section 9 for the outpost's focus judgement: `live_focus_element`, `own_element_focused`, the moved-on rules (2818fd4, 5076b5a, 8be2223), `reissue_unless_focused`, `FocusQuery::require_focus` in the outpost (it moves to the listener), `FOCUS_READ_WAIT`, `Uia::within`, `GetFocusedElement` on the focus path, the console rules and console `report_foreign_window` (with section 5.4), `overtaken` and the take-back, whole-batch MSAA collapsing, UIA's counting in MSAA's limits, `msaa_focus_waiting`.
- `element_of_moved_focus` and `Uia::element_by_name_and_position`: replaced by the path walk over runtime ids from the sender's own chain.
- "Tab control" before "list" in Windows Terminal: gone, since the ancestors are the sender's.
- What stays that section 9 removed under (c): `follow_focus_window` and `adopt_focus_element` (dd9114f), for the gap until the outpost has its element; the listener's UIA focus registration.

### 12.9 Migration in small steps

Steps 1 to 4 of section 10.3 (time limits, the probe, Core's single channel, the MSAA limiter) do not depend on the option and come first. Then:

5. The listener's reading threads, judging and reading each UIA focus from its sender, with the watchdog and time limits; the outpost reports the listener's result instead of judging, and finds its element by path afterwards; Core's duplicate rule. The moved-on rules, name-and-position and `GetFocusedElement` on the focus path go. Tests: listener unit tests for thread creation, per-application order and the watchdog; mockapp, a focus whose sender lost the focus before its thread read it is dropped; a focus is read while another application's thread is held (E-concurrency kept as a test); the outpost finds its element by path and follows it; a focus raised from an element outside the tree is reported with its own ancestors and no outpost element. Scenarios: all `windows_terminal_` and `conhost_`, `explorer_folder_window` ten times, the settings scenarios.
6. Markers and ordering in the outpost's intake (12.4), relayed through the supervisor within its 1 to 2 ms worst-case budget. Tests: intake unit tests that an event behind a marker waits for its result or drop; a mockapp test that a name change raised just after a focus is handled after it.
7. MSAA focus, foreground and menu events in the outpost's own hook, with the listener's handoff at the hook's install time. Tests: an MSAA focus raised before the hook is in is handled once, from the listener; one after it, once, from the outpost.
8. Reserved spares, without desktop registrations.
9. Arrival order in the intake (removing 42d9caa and ce53dbc), console windows in one outpost, background speech across a foreground change, the remaining watchdogs, and the documentation including D13's amendment, as steps 7 to 11 of section 10.3.

### 12.10 Decisions for Dickson under (b)

- Option (b) and D13's amendment (12.3).
- The marker wait in the outpost (later events of an application wait for its focus's judgement, as in NVDA).
- An element the outpost cannot reach by path (Windows Terminal's tabs): reported from the listener's read, review and navigation starting from its nearest found ancestor.
- MSAA focus-shaped events moving into the outpost's hook, with the listener's handoff by install time.
- A live check of a hung Win32 window against the listener's desktop-wide registration (12.3), before step 5.
- From section 10.2, carried over: the timeout value, the background drop, the single probe ask, arrival order for Core.

### 12.11 Experiment E-hung-proxy: results (run 2026-10-10, twice)

- Baseline, P1 answering: the WinEvents raised for the tree view reached none of the test process's registrations (the name change's mapping is turned off by `verbatim_uia::proxy`, as NVDA turns it off; the focus mapping is kept, as NVDA keeps it, but UIA reported no focus for a window that is not the system's focus).
- Control, P1 answering: P2's focus reached its scoped handler 1 ms after it was raised.
- Control, P1 hung (stalled 15 s) and no WinEvent naming its window: P2's focus arrived in 0 ms. A hung application alone delays nothing.
- P1 hung, then `EVENT_OBJECT_NAMECHANGE` and `EVENT_OBJECT_FOCUS` raised for its tree view: P2's focus, raised 200 ms later, reached its handler 14,800 ms after it was raised, at the end of P1's stall. P2's name change never arrived within 25 s at the desktop-wide property registration. P1's events were never delivered. A read of P2 on another thread with its own client took 1 ms throughout.
- The same with the transaction and connection timeouts at 1 second: identical, 14,803 ms. Those timeouts do not bound UIA's own proxy work on a WinEvent.

Conclusion: in one client process, a WinEvent for a hung window that UIA proxies (most likely the focus mapping, the only one left on) stops UIA's delivery of every other application's events to that process until the hung window answers, with no timeout. Explicit reads on other threads are not affected. This is the exposure 12.3 named, and it is real: under option (b) the listener's desktop-wide focus registration, and today's listener equally, can be held by any hung Win32 window that raises or is named in a focus WinEvent, for as long as it stays hung. NVDA shares it (one client, the focus mapping kept). Containing it needs a decision for Dickson: for example, the listener's supervisor noticing that UIA focus facts have stopped while WinEvent focus facts for other applications keep arriving, and restarting the listener's UIA side, which loses the events held; or the focus mapping turned off too, if NVDA-equivalent focus for proxied windows can come from MSAA (the outposts already read such windows through MSAA). Not measured: how long UIA waits on a window hung for longer than 15 s.

#### Containment option 2, tested (2026-10-10, later): clearing the focus mapping does not contain it

Dickson chose to clear the WinEvent-to-focus mapping in the listener. Measured with mockapp (`e_mapping`, `e_hung_cause` in `crates/mockapp/tests/experiments.rs`, commit 8b491dc):

- What the mapping holds, on a client after Verbatim's own clearing: focus is mapped in only two entries, both for class `SysListView32`, from `EVENT_OBJECT_FOCUS` (0x8005). The same entries map "added to selection" (0x8007) and structure changes; a default entry (no class) maps "added to selection", structure changes, async content loaded (from 0x0105) and system alert (from 0x0002). No entry maps menu or tooltip events from WinEvents. Clearing focus changes nothing else in any entry.
- Its reach: per client object. After clearing focus on one client, another client on another thread of the same process still mapped focus, a client created afterwards did too, and another process did too. So the change can never affect NVDA or any other UIA client, and in Verbatim it must be made on the client that holds the focus registration.
- The stall with the mapping cleared: unchanged (14,789 ms against 14,801 ms). The cause, isolated case by case with an 8-second stall:
  - a focus WinEvent for the hung tree view held another application's focus event 7.8 s with the desktop focus handler, with only a group-scoped focus registration (no desktop registration at all), and with the focus mapping cleared;
  - a name-change WinEvent held nothing (2 to 3 ms);
  - with no focus registration of any kind in the process, a focus WinEvent for the hung window held nothing: another application's name change arrived in 1 ms; adding a desktop focus handler made the same name change wait 7.8 s.
- So the stall comes from UIA's own focus tracking, which any focus registration in a process turns on, reacting to every `EVENT_OBJECT_FOCUS` by asking the window it names, and holding all of that process's UIA event delivery while it waits. The mapping is not involved: the tree view's entry does not even map focus. Clearing it would only remove UIA focus events for Win32 list views, which Verbatim reads through MSAA anyway.

What this means for containment: no setting found removes the stall. What does contain it is where the focus registration lives. Under option (b) only the listener holds a UIA focus registration; outposts hold none, so their property and text events are never held by another application's hung window (shown above: no focus registration, no stall). The listener's UIA focus delivery is held while a hung window is named in a focus WinEvent, for as long as it stays hung (no UIA timeout bounds it); its MSAA hooks are on their own thread and are expected to keep delivering (not yet measured). The decision for Dickson is what to do in that state: for example, the supervisor noticing that the listener's UIA focus facts have stopped while its MSAA focus facts for other applications keep arriving, and then taking focus from MSAA alone until UIA recovers, or restarting the listener's UIA side in a separate process. NVDA has the same exposure: one process, one focus registration, and its focus mapping left on.

#### Cross-process containment and MSAA's view of UIA focus (mockapp `e_cross`, commit b0a1989)

A listener stand-in process held the desktop focus registration, a name registration on mockapp B's window, and out-of-context focus and foreground hooks for every process; the test process, an outpost stand-in, held no focus registration, only B's name and text-changed registrations, explicit reads, and a process-scoped hook on another mockapp (MSAA). With mockapp's tree view hung and named in a focus WinEvent:

- the outpost stand-in: B's name change 1 ms, B's text change 0 ms, an explicit read of B 0 ms, its hook's name change 0 ms; the same as the baseline (0 to 1 ms);
- the listener stand-in: its own UIA name event for B 9,697 to 9,699 ms (held for the stall), while its MSAA focus and foreground hooks delivered in 0 ms.

So the stall stays inside the process that holds the focus registration, and inside that process it holds only UIA; its WinEvent hooks keep delivering. Under option (b) a hung window stalls the listener's UIA focus only, and MSAA focus facts keep arriving for every application.

What MSAA sees of a UIA provider's focus: each `UiaRaiseAutomationEvent` focus from mockapp's provider produced an `EVENT_OBJECT_FOCUS` WinEvent within 0 to 2 ms (UIA's bridge), on the provider's window, with a positive object id that changes per event (3, 1, 2, then 6, 3, 4) and child 0, during the stall as before it. `AccessibleObjectFromEvent` on that address failed with `E_FAIL`, in the callback and afterwards. That may be mockapp's window procedure (it hands UIA only the root object id), so whether a real UIA application's bridge answers is checked live: the probe now acquires every MSAA focus event's object and logs its role and name, for every process, beside the UIA routes.

#### Live coverage results, and Dickson's decision (2026-10-10, runs about 23:10)

From the probe's logs (`target\e2e-artifacts\experiment-*.log` in this worktree):

- An MSAA `EVENT_OBJECT_FOCUS` arrived for every focus change in every target.
- `AccessibleObjectFromEvent` gave a matching, named object for File Explorer's items (role 34, list item: "alpha", "beta", "Inner"), for Settings (Back, the account item), and for Windows Terminal's first window (the command palette's edit, the terminal).
- It gave only an unnamed pane for Windows Terminal's second window.
- Windows 11 Notepad's menu items failed with `E_FAIL`, and its text area came back as unnamed editable text.

So MSAA could stand in for some UIA focus changes but not all, and what it would say differs from the UIA announcement where it works at all.

Decision (Dickson, 2026-10-10): do as NVDA does, with no MSAA fallback. While a hung window holds the listener's UIA focus, UIA focus is simply not announced until the window answers; announcing from MSAA would sound different and confuse users, and a visible failure is better. Option (b) keeps the stall to the listener's UIA focus: outposts' UIA events, explicit reads and every MSAA hook keep working (`e_cross`).

Live scenarios prepared to check coverage (collect-only, `experiment` group): `experiment_focus_mapping_msinfo` and `experiment_focus_routes_msinfo` (msinfo32's Win32 tree and list views, with and without the probe's focus mapping cleared), and `experiment_focus_mapping_text_box` (the Windows Forms text box, cleared). The probe now also logs the MSAA focus hook, so each UIA focus can be matched with the MSAA focus Verbatim would read instead.

The design as it was written before the run:

The gap 12.3 names: UIA proxies a plain Win32 window that has no provider of its own, and the proxy runs in the client process, the listener. Does a hung proxied window hold up the listener's UIA delivery for other applications?

Setup, in mockapp's test harness (one process per test, on a desktop of its own):

- P1: mockapp with `tree_view.json` and the MSAA backend; its comctl32 tree view (`tests/common/tree_view.rs`, `tree_view`) is a Win32 window with no UIA provider, which UIA reads through its own proxy. `stall <ms>` blocks P1's window thread, which owns the tree view, so it stops answering messages: hung, as far as `IsHungAppWindow` is concerned, after its five seconds.
- P2: mockapp with `small.json` and the UIA backend, a server-side provider.
- In the test process, as the listener would have them: a desktop-wide `AddFocusChangedEventHandler` with the minimal cache; a desktop-wide property-changed registration for the name; and a group-scoped focus registration on P2's window (E-scope showed it delivers on this desktop).

Steps:

1. Baseline: `NotifyWinEvent(EVENT_OBJECT_NAMECHANGE)` and `NotifyWinEvent(EVENT_OBJECT_FOCUS)` for the tree view's window, raised from the test process (any process may raise a WinEvent for any window), while P1 answers. Record which registrations receive a UIA event for them, and after how long, and the provider calls nothing (P1 has no provider). If no registration receives them on a test's own desktop, the experiment says so and the live check moves to the e2e machine with a real Win32 application (the step that needs Dickson's go-ahead, since it takes the desktop).
2. Stall P1 for 15 seconds. Raise the same two WinEvents for the tree view, so UIA's proxy in the test process must talk to the hung window to build the sender.
3. 200 ms later (the proxy is by then inside its first call to P1), raise P2's focus (`focus btn1`) and a name change (`set-name btn1 Renamed`). Record when each reaches its handler, against when it was raised and when P1's stall ends.
4. Also, during the stall, a read on another thread of the test process: P2's name through a second client (E-concurrency's case, repeated with a hung proxy rather than a held provider).
5. Repeat step 2 to 4 with the transaction and connection timeouts at 1 second, to see whether they bound the proxy's calls.

What decides it: if P2's events arrive within a few milliseconds of being raised while P1 is hung, UIA's delivery in one process is per event source for proxies too, and option (b)'s listener is contained. If they wait for P1's stall or a timeout, one hung Win32 window delays every application's UIA focus events in the listener, as it does in today's listener and in NVDA's single flusher; the design then needs the listener's registrations split (for example one listener process for proxied windows), which goes to Dickson with the numbers.

## 13. The stale-focus race between the listener and the outpost

### 13.1 The race, exactly

Under option (b) an application's UIA focus events reach its outpost by one route (listener, then the supervisor's relay in verbatim.exe) and its other UIA events (the focus-following property group, the text group, selection, notifications) by another (the outpost's own handlers). The outpost lags the listener by the reading thread's time plus the relay, whose budget is 1 to 2 ms in the worst case.

The window: from the moment the listener's callback receives a newer focus F2 in application A, to the moment A's outpost handles F2's result. During it the outpost may still:

- retarget its focus-following and text subscriptions to F1's element (once its path walk finds it), so for that window it listens to F1 and not F2;
- start or continue a caret watch, a text read or a terminal read on F1;
- handle events its own handlers received from F2's element (raised just after F2 took the focus) while F1 is still its focus, so they are judged against F1 and dropped as not concerning the focus, or, with dd9114f's window-wide following, kept with no element.

The marker (12.4) narrows this: once the marker for F2 is in the intake, later entries wait behind it. But an event from the outpost's own handlers can reach the intake before F2's marker, because the two routes deliver independently. A relay within its 1 to 2 ms budget shrinks the gap to that; it cannot remove it.

The guards today and under (b):

- Core culls F1's focus speech once F2 is applied (`FocusValidity`, `crates/verbatim-core/src/reduce.rs` 791 to 856), and drops a focus observed before a newer one from another outpost (`is_stale_focus`, 237 to 250).
- Results are tied to node ids: a property change, caret move or text read reported for F1's node after F2 is applied concerns a node that is not the focus, and Core speaks such events only for the focus and its ancestors.
- So nothing wrong is spoken about F1 after F2. What can go wrong is loss: a change on F2 raised in the window (a name or value change, the first caret move or typed character after Tab moved into an edit field, a terminal's first output) arrives while the outpost still has F1, is judged against F1 and dropped, and is never repeated. A user would hear the new field announced but miss the first character's echo, or a value change made right after focusing.

How NVDA avoids it: one process, one client, every UIA event passing through one rate-limited flusher thread in arrival order and then one main queue (`nvda-answers.md` section 1). An event raised after F2 cannot be handled before F2. NVDA has a smaller gap of its own: its local property group moves to F2 only when the main thread executes F2's `gainFocus` (`UIAHandler/__init__.py` 697 to 739), so a property change of F2 raised before that is not received at all (knowledge from the source; not measured).

### 13.2 Designs with one route per application and nothing discarded

Three candidates.

**(b1) The listener is the only UIA route.** The listener holds every UIA registration: focus, the focus-following property and text groups per application, selection, menu opened, notifications. Each application's reading thread handles all of its events in arrival order. Everything that needs the element at event time runs there: the focus read, property reads on the focus, the caret read for typed character echo, the terminal's read on a text change. The outpost holds no UIA registrations; it keeps MSAA, and does only explicit reads on elements it finds by path (navigation, review, say-all).

- Closes the race: one route, one order, per application, in one thread, as in NVDA but per application.
- Cost: no extra provider calls. But the listener becomes the UIA half of every outpost: the text and terminal reading code (`crates/verbatim-outpost/src/text`, `terminal`) runs in the listener for UIA, with the anchors and caches it keeps; a crash in it, or in a provider's proxy loaded into it, takes every application's UIA events down until the listener restarts. Abandoned threads are reclaimed only by restarting the listener, for every application at once. That gives up D9's reasons (reclamation, blast radius) for UIA, which is most of what Verbatim reads.
- Also: the focus-following groups move on each focus (retargeting calls into the application), so the listener's per-application threads must also own those registrations' moves, which Microsoft's threading page asks to do on one thread: all registrations would be moved by the listener's one registration thread, a point every application's moves then wait on.
- Risk: high. Recommend against.

**(b2) Hand-over per window: the outpost takes over its windows' UIA events, focus included, at recorded points, and the listener covers the rest.** This is Dickson's first example, made per window, combined with (c)'s scoped focus registration but without (c)'s standby or any discard.

- Each outpost registers, on each of its application's top-level windows, one event handler group holding the focus event (the route E-scope found), the selection, menu opened and notification events, with the focus-following and text groups following its own focus as today. Its focus events carry the sender, so it never needs the listener's read for a window it has registered, and every event of that window reaches it by one route, its own handlers.
- The listener keeps its desktop-wide focus registration and reading threads (option b) for every window an outpost has not yet registered: an application with no outpost, and a new window of a served application in the 1 to 25 ms (E-cost) before its registration is confirmed.
- The hand-over for a window, like the MSAA one: the outpost registers on the window, stamps the time the registration returned, and tells the listener through the supervisor. The listener forwards that window's focus results until it has that message and then sends "ended for this window". In the overlap both routes may carry the same focus event; duplicates are matched by multiset on the sender's runtime id and the event's kind (a UIA event has no timestamp; the runtime id plus the order of arrival from each route stands in). As with MSAA, every event is handled exactly once and nothing unduplicated is dropped.
- Which window an event is in: the cached native window handle's root (`GetAncestor`, local), or for a windowless sender the keyboard focus window of its process (`GetGUIThreadInfo`, local, as today's listener does).
- Closes the race for registered windows, which is almost all the time. The remaining window is the registration delay of a new window, during which the listener's route serves it and the outpost's own handlers have no registration on it, so there is still only one route.
- Cost: for a registered window, two cache fills per focus event (the outpost's scoped registration, measured at 47 to 90 provider calls in mockapp, and the listener's desktop-wide one), and the listener's callback dropping the event after a local window lookup because the window is handed over. That drop is not a discard of an event: the same event has been received by the outpost's own registration. The listener's reading thread makes no call for it.
- Risks: the scoped focus route is undocumented and refused by the direct call (E-scope); a Windows change could stop it, which must be caught by a mockapp test that fails if the group-scoped focus registration stops delivering. Whether it delivers focus events for proxied Win32 windows and for real UIA applications is unmeasured; the live check of 12.11's kind must include it. A registration that times out reports success (E-TT), so the hand-over must be confirmed by the first event arriving by the outpost's route, or the window stays with the listener: the outpost reports the window handed over only when its registration returned in time, and the listener keeps serving a window whose hand-over was not confirmed.

**(b3) The outpost orders its two routes by the time each event was observed.** Both routes stamp a common monotonic clock (`QueryPerformanceCounter`) at receipt; the outpost's intake holds each entry until no earlier entry can still arrive from the other route, which it knows from a watermark the listener sends with every message and, when idle, every few milliseconds, through the supervisor.

- Closes the race in order, without moving any registration.
- Cost: every event of every application waits up to the watermark interval plus the pipe; a heartbeat every few milliseconds is a timer, and the wait is added to every event's latency, against the 1 to 2 ms rule. The F2 lag (the reading thread's read) still delays F2's own handling, and events behind it wait for it, as with the marker.
- Risk: medium; it trades the race for a fixed delay, and the listener's reading time still sits in front of the application's events.

### 13.3 Recommendation: option (b) with Core's guards; (b2) only as an experiment

Option (b), as section 12 describes it, is the plan of record (Dickson, 2026-10-10), with the stale-focus window of 13.1 bounded by the guards there and measured by the scenario of section 14. (b2) is an experiment, not a plan, run only to know whether it works; Dickson expects it not to. The reason is that nothing supports the route it rests on:

- Microsoft's guide to subscribing to UIA events offers only `AddFocusChangedEventHandler` for focus, and the direct `AddAutomationEventHandler` call refuses the focus event (E-scope).
- NVDA's selective registration (NVDA issues 11209 and 11214) deliberately keeps focus global, and registers it with `AddFocusChangedEventHandler` (`UIAHandler/__init__.py` 593).
- James Teh wrote, in Mozilla bug 1654970, "focus event listening is global; you can't restrict it to specific processes".
- No public discussion was found of registering focus in an event handler group. That E-scope saw a group deliver focus events on a test's own desktop, where nothing has the system focus, is an observation of undocumented behaviour, which may not hold for real applications or across Windows updates.

The (b2) live check (prepared on the branch: the four `experiment_focus_routes` scenarios and the probe, below) and E-hung-proxy are run as experiments. (b1) is not recommended; (b3) stays recorded as an option if the window of 13.1 proves too costly.

### 13.4 If (b2) were adopted: the UIA hand-over without duplicates or loss

Kept for completeness, since (b2) is an experiment. The protocol, per window W of application A:

1. The outpost registers its group on W. If the registration call took longer than the transaction timeout, it is treated as failed (E-TT: it reports success anyway), removed, and retried later; the listener keeps W.
2. From the moment the registration returns, the outpost holds every event its W registration delivers (list H, in arrival order) and sends "registered W" through the supervisor.
3. The listener puts "registered W" into A's reading thread's queue, so it takes its place among A's events in arrival order. Events before it are judged and read as usual. From it on, the listener judges and reads nothing for W: for each W event it keeps the sender and sends only an identity record (event kind, runtime id, property id), in order (list S).
4. The outpost matches: S is a run of events raised before the registration and delivered late to the listener (list P), followed by events also in H, in the same order, because UIA delivers one provider process's events to one registration in the order they were raised (assumption A1, as `crates/mockapp/tests/events.rs` relies on). It takes the smallest P for which the rest of S is a prefix of H, asks the listener to read P's events (it still holds their senders) and handles them, then H's events in order.
5. It sends "confirmed W" once S contains a record matched into H; the listener then sends "ended W" after its last record, and frees the senders. Events after the end reach only the outpost.

Exactly where this can still fail:

- No timestamps exist for UIA events, so a run of identical identities (the same kind on the same element) straddling the registration cannot be split: it is handled as one. NVDA's own rate limiter keeps one event per element and kind per flush, and the outpost reads changed properties live, so the outcome is what NVDA would give; but it is a coalescing, not exact once-only handling.
- If the events just before the registration repeat the events just after it in the same order, and the listener receives the earlier ones late, step 5 can confirm on a pre-registration event; a pre-registration event delivered to the listener after "ended W" is then lost. This needs a repeated sequence and a delivery lag longer than the relay's round trip.
- Everything rests on A2: a registration that returned in time delivers every event raised after it. If it does not (the live check exists to find out), H misses events, they appear in S unmatched, are treated as P and handled during the overlap, and are lost after it ends.

Edge cases: W closing during the hand-over ends it at once (the outpost reports W gone; the listener sends "ended W"; held and matched events are handled; nothing more arrives). The outpost dying: the supervisor tells the listener, which reads every sender it still holds for A's windows and serves them again, so what the outpost held and the listener also received is recovered; events only the dead outpost held (raised after registration, for which the listener had sent records) are recovered from the listener's held senders, since the listener received them too.

Tests, were it adopted: a focus before the registration is reported once, from the listener; one after "ended" once, from the outpost; one in the overlap once; a matcher unit test with identical identities straddling the registration (coalesced to one) and with a repeated two-event sequence (the residual above, asserted as the known limitation); a registration slower than the timeout leaves W with the listener; the outpost killed during the overlap loses nothing the listener received.

## 14. Measuring the stale-focus window under option (b)

What it measures: how often a change raised by the newly focused control, in the moment after the focus moved, is lost because the outpost still had the previous focus (13.1). The cleanest case is typed-character echo: Tab into a text field, then type one character at once.

Scenario, `text_box_first_character_after_tab` (Windows Forms; the harness's text box script extended with a second box):

- The window holds two Windows Forms text boxes, "First" and "Second", the second empty. Setup opens it and asserts its announcement.
- One iteration: from "First", press Tab and then type "x" as the next key, with no wait between them beyond the harness sending them in one `SendInput` batch (the agent's `type_text` with a leading Tab, or a key list `["tab", "x"]`); assert the speech: "Second edit" (the focus) and "x" (the echo), exactly, with the echo heard in full. Then Shift+Tab back to "First" and Control+A, Delete in "Second" first so every iteration starts the same: the reset is its own step with its own exact speech.
- 100 iterations. The scenario does not stop at a missed echo: it counts iterations whose speech lacked "x" exactly (the speech assertion per iteration is replaced by an exact classification into "both", "focus only", or anything else, which fails the run), and reports the count with the iteration numbers and each one's trace id. The UIA receiving thread's time and the relay's per-message times are logged for the missed ones.
- A second variant in Windows 11 Notepad (local only) with two tabs is not suitable, since Control+Tab between tabs has its own rules; the second UIA target is the settings dialog's two text fields if Verbatim's dialog has them, else none.

How to read it: NVDA, run by hand on the same window 100 times, gives the baseline: its own gap (its local property group moves to the new focus only when its main thread runs the focus) can lose the echo too, though NVDA echoes typed characters from the keyboard hook rather than from UIA, so it should miss none. A Verbatim count above NVDA's is the cost of 13.1's window, to be weighed against (b3)'s fixed delay. This is a measurement scenario, so its output is a count, recorded per run, not a pass or fail on a tolerance (`docs/testing.md`, "No tolerances that let a regression pass").

## 15. Decisions for Dickson before option (b) is built

1. The marker wait. When the listener receives a UIA focus, it sends a marker at once through verbatim.exe. The outpost then holds that application's later events until the focus's result arrives, so nothing raised after the focus is handled before it. NVDA: the same in effect, since its UIA events pass through one flusher thread in order. Recommend: adopt.
2. MSAA focus, foreground and menu events move into each outpost's own hook. The listener forwards them only for an application whose outpost's hook is not yet in, then sends "forwarding ended" through verbatim.exe. Copies seen by both in the overlap are matched by counting identical events from each source, so each is handled once and none is dropped. NVDA: one hook, one queue, so the question does not arise. Recommend: adopt, as one route per application.
3. Elements the outpost cannot find again, such as Windows Terminal's tab focus elements, which are not in its tree. They are announced from the listener's read, which is correct and drops "tab control". Review and navigation then start from the nearest ancestor that was found. NVDA: has the element, so it navigates from the tab itself. Recommend: accept, since the focus moves to the terminal a few milliseconds later.
4. Time limits.
   - UIA's transaction and connection timeouts at 5 seconds, set once per process; today any new client resets them to 10 seconds for the whole process.
   - Each listener reading thread gets a watchdog, as do stage-1 work and the subscription threads.
   - Operations made of many calls (searches, classic walks) get their own deadlines or become one remote operation.
   - A hung window's stall of the listener's UIA focus is left as NVDA leaves it, with no fallback (decided).
   NVDA: UIA's defaults of 2 and 20 seconds, and a 10-second watchdog that cannot cancel UIA calls. Recommend: 5 seconds.
5. Background speech across a foreground change. Speech from another application that was observed before the switch, but reaches Core after it, is dropped. NVDA: such speech was queued before the foreground change, which cancels it. Recommend: adopt.
6. The UIA probe asks once, trusting a "no" for half a second. NVDA: the same. Its cost: a window busy for more than about a second when first asked (Windows 11 Notepad while starting) is read through MSAA for that half second. Recommend: adopt, and check the Notepad scenarios ten times.
7. Core takes outpost events and key commands from one channel, in the order they reach it, instead of picking at random between two. NVDA: keys and events share one queue. Recommend: adopt; a typed character's echo always comes before the output it causes.
8. The "first character after Tab" scenario, which counts how often a change raised just after a focus moves is lost to the stale-focus window. NVDA: its baseline is taken by hand on the same window. Recommend: build it as a measurement that reports a count, not a pass or fail. Its target must be a UIA field whose echo depends on UIA events; the Windows Forms text box may echo from the keyboard hook, which the race cannot affect.
9. D13's amendment (section 12.3).
   - The listener judges and reads each UIA focus from its sender, on a thread per application, with time limits and watchdogs.
   - Its callbacks, writer and registration thread make no cross-process call.
   - A slow or hung application holds only its own reading thread, except that a hung window named in a focus WinEvent holds the listener's UIA focus delivery (measured, NVDA's exposure too).
   NVDA: one process with no isolation. Recommend: adopt the wording.

Still unproven, to be shown before or while building:

- That UIA delivers one provider process's events to one registration in the order they were raised. The marker ordering and the MSAA hand-over's counting rely on it; it is observed in mockapp, not documented.
- That the relay through verbatim.exe meets its 1 to 2 ms worst case. It is being investigated separately; today's 90th percentile is up to 36 ms.
- The outpost's path walk to find its own element: its cost and success rate on real applications are not measured.
- How often the stale-focus window loses a change (item 8).
- How long UIA waits on a window hung for more than 15 seconds before the listener's UIA focus resumes.
- That the multiset matching in the MSAA hand-over is exact: it is shown by reasoning, and its tests are specified but not written.
- Whether Windows 11 Notepad's first focus, read through MSAA after a single slow probe, sounds different (item 6).

Changes to section 12.9's migration under (b2): step 5 builds option (b) as described (the listener judges and reads; the outpost reports the result); a new step after it adds the outpost's per-window focus registration and the hand-over, with tests that (1) a focus raised in a window before its registration is reported once, from the listener; (2) one raised after the hand-over ended is reported once, from the outpost, with the sender; (3) one received by both in the overlap is reported once; (4) a group-scoped focus registration delivers mockapp's focus events (the E-scope result, kept as a test so a Windows change is caught); (5) a window whose registration timed out stays with the listener.
