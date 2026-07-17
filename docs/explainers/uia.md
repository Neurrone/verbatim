# UI Automation

UIA is Microsoft's modern accessibility API and the only one still being
developed. It has a genuinely different architecture from MSAA/IA2 — a
brokered tree, batched property fetches, typed patterns and events — and its
own distinct failure modes. NVDA's usage is in [The UIA client](../nvda/uia.md) and
[UIA remote operations](../nvda/uia-remote-ops.md).

## Architecture: three parties, not two

An MSAA call goes client-to-app. A UIA call goes client, into the *UIA
core* (`UIAutomationCore.dll`, loaded in both processes, brokered by the
system), then to the app's *provider*. The client-side object model
(`IUIAutomation`, `IUIAutomationElement`) is deliberately decoupled from the
provider-side one (`IRawElementProviderSimple` and friends): the core walks
providers, fabricates proxies for MSAA-only apps, merges subtrees from
different processes (crucial for out-of-process browser content and XAML
islands), and serves some properties (bounding rectangles, runtime IDs, the
window pattern) from cached or system data without touching the app at all.

Consequences worth internalizing:

- The client never holds a direct pointer into the app; `AddRef`-ing an
  element does not pin app memory. Elements are snapshots-with-a-handle,
  identified by *runtime ID* (an int array, comparable across element
  instances while the node lives).
- Cross-process fetches go through UIA's own channel with a
  *configurable timeout* (`IUIAutomation2::ConnectionTimeout`, default on
  the order of a couple of minutes — far too long for a screen reader;
  clients that care must lower it). A hung provider still hurts, but
  bounded-hurt is achievable without the heroics MSAA requires.
- The system MSAA-to-UIA proxy means a UIA client sees *something* for
  every window; whether it is good is another matter ([The accessibility landscape](accessibility-landscape.md)).

## Elements, properties, patterns

- **Tree**: elements form one desktop-rooted tree. Three standard filtered
  *views*: raw (everything), control (elements that are controls), content
  (what a document reader wants). Clients walk with `IUIAutomationTreeWalker`
  over a chosen *condition* — walking is a sequence of cross-process
  calls unless cached (below).
- **Properties** are fetched by numeric ID (`UIA_NamePropertyId`,
  `UIA_ControlTypePropertyId` — about 50 control types —
  `UIA_BoundingRectanglePropertyId`, …). Everything is
  `GetCurrentPropertyValue(id)` returning a `VARIANT`; there are typed
  convenience getters.
- **Patterns** express capabilities: `Invoke`, `Toggle`, `Value`,
  `RangeValue`, `SelectionItem`, `ExpandCollapse`, `ScrollItem`, `Window`,
  `LegacyIAccessible` (an escape hatch exposing the MSAA view through UIA),
  and above all `Text` / `TextRange`: a real text model with endpoints,
  unit-based movement (character, word, line, paragraph, page), formatting
  attributes, and embedded-object children. Terminals and modern
  document apps are read through the Text pattern.

## Caching: the batching mechanism

Every `Current*` accessor on an element is a cross-process call. The
sanctioned fix is a *cache request*: declare up front which properties and
patterns you want, then any operation that returns elements (find, tree
walk, event registration, `BuildUpdatedCache`) fetches them all in one round
trip; afterwards you read `Cached*` accessors without touching the app.
Well-written clients do essentially all their reading through caches
attached to event registrations and walks. This is UIA's answer to the
round-trip tax that MSAA answers with injection — and when caching is still
too slow (string-heavy bulk reads), *remote operations* execute a client
authored program inside the provider process in one shot
([UIA remote operations](../nvda/uia-remote-ops.md)).

## Events

Clients subscribe with handler interfaces: focus changed (global),
automation events by ID (window opened, invoked, notification…), property
changed (with an explicit property list), structure changed, text-edit and
active-text-position events, and *notification events* — the modern
free-form channel by which an app pushes an announcement string with
processing hints (important/all, etc.). A cache request rides along, so the
handler receives elements with the needed properties prefetched.

Delivery: UIA raises client callbacks on *background MTA threads*, not your
UI thread. The rules that keep this sane:

- A client thread that registers handlers should live in the MTA. STA
  registration invites deadlocks (event delivery marshals into your STA
  while you are mid-call into UIA).
- Never make blocking UIA calls *inside* a handler; you are on UIA's
  callback thread and can stall the whole event pipeline. Queue and return.
- Registration and unregistration are themselves expensive cross-process
  operations (they touch every provider in scope); add/remove churn is a
  known performance sink, and Windows 11 added `IUIAutomation6` "event
  coalescing" and "connection recovery" options to soften handler cost.

## Provider side, briefly

Apps expose UIA by answering `WM_GETOBJECT` with
`UiaReturnRawElementProvider`, implementing `IRawElementProviderSimple`
(properties, pattern objects) plus fragment interfaces for subtree
structure and `IRawElementProviderFragmentRoot` at the window. Providers
raise events with `UiaRaiseAutomationEvent` and friends. Two facts matter
even for a client-only project: a provider answers on whatever thread the
call arrives on (UI thread for message-delivered requests — so a busy UI
thread stalls its UIA surface just like MSAA), and providers can declare
`ProviderOptions_UseComThreading` and other options that change threading
behavior. Verbatim is also a *provider* for its own GUI windows, which is
why the provider model appears in this repo at all.

## Working characteristics

- Rich, typed, batched, and the only path to modern shell UI. The text
  pattern is the best text model of the three APIs.
- Latency is workable *only* with disciplined caching; naive
  property-by-property UIA is slower than naive MSAA.
- Fidelity varies per app: Chromium's and Gecko's UIA are younger than
  their IA2; some information (and some entire apps) is still IA2-only or
  better over IA2.
- The FocusChanged event firehose plus per-event caching is the standard
  screen reader input; missed or duplicated focus events under load are a
  known UIA client reality that clients defensively de-duplicate.

## References

- [UI Automation Fundamentals (Microsoft Learn)](https://learn.microsoft.com/en-us/windows/win32/winauto/entry-uiauto-win32)
- [IUIAutomation6 (Microsoft Learn)](https://learn.microsoft.com/en-us/windows/win32/api/uiautomationclient/nn-uiautomationclient-iuiautomation6) —
  event coalescing and connection recovery; and
  [IUIAutomation2::ConnectionTimeout (Microsoft Learn)](https://learn.microsoft.com/en-us/windows/win32/api/uiautomationclient/nf-uiautomationclient-iuiautomation2-get_connectiontimeout)
- [Chromium's UIA provider docs](https://chromium.googlesource.com/chromium/src/+/main/docs/accessibility/uiautomation.md)
