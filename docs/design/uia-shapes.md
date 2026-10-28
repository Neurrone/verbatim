# UIA shapes: every cached read declared with the request that fills it

Status: adopted and built. The mechanism, its types and the shapes that
exist are described in `docs/crates/verbatim-uia.md` ("The shape module",
`NodeIdRegistry`), the remote programs' side in
`docs/crates/verbatim-uia-rops.md`, and the theme's gates in
`docs/crates/verbatim-outpost.md`. This document keeps the problem, the
alternatives weighed, the reasons for the choices, and their limits.

## 1. Summary

- Every read of a UI Automation element declares exactly the properties
  it uses, and the cache request is built from the same declaration, so
  only answers from the cache are read, with no "is it cached?" checks and
  no live fallbacks of the kind NVDA makes (Dickson's request, 2026-10-13).
- A shape is a struct declared once, by a macro private to `verbatim-uia`,
  which generates its request and its read from one field list. A request
  is a `Request<S>`, an element it filled a `Cached<S>`, one handed to
  another thread an `Agile<S>`, and everything else a `Live`, with no
  cached reads. Theme gating is part of the type: a gated field reads as an
  `Option`, and request and read take the gates from one value.
- The compiler enforces most guarantees inside `verbatim-uia`; a
  `cargo xtask ci` scan enforces the rest outside it; what a remote program
  puts in its caches is an audited trust boundary, checked by tests and by
  a count of failed cached reads (section 3.5).

## 2. The problem

### 2.1 What a read outside the cache costs

UI Automation answers a cached read of a property the element's cache
request did not hold by raising and reporting an error inside itself. The
result is right (the property reads as unsupported), but the error path is
expensive. In `conhost_flood`, over 6 seconds, the console's outpost used
7.5 seconds of processor time, 3.5 of them on UI Automation's callback
threads, most of it raising, unwinding and formatting those errors: a text
event's request held three properties and the handler read 31. Reading only
the registration's properties cut the callback threads to 0.83 seconds.
The cost falls on whichever thread reads, so the same mistake on the worker
or the listener's reading threads costs those threads alike.

Before shapes, one mapping function read 31 cached properties whatever
request filled the element, cache requests were built separately from
lists, and nothing connected the two. The same pattern was found at the
listener, which read 25 properties absent from its request on every
desktop-wide event, and at about twenty fetch sites, which read up to 7
absent properties per element when the theme turned details off. A
run-time filter against a second copy of the list fixes one path but is the
"is it cached?" test by another name and does not stop the next path.

### 2.2 NVDA, for comparison

NVDA reads a property from an element whose cache this core cycle holds
it, and otherwise by a live cross-process read
(`_getUIACacheablePropertyValue`, `nvda/source/NVDAObjects/UIA/__init__.py`).
Its cached values come from its base cache request (13 properties and the
Text pattern) and from prefetches for one purpose. So NVDA never reads an
absent cached property, but pays a cross-process call for each property it
did not prefetch. Verbatim's rule is that mapping reads only the cache
(architecture section 4, "cache requests everywhere"); this design makes
that rule checkable.

## 3. The design

### 3.1 Shapes

A shape's fields are what one purpose reads of an element. Groups are
where mapping lives: the role group holds the five properties a role is
mapped from and has the method that maps them, so every read maps a role
the same way. A property that two groups share is requested once.

- Read kinds. Readers differ in ways a Rust type does not show: a
  one-based number reads zero as unsupported, an `optional_bool` ignores
  UI Automation's defaults where a `bool` does not, a window handle reads 0
  for none. So each property names its kind, and the field's Rust type is
  the kind's, never written separately, so kind and type cannot disagree.
- Patterns are declared as read (the pattern object) or prefetched (held by
  the request, read by nothing), which is how the listener's event shape
  keeps NVDA's base request's Text pattern. Patterns are never gated.
- Every field is read eagerly. A value that means something only when its
  pattern is available, such as the range value or `ValueIsReadOnly`, is
  read anyway, and the condition stays in the group's mapping method,
  where it must not be simplified away, because UI Automation answers
  defaults for unsupported patterns.
- Every shape is declared in the shape module, including those only one
  other crate uses, because Rust allows inherent methods only in the
  defining crate; mapping that belongs to another crate is free functions
  there over the plain struct a read returns.

### 3.2 The macro and sealing

A declarative macro (`macro_rules!`), not exported, generates each shape.
No procedural macro is needed. `Shape`, `Ungated` and `Covers<T>` are
sealed, so no other crate can declare a shape, and a shape with an empty
property list and a read that reads anything cannot be written outside the
shape module.

- Property lists are `&'static` slices computed at compile time, because
  nothing generic over a shape can have an array sized from it on stable
  Rust (it needs `generic_const_exprs`).
- A request is computed at run time as the union of what its gates turn
  on, so a property both gated and ungated is requested whenever the
  ungated field asks for it.
- A shape declared with no gates accepts no gated field and passes each
  group `()` as its gates, so a shape that claims to be ungated and holds a
  gated group does not compile.
- "This shape covers that one" is a compile-time assertion that fails the
  build (error E0080) when the covered shape's properties or patterns are
  missing.
- Tree scope and filter belong to the fill call, not the shape.

### 3.3 `Cached`, `Live` and `Agile`

- `Cached<S>` and `Agile<S>` have no public constructor; `Live` has one,
  since it has no cached reads to misuse.
- `Cached<S>` is neither `Send` nor `Sync`, since the element is neither.
  `Agile<S>` is both with no `unsafe` of Verbatim's, because the agile
  reference is and the gates are `Copy`, `Send` and `Sync` by the trait's
  bound. Its marker is `PhantomData<fn() -> S>`, not `PhantomData<S>`,
  because with the latter a shape holding a cached pattern object would
  make its `Agile` neither `Send` nor `Sync`.
- Every hand-off between threads carries an `Agile<S>`, so the shape
  survives it. Within the process the cache survives too: the element
  aggregates the free-threaded marshaler, so resolving returns the same
  pointer and cache. A resolve on a single-threaded apartment thread,
  which would go through a proxy, is not verified; every Verbatim thread
  that uses UI Automation is in the multithreaded apartment, and if that
  ever changed the failed-read count (section 3.6) would show it.
- Deliberate live reads stay deliberate: the legacy checked state of a
  menu item, `HasKeyboardFocus` before a focus read, and the
  `ControllerFor` relation are calls made where a rule says to make them,
  never a fallback for a property a cache lacked.
- `Live` must expose the raw element, since remote programs import it and
  pattern and text code call it, so the `windows` crate's own cached
  getters are callable on it. The type system cannot stop that; the scan
  does (section 3.5, item 7).

### 3.4 Requests and registrations

Every function that fills a cache takes a `Request<S>` alone and returns
`Cached<S>` with the request's gates, so a request kept for a query cannot
be paired with other gates. A call that needs no cache returns `Live`.

Event registrations take only ungated shapes, whose handlers receive
`Cached<S>`; the registration erases the type inside, so one registration
holds parts of several shapes. A handler that occasionally needs more than
its event shape holds refreshes the element into a second declared shape,
one call when it happens (section 8.2 says why event shapes stay minimal).

Remote operations take a token, `Fill<S>`, in place of a property list,
and their inputs are typed by shape (section 7).

### 3.5 The guarantees, and what enforces each

Enforced by the compiler, inside `verbatim-uia`:

1. Only a shape's generated read reads a cache through Verbatim's readers:
   the kind readers are private to the shape module, the macro to the
   crate, `Shape` is sealed, and the read takes a crate-private `Reader`.
2. An element read as `S` was filled by `S`'s request: a `Cached<S>` is
   made only by a fill from a `Request<S>`, by a registration's part for
   `S`, by resolving an `Agile<S>`, by `narrow` where `S` covers the
   target, and by `Fill<S>::claim`, the trust boundary of item 8.
3. Request and read agree on the gates: the request keeps them, every fill
   copies them, and a gated field reads as `Option`, so every user handles
   "not fetched".
4. Event registrations and covered shapes have no gates (`Ungated`).
5. A shaped element crosses threads only as `Agile<S>`.

Checked by a unit test:

6. A shape's read names exactly the properties and patterns its request
   carries, for every combination of gates, in both directions, so an
   over-fetch fails as a missing read does (section 3.6).

Enforced by the `cargo xtask ci` scan, outside the shape module:

7. No raw cached read and no raw refill: the scan fails on `GetCached` or
   a `.Cached` getter anywhere but the shape module and the read of cached
   children, on a raw cache refill outside `verbatim-uia`, and on
   `Fill::claim` outside `verbatim-uia-rops`. This is the scan's guarantee,
   not the compiler's, because `Live` exposes the raw element.

An audited trust boundary, checked by tests and by the failed-read count:

8. A remote program fills its caches from `Fill<S>`'s list. That it builds
   its caches from that list and nothing else is reviewed code in
   `verbatim-uia-rops`, tested by mockapp's `remote_ops.rs`, not a property
   the compiler proves.
9. The absent record a remote program returns (section 7) is taken as
   given. A wrong record makes a property read as unsupported or makes a
   failed read; the second is counted.

Not enforced by any of these: that a theme maps to the right gates
(section 4.3). Outside any guarantee: whether a provider supports a
property (it reads as unsupported), whether the element is still alive,
and whether a shape declares properties no user consumes, a cost kept down
by one shape per purpose (section 8.3).

### 3.6 The checks: a recording test and a failed-read count

The recording test reads every shape, for every combination of its gates,
against a source that answers every read and records it, and asserts that
the set read, with the prefetched patterns, equals the request exactly. It
covers the macro and any hand-written group in both directions and costs
nothing at run time. It was chosen over a debug-build check of each read
against the carried list, which would be the "is it cached?" check by
another name and would check the macro against itself.

The failed-read count counts every cached read UI Automation answers with
a failure code. It is a thread-local kept in every build profile, so exact
counts in tests do not depend on the profile, and it is not part of
`CallCounts`, whose total means calls. It is gathered per owner (each
registration's callbacks, the listener's reading threads, the outpost's
worker), not into one global counter, because the mockapp tests run
outposts in the test process and in parallel. Nothing panics on a UI
Automation callback thread, where a panic would abort the process. The
count catches what no declaration can: a remote program leaving a property
out, a wrong absent record, or a cache that did not survive a hand-off.

## 4. Themes and run-time gating

### 4.1 Gates

Each gate belongs to one field and is named after it: a gated property
adds a `bool`, a gated group adds an `Option` of the group's own gates
(`None`: not fetched), and an ungated group adds its gates as they are, so
nested shapes' gates nest. A gated group switched off has no inner gates
to set, so "group off, inner gate on" cannot be written.

- One gate per field rather than a header of named gates: a `macro_rules!`
  macro cannot remove repeated identifiers, and with gates named after
  fields a repeat is a duplicate struct field, which rustc rejects. Two
  properties that must switch together are one group. A gate shared by
  separate fields is needed only for text attributes (section 11); if an
  element shape ever needs one, the header is the way, and it changes the
  macro alone.
- "Not fetched" reads as a plain `Option`, not a dedicated `Gated<T>`
  without `flatten` and `unwrap_or_default`. The two uses where not fetched
  must not mean absent, the layout judgment and dialog text, read ungated
  fields (decisions 4 and 5), so every remaining gated field feeds speech,
  for which not fetched means the indication is off; a `Gated<T>` would add
  a `match` at every use for no case that needs it.
- The bounding rectangle is not gated by the theme; it is in the `Place`
  group, gated by the kind of read (decision 9).

### 4.2 A worked example: a focus read with descriptions off

With the description indication off, the theme's fetches give
`description: false` and the node gates `description: None`. The request
then lacks `FullDescription` and `HelpText`, so the provider is not asked
for either; the read sets the description to `None` without touching the
cache, so no failed read is made; and Core receives no description, as the
setting asks. The focus's ancestors are read as `Ancestor`, whose ungated
description the layout judgment reads, so an unnamed grouping with a
description is focus context whatever the theme (decision 4), while the
ancestor's spoken description still follows the gate.

### 4.3 What the compiler cannot check

That a gate comes from the right indication is the one link the types
cannot make, because the theme is in `verbatim-model`, a platform-neutral
crate that knows nothing of UI Automation. The mapping is one function per
gated shape in `verbatim-outpost`, with one unit test per gated field:
turning that one setting off turns off that one gate and no other.

## 5. Behaviour coupled to a gate

Two readers need a value whatever the setting, as NVDA's do
(`nvda/source/NVDAObjects/__init__.py`, `presentationType` and
`basicText`): the layout judgment, which counts an unnamed window,
property page or grouping as layout unless it has a description, and would
otherwise judge a chain differently when the listener read it with every
detail; and dialog text, which includes each child's description. Both read
ungated description groups (decisions 4 and 5).

## 6. The kept-element registry

The outpost's node registry keeps an element per node, filled by several
requests (fetches, property events, text events), and its readers cannot
know which request filled the element they get. Two ways were weighed:

- An enum of the kinds kept, each with its own `Agile`. It keeps everything
  known, but every reader handles every kind, and a new kind changes every
  reader.
- One shape, `Kept` (native window handle and class name), that every kept
  kind covers, stored as `Agile<Kept>`; a reader that needs more refreshes
  the element into a declared shape, a call it makes knowingly.

The second was chosen (decision 2): every reader was served by it without
a new call, given that the remote focus query's input, which needs the
control type, is typed as `FocusInput` (section 7) so `Kept` need not grow.

## 7. Remote operations

`verbatim-uia-rops` is another crate and cannot use `verbatim-uia`'s
private constructors, so it is handed a token, `Fill<S>`, giving the
program's property and pattern lists and its classic fallback's request,
and the only way to wrap a returned element as `Cached<S>` (`claim`).
`Fill<S>` is neither `Clone` nor `Copy`, and its `claim` is allowed by the
scan only inside `verbatim-uia-rops`. Nothing in the type ties a token to
one program run or proves the program used its list (section 3.5, item 8).

The programs' inputs are typed too: the focus query's input is a
`Cached<FocusInput>` (control type and native window handle) and the step
query's a `Cached<Kept>`, each read before the program runs. A caller
holding only a `Cached<Kept>` cannot hand it to the focus query, which is
what makes removing the control type's live fallback safe (decision 8).
Whether a program's `PopulateCache` on an imported input element leaves
the client-side input's cache alone is not verified; the inputs are read
before the program runs, so nothing depends on it.

The caveat a static shape cannot express: a cache filled by a remote
program stores a property's default where a locally built cache stores
"not supported". For `IsDataValidForForm` the default, false, would read as
an invalid entry, so the program checks support and leaves the property out
of an element that does not support it, and a read of the absent property
would then be a failed read. Three resolutions were weighed:

- A per-element absent record: the program returns the support mask it
  already computes, one integer per element, and a read of a property the
  record holds answers "not supported" without asking UI Automation. This
  is Verbatim's own record of what its program did, like the gates, not an
  "is it cached?" check.
- The program always fills the property with its default and returns the
  mask: a simpler program, but the same record is needed on the read side,
  since the default cannot be told from a real false.
- Leaving `IsDataValidForForm` out of every remotely read shape: loses the
  invalid-entry state, which NVDA reports.

The first was chosen (decision 3): the program changes by one result per
element, and its tested request-per-combination logic stays.

## 8. Costs and trade-offs

### 8.1 Cache sizes

Properties per request, from the lists (the cost per property in a
provider is not measured): the listener's events 15 and the Text pattern,
two more than NVDA's base request (decision 6); the outpost's property
events 23; text events 3; `Node` 22 to 34; `Ancestor` 27 to 37; a dialog's
child 24. Failed reads on every one of these paths are none.

### 8.2 Event shapes stay minimal

UI Automation fills an event's cache in the application for every event it
delivers, including those Verbatim then drops, which is NVDA's reason for
its small base request. A property added to an event shape is paid on every
event at the application's event rate, a flood included; one added to a
fetch shape is paid once per call. So event shapes take no gates (an
event's needs do not change with the theme) and carry what their handlers
read, no more.

### 8.3 Caching more than a path needs

Shapes are one per purpose, not one per call site: every node reported to
Core is a `Node`, though a neighbour's rectangle may go unused. The extra
properties on a one-call fetch are a few more provider answers inside a
round trip already being made, though providers vary and some properties,
such as the rectangle or a description, cost more than others. Splitting
shapes per call site would make the shapes harder to keep right for little
gain. Where a fetch is made in a loop, as dialog text's children and the
tree dump are, its shape is kept free of properties the loop never reads,
and a read of many elements never holds the place (decision 9).

### 8.4 Local costs

A cache request is built per query; building one is local work with no
cross-process call. A shape's read copies every declared property out of
the cache, including ones a particular user ignores; such reads are
in-process, estimated at a few microseconds each, against the failed reads
they replace.

## 9. Limitations

- The scan, not the compiler, keeps raw cached reads out of other crates,
  because `Live` exposes the raw element.
- A remote program's fills and absent records are trusted as reviewed and
  tested code, with the failed-read count as the check.
- Whether a provider supports a property, and whether an element is still
  alive, are outside any shape's guarantee.
- A resolve of an `Agile<S>` on a single-threaded apartment thread is not
  verified (section 3.3).
- Formatting attributes are not shapes yet (section 11).

## 10. Decisions

Settled by Dickson on 2026-10-13 unless dated otherwise.

- Decision 1. Declarative macro or hand-written shapes. Decided: the
  macro, with the recording test as its check, so a shape's request and
  read come from one field list and a drift fails the build, not only
  `cargo xtask ci`.
- Decision 2. The kept-element registry (section 6). Decided: one `Kept`
  shape that every kept kind covers, stored as `Agile<Kept>`, with readers
  that need more refreshing into a declared shape explicitly.
- Decision 3. `IsDataValidForForm` in remotely filled caches (section 7).
  Decided: the absent record, from the program's existing support test; it
  is part of the audited trust boundary (section 3.5, item 9).
- Decision 4. The description of ancestors and the layout judgment
  (section 5). The layout judgment always has ancestors' descriptions,
  whatever the theme, as NVDA's `presentationType` does: `Ancestor` holds
  an ungated `Description` group beside `Node`'s gated one, so the outpost
  judges ancestors as the listener does. It costs two properties per
  ancestor read by the outpost with descriptions off.
- Decision 5. Dialog text (section 5). Dialog text always includes each
  control's description, whatever the theme, as NVDA's `basicText` does,
  through an ungated `Description` group in `DialogText`; and nothing of
  the dialog's own element is read.
- Decision 6. The listener's event role. Decided: widened. The listener's
  request adds `IsDialog` and `IsTogglePatternAvailable` to NVDA's base
  request, so `ListenerEvent` reads the role through `RoleCore`, refined
  for dialogs and toggle buttons as every other read is, at the cost of two
  properties per desktop-wide event.
- Decision 7. The text event's log line logs the runtime id and class name
  only.
- Decision 8. The remote focus query's live fallback for its input's
  control type is removed, made safe by the typed `Cached<FocusInput>`
  input.
- Decision 9 (Dickson, 2026-10-24). A read of many elements, a tree walk,
  a list of children, an ancestor chain, never holds `BoundingRectangle`
  or `IsOffscreen`; the two are read only for the one element that needs
  them, as NVDA's base cache, on every element it walks to or is sent with
  an event, holds neither (`UIAHandler/__init__.py`,
  `baseCachePropertyIDs`), and NVDA reads them per object. For some
  elements UIA answers either only by asking the application's UI thread,
  a top-level window's title bar and its buttons among them, for which it
  sends `WM_GETTITLEBARINFOEX`; Windows answers that for one window at a
  time across the desktop, about 6 ms alone and seconds under load. The
  two are the `Place` group, gated in `Node`: on for a focus, a
  navigation step's neighbour, a selected item, a window; off for
  ancestors and the tree dump unless it is asked for locations. No event
  registration's cache holds either, `PropertyEvent`'s among them, so a
  state change's set, which replaces the focus's, is never off the
  screen. The shell's tray and taskbar list reads `IsOffscreen` for every
  button, to leave hidden ones out, and, for a click, the position of each
  button with the clicked item's runtime id or name, normally one, never
  in the list (Dickson, 2026-10-24).

## Later: extensions

Wasm extensions (D6, milestone M5) never hold a UI Automation element: the
elements are in the outposts, another process, and a Wasm guest cannot
hold a COM object. An app module will declare, when it is activated, the
properties it wants for a purpose (focus, navigation), validated by Core
against the properties UI Automation defines and the extension's grant,
and sent to its application's outpost, which adds them as a run-time group
to its requests and returns their values in the snapshot; a hook needing
something unusual makes one declared fetch of a node. The run-time list is
the one value that builds the request and that the read iterates, so the
guarantee holds by construction for extension properties as the compiler
holds it for Core's. Extension properties join fetches only; adding them
to event registrations, which UI Automation fills for every event
delivered, is a separate, budgeted grant. Nothing of this is built until
the extensions are (Dickson, 2026-10-13): the shapes code carries no
run-time group until then.

## 11. Later: formatting attributes

Formatting is not element properties but text range attributes (font,
size, weight, italic, underline, strikethrough, colours, bullet style,
link, and the annotation types that carry spelling and grammar errors),
read by the caret's remote operation or classically with
`GetAttributeValues`, and gated by the same document-formatting settings
in two places in the outpost: which to read, and what they mean. A read of
an unsupported attribute returns UI Automation's "not supported" object
rather than an error, so the cost of section 2 does not arise, which is why
attributes are outside this design.

What the same pattern would give is agreement: one declaration generating
the attribute list for given gates and the read of the results into a
struct with `Option` fields for gated attributes. Its gates would need the
gates header of section 4.1, since the annotation types are read for either
spelling or grammar errors. Three attribute shapes would cover the reads:
the caret's formatting, gated by the document-formatting settings; the
language of a unit, never gated; and the console's provider check, the font
name alone. The set of attributes a provider was found not to support would
stay a run-time subtraction from the request, since it is learned from
answers. It deserves a design of its own when taken up.
