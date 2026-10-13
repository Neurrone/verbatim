# UIA shapes: every cached read declared with the request that fills it

Proposed 2026-10-13, at Dickson's request after items 12 and 13 of
`docs/design/wake-ups.md`. He asked whether every read of a UI Automation
element can declare exactly the properties it uses, so that the cache
request is built from the same declaration and only answers from the cache
are used, with no "is it cached?" checks and no live fallbacks of the kind
NVDA makes. It can; this document sets out how, with an inventory of every
cached read in the workspace and the shape each one would take.

Researched on main at 8511cd1, with the working tree's uncommitted
event-read fix in place (`verbatim-uia/src/map.rs` and
`verbatim-outpost/src/outpost/mod.rs`); line numbers are of that working
tree. Nothing was built or run. Code is cited as `crate/src/file.rs:line`,
the crate folder under `crates/`; NVDA as `nvda/source/...:line`. A
statement marked "established" comes from the code or from a recorded
measurement; one marked "estimated" is reasoning, with the reasoning given,
for the migration's measurements to confirm.

## 1. Summary

- Verbatim maps an element through one function, `parts_of`
  (`verbatim-uia/src/map.rs:601`), which reads 31 distinct cached
  properties whatever cache request filled the element. UI Automation
  answers a read of a property its cache lacks by raising and reporting an
  error inside itself. The results are right (the property reads as
  unsupported), but in a console flood the outpost's callback threads spent
  3.5 seconds of processor time in a 6-second window, most of it in that
  error path; restricting the event reads to their registration's
  properties cut it to 0.83 seconds (established, `wake-ups.md` item 12).
- The audit below finds the same pattern in five more places, among them
  the listener, which reads 25 properties absent from NVDA's base cache on
  every desktop-wide focus, selection and notification event, and about
  twenty fetch sites that read up to 7 absent properties per element when
  the theme turns details off (established, section 5).
- The design: a shape is a struct declared once with a macro, each field a
  property or a group that is itself a shape. The macro generates the
  shape's property list, its cache request, and the one function that reads
  the cache into the struct. An element filled by a shape's request has the
  type `Cached<S>`; only the functions that built that request can make one.
  Everything else uses `Live`, which has no cached reads. The compiler then
  guarantees that a read names only properties its request carried.
- Theme gating becomes part of the type: a gated group is an `Option` in the
  struct, the request includes it only when its gate is on, and the gate
  travels inside `Cached<S>`, so request and read cannot disagree and every
  user must handle "not fetched".
- Two things a static declaration cannot express need a decision: the
  outpost's node registry keeps elements filled by four different requests
  (section 6), and a remotely filled cache leaves `IsDataValidForForm` out
  for an element that does not support it (section 7).
- Migration is six steps, each passing `cargo xtask ci`, starting with the
  listener and a one-line fix in the worker (section 9). Eight decisions for
  Dickson are in section 10.

## 2. The problem and what was measured

### 2.1 How an element is read today

Every UIA read in the workspace goes through `ElementExt`
(`verbatim-uia/src/element.rs:45`), whose `cached_value`,
`cached_value_ignoring_default`, `cached_bool`, `cached_string`,
`cached_bounding_rectangle`, `cached_control_type`, `cached_framework_id`
and `cached_pattern` call `GetCachedPropertyValue` and its relatives and
turn any error into `None` (`element.rs:199` to 250). The cache requests are
built separately, from lists in `verbatim-uia/src/cache.rs` and from ad hoc
slices at call sites. Nothing connects the list a request was built from to
the properties later read from the element it filled. The documentation of
`snapshot_parts_from_cached_element` says the element "should be built with
the base cache request", and many call sites carry a comment saying which
request filled their element; several of those comments are now wrong
(section 5).

### 2.2 What a read outside the cache costs (established)

From `wake-ups.md` item 12: in `conhost_flood`, 6 seconds from 8 seconds
in, the console's outpost used 7.5 seconds of processor time, 3.5 of them
on UI Automation's callback threads. The CPU samples put a third of that
under Verbatim's text event handler reading the event's element and the
rest in UI Automation raising, unwinding and formatting errors. A text
event's cache request holds three properties; the handler read 31. After
the fix in 2.3 the same window had 0.83 seconds on the callback threads and
3.2 in the outpost.

The cost falls on whichever thread reads, so the same mistake on the
worker or the listener's reading threads costs those threads in the same
way. The audit (section 5) counts the absent reads per site; nothing beyond
item 12 has been measured.

### 2.3 The fix in the working tree, and why it is not enough

The uncommitted fix adds a private trait, `CachedReads` (`map.rs:296`),
implemented for the element itself and for a `Restricted` wrapper
(`map.rs:351`) that answers "not cached" for any property outside a given
list without asking UI Automation. The outpost's event capture passes its
registration's list (`snapshot_parts_from_event_cache`, `map.rs:578`;
`capture`, `verbatim-outpost/src/outpost/mod.rs:973`, called with
`FOCUS_EVENT_PROPERTIES` at 1027 and `TEXT_EVENT_PROPERTIES` at 1048 and
1075). A mockapp test compares restricted and full reads
(`mockapp/tests/uia_tree.rs`, `event_cache_reads_are_the_cache_reads`).

It fixes the two paths item 12 measured. It does not stop the next one:
the list passed to the reader is a second copy of the list the registration
was built from, a run-time check on each property is the "is it cached?"
test Dickson wants gone, and every other reader still uses the unrestricted
path. The design below keeps the fix's effect and removes the need for it.

### 2.4 NVDA, for comparison

NVDA reads a property through `_getUIACacheablePropertyValue`
(`nvda/source/NVDAObjects/UIA/__init__.py:1172` to 1184): from an element
whose cache this core cycle holds the property, otherwise by a live
cross-process read. Its cached values come from its base cache request
(13 properties and the Text pattern, `cache.rs:150` to 175) and from
prefetches it makes for one purpose. So NVDA never reads an absent cached
property, but pays a cross-process call for each property it did not
prefetch. Verbatim's rule since M1 is that mapping reads only the cache
(architecture section 4, "cache requests everywhere"); this design keeps
that rule and makes it checkable.

## 3. The design

### 3.1 Shapes

A shape is a plain struct whose fields are what one purpose reads of an
element. A field is either a property, with the Rust type it reads as, or a
group, which is itself a shape. Groups are where the mapping lives: the
role group holds the five properties a role is mapped from and has a
method that maps them, exactly as `parts_of` does today.

```rust
uia_shape! {
    /// What a role is mapped from (`map.rs` `parts_of`).
    pub struct RoleCore {
        control_type: ControlType => Option<i32>,
        class_name: ClassName => Option<String>,
        is_dialog: IsDialog => bool,
        toggle: IsTogglePatternAvailable => bool,
        hwnd: NativeWindowHandle => isize,
    }
}

impl RoleCore {
    /// The role, refined as `parts_of` refines it today.
    pub fn role(&self) -> Role { /* role_from_control_type, terminal, dialog */ }
}

uia_shape! {
    /// A node as Core is told of it: a focus, a neighbour, a tree dump.
    pub struct Node {
        role: RoleCore,
        name: Name => Option<String>,
        value: ValueSet,
        states: States,
        #[gated(description)] description: Description,
        #[gated(shortcut)] shortcut: Shortcut,
        #[gated(position)] position: Position,
        #[gated(level)] level: Level => Option<u32>,
        rect: BoundingRectangle => Option<Rect>,
    }
}
```

`ValueSet` is the value group (`ValueValue`, `IsRangeValuePatternAvailable`,
`RangeValueValue`), `States` the fifteen state properties
`states_from_cached` reads (`map.rs:388`), `Description` the two the
description is read from (`map.rs:450`), `Shortcut` the access and
accelerator keys, and `Position` the position in set and set size. A
property that two groups share, such as `IsTogglePatternAvailable` in the
role and the states, is requested once.

A pattern can be a field too, for the selection container
(`selection: pattern Selection`), and reads as the cached pattern object.

### 3.2 The macro and what it generates

`uia_shape!` is a declarative macro (`macro_rules!`) in `verbatim-uia`. For
a shape `S` it generates:

```rust
impl Shape for Node {
    /// One bool per gated field: here description, shortcut, position, level.
    type Gates = NodeGates;
    /// Every property of every field and group, without repeats.
    const PROPERTIES: &'static [UIA_PROPERTY_ID] = /* computed at compile time */;
    /// The request for `gates`: PROPERTIES without the groups gated off.
    fn request(uia: &Uia, gates: &NodeGates) -> windows::core::Result<Request<Self>>;
    /// The only code that reads the cache.
    fn read(cached: &Cached<Self>) -> Self;
}
```

`PROPERTIES` is a constant: the macro concatenates each field's list and a
`const fn` removes repeats into a fixed array. A shape with no gated field
has `Gates = ()`. `read` calls each group's `read` and each property's
typed accessor, and for a gated field returns `None` without touching the
cache when the gate carried in `Cached` is off. The typed accessors are
today's readers moved into the shape module: `read_bool`, `read_i32`,
`read_string`, the reads that ignore defaults, and the rectangle, with
today's handling of UI Automation's defaults (`map.rs:296` to 448)
unchanged.

The macro can also declare that one shape covers another
(`covers(Kept)`), which generates a conversion and a compile-time
assertion that every property of the covered shape is in the covering
shape's `PROPERTIES` with no gate. Section 6 uses it.

Whether a `macro_rules!` macro or hand-written impls is decision 1. A
procedural macro is not proposed: it would need a new crate and the `syn`
and `quote` dependencies for nothing the declarative form cannot do here
(estimated, from the field syntax above, which is a list of
`name: Property => Type` and `name: Group` entries with an optional
attribute).

### 3.3 `Cached` and `Live`

```rust
/// An element whose cache was filled by S's request with `gates`.
pub struct Cached<S: Shape> {
    element: IUIAutomationElement,
    gates: S::Gates,
    absent: Absent, // section 7; empty for a local fill
    shape: PhantomData<S>,
}

impl<S: Shape> Cached<S> {
    pub fn read(&self) -> S { S::read(self) }
    pub fn live(&self) -> &Live { /* the same element, as Live */ }
    pub fn runtime_id(&self) -> Vec<i32> { /* GetRuntimeId, local */ }
}

/// An element for calls into the application: patterns, text ranges,
/// live property reads, and refreshing its cache into a shape.
pub struct Live { element: IUIAutomationElement }
```

`Cached<S>` has no public constructor. `Live` has no cached reads at all.
The runtime id is on both, since UI Automation keeps it with every element
and reading it never fails for want of a cache (`calls.rs` module
documentation).

Deliberate live reads stay deliberate: the legacy checked state of a menu
item (`map.rs:205`, NVDA 2027.1's rule), `HasKeyboardFocus` before a focus
read, and the `ControllerFor` relation (`worker.rs:829`) are methods of
`Live`, called where a rule says to make the call, never as a fallback for
a property a cache lacked.

### 3.4 Fetching and registering

Every function that fills a cache takes a shape and returns `Cached<S>`:

```rust
let focus: Cached<Node> = uia.focused::<Node>(gates)?;
let window: Cached<WindowLookup> = uia.element_from_handle::<WindowLookup>(hwnd, ())?;
let parent: Option<Cached<Ancestor>> = walker.parent::<Ancestor>(focus.live(), gates)?;
let fresh: Cached<HelpTextRefresh> = element.refresh::<HelpTextRefresh>(uia, ())?;
let children: Vec<Cached<DialogText>> = uia.children::<DialogText>(dialog, ())?;
```

The same holds for `FindFirstBuildCache`, `NormalizeElementBuildCache`,
the search by runtime id and the path search (`focus_path::find`). A call
that needs no cache at all returns `Live`: `checks.rs:66` asks for a
window's element only to search under it.

An event registration's parts take a shape, and their callbacks receive
`Cached<S>` built by `verbatim-uia/src/subscribe.rs` from the element UI
Automation delivers:

```rust
Part::shaped::<PropertyEvent>(properties, move |event: Cached<PropertyEvent>, id| { ... });
Part::shaped::<TextEvent>(caret, move |event: Cached<TextEvent>| { ... });
```

Event shapes take no gates (section 8.2 says why). A handler that
occasionally needs more than its event shape holds refreshes the element
into a second declared shape, one call when it happens, as the help text
and `ControllerFor` handlers already do (`refreshed_for`, `worker.rs:801`).

Remote operations live in another crate, `verbatim-uia-rops`, which cannot
use a private constructor. `verbatim-uia` hands it a token instead:
`Fill<S>`, made by `uia.fill::<S>(gates)`, which carries the property list
for the program's cache requests and is the only way to wrap the
program's returned elements as `Cached<S>` (section 7).

### 3.5 The guarantees, and what enforces each

1. A read names only properties its shape declares. Enforced by
   construction: `S::read` is generated from the same field list as
   `S::PROPERTIES`, and no other code can read the cache.
2. An element read as `S` was filled by `S`'s request. Enforced by the
   compiler: `Cached<S>` is made only by the functions that built that
   request, by registrations, and through `Fill<S>`.
3. A gated group is read only when its request carried it. Enforced by the
   compiler: request and read take the gates from the one value stored in
   `Cached<S>`, and the field's type is `Option`, so every user handles
   "not fetched".
4. No code outside the shape module reads a cache. Enforced in two parts:
   the `cached_*` methods leave `ElementExt`'s public API, and a
   `cargo xtask ci` check fails on `GetCachedPropertyValue`,
   `GetCachedPropertyValueEx`, `GetCachedPattern`, `GetCachedPatternAs`,
   `GetCachedChildren` used for properties, or a typed `Cached*` getter
   anywhere but the shape module, the way `xtask/src/waits.rs` scans
   sources for fixed sleeps.
5. A read outside the carried request fails a test. Two checks in debug
   builds, which the unit tests, mockapp tests and end-to-end suite all
   use: the shape module's one accessor checks each property against the
   list the `Cached` carries (its shape's request for its gates, minus the
   absent record) and logs an error and counts a violation when it is not
   there; and every cached read that UI Automation answers with an error
   is counted, per thread, in `CallCounts` (`verbatim-uia/src/calls.rs`)
   as a new `failed_cached_reads` field. The first catches a fault in the
   macro or a hand-written group; the second catches what no declaration
   can, such as a remote program leaving a property out. Because
   `call_counts.rs` compares counts exactly, every test there asserts zero
   failed reads with no new assertion. Neither check panics: a panic on a
   UI Automation callback thread would abort the process.
6. What the compiler cannot check: that a theme maps to the right gates.
   That is one function per gated shape, from `Fetches`
   (`verbatim-model/src/theme.rs:864`) to the shape's gates, with a unit
   test per gated field (section 4.3).

What stays outside any guarantee: whether a provider supports a property
(it reads as unsupported, as today), whether the element is still alive,
and whether a shape declares properties no user consumes. The last is a
cost, not a fault, and is kept down by one shape per purpose (section 8).

## 4. Themes and run-time gating

### 4.1 Gates

A gated field, `#[gated(description)] description: Description`, adds a
`bool` named `description` to `S::Gates`, makes the field's type
`Option<Description>`, and puts the group's properties in the request only
when the gate is on. A gate may name more than one setting:
`#[gated(any(spelling_errors, grammar_errors))]`, as the annotation types
are read for either (`verbatim-outpost/src/text/uia.rs:811`).

The gates of the four detail groups are today's `Fetches` fields
`description`, `shortcut`, `position` and `level`, and the request is
today's `cached_properties(fetches)` (`cache.rs:226`) without the
properties no reader of that shape reads. The bounding rectangle is not
gated today and is not gated here.

### 4.2 A worked example: a focus read with descriptions off

The theme reports descriptions off, so `Theme::fetches`
(`theme.rs:689`) gives `description: false`, and the outpost's gates for
`Node` are `NodeGates { description: false, shortcut: true, position: true,
level: true }`.

```rust
let gates = node_gates(context.fetches());
let focus: Cached<Node> = uia.focused::<Node>(gates)?;
let node = focus.read();
assert!(node.description.is_none()); // not fetched
```

- The request is `Node::PROPERTIES` without `FullDescription` and
  `HelpText`: 29 properties instead of 31, so the provider is not asked for
  either.
- `Node::read` sees the gate off and sets `description` to `None` without
  reading the cache. Today the same read asks UI Automation for
  `FullDescription`, gets an error, then asks for `HelpText` and gets
  another (`map.rs:460`).
- The conversion to `NodeDetails` gives `description: None`, which is what
  Core receives today; nothing heard changes.
- Any code that uses the description must say what "not fetched" means to
  it. For speech that is simple: the indication is off. For the layout
  judgment it is not (`client.rs:1110`, section 5.4), and the type makes
  that a visible choice instead of a silent `None`.

### 4.3 What the compiler cannot check

That `description: false` comes from the description indication and not
from another is the one link the types cannot make, since the theme is in
`verbatim-model`, a platform-neutral crate that knows nothing of UI
Automation. The mapping is one function per gated shape in
`verbatim-outpost` (from `Fetches`), with one unit test per gated field:
turning that one setting off in a theme turns off that one gate and no
other.

### 4.4 Formatting: text attributes

Formatting is not element properties but text range attributes: font name,
size, weight, italic, underline and its style, strikethrough, foreground
and background colours, bullet style, link, and the annotation types that
carry spelling and grammar errors. They are read by the caret's remote
operation (`verbatim-uia-rops/src/caret.rs`, `TextAttribute` at 74 and the
attribute set at 142) or classically with `GetAttributeValues`, one
attribute at a time where a provider lacks it
(`verbatim-uia/src/text.rs:385` to 410). Which to read is decided by
`attributes_for(fetches)` (`verbatim-outpost/src/text/uia.rs:807` to 836),
and what they mean by `attributes_of(run, fetches)` (845), the same gates
applied in two places.

A read of an attribute a provider does not support returns UI Automation's
"not supported" object rather than an error (established, `text.rs:262` to 265
documentation), so the cost of section 2 does not arise here. What the
same pattern gives is agreement: one declaration, `uia_attributes!`,
generating the attribute list for gates (which feeds both the remote
program and `GetAttributeValues`) and the read of the results into a
struct with `Option` fields for gated attributes. Three attribute shapes
cover today's reads:

- `AttributeSet`, gated by the document-formatting settings, for the
  caret's formatting.
- `Culture`, never gated, for the language of a unit (`text.rs:447`).
- `ConsoleProbe`, `FontName` alone, for the console's provider check
  (`verbatim-uia/src/checks.rs:81`).

The set of attributes a provider was found not to support
(`CaretAnswer::unsupported`, `caret.rs:335`, kept per node by
`text_reads.rs`) stays a run-time subtraction from the request: it is
learned from answers, so no declaration can hold it.

## 5. The inventory

### 5.1 The cache lists today

- `CACHED_PROPERTIES` (`cache.rs:67`): 34 properties, used by
  `base_cache_request` and by `Registration::new` (`subscribe.rs:395`).
- `cached_properties(fetches)` (`cache.rs:226`) and `cache_request_for`
  (`cache.rs:261`): `CACHED_PROPERTIES` without `FullDescription` and
  `HelpText` when descriptions are off, `AccessKey` and `AcceleratorKey`
  for shortcuts, `PositionInSet` and `SizeOfSet` for positions, and
  `Level`: 27 to 34 properties. The outpost's per-query request is this
  (`outpost/mod.rs:332`).
- `TEXT_EVENT_PROPERTIES` (`cache.rs:110`): `ProcessId`,
  `NativeWindowHandle`, `ClassName`.
- `FOCUS_EVENT_PROPERTIES` (`cache.rs:125`): 24, which is
  `CACHED_PROPERTIES` without the seven detail properties,
  `BoundingRectangle`, `IsContentElement` and `IsControlElement`.
- `NVDA_BASE_PROPERTIES` (`cache.rs:162`): 13 properties, and the Text
  pattern added by `nvda_base_cache_request` (`cache.rs:186`).
- `DESCRIPTION_PROPERTIES` (`map.rs:450`): `FullDescription`, `HelpText`.
- `SUGGESTIONS_PROPERTIES` (`map.rs:673`): `IsTextPatternAvailable`,
  `AutomationId`.
- Ad hoc lists: `[NativeWindowHandle]` (`nearest.rs:55`), `[FrameworkId]`
  (`checks.rs:92`), `[Selection2FirstSelectedItem]` and the Selection
  pattern (`client.rs:635`).

`parts_of` reads 31 distinct properties: all of `CACHED_PROPERTIES` but
`ProcessId`, `IsContentElement` and `IsControlElement`. On an element with
none of them cached it makes 28 reads that can miss beyond the two it
skips (`RangeValueValue` is read only when its pattern is available, and
`ValueIsReadOnly` only when the Value pattern is), and
`IsTogglePatternAvailable` is read twice, once for the role and once for
the states.

### 5.2 Bugs: reads outside the cache (established, from the code)

1. The listener. `capture` (`verbatim-outpost/src/listener.rs:602` to
   605), called for selections (759) and notifications (788), and the
   focus reader's `read` (`verbatim-outpost/src/focus_read.rs:347`) call
   `event_parts` (`focus_read.rs:936`), which runs the unrestricted
   `snapshot_parts_from_cached_element` on an element filled by NVDA's
   base request. Of `parts_of`'s reads, only `ControlType`, `ClassName`,
   `NativeWindowHandle`, `Name` and `HasKeyboardFocus` are cached, so each
   desktop-wide event makes 25 failed reads (24 distinct). `event_parts`
   then keeps only the runtime id, role, name and the focused state. The
   role it keeps already lacks the toggle and dialog refinements, since
   `IsTogglePatternAvailable` and `IsDialog` read as false; the outpost
   reads the role afresh for that reason (`focus_read.rs:516` to 518).
2. Theme-gated fetches. About twenty sites read a full snapshot from an
   element filled by `cache_request_for(fetches)` or
   `cached_properties(fetches)`: up to 7 failed reads per element when all
   four details are off (`FullDescription` and `HelpText`, the two keys,
   the two position properties, `Level`). The sites:
   `outpost/read.rs:174`, 471, 557 and 558, 726, 1033, 1091, and 1154 (the
   tree dump, through `walk_tree`, `client.rs:1065`);
   `verbatim-uia/src/client.rs:603`, 710, and 802 to 807 (`take_ancestor`);
   `outpost/worker.rs:2296` and 2317; and `dialog_text.rs:224` for each
   child (section 5.5).
3. `controlled_selection` (`worker.rs:2293`) reads a whole snapshot of the
   focus's element to take its runtime id. The element is the registry's
   kept element (`worker.rs:2263` to 2266) or one found by its path
   (2269, gated). A kept element's cache may be `FOCUS_EVENT` (8 failed
   reads: seven details and the rectangle), `TEXT_EVENT` (28 failed
   reads, 27 distinct) or gated (up to 7). It should be
   `verbatim_uia::runtime_id(&focused)`, which reads no property. The
   comment above it, "Both elements were built with the base cache
   request", is wrong for this element.
4. `UiaObject::new` (`dialog_text.rs:224`) reads a snapshot of the dialog's
   element, which for a dialog the outpost holds is the kept element
   resolved without a refresh (`read.rs:696`): any of the kept caches. The
   root's parts are never used: `dialog_text` gathers from the dialog's
   children alone (`dialog_text.rs:120` to 184), so this read can go
   entirely.
5. `text_event_from`'s log line (`text_reads.rs:221` to 242, an `info`
   message written when a caret or text event replaces an element that had
   no text pattern, which is rare) reads `Name`, `ClassName` and
   `FrameworkId` of the old kept element and of the event's
   `TEXT_EVENT` element. `Name` is not in `TEXT_EVENT`; `FrameworkId` is
   in no cache but NVDA's base.

A design problem underlies 3, 4 and 5: the node registry keeps elements
filled by four different requests, and its readers cannot know which.
Section 6 takes it up.

A sixth read is a live fallback rather than a miss:
`wants_selected_child` (`verbatim-uia-rops/src/focus.rs:226` to 233) reads
the control type live when it is not cached. Every element it is given
carries the control type today (NVDA's base, the gated request), so the
fallback never runs; under shapes it goes (decision 8).

### 5.3 Over-fetches (correct, but wasteful)

- `hosts_console_window` (`read.rs:789` to 804) fills 27 to 34
  properties of a console window's element to read `ProcessId`.
- The shell's notification-area list (`verbatim-gui/src/shell_items.rs:145`
  and 269 to 281) fills all 34 to read five: `NativeWindowHandle`,
  `ControlType`, `Name`, `IsOffscreen`, `BoundingRectangle`.
- `console_reports_formatting` (`checks.rs:66`) fills all 34 for the
  console window and reads none of them: it only searches under it.

### 5.4 Behaviour coupled to a gate

- `is_layout` (`client.rs:1110`, the description at 1119) judges an
  unnamed window, property page or grouping as layout unless it has a
  description. With descriptions off, an ancestor read by the outpost has
  none, so an unnamed grouping with a description is not reported as focus
  context. NVDA's `presentationType` uses the description whatever the
  setting (`nvda/source/NVDAObjects/__init__.py:947` to 955). The listener
  reads ancestors with every detail (`Fetches::default()`,
  `focus_read.rs:374`), so the same grouping is reported when the listener
  read the chain and not when the outpost did. Decision 4.
- Dialog text includes each child's description (`text_of`,
  `dialog_text.rs:186`), read from the gated request, so with descriptions
  off a dialog's text loses them. NVDA's dialog text uses `basicText`,
  which includes the description whatever the setting
  (`nvda/source/NVDAObjects/__init__.py:1450` to 1458). Decision 5.

### 5.5 Every site, by component, with its shape

The listener (`verbatim-outpost/src/listener.rs`). Its registration
(817) uses NVDA's base request; every event element is `ListenerEvent`.

- 602 to 605, `capture`: `ProcessId`, `NativeWindowHandle`, then
  `event_parts`. Bug 1. Becomes a read of `ListenerEvent`.
- 687 and 693, `queue_for_reading`: `ProcessId`, `NativeWindowHandle`.
  Covered.
- 785, notifications: `ClassName`, `AutomationId`
  (`cached_ignores_notifications`, `map.rs:699`). Covered.

The listener's focus reader (`verbatim-outpost/src/focus_read.rs`).

- 344, `NativeWindowHandle` of the event element. Covered.
- 347, `event_parts`. Bug 1.
- 515, the focused element refreshed by the ancestry read with every
  detail (`FocusQuery::refresh`, properties from
  `cached_properties(Fetches::default())` at 374). Covered, subject to
  section 7's caveat when read remotely. Shape: `Ancestor` with every gate
  on, read through its `Node` group.
- 602, the selected child from the same read. Covered; the same shape.
- 763 and 766, `log_sender`: `ClassName` and `FrameworkId` of the event
  element. Covered.
- 805 to 842, the classic walk to the top level, a walker step per parent
  with every detail; each parent read by `ancestor_fact` (965 to 971,
  snapshot, window handle, and `IsContentElement` with
  `IsControlElement`). Covered. Shape: `Ancestor`.
- 865 to 886, `mark_window`: the window's element from its handle with
  the base request, then snapshot and content flag. Covered. Shape:
  `Ancestor`, 33 properties instead of 34.
- 936, `event_parts`: bug 1; it goes.

The outpost's event registrations (`verbatim-outpost/src/outpost/mod.rs`,
1086 to 1093). Property events use `FOCUS_EVENT_PROPERTIES`; caret, text
and active text position events use `TEXT_EVENT_PROPERTIES`. `capture`
(973 to 996) reads restricted, uncommitted. Shapes: `PropertyEvent` and
`TextEvent`.

The outpost's worker (`verbatim-outpost/src/outpost/worker.rs`).

- 776, the help text change's description, from the element refreshed
  with `DESCRIPTION_PROPERTIES` by `refreshed_for` (801 to 814). Covered.
  Shape: `HelpTextRefresh`.
- 826, `cached_reports_suggestions` after a refresh with
  `SUGGESTIONS_PROPERTIES`. Covered. Shape: `SuggestionsRefresh`.
- 2293, `controlled_selection`'s controller. Bug 3. No shape: the runtime
  id.
- 2296, the controlled selection's item, from the gated request (2276).
  Bug 2. Shape: `Node`.
- 2317, `fresh_selection`'s item found under the focus with the gated
  request (2348 to 2351). Bug 2. Shape: `Node`.
- 2365, `uia_node` keeps the event's element (`PropertyEvent`) or the
  found one (gated). See section 6.
- 2949, `follow_focus_element` keeps an element found by its path
  (2901, gated). See section 6.

The outpost's reads (`verbatim-outpost/src/outpost/read.rs`). The request
is `cache_request_for(fetches)` unless said otherwise.

- 174, `window_snapshot`: a window's element from its handle. Bug 2.
  Shape: `Node`.
- 232 and 266, the selected child (`client.rs:603`) and the ancestor chain
  (`client.rs:802` to 807). Bug 2. Shapes: `Node` and `Ancestor`.
- 436 to 471, the focus's ancestry through a remote operation, with the
  selected child read at 471. Bug 2. Shape: `Ancestor` for the query,
  `Node` for the child.
- 557 and 558, the UIA window an MSAA walk crosses into: snapshot, then
  `is_focus_context` (`client.rs:1135`, which reads the content flags).
  Bug 2. Shape: `Ancestor`.
- 696, the dialog's kept element, resolved without a refresh, for dialog
  text. Bug 4.
- 726, `focused_control`: the focused element. Bug 2. Shape: `Node`
  (named `FocusRead` in section 4.2's sense, the same shape).
- 802, `hosts_console_window`: `ProcessId`. Over-fetch. Shape:
  `ProcessCheck`.
- 868, a node's element found by its path, then kept. See section 6.
- 1000, `uia_remote_step` (978): `NativeWindowHandle` of the kept element.
  Covered: every kept cache holds it.
- 1033, the navigation step's neighbour. Bug 2. Shape: `Node`.
- 1061, `corrected_backend`: `NativeWindowHandle` of a kept element.
  Covered.
- 1091, a window's element from its handle. Bug 2. Shape: `Node`.
- 1154, `dump_tree`. Bug 2. Shape: `TreeDump`, which is `Node`.

The outpost's text reads (`verbatim-outpost/src/outpost/text_reads.rs`).

- 172, `ClassName` of the kept element. Covered: every kept cache holds
  it.
- 226 to 228, the log line. Bug 5.
- 244, keeps the event's `TextEvent` element. See section 6.

Dialog text (`verbatim-outpost/src/dialog_text.rs`).

- 224, `UiaObject::new` reads every object's snapshot: the root (bug 4)
  and each child from `children_with` (`client.rs:248`), whose request is
  the gated list the caller passes (`read.rs:693`), so up to 7 failed
  reads per child with details off (bug 2). Shape: `DialogText` for the
  children; the root becomes `Live`.

`verbatim-uia`.

- `client.rs:603`, `selected_child`. Bug 2. Shape: `Node`.
- `client.rs:645` and 651, `selected_element`: the container refreshed
  with `[Selection2FirstSelectedItem]` and the Selection pattern. Covered.
  Shape: `SelectionContainer`.
- `client.rs:710`, `navigate`. Bug 2. Shape: `Node`.
- `client.rs:802` to 807, `take_ancestor`. Bug 2. Shape: `Ancestor`.
- `client.rs:1065`, `walk_recursive`. Bug 2 through `read.rs:1154`; the
  mockapp tests call it with the full request. Shape: `TreeDump`.
- `checks.rs:66`, base request, no reads. Over-fetch. No shape: `Live`.
- `checks.rs:94`, `FrameworkId`. Covered. Shape: `FrameworkCheck`.
- `nearest.rs:107`, `NativeWindowHandle` after a normalize with that
  property alone. Covered. Shape: `WindowLookup`.

`verbatim-uia-rops`.

- `focus.rs:228`, the control type of the query's element, with a live
  fallback. Covered (decision 8).
- `focus.rs:238`, `own_window`, `NativeWindowHandle` of the query's
  element. Covered.
- `focus.rs:459` to 518, `RemoteCache`. Section 7.
- `navigate.rs:96`, `NativeWindowHandle` of the step's element, the kept
  element (`read.rs:991` to 997). Covered.
- `controlled.rs:188`, `path.rs:151`, `focus.rs:553`, `navigate.rs:144`:
  classic fallbacks building the query's list. Covered; they take the
  shape's request.

`verbatim-gui`.

- `shell_items.rs:269` to 281. Over-fetch. Shape: `ShellButton`.

### 5.6 The shapes

Event shapes, without gates:

- `ListenerEvent`: NVDA's 13 and the Text pattern, as now. Its role is
  read through a reduced group, `EventRole` (`ControlType`, `ClassName`,
  `NativeWindowHandle`), which gives exactly the role `event_parts` gives
  today (decision 6).
- `PropertyEvent`: today's 24 (`RoleCore`, `Name`, `ValueSet`, `States`,
  `ProcessId`).
- `TextEvent`: today's 3.

Fetch shapes:

- `Kept`: `NativeWindowHandle` and `ClassName`, which is all any reader of
  a kept element reads. It is never requested; every shape whose elements
  are kept declares `covers(Kept)`. `ProcessId` is in every kept cache
  too, but no reader of a kept element reads it.
- `Node`: `RoleCore`, `Name`, `ValueSet`, `States`, the four gated detail
  groups, `BoundingRectangle`: 24 to 31 properties.
- `Ancestor`: `Node` and `IsContentElement`, `IsControlElement`: 26 to 33,
  or 28 to 33 with decision 4's recommendation.
- `TreeDump`: `Node`.
- `DialogText`: `RoleCore`, `Name`, `ValueSet`, `States`, `Description`
  never gated: 25.
- `WindowLookup`: `NativeWindowHandle`.
- `ProcessCheck`: `ProcessId`.
- `FrameworkCheck`: `FrameworkId`.
- The empty shape: no request; the fetch returns `Live`.
- `SelectionContainer`: `Selection2FirstSelectedItem` and the Selection
  pattern.
- `ControlledSelection`: nothing for the controller (its runtime id), and
  `Node` for the item.
- `HelpTextRefresh` (`Description`) and `SuggestionsRefresh`
  (`IsTextPatternAvailable`, `AutomationId`), chosen per property id as
  `refreshed_for` chooses today.
- `ShellButton`: `NativeWindowHandle`, `ControlType`, `Name`,
  `IsOffscreen`, `BoundingRectangle`.

Attribute shapes: `AttributeSet` (gated), `Culture`, `ConsoleProbe`
(section 4.4).

## 6. The kept-element registry

`NodeIdRegistry` (`verbatim-uia/src/registry.rs:107`, `id_for_element`)
keeps an agile reference to an element per node, so later queries and
events reach it without a search. It is filled from elements of four
requests:

- the gated request: `snapshot_from_cached_element` (`map.rs:728`), which
  most fetches go through; `read.rs:868`; `worker.rs:2949`;
- `FOCUS_EVENT`: `worker.rs:2365`, a property event's element;
- `TEXT_EVENT`: `text_reads.rs:244`;
- the full request, where `snapshot_from_cached_element` is called on an
  element filled by `CACHED_PROPERTIES` (tests, and `worker.rs:2295`'s
  re-keeping of the focus's element, whatever filled it).

Readers of kept elements: `worker.rs:2293`, `read.rs:696` (dialog text),
`text_reads.rs:172` and 226, `read.rs:1000` and 1061, and
`verbatim-uia-rops/src/navigate.rs:96` through `read.rs:993`. They cannot
know which request filled the element they get. Most read only the window
handle and class name, which every one of the four holds, and are right by
luck; bugs 3, 4 and 5 are the ones that read more.

Under shapes the registry must record the shape with the element. Two
ways:

- An enum of the kinds kept: `Kept::Node(Cached<Node>)`,
  `Kept::PropertyEvent(..)`, `Kept::TextEvent(..)`. A reader matches and
  takes what each kind offers. It keeps everything known, but every reader
  then handles every kind, and a new kept kind changes every reader.
- One shape, `Kept`, that every kept kind covers. `id_for_element` takes
  `impl Into<Cached<Kept>>`, which `covers(Kept)` generates and checks at
  compile time. A reader gets `Cached<Kept>`, with the window handle and
  class name; one that needs more refreshes the element into a declared
  shape, which is a call it makes knowingly.

With the second, every reader in the list is served without a new call:
`worker.rs:2293` reads only the runtime id (bug 3's fix); `read.rs:1000`,
1061, `navigate.rs:96` and `text_reads.rs:172` read what `Kept` holds;
the dialog's root needs nothing (bug 4); and the log line reads the
runtime id and class name only (bug 5, decision 7). `resolve_uia_element`
(`read.rs:836`) already refreshes a kept element with the query's request
before reading it, which under shapes is `Live::refresh::<S>`. Decision 2
recommends the second.

## 7. Remote operations

The remote programs take a property list today: `FocusQuery::properties`,
`StepQuery::properties`, `ControlledQuery::properties` and the path
search's (`verbatim-uia-rops`, `focus.rs`, `navigate.rs`, `controlled.rs`,
`path.rs`). Under shapes they take `Fill<S>`, whose list is
`S`'s request for its gates, and use it exactly where the slice is used
now: to build the program's cache requests (`RemoteCache::new`,
`focus.rs:474`) and the classic fallback's request. The elements the
program returns are wrapped by `Fill<S>::claim`, giving `Cached<S>` with
the same gates.

One caveat a static shape cannot express. A cache filled by a remote
program's `PopulateCache` stores a property's default where a locally built
cache stores UI Automation's "not supported" value (established, the
comments at `cache.rs:60` to 65 and `focus.rs:449` to 457). For
`IsDataValidForForm` that default is false, which reads as an invalid
entry, so `RemoteCache` checks support in the program and leaves the
property out of an element's cache when the element does not support it,
with one request per combination (`LEFT_OUT_WHEN_UNSUPPORTED`,
`focus.rs:459`; `populate`, 498 to 518). `states_from_cached` then reads
the absent property, which fails inside UI Automation: one failed read per
remotely filled element without support (established from the code; how
many elements lack support has not been counted, and is estimated to be
most of them, since the property belongs to form fields).

Three resolutions:

- A per-element absent record. The program already computes a bit mask of
  the checked properties each element supports; it adds that mask to its
  results, one integer per element, and `Fill<S>::claim` stores it in the
  `Cached<S>` it makes (the `absent` field of section 3.3). The read of
  `IsDataValidForForm` returns "not supported" from the record without
  asking UI Automation. A local fill's record is empty. This is not an
  "is it cached?" check: it is Verbatim's own record of what its program
  did, like the gates.
- The program always fills the property, with its default, and returns the
  mask. Simpler program (one request instead of one per combination), same
  record on the read side; it needs the record all the same, since the
  default cannot be told from a real false.
- Leave `IsDataValidForForm` out of every remotely read shape. It loses
  the invalid-entry state for remotely read elements, which NVDA reports
  (`nvda/source/NVDAObjects/UIA/__init__.py:2041` to 2047, read ignoring
  the default, so "not supported" is valid).

Decision 3 recommends the first: the program changes by one result per
element, and its tested request-per-combination logic stays.

## 8. Costs and trade-offs

### 8.1 Cache sizes, before and after

Properties per request (established by counting the lists; the cost per
property in a provider is not measured):

- The listener's events: 13 and the Text pattern, unchanged; failed reads
  per event from 25 to none.
- The outpost's property events: 24, unchanged. Text events: 3, unchanged.
  Failed reads already none with the working tree's fix.
- Node fetches (focus, neighbours, selections, windows, tree dumps): 27 to
  34 today, 24 to 31 as `Node`, since `ProcessId`, `IsContentElement` and
  `IsControlElement` leave the paths that never read them; failed reads
  from up to 7 to none.
- Ancestors: 27 to 34 today, 26 to 33 as `Ancestor`, or 28 to 33 with the
  description always fetched (decision 4).
- Dialog text children: 27 to 34 today, 25 as `DialogText`.
- Console process check: 27 to 34 today, 1. Shell list: 34, 5. Console
  provider check: 34, none.

### 8.2 Event shapes stay minimal

UI Automation fills an event's cache in the application for every event it
delivers, including the ones Verbatim then drops (established:
`cache.rs:150` to 161 records NVDA's reason for its small base request, and
`wake-ups.md` item 12 measured the delivery cost). A property added to an
event shape is paid on every event at the application's event rate, a
flood included; a property added to a fetch shape is paid once per call.
So event shapes take no gates (the gates exist to trim fetches, and an
event's needs do not change with the theme) and carry what their handlers
read, no more. A handler that sometimes needs more fetches a second
declared shape when it does, as the help text and `ControllerFor` handlers
already do.

### 8.3 Caching more than a path needs

Dickson raised the cost of a shape fetching properties some of its users do
not read. Shapes are one per purpose, not one per call site: every node
reported to Core is a `Node`, though a neighbour's rectangle may go unused.
The extra properties on a one-call fetch are a few more provider answers
inside a round trip already being made (estimated: each property is a
provider call in the application, and providers vary, some such as the
rectangle or a description costing more than others; `call_counts.rs`'s
provider-side counts show the change for mockapp's providers in each
migration step). Splitting shapes per call site would make the inventory
above harder to keep right for little gain. Where a fetch is made in a
loop, as dialog text's children and the tree dump are, its shape is
checked for properties its loop never reads.

### 8.4 Local costs

A cache request is built per query today (`outpost/mod.rs:332`), and
shapes keep that; building one is local work with no cross-process call
(`calls.rs` module documentation). A shape's read copies every declared
property out of the cache, including ones a particular user ignores;
cached reads of present properties are in-process (estimated to be a few
microseconds each, set against the failed reads they replace).

## 9. Migration

Each step is small, passes `cargo xtask ci`, runs the mockapp tests it
touches and the end-to-end scenarios related to the paths it changes, and
updates the `docs/crates/` file of any crate whose public API it changes
(`verbatim-uia.md`, `verbatim-uia-rops.md`, `verbatim-outpost.md`). Tests
follow `docs/testing.md`: one focused test per stated behaviour, exact
counts, no new test files beyond the neighbouring ones. The working tree's
event-read fix is not committed on its own (Dickson, 2026-10-13: fix this
systematically); step 2 replaces it with the event shapes.

### Step 1. Count failed cached reads; the controller's runtime id

- Add `failed_cached_reads` to `CallCounts` (`calls.rs`), counted in
  `ElementExt`'s cached reads when UI Automation answers with an error,
  reported with each worker entry as calls are.
- `controlled_selection` takes the controller's id from
  `verbatim_uia::runtime_id(&focused)` (`worker.rs:2293`); the wrong
  comment goes.
- Tests: `call_counts.rs`'s expected counts gain the new field, which
  records today's failed reads where a test's path has them; one test in
  `controller_for.rs` that a controlled selection whose focus element was
  kept from a text event makes no failed cached read.

### Step 2. The shape module, the macro, and the event shapes

- `uia_shape!`, `Shape`, `Cached`, `Live` and the shape module's accessors,
  in `verbatim-uia`. Registrations take shapes (`subscribe.rs`).
- The listener: `ListenerEvent` for its registration (`listener.rs:817`)
  and every reader of its elements (`capture`, `queue_for_reading`, the
  notification check, `focus_read.rs` `read` and `log_sender`, and the
  query's `wants_selected_child`). `event_parts` goes.
- The outpost: `PropertyEvent` and `TextEvent` for its registrations;
  `snapshot_parts_from_event_cache`, `native_window_handle_from_event_cache`
  and `Restricted` go.
- Tests: unit tests of the macro in `verbatim-uia` (a shape's
  `PROPERTIES` is its fields' union without repeats; a gated group is in
  the request only when its gate is on; a gated group reads `None` when
  off). In `events.rs`, one test that the listener's selection and
  notification facts are read with no failed cached read. The uncommitted
  comparison test in `uia_tree.rs` is replaced by one that each event
  shape's read of every scripted element gives the fields the full read
  gives. A trace of `conhost_flood` confirms the callback threads' time
  matches item 12's 0.83 seconds.

### Step 3. The kept-element registry

- `id_for_element` takes `Cached<Kept>` (decision 2); readers take
  `Cached<Kept>`; the dialog's root becomes `Live` and is not read; the
  log line reads the runtime id and class name.
- Tests: in `call_counts.rs`, `uia_dialog_text_costs_exactly` with the
  dialog's element kept from a property event, no failed cached read; in
  `uia_tree.rs` or `events.rs`, a node kept from a text event navigated
  remotely with no failed read.

### Step 4. Fetch shapes

- `verbatim-uia`'s fetch functions take shapes; `Node`, `Ancestor`,
  `TreeDump`, `DialogText`, `SelectionContainer`, the refresh shapes,
  `WindowLookup`, `ProcessCheck`, `FrameworkCheck` and the empty shape
  replace the lists; the outpost converts at every site in section 5.5.
- `verbatim-uia-rops` takes `Fill<S>`, returns the absent record
  (decision 3), and loses `wants_selected_child`'s fallback (decision 8).
- The gate functions from `Fetches`, with a unit test per gated field.
- `verbatim-gui`'s shell list takes `ShellButton`.
- This step may be split by crate if its diff is large; each part passes
  CI.
- Tests: in `call_counts.rs`, a focus read with descriptions off makes no
  failed cached read, and the provider-side counts drop by the properties
  removed; in `remote_ops.rs`, a remotely read element without
  `IsDataValidForForm` reads as a valid entry with no failed read; the
  gate unit tests. End-to-end: the focus, selection, dialog, navigation
  and terminal scenarios.

### Step 5. Formatting attributes

- `uia_attributes!` and the three attribute shapes; `attributes_for` and
  the matching half of `attributes_of` go; the learned unsupported set
  stays a subtraction.
- Tests: a unit test per gate (its attributes in the list exactly when
  on); the caret formatting tests in mockapp unchanged; the Notepad
  formatting scenarios.

### Step 6. Close the API, and the CI check

- `ElementExt`'s `cached_*` methods become private to the shape module;
  `cached_properties`, `cache_request_for`, `base_cache_request` and the
  property lists leave the public API.
- The `cargo xtask ci` check of section 3.5, item 4, with a unit test in
  `xtask` that a cached read outside the shape module fails it, as
  `waits.rs` is tested.

## 10. Decisions for Dickson

1. Declarative macro or hand-written shapes. A `macro_rules!` macro
   generates the property list and the read from one field list, which is
   the guarantee. Hand-written impls, each with a `PROPERTIES` constant and
   a `read` function, need no macro but let the two drift, caught only at
   run time by the debug-build check. Recommend: the macro.
2. The kept-element registry (section 6): an enum of kept kinds, or one
   `Kept` shape that every kept kind covers, with readers that need more
   refreshing explicitly. Recommend: the `Kept` shape; every current reader
   is served by it without a new call.
3. `IsDataValidForForm` in remotely filled caches (section 7): an absent
   record from the program's existing support test, the program always
   filling the property with the same record, or the state dropped for
   remote reads. Recommend: the absent record, with the program changed by
   one result per element.
4. The description of ancestors, and `is_layout` (section 5.4). Fetch the
   description for `Ancestor` whatever the theme, so the layout judgment
   is NVDA's under every theme and the outpost judges ancestors as the
   listener does; it costs two properties per ancestor read by the
   outpost. Or gate it, and decide that "not fetched" means "no
   description" for the layout judgment, which keeps today's behaviour and
   its departure from NVDA. Recommend: always fetched.
5. Dialog text (section 5.4 and bug 4). Fetch the children's description
   whatever the theme, as NVDA's dialog text includes it, and read nothing
   of the dialog's root element, which no rule uses, instead of refreshing
   a kept root. Recommend both.
6. The listener's event role. `ListenerEvent` reads the role through
   `ControlType`, `ClassName` and `NativeWindowHandle` alone, giving the
   role it gives today; adding `IsDialog` and `IsTogglePatternAvailable`
   to the listener's request would refine it but departs from NVDA's base
   request, which is a decision on record, and the outpost reads the role
   afresh anyway. Recommend: the reduced group.
7. The log line in `text_event_from` (bug 5): log the runtime id and class
   name only, or refresh both elements for the name and framework. It is
   an `info` line for a rare event. Recommend: runtime id and class name.
8. `wants_selected_child`'s live fallback in `verbatim-uia-rops`
   (section 5.2): remove it, since every shape given to the focus query
   carries the control type. Recommend: remove.
