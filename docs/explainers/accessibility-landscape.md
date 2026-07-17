# The Windows accessibility landscape

Windows has accumulated three generations of accessibility API, and all three
are alive on a current Windows 11 desktop. A screen reader cannot pick one; it
meets whatever each application exposes. This file maps who exposes what and
how the layers interconnect.

## The three APIs in one paragraph each

**MSAA (Microsoft Active Accessibility, 1997).** A single COM interface,
`IAccessible`, exposed per UI element (or per window with numeric child IDs
for simple children). It offers a name, a role, a state bitmask, a value, a
location, simple navigation, and a `WinEvents` notification channel. It has no
concept of rich text, no reliable unique node identity, and a fixed 1990s role
vocabulary. Win32 common controls (buttons, list views, tree views, menus) get
MSAA support from the system for free, which is why it still matters: legacy
and internal line-of-business apps often expose nothing better.

**IAccessible2 (IA2, 2006).** Not a Microsoft API: an open standard from the
Linux Foundation, created because MSAA could not express what Firefox needed
and Microsoft's then-new alternative (UIA) was not ready or cross-platform.
IA2 piggybacks on MSAA — you obtain an MSAA `IAccessible` first, then ask it
for the IA2 interfaces — and adds rich text (`IAccessibleText`), hyperlinks,
tables, relations between nodes, an extended role set, and object attributes
(key-value strings). Its implementors are the Gecko and Chromium families
(Firefox, Chrome, Edge, Electron apps) and LibreOffice. If you care about web
browsers through the injection path, you care about IA2.

**UIA (UI Automation, 2005, heavily revised since).** Microsoft's designated
successor. A different object model: elements form a tree owned by a system
service; properties are fetched by ID; capabilities are expressed as
*patterns* (Invoke, Value, Toggle, Text, and so on); notifications are typed
events. WPF, UWP/WinUI, and modern Windows shell surfaces (Start menu,
Settings, taskbar) are UIA-native. Chromium and Firefox also expose UIA,
with varying fidelity. UIA is the only API of the three that Microsoft still
develops.

## Who exposes what

- Win32 common controls: MSAA natively (system-provided), plus a UIA view via
  the system's MSAA-to-UIA proxy (see below). Extra semantics are often only
  available through control-specific window messages (for example
  `TVM_GETNEXTITEM` for tree views), not through the accessibility API at all.
- WPF, UWP, WinUI 2/3, modern shell: UIA natively. No real MSAA beyond the
  automatic degraded bridge.
- Chromium (Chrome, Edge, Electron): both IA2 and UIA. Historically IA2 was
  the complete implementation; Chromium's UIA has matured but screen readers
  still differ on which they pick per version.
- Gecko (Firefox, Thunderbird): IA2 first-class; UIA support landed recently
  and is still second-choice for most screen readers.
- Office desktop apps: a mix. Word and Excel expose UIA (used for modern
  document reading) but screen readers historically read them through the
  Office COM object models (a fourth, non-accessibility channel) because the
  accessibility APIs were too incomplete.
- Java (AWT/Swing): none of the three natively; the Java Access Bridge
  translates the Java Accessibility API over its own channel.
- Terminals: modern Windows Terminal and the in-box console expose UIA text
  ranges.

## The bridges

Windows runs automatic translation layers so clients of one API can see apps
that only implement another. These bridges are load-bearing for every screen
reader and are a common source of fidelity loss — when behavior differs
between what an app implements and what a client sees, suspect the bridge.

- **MSAA-to-UIA proxy**: the system fabricates UIA elements over anything
  exposing only `IAccessible`. This is how a UIA client sees classic Win32
  apps. The fabricated elements have the lowest-common-denominator properties
  MSAA can express.
- **UIA-to-MSAA bridge**: the reverse; an MSAA client sees a degraded
  `IAccessible` view of UIA-only apps.
- **IA2 is invisible to UIA**: the bridges know nothing of IA2's extensions.
  A UIA client looking at Firefox through the proxy loses everything IA2
  added. This is why IA2-era screen readers keep an IA2/MSAA client stack
  alive rather than going UIA-only.

## Events, in one view

Each API has its own notification channel, and a screen reader listens to all
of them simultaneously:

- MSAA/IA2: WinEvents — `SetWinEventHook` callbacks carrying a window handle
  plus object and child IDs, which the client resolves to an `IAccessible`
  after the fact. See [Windows and messages](windows-and-messages.md).
- UIA: typed event subscriptions (focus changed, property changed, structure
  changed, notification) delivered on client threads, usually with a cache of
  requested properties attached.
- Everything else: window messages, `WM_GETOBJECT` requests, and per-control
  message protocols.

## Why screen readers also inject code

The pull model of all three APIs shares a cost: every property fetch is a
cross-process round trip (see [COM](com.md) and [Windows IPC](ipc.md)). For reading a whole web
page — thousands of nodes, dozens of properties each — that is far too slow
over MSAA/IA2, whose granularity is one property per call. The classic
solution (NVDA, JAWS) is to inject a DLL into the target process and build the
document representation in-process, shipping it out in bulk. UIA's answers to
the same problem are built-in batching (caching) and, recently, remote
operations. [Process injection](../nvda/process-injection.md) and
[UIA remote operations](../nvda/uia-remote-ops.md) cover the two approaches concretely.
