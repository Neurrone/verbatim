# UIA shapes: every cached read declared with the request that fills it

Status: adopted; built as of 6ab23765 (2026-10-13), all five steps of section 9.

Proposed 2026-10-13, at Dickson's request after items 12 and 13 of
`docs/design/wake-ups.md`. He asked whether every read of a UI Automation
element can declare exactly the properties it uses, so that the cache
request is built from the same declaration and only answers from the cache
are used, with no "is it cached?" checks and no live fallbacks of the kind
NVDA makes. It can; this document sets out how, with an inventory of every
cached read in the workspace and the shape each one would take.

Revised the same day after an adversarial review, whose findings and what
this document now does about each are in section 12. Dickson asked that
the design use Rust's type system to make invalid states unrepresentable
and that it be checked for soundness; the review was that check. He also
decided that the three faults found in passing are to be fixed (decisions
4 and 5, section 10).

Researched on main at 8511cd1, with the working tree's uncommitted
event-read fix in place (`verbatim-uia/src/map.rs` and
`verbatim-outpost/src/outpost/mod.rs`); line numbers are of that working
tree, which also holds an uncommitted third subscription group in
`verbatim-uia/src/subscribe.rs` (section 3.4). Nothing was built or run
in the repository; the claims about what stable Rust accepts were checked
with `cargo check` in a scratch crate outside it, whose recording test
(each shape's read against its request, for every combination of gates)
also passes under `cargo test`. Code is cited as
`crate/src/file.rs:line`, the crate folder under `crates/`; NVDA as
`nvda/source/...:line`. A statement marked "established" comes from the
code, from a recorded measurement, or from the scratch crate; one marked
"estimated" is reasoning, with the reasoning given, for the migration's
measurements to confirm.

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
- The design: a shape is a struct declared once, in `verbatim-uia`'s shape
  module, with a macro private to that crate. Each field is a property
  with a named read kind, a group that is itself a shape, or a cached
  pattern. The macro generates the shape's property list, its request for
  given gates, and the one function that reads the cache into the struct.
  A request is a `Request<S>`, which keeps its gates; an element it filled
  is a `Cached<S>`, which keeps the same gates; an element handed to
  another thread is an `Agile<S>`; everything else is a `Live`, which has
  no cached reads.
- Theme gating is part of the type: a gated property's gate is a `bool`,
  a gated group's gate is an `Option` of the group's own gates, and the
  field reads as an `Option`. Request and read take the gates from the one
  value the request was built with, so they cannot disagree, and every user
  handles "not fetched".
- What enforces what (section 3.5). Inside `verbatim-uia` the compiler
  does: the shape trait is sealed, the macro and the cache reader are
  private, `Cached<S>` has no public constructor, `Agile<S>` crosses
  threads with no `unsafe`, event registrations accept only shapes without
  gates, and "this shape covers that one" is a compile-time assertion.
  That a shape's read names exactly its request's properties follows from
  the macro's single field list and is checked by a unit test for every
  combination of gates. Outside `verbatim-uia`, a `cargo xtask ci` scan
  rejects raw cached reads and raw cache refills, and confines the remote
  operations' token, `Fill<S>`, to `verbatim-uia-rops`. What a remote
  program puts in its caches, and the record it returns of properties it
  left out, is an audited trust boundary, checked by tests and by a count
  of failed cached reads, not by the compiler.
- Decided by Dickson on 2026-10-13: the layout judgment and dialog text
  read descriptions whatever the theme, as NVDA does, and the dialog's own
  element is no longer read (sections 5.4 and 10).
- Two things a static declaration cannot express need a decision: the
  outpost's node registry keeps elements filled by four different requests
  (section 6), and a remotely filled cache leaves `IsDataValidForForm` out
  for an element that does not support it (section 7).
- Migration is five steps, each passing `cargo xtask ci`, with the old and
  new entry points side by side until the last (section 9). Formatting
  attributes are left for later (section 11). Six decisions remain for
  Dickson (section 10).

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
The private trait itself stays, as the seam between the shape module's
reader and its sources: the element, and a recording source in tests
(section 3.6).

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
element. A field is a property, read with a named read kind; a group,
which is itself a shape; or a cached pattern. Groups are where the mapping
lives: the role group holds the five properties a role is mapped from and
has a method that maps them, exactly as `parts_of` does today.

```rust
uia_shape! {
    /// What a role is mapped from (`map.rs` `parts_of`).
    pub struct RoleCore {
        gates: none;
        properties {
            control_type: ControlType => control_type,
            class_name: ClassName => string,
            is_dialog: IsDialog => bool,
            toggle: IsTogglePatternAvailable => bool,
            hwnd: NativeWindowHandle => window_handle,
        }
    }
}

impl RoleCore {
    /// The role, refined as `parts_of` refines it today.
    pub fn role(&self) -> Role { /* role_from_control_type, terminal, dialog */ }
}

uia_shape! {
    /// A node as Core is told of it: a focus, a neighbour, a tree dump.
    pub struct Node {
        gates: NodeGates;
        groups {
            role: RoleCore,
            value: ValueSet,
            states: States,
            #[gated] description: Description,
            #[gated] shortcut: Shortcut,
            #[gated] position: Position,
        }
        properties {
            name: Name => string,
            #[gated] level: Level => one_based,
            rect: BoundingRectangle => rect,
        }
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

Read kinds. Today's readers differ in ways a Rust type does not show: a
one-based number reads zero as unsupported (`map.rs`, `cached_one_based`),
an `Option<bool>` read ignores UI Automation's defaults where a `bool`
read does not, and a window handle reads 0 for none. So each property
names its kind, and the field's Rust type is the kind's, not written
separately, so a kind and a type cannot disagree. There is one kind per
reader of `map.rs:296` to 448 and `element.rs:47` to 96: `bool`,
`optional_bool` (ignoring defaults), `i32`, `f64`, `string`, `one_based`,
`rect`, `control_type`, `framework_id` and `window_handle`, with today's
handling of defaults unchanged. Two kinds read a property whose value is
an element: `element`, the element it names, and `names_any`, whether a
relation names any element.

Patterns. A `patterns { selection: Selection }` section declares a cached
pattern read as the pattern object (`SelectionContainer`). A
`prefetch { Text }` section declares a pattern the request holds and no
read uses, which is how `ListenerEvent` keeps NVDA's base request's Text
pattern (`listener.rs:817`). Patterns are never gated. The shape's
`PATTERNS` constant lists both.

The macro reads every field eagerly. Today `RangeValueValue` is read only
when the range value pattern is available and `ValueIsReadOnly` only when
the Value pattern is; reading them unconditionally is harmless, since both
are cached, but the conditions that decide whether their values mean
anything stay in the group's mapping method, for the reason the comments at
`cache.rs:60` to 65 give, and must not be "simplified" away.

Every shape is declared in the shape module, including those only one
other crate uses: `ShellButton` (`verbatim-gui`), `HelpTextRefresh`,
`SuggestionsRefresh` and `FocusInput`. Rust allows inherent methods only
in the defining crate, so mapping that belongs to another crate becomes
free functions there over the plain struct a read returns; the struct's
fields are public and hold plain values.

### 3.2 The macro, the trait, and sealing

`uia_shape!` is a declarative macro (`macro_rules!`) private to
`verbatim-uia`: it is not `#[macro_export]`ed, so no other crate can
declare a shape. For a shape `S` it generates the struct, its gates type
and:

```rust
mod sealed { pub trait Sealed {} }

pub trait Shape: sealed::Sealed + Sized {
    /// A bool per gated property; each group's gates, as an `Option` for a
    /// gated group (`None`: not fetched). `()` for a shape declared with
    /// `gates: none`.
    type Gates: Copy + Send + Sync + 'static;
    /// Every property of every field and group, without repeats.
    const PROPERTIES: &'static [UIA_PROPERTY_ID];
    /// The properties of ungated fields at every depth: what every request
    /// of this shape carries, whatever its gates.
    const UNGATED: &'static [UIA_PROPERTY_ID];
    /// The patterns the request holds, read and prefetched.
    const PATTERNS: &'static [UIA_PATTERN_ID];
    /// Every gate on, as the listener reads ancestors and tests fetch.
    fn all_on() -> Self::Gates;
    #[doc(hidden)]
    fn push_request(gates: &Self::Gates, out: &mut Vec<UIA_PROPERTY_ID>);
    /// The only code that reads a cache. `Reader` is private to the crate,
    /// so no other crate can call it.
    #[doc(hidden)]
    fn read_from(reader: &Reader<'_>, gates: &Self::Gates) -> Self;
}

/// A shape with no gate at any depth: what an event registration and a
/// covered shape must be.
pub trait Ungated: Shape<Gates = ()> {}
/// `Self`'s request holds every property and pattern of `T`'s, ungated.
pub trait Covers<T: Ungated>: Shape {}
```

`Shape`, `Ungated` and `Covers` are sealed: `Sealed` is in a private
module, so no other crate can implement them, and a shape with an empty
`PROPERTIES` and a read that reads anything cannot be written outside the
shape module.

How each part is generated:

- `PROPERTIES` and `UNGATED` are constants computed at compile time in two
  passes, as `const` items in a block expression: one `const fn` counts the
  distinct properties of the fields' lists, a second fills a fixed array of
  that length, whose length comes from the inner `const` item, never from
  `Self`. Nothing generic over `S` can have an array sized from `S` on
  stable Rust (it needs `generic_const_exprs`), so every list is a
  `&'static [UIA_PROPERTY_ID]` (established in the scratch crate).
- The request is computed at run time, not subtracted from a constant:
  `push_request` pushes every ungated property, each gated property whose
  bool is on, each ungated group's request for its gates, and each gated
  group's request when its gate is `Some`, and the caller removes repeats.
  So a property that is both gated and ungated, such as the description in
  `Ancestor` (section 4.1), is requested whenever the ungated field asks
  for it.
- `read_from` calls each group's `read_from` with that group's gates and
  each property's kind reader, and for a gated field returns `None`
  without touching the cache when its gate is off. Because a nested group
  is read through `read_from` with a `Reader`, not through a
  `Cached<Group>`, a group can be read inside any shape that holds it.
- `gates: none` makes `Gates` be `()` and implements `Ungated`. That arm of
  the macro accepts no `#[gated]` field and passes each group `&()` as its
  gates, which compiles only when the group's gates are `()`; so a shape
  that claims to be ungated and holds a gated group does not compile.
- `covers: Kept, FocusInput;` implements `Covers` for each and adds a
  compile-time assertion that every property and pattern of the covered
  shape is in this shape's `UNGATED` and `PATTERNS`. A covered shape must
  be `Ungated`. The assertion fails the build with error E0080 when it
  should (established in the scratch crate). Sections 6 and 7 use it.

Tree scope and tree filter are not part of a shape: they belong to the
fill call (section 3.4).

Whether a macro or hand-written impls is decision 1. A procedural macro is
not needed for any of the above (established, in that the scratch
crate's `macro_rules!` prototype of groups, gated groups with nested
gates, gated properties, read kinds and a recording test compiles; the gate-free arm is
estimated, being a second arm of the same form whose check is the
compiler's type check of `&()` against each group's gates).

### 3.3 `Cached`, `Live` and `Agile`

```rust
/// An element whose cache was filled by S's request with `gates`.
pub struct Cached<S: Shape> {
    live: Live,
    gates: S::Gates,
    absent: Absent, // section 7; empty for a local fill
    shape: PhantomData<fn() -> S>,
}

impl<S: Shape> Cached<S> {
    pub fn read(&self) -> S { /* S::read_from with a Reader over the element and `absent` */ }
    pub fn live(&self) -> &Live { &self.live }
    pub fn runtime_id(&self) -> windows::core::Result<Vec<i32>> { /* GetRuntimeId, local */ }
    pub fn agile(&self) -> windows::core::Result<Agile<S>> { /* AgileReference::new */ }
    pub fn narrow<T: Ungated>(&self) -> Cached<T> where S: Covers<T> { /* same element, gates () */ }
}

impl<S: Shape> Clone for Cached<S> { /* by hand: a derive would require S: Clone */ }

/// A `Cached<S>` for another thread of this process.
pub struct Agile<S: Shape> {
    element: AgileReference<IUIAutomationElement>,
    gates: S::Gates,
    absent: Absent,
    shape: PhantomData<fn() -> S>,
}

impl<S: Shape> Agile<S> {
    pub fn resolve(&self) -> windows::core::Result<Cached<S>> { /* AgileReference::resolve */ }
}

/// An element for calls into the application: patterns, text ranges,
/// live property reads, imports into remote programs, and refreshing its
/// cache into a shape.
pub struct Live { element: IUIAutomationElement }

impl Live {
    pub fn new(element: IUIAutomationElement) -> Self;
    pub fn element(&self) -> &IUIAutomationElement;
    pub fn refresh<S: Shape>(&self, request: &Request<S>) -> windows::core::Result<Cached<S>>;
}
```

`Cached<S>` and `Agile<S>` have no public constructor; `Live` has one,
since it has no cached reads to misuse. `live()` returns the stored field,
so no `unsafe` cast is needed. `Absent` is a plain integer (section 7).
The runtime id is on `Cached`, since UI Automation keeps it with every
element and reading it never fails for want of a cache (`calls.rs` module
documentation); `Live` offers it too.

Threads. `IUIAutomationElement` implements neither `Send` nor `Sync` in
`windows` 0.62.2, so `Cached<S>` and `Live` are neither, which is right.
`AgileReference<T>` is `Send` and `Sync` by an `unsafe impl` in
`windows-core` (`agile_reference.rs:32` to 33), the gates are `Send`,
`Sync` and `Copy` by the trait's bound, and `Absent` is an integer, so
`Agile<S>` is `Send` and `Sync` with no `unsafe` of Verbatim's. The marker
is `PhantomData<fn() -> S>`, not `PhantomData<S>`: with the latter, a
shape holding a cached pattern object, such as `SelectionContainer`, would
make its `Agile` neither `Send` nor `Sync` (both established in the
scratch crate). Verbatim never writes `unsafe impl Send` for `Cached`.

Every hand-off between threads carries an `Agile<S>`, so the shape
survives it:

- the listener's callback to its reading threads (`listener.rs:715`,
  resolved at 404): the job carries `Agile<ListenerEvent>`;
- the outpost's callback to its worker (`outpost/mod.rs:994`, the event's
  element at `intake.rs:125`): `Agile<PropertyEvent>` for property events
  and `Agile<TextEvent>` for caret, text and active text position events,
  one per kind of event;
- the node registry (`registry.rs:75`): `Agile<Kept>` (section 6);
- dialog text, which resolves the dialog's kept element on the bounded
  client's thread (`read.rs:696`): `Agile<Kept>`.

A registration's scope (`Scope::Elements`, `subscribe.rs`) keeps untyped
agile references, since nothing reads their caches.

Does the cache survive the hand-off? Within the process, yes, as far as
the repository shows: `docs/design/focus-pipeline.md:444` records that the
element aggregates the free-threaded marshaler, so resolving an agile
reference in the same process returns the same pointer and its cache, and
today's code already relies on that (`read.rs:1000`, `text_reads.rs:172`).
Not verified: a resolve on a single-threaded apartment thread, which
would go through a proxy (every Verbatim thread that uses UI Automation is
in the multithreaded apartment, so it does not arise today); if it ever
did, the reads would fail, and the failed-read count (section 3.6) would
show it.

Deliberate live reads stay deliberate: the legacy checked state of a menu
item (`map.rs:205`, NVDA 2027.1's rule), `HasKeyboardFocus` before a focus
read, and the `ControllerFor` relation (`worker.rs:829`) are methods of
`Live`, called where a rule says to make the call, never as a fallback for
a property a cache lacked.

`Live` must expose the raw element: remote programs import it, and
patterns and text code call it. The `windows` crate's own cached getters
(`GetCachedPropertyValue`, `GetCachedPatternAs`, `GetCachedChildren`,
`CachedName`, `CachedClassName`, `CachedControlType`,
`CachedNativeWindowHandle` and the rest) are then callable on it anywhere.
The type system cannot stop that; the `cargo xtask ci` scan does
(section 3.5).

### 3.4 Requests, fetching and registering

A request is built for a shape and its gates, and keeps both:

```rust
/// S's cache request for the gates it was built with.
pub struct Request<S: Shape> {
    request: IUIAutomationCacheRequest,
    gates: S::Gates,
    shape: PhantomData<fn() -> S>,
}
```

`uia.request::<S>(gates)` builds one; `uia.request_fixed::<S>()`, for
`S: Ungated`, builds one with `()`. Every function that fills a cache
takes `&Request<S>` alone and returns `Cached<S>` with the request's
gates, so a request cached for a query, as the outpost caches its request
per query today (`outpost/mod.rs:332`), cannot be paired with other gates:

```rust
let nodes = uia.request::<Node>(node_gates(context.fetches()))?;
let focus: Cached<Node> = uia.focused(&nodes)?;
let parent: Option<Cached<Ancestor>> = walker.parent(focus.live(), &ancestors)?;
let fresh: Cached<HelpTextRefresh> = event.live().refresh(&help_text)?;
let children: Vec<Cached<DialogText>> = uia.children(dialog.live(), &dialog_text)?;
```

The same holds for `FindFirstBuildCache`, `NormalizeElementBuildCache`,
the search by runtime id and the path search (`focus_path::find`). The
fill call, not the shape, sets the tree scope and filter: `children` sets
the element and its children with the raw view as `client.rs:248` to 267
does today, and returns the children alone. A call that needs no cache
returns `Live`: `checks.rs:66` asks for a window's element only to search
under it.

Event registrations. Today `Part`'s fields are all public
(`subscription`, `properties`, `extended_only`, `text_changes`,
`text_pattern`), so any list can be paired with any callback. Under shapes
the fields are private and a part is made by
`Part::shaped::<S>(subscription)` for `S: Ungated`, whose handlers receive
`Cached<S>`; the shape's request is built inside, and the type is erased
there, so a registration still holds parts of several shapes. Event shapes
take no gates (section 8.2 says why), which `Ungated` enforces.
`.extended()` stays a builder method, `with_text_pattern` goes (the
pattern comes from the shape's `prefetch`), and the callback wrapper in
`subscribe.rs` makes the `Cached<S>` from the element UI Automation
delivers and counts failed reads around the callback (section 3.6).

The working tree also holds, uncommitted, a third handler group,
`Group::ExtendedWithoutTextChanges`, with `Part::text_changes` marking the
parts it leaves out, for the console host's text area, whose output the
outpost learns of from console updates. Shapes leave it as it is:
`Group::holds` decides membership from the part's two marks, and
`.text_changes()` stays a builder method on a shaped part, as
`.extended()` does. The working tree's private `CachedReads` trait stays
too, as the seam between the shape module's reader and its sources
(section 3.6).

A handler that occasionally needs more than its event shape holds
refreshes the element into a second declared shape, one call when it
happens, as the help text and `ControllerFor` handlers already do
(`refreshed_for`, `worker.rs:801`).

Remote operations take a token, `Fill<S>`, in place of a property list,
and their inputs are typed by shape (section 7).

### 3.5 The guarantees, and what enforces each

Enforced by the compiler, inside `verbatim-uia`:

1. Only a shape's generated read reads a cache through Verbatim's
   readers. The kind readers are private to the shape module, the macro
   is private to the crate, `Shape` is sealed, and `read_from` takes a
   crate-private `Reader`, so no other crate can declare a shape or call a
   read except through `Cached<S>::read`.
2. An element read as `S` was filled by `S`'s request. A `Cached<S>` is
   made only by `verbatim-uia`'s fills from a `Request<S>`, by the
   registration's wrapper for a `Part::shaped::<S>`, by `Agile<S>::resolve`
   from an `Agile<S>` that `Cached<S>::agile` made, by `narrow` where
   `S: Covers<T>` and the assertion of section 3.2 holds, and by
   `Fill<S>::claim`, which is the trust boundary of item 8.
3. Request and read agree on the gates. `Request<S>` keeps the gates it
   was built with, every fill copies them into the `Cached<S>`, and a
   gated field reads as `Option`, so every user handles "not fetched".
4. Event registrations and covered shapes have no gates: `Ungated`, whose
   only implementations come from the macro's gate-free arm.
5. A shaped element crosses threads only as `Agile<S>`: `Cached<S>` is
   neither `Send` nor `Sync`, and `Agile<S>` is both without `unsafe`.

Checked by a unit test:

6. A shape's read names exactly the properties and patterns its request
   carries, for every combination of gates. By construction the macro
   generates both from one field list; the recording test of section 3.6
   checks it in both directions, so an over-fetch fails as a missing read
   does.

Enforced by the `cargo xtask ci` scan, outside the shape module:

7. No raw cached read and no raw refill. The scan, written as
   `xtask/src/waits.rs` scans for fixed sleeps, fails on a match of
   `GetCached\w*` or `\.Cached[A-Z]\w*\(` anywhere but the shape module
   and the children read in `client.rs`, on `build_updated_cache` outside
   `verbatim-uia` (a raw refill can no longer become a `Cached<S>`, but
   invites a raw read), and on `Fill<S>`'s `fill` and `claim` outside
   `verbatim-uia-rops`. This is the scan's guarantee, not the compiler's:
   `Live` exposes the raw element (section 3.3).

An audited trust boundary, checked by tests and by the failed-read count:

8. A remote program fills its caches from `Fill<S>`'s list. `Fill<S>`
   hands `verbatim-uia-rops` the list and the classic fallback's request,
   and `claim` wraps the program's returned elements as `Cached<S>`; that
   the program builds its `RemoteCache` from that list and nothing else is
   reviewed code in `verbatim-uia-rops`, tested by `remote_ops.rs`, not a
   property the compiler proves.
9. The absent record a remote program returns (section 7) is taken as
   given. A wrong record makes a property read as unsupported or makes a
   failed read; the second is counted.

Not enforced by any of these: that a theme maps to the right gates. That
is one function per gated shape, from `Fetches`
(`verbatim-model/src/theme.rs:864`) to the shape's gates, with a unit test
per gated field (section 4.3).

What stays outside any guarantee: whether a provider supports a property
(it reads as unsupported, as today), whether the element is still alive,
and whether a shape declares properties no user consumes. The last is a
cost, not a fault, and is kept down by one shape per purpose (section 8).

### 3.6 The checks: a recording test and a failed-read count

The recording test. The shape module's `Reader` reads through the working
tree's private `CachedReads` trait (`map.rs:296`), implemented for the
element and, in `verbatim-uia`'s unit tests, for a recording source that
answers every read and records its property or pattern. For every shape
and every combination of its gates (the macro generates the list of
combinations for tests: 16 for `Node`, whose four gates are its only ones,
and the same for `Ancestor`, which holds it), the test reads the shape
against the recorder and asserts that the set read, with the prefetched
patterns, equals the request for those gates exactly. It covers the
macro, any hand-written group, and both directions, costs nothing at run
time, and meets `docs/testing.md`'s exact-assertion rule. It replaces the
first draft's debug-build check of each read against the carried list,
which was the "is it cached?" check by another name and checked the macro
against itself.

The failed-read count. Every cached read that UI Automation answers with
an error whose code is a failure is counted. `GetCachedPatternAs` answers a
requested pattern the element does not support with a null pointer, which
`windows` 0.62 reports as an error whose code is `S_OK`; that is not a
failed read and is not counted, as `element.rs`'s `found` already tells the
two apart. The count is a thread-local integer in `verbatim-uia`, kept in
every build profile (one increment on an error path), so exact counts do
not depend on the profile. It is not a field of `CallCounts`: that is a
serialized wire type in `verbatim-model` whose `total()` means calls, and a
failed read is not a call. The thread-local is gathered where the reads
happen:

- the callback wrapper in `subscribe.rs` takes the thread-local
  difference around each callback and adds it to an `Arc<AtomicU32>` that
  the `Registration` owns, so UI Automation's callback threads, where
  item 12's cost was, are counted;
- the listener's reader takes its own difference per job, into a counter
  the listener owns;
- the outpost's worker takes its difference per entry, into a counter the
  outpost owns.

Tests read the counter of the registration, listener or outpost they made,
or the thread-local of their own thread where the focus reader runs on it
(`mockapp/tests/common/outpost.rs`). One global counter would not do: the
mockapp tests run the outpost in the test process
(`mockapp/tests/common/outpost.rs:1`) and run in parallel. Nothing in a
release reads the counters beyond keeping them; nothing panics on a UI
Automation callback thread, where a panic would abort the process.

The count catches what no declaration can: a remote program leaving a
property out, a wrong absent record, or a cache that did not survive a
thread hand-off.

## 4. Themes and run-time gating

### 4.1 Gates

Each gate belongs to one field and is named after it:

- a gated property, `#[gated] level: Level => one_based`, adds
  `level: bool` to the shape's gates, and the field reads as an `Option`;
- a gated group, `#[gated] description: Description`, adds
  `description: Option<DescriptionGates>`, where `None` means not fetched
  and `Some` carries the group's own gates; the field reads as an `Option`;
- an ungated group adds its gates as they are, `()` for a gate-free group,
  so a nested shape's gates nest as its groups do:
  `AncestorGates { node: NodeGates, description: () }`.

A gated group switched off has no inner gates to set, so "group off, inner
gate on" cannot be written. The request is the run-time union of section
3.2. A property both gated and ungated is requested whenever the ungated
field asks for it: `Ancestor` holds `Node`, whose description is gated,
and its own ungated `Description` group (decision 4), so its request
always carries `FullDescription` and `HelpText`, and the node's gated
description reads them only when its gate is on.

The gates of `Node` are written out once, in the outpost's mapping from
the theme:

```rust
fn node_gates(fetches: &Fetches) -> NodeGates {
    NodeGates {
        role: (),
        value: (),
        states: (),
        description: fetches.description.then_some(()),
        shortcut: fetches.shortcut.then_some(()),
        position: fetches.position.then_some(()),
        level: fetches.level,
    }
}
```

The review proposed a gates header instead, a list of named gates that
fields refer to, because a `macro_rules!` macro cannot remove repeated
identifiers and two fields gated by one name would declare that gate
twice. With one gate per field, named after the field, the problem does
not arise: field names are unique, and a repeat would be a duplicate
struct field, which rustc rejects (error E0124). Two properties that must
switch together are one group, as `Shortcut` and `Position` are. A gate
shared by separate fields, or `any(spelling_errors, grammar_errors)`, was
needed only for text attributes (section 11), not for any element shape.
If an element shape ever needs one, the header is the way, and it changes
the macro alone.

The gates of the four detail groups are today's `Fetches` fields
`description`, `shortcut`, `position` and `level`, and `Node`'s request is
today's `cached_properties(fetches)` (`cache.rs:226`) without the
properties no reader of that shape reads. The bounding rectangle is not
gated today and is not gated here.

"Not fetched" reads as `None`, and an `Option` can be collapsed by
`flatten` or `unwrap_or_default` into "absent". The review suggested a
dedicated `Gated<T>` type without those methods. It is not adopted: the
two uses where "not fetched" must not mean "absent", the layout judgment
and dialog text, now read ungated fields (decisions 4 and 5), so every
remaining gated field feeds speech, for which not fetched means the
indication is off. A `Gated<T>` would add a `match` at every use for no
case that needs it.

### 4.2 A worked example: a focus read with descriptions off

The theme reports descriptions off, so `Theme::fetches`
(`theme.rs:689`) gives `description: false`, and `node_gates` gives
`description: None` with the other three on.

```rust
let nodes = uia.request::<Node>(node_gates(context.fetches()))?;
let focus: Cached<Node> = uia.focused(&nodes)?;
let node = focus.read();
assert!(node.description.is_none()); // not fetched
```

- The request is `Node`'s union without `FullDescription` and `HelpText`:
  29 properties instead of 31, so the provider is not asked for either.
- `Node`'s read sees the gate is `None` and sets `description` to `None`
  without reading the cache. Today the same read asks UI Automation for
  `FullDescription`, gets an error, then asks for `HelpText` and gets
  another (`map.rs:460`).
- The conversion to `NodeDetails` gives `description: None`, which is what
  Core receives today; nothing heard changes.
- The focus's ancestors are read as `Ancestor`, whose ungated description
  the layout judgment reads (`client.rs:1110`), so an unnamed grouping with
  a description is focus context whatever the theme (decision 4), while
  the ancestor's spoken description still follows the gate.

### 4.3 What the compiler cannot check

That `description: false` comes from the description indication and not
from another is the one link the types cannot make, since the theme is in
`verbatim-model`, a platform-neutral crate that knows nothing of UI
Automation. The mapping is one function per gated shape in
`verbatim-outpost` (from `Fetches`), with one unit test per gated field:
turning that one setting off in a theme turns off that one gate and no
other.

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
fallback never runs; under shapes it goes, with the query's input typed
so that it always carries the control type (section 7, decision 8).

### 5.3 Over-fetches (correct, but wasteful)

- The shell's notification-area list (`verbatim-gui/src/shell_items.rs:145`
  and 269 to 281) fills all 34 to read five: `NativeWindowHandle`,
  `ControlType`, `Name`, `IsOffscreen`, `BoundingRectangle`.
- `console_reports_formatting` (`checks.rs:66`) fills all 34 for the
  console window and reads none of them: it only searches under it.

### 5.4 Behaviour coupled to a gate (decided)

- `is_layout` (`client.rs:1110`, the description at 1119) judges an
  unnamed window, property page or grouping as layout unless it has a
  description. With descriptions off, an ancestor read by the outpost has
  none, so an unnamed grouping with a description is not reported as focus
  context. NVDA's `presentationType` uses the description whatever the
  setting (`nvda/source/NVDAObjects/__init__.py:947` to 955). The listener
  reads ancestors with every detail (`Fetches::default()`,
  `focus_read.rs:374`), so the same grouping is reported when the listener
  read the chain and not when the outpost did.
- Dialog text includes each child's description (`text_of`,
  `dialog_text.rs:186`), read from the gated request, so with descriptions
  off a dialog's text loses them. NVDA's dialog text uses `basicText`,
  which includes the description whatever the setting
  (`nvda/source/NVDAObjects/__init__.py:1450` to 1458).

Decided by Dickson on 2026-10-13, with bug 4: the layout judgment always
has ancestors' descriptions, through an ungated `Description` group in
`Ancestor`; dialog text always includes each control's description,
through an ungated `Description` group in `DialogText`; and the dialog's
own element is no longer read. All three land in migration step 3
(section 9).

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
  text. Bug 4. Becomes an `Agile<Kept>` resolved on the bounded client's
  thread, of which nothing is read (decided, section 5.4).
- 726, `focused_control`: the focused element. Bug 2. Shape: `Node`
  (named `FocusRead` in section 4.2's sense, the same shape).
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
  children. The root is not read (decided, section 5.4): `dialog_text`
  asks the root only for its children (`dialog_text.rs:120` to 140), so
  it becomes a type of its own holding a `Live` and offering `children`
  alone, while `UiaObject` keeps `role` and `states`, which it serves for
  the root as well today (`dialog_text.rs:210` to 240), for the children.

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
- `nearest.rs:107`, `NativeWindowHandle` after a normalize with that
  property alone. Covered. Shape: `WindowLookup`.

`verbatim-uia-rops`.

- `focus.rs:228`, the control type of the query's element, with a live
  fallback. Covered (decision 8). Becomes a read of the query's
  `Cached<FocusInput>` (section 7).
- `focus.rs:238`, `own_window`, `NativeWindowHandle` of the query's
  element. Covered. Read from `FocusInput`.
- `focus.rs:459` to 518, `RemoteCache`. Section 7.
- `navigate.rs:96`, `NativeWindowHandle` of the step's element, the kept
  element (`read.rs:991` to 997). Covered. Read from the query's
  `Cached<Kept>` (section 7).
- `controlled.rs:188`, `path.rs:151`, `focus.rs:553`, `navigate.rs:144`:
  classic fallbacks building the query's list. Covered; they take the
  shape's request.

`verbatim-gui`.

- `shell_items.rs:269` to 281. Over-fetch. Shape: `ShellButton`, declared
  in `verbatim-uia`'s shape module.

### 5.6 The shapes

All are declared in `verbatim-uia`'s shape module (section 3.1).

Event shapes, declared `gates: none`:

- `ListenerEvent`: NVDA's 13 properties and, by decision 6,
  `IsDialog` and `IsTogglePatternAvailable`, each a field so that the read
  covers the request exactly (section 3.6), and the Text pattern as a
  prefetch, as now. Its role is read through `RoleCore`, refined for
  dialogs and toggle buttons as every other read is. It covers `Kept` and
  `FocusInput`.
- `PropertyEvent`: today's 24 (`RoleCore`, `Name`, `ValueSet`, `States`,
  `ProcessId`). It covers `Kept`.
- `TextEvent`: today's 3. It covers `Kept`.

Fetch shapes:

- `Kept`: `NativeWindowHandle` and `ClassName`, which is all any reader of
  a kept element reads. It is never requested; every shape whose elements
  are kept declares `covers: Kept`. `ProcessId` is in every kept cache
  too, but no reader of a kept element reads it.
- `FocusInput`: `ControlType` and `NativeWindowHandle`, what the remote
  focus query reads of its input element (section 7). Never requested;
  covered by `ListenerEvent` and `Node`.
- `Node`: `RoleCore`, `Name`, `ValueSet`, `States`, the four gated detail
  groups, `BoundingRectangle`: 24 to 31 properties. It covers `Kept` and
  `FocusInput`.
- `Ancestor`: `Node`, an ungated `Description` group, and
  `IsContentElement`, `IsControlElement`: 28 to 33 (decision 4). It covers
  `Kept`.
- `TreeDump`: `Node`.
- `DialogText`: `RoleCore`, `Name`, `ValueSet`, `States`, an ungated
  `Description` group: 25 (decision 5). The dialog's own element is not a
  shape: it is a `Live`, in a root type of its own (section 5.5).
- `WindowLookup`: `NativeWindowHandle`.
- The empty shape: no request; the fetch returns `Live`.
- `SelectionContainer`: `Selection2FirstSelectedItem` and the Selection
  pattern.
- `ControlledSelection`: nothing for the controller (its runtime id), and
  `Node` for the item.
- `HelpTextRefresh` (`Description`) and `SuggestionsRefresh`
  (`IsTextPatternAvailable`, `AutomationId`, `ControllerFor`), chosen per
  property id as `refreshed_for` chooses today.
- `ShellButton`: `NativeWindowHandle`, `ControlType`, `Name`,
  `IsOffscreen`, `BoundingRectangle`.
- `TextAreaPattern`: the Text pattern, which `ListenerEvent` prefetches
  and covers, for the focus listener's console check of a text area.

Every shape but `Node`, `Ancestor` and `TreeDump` is declared
`gates: none`.

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

Under shapes the registry must record the shape with the element, and it
stores an `Agile`, since the elements it keeps are resolved on other
threads (section 3.3). Two ways:

- An enum of the kinds kept: `Kept::Node(Agile<Node>)`,
  `Kept::PropertyEvent(..)`, `Kept::TextEvent(..)`. A reader matches and
  takes what each kind offers. It keeps everything known, but every reader
  then handles every kind, and a new kept kind changes every reader.
- One shape, `Kept`, that every kept kind covers. `id_for_element` takes
  `Cached<Kept>`, which a caller makes with `narrow`, checked at compile
  time by `covers: Kept`, and the registry stores `Agile<Kept>`. A reader
  resolves it to a `Cached<Kept>`, with the window handle and class name;
  one that needs more refreshes the element into a declared shape, which
  is a call it makes knowingly.

With the second, every reader in the list is served without a new call:
`worker.rs:2293` reads only the runtime id (bug 3's fix); `read.rs:1000`,
1061 and `text_reads.rs:172` read what `Kept` holds; the remote step query
takes its input as `Cached<Kept>` and reads the window handle from it
(`navigate.rs:96`, section 7); the dialog's root needs nothing (bug 4);
and the log line reads the runtime id and class name only (bug 5,
decision 7). `resolve_uia_element` (`read.rs:836`) already refreshes a kept
element with the query's request before reading it, which under shapes is
`Live::refresh`. The remote focus query, whose input is not a kept element
but needs the control type, takes `Cached<FocusInput>` (section 7), so
`Kept` need not grow for it. Decision 2 recommends the second.

## 7. Remote operations

The remote programs take a property list today: `FocusQuery::properties`,
`StepQuery::properties`, `ControlledQuery::properties` and the path
search's (`verbatim-uia-rops`, `focus.rs`, `navigate.rs`, `controlled.rs`,
`path.rs`). `verbatim-uia-rops` is another crate and cannot use
`verbatim-uia`'s private constructors, so it is handed a token:

```rust
/// What a remote program needs to fill caches as S's request does, and
/// the only way to wrap the elements it returns as `Cached<S>`.
pub struct Fill<S: Shape> { /* the gates, the property and pattern lists; not Clone or Copy */ }

impl<S: Shape> Fill<S> {
    #[doc(hidden)]
    pub fn new(request: &Request<S>) -> Self;
    /// The properties the program's cache requests hold.
    pub fn properties(&self) -> &[UIA_PROPERTY_ID];
    /// The classic fallback's request, built from the same list.
    pub fn request(&self, uia: &Uia) -> windows::core::Result<IUIAutomationCacheRequest>;
    #[doc(hidden)]
    pub fn claim(&self, element: IUIAutomationElement, absent: Absent) -> Cached<S>;
}
```

A query holds a `Fill<S>` in place of its property slice. The program
builds its cache requests (`RemoteCache::new`, `focus.rs:474`) from
`properties()`, and the classic fallback (`controlled.rs:188`,
`path.rs:151`, `focus.rs:553`, `navigate.rs:144`) uses `request()`, so
both lists come from the token. The elements the program returns are
wrapped by `claim`, giving `Cached<S>` with the token's gates and the
element's absent record. `Fill<S>` is neither `Clone` nor `Copy`; `new`
and `claim` are hidden from the documentation and allowed by the
`cargo xtask ci` scan only inside `verbatim-uia-rops` (section 3.5,
item 7). Nothing in the type ties a token to one program run or proves
the program used its list: that is the audited trust boundary of section
3.5, item 8.

The programs' inputs are typed too. Today they read their input elements'
caches: the focus query reads the control type (`focus.rs:228`,
`wants_selected_child`) and the window handle (`focus.rs:238`,
`own_window`), and the step query the window handle (`navigate.rs:96`).
Under shapes `verbatim-uia-rops` cannot read a cache, so the focus query's
element is a `Cached<FocusInput>`, which the caller makes with `narrow`
from a `ListenerEvent` or a `Node`, and the step query's is a
`Cached<Kept>` from the registry; each reads its input with `read()`. A
caller holding only a `Cached<Kept>` cannot hand it to the focus query,
which is what makes removing the control type's live fallback safe
(decision 8). Whether a program's `PopulateCache` on an imported input
element leaves the client-side input's cache alone has not been verified;
the inputs are read before the program runs, so the reads above do not
depend on it.

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
  `Cached<S>` it makes (the `absent` field of section 3.3). The reader
  answers a read of `IsDataValidForForm` as "not supported" from the record
  without asking UI Automation. A local fill's record is empty. This is
  not an "is it cached?" check: it is Verbatim's own record of what its
  program did, like the gates. It is trusted as the program returns it
  (section 3.5, item 9).
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

- The listener's events: 15 and the Text pattern, two more than today by
  decision 6; failed reads per event from 25 to none.
- The outpost's property events: 24, unchanged. Text events: 3, unchanged.
  Failed reads already none with the working tree's fix.
- Node fetches (focus, neighbours, selections, windows, tree dumps): 27 to
  34 today, 24 to 31 as `Node`, since `ProcessId`, `IsContentElement` and
  `IsControlElement` leave the paths that never read them; failed reads
  from up to 7 to none.
- Ancestors: 27 to 34 today, 28 to 33 as `Ancestor`, whose description is
  always fetched (decision 4, decided); failed reads from up to 7 to none.
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

Each step passes `cargo xtask ci`, runs the mockapp tests it touches and
the end-to-end scenarios related to the paths it changes, and updates the
`docs/crates/` file of any crate whose public API it changes
(`verbatim-uia.md`, `verbatim-uia-rops.md`, `verbatim-outpost.md`). From
step 2 to step 5 the new entry points sit beside the old ones (`Part::new`
beside `Part::shaped`, the list-taking fetches beside the request-taking
ones, `id_for_element` on a raw element), so every step compiles on its
own and each call site moves when its step comes; step 5 removes the old
ones. Until then a site that has a `Cached<S>` but must call an old entry
point passes `live().element()`, which is the untyped use step 5 ends.
Tests follow `docs/testing.md`: one focused test per stated behaviour,
exact counts, no new test files beyond the neighbouring ones. The working
tree's event-read fix is not committed on its own (Dickson, 2026-10-13:
fix this systematically); step 2 replaces it. The working tree's third
subscription group (section 3.4) is separate work and lands on its own.

The failed-read expectations. Step 1 adds a failed-read count to every
operation `call_counts.rs` measures, with each number recorded from the
code at that step, not predicted: nonzero exactly where section 5.2 says a
path reads outside its cache. Each later step names the expectations it
takes to zero, and after step 4 every one is zero. About 70 call sites in
15 mockapp test files use the lists and readers that go (most in
`remote_ops.rs`, `call_counts.rs`, `events.rs`, `instruction_limit.rs` and
`controller_for.rs`); each step lists the files it moves, and the public
shapes are enough for an integration test in another crate, which cannot
declare a shape of its own.

### Step 1. Count failed cached reads; the controller's runtime id

- The thread-local failed-read count of section 3.6, kept in every
  profile, counted in `ElementExt`'s cached reads when the error's code is
  a failure, and gathered by the registration's callback wrapper, the
  listener's reader and the outpost's worker into counters they own.
- `controlled_selection` takes the controller's id from
  `verbatim_uia::runtime_id(&focused)` (`worker.rs:2293`); the wrong
  comment goes.
- Tests: `call_counts.rs`'s expectations gain the failed-read count,
  recorded against main without the working tree's event-read fix, so the
  outpost's property and text event paths show their failed reads here;
  one test in `controller_for.rs` that a controlled selection whose focus
  element was kept from a text event makes no failed cached read.
- Mockapp files: `call_counts.rs`, `controller_for.rs`.

### Step 2. The shape module, typed handles, and the event shapes

- In `verbatim-uia`: the shape module (`Shape`, `Ungated`, `Covers`, all
  sealed; `uia_shape!`, private; `Reader` over the private `CachedReads`;
  the kind readers moved out of `ElementExt`'s public use), `Cached`,
  `Live`, `Agile`, `Request`, and `Part::shaped`.
- Shapes: `ListenerEvent`, `PropertyEvent`, `TextEvent`, `Kept` and their
  groups.
- The listener: `ListenerEvent` for its registration (`listener.rs:817`),
  its job carrying `Agile<ListenerEvent>`, and every reader of its
  elements (`capture`, `queue_for_reading`, the notification check,
  `focus_read.rs` `read` and `log_sender`). `event_parts` goes.
- The outpost: `PropertyEvent` and `TextEvent` for its registrations, the
  worker's event carrying `Agile<PropertyEvent>` or `Agile<TextEvent>`;
  `snapshot_parts_from_event_cache`, `native_window_handle_from_event_cache`
  and `Restricted` go. The two sites that keep an event's element
  (`worker.rs:2365`, `text_reads.rs:244`) still call the old
  `id_for_element` until step 4.
- Tests: in `verbatim-uia`, the recording test of section 3.6 over every
  shape so far, and one test that a shape's `PROPERTIES` is its fields'
  union without repeats. The compile-time assertions (`covers`, the
  gate-free arm) are checked by compiling; a test that a wrong declaration
  fails to compile would need a new dependency such as `trybuild`, and is
  not proposed. In `events.rs`, one test that the listener's selection and
  notification facts are read with no failed cached read. The uncommitted
  comparison test in `uia_tree.rs` is replaced by one that each event
  shape's read of every scripted element gives the fields the full read
  gives. In `call_counts.rs`, the event paths' failed reads become zero;
  provider-side counts are unchanged, since the event requests are today's
  lists.
- Measurement, recorded rather than asserted: a `cargo xtask measure`
  trace of `conhost_flood`, with the callback threads' processor time in
  the same window as item 12, written into `wake-ups.md` beside item 12's
  0.83 seconds. It is not a pass condition: `docs/testing.md` forbids loose
  tolerances, and the number varies by machine.
- Mockapp files: `events.rs`, `uia_tree.rs`, and `call_counts.rs`'s uses
  of `FOCUS_EVENT_PROPERTIES` and `TEXT_EVENT_PROPERTIES`.

### Step 3. Fetch shapes, remote operations, and the decided fixes

- `verbatim-uia`'s fetches take `&Request<S>`, beside the old ones;
  `Node`, `Ancestor`, `TreeDump`, `DialogText`, `SelectionContainer`, the
  refresh shapes, `WindowLookup`,
  `FocusInput`, `ShellButton` and the empty shape replace the lists at
  every site in section 5.5 but the registry's readers.
- `verbatim-uia-rops` takes `Fill<S>`, returns the absent record (decision
  3), takes the focus query's input as `Cached<FocusInput>`, and loses
  `wants_selected_child`'s fallback (decision 8). The step query's input
  stays a raw kept element until step 4. Since the queries' signatures
  change, `verbatim-uia-rops` and its callers move in one commit; the other
  sites may follow in further commits, each passing CI.
- Decided by Dickson (section 5.4): `Ancestor`'s ungated description, read
  by `is_layout`; `DialogText`'s ungated description; and the dialog's
  root as its own type holding a `Live` and offering only `children`,
  since `dialog_text` asks the root for nothing else, while `UiaObject`
  keeps `role` and `states` for the children (`dialog_text.rs:120` to 140
  and 210 to 240).
- The gate functions from `Fetches`, with a unit test per gated field.
- `verbatim-gui`'s shell list takes `ShellButton`.
- The sites that keep a fetched element (`map.rs:728`, `read.rs:868`,
  `worker.rs:2295` and 2949) still call the old `id_for_element` until
  step 4.
- Tests: in `call_counts.rs`, a focus read with descriptions off makes no
  failed cached read, the gated fetch paths' failed reads become zero, and
  the provider-side counts drop by exactly the properties removed (the new
  numbers recorded in this step); `uia_dialog_text_costs_exactly` with
  descriptions off includes a child's description. In `remote_ops.rs`, a
  remotely read element without `IsDataValidForForm` reads as a valid
  entry with no failed read. In `focus_reports.rs`, with descriptions off,
  an unnamed grouping with a description is reported as focus context.
  The gate unit tests. End-to-end: the focus, selection, dialog,
  navigation and terminal scenarios.
- Mockapp files: `remote_ops.rs`, `instruction_limit.rs` (the queries'
  lists become tokens), `call_counts.rs` (the focus query, its uses of
  `CACHED_PROPERTIES` and `snapshot_from_cached_element`),
  `controller_for.rs`, `arbitration.rs`, `focus_reports.rs`.

### Step 4. The kept-element registry

- `id_for_element` takes `Cached<Kept>` and the registry stores
  `Agile<Kept>` (decision 2); every keeping site narrows its `Cached<S>`;
  readers resolve a `Cached<Kept>`; the step query's input becomes
  `Cached<Kept>`; dialog text resolves the dialog's `Agile<Kept>` on the
  bounded client's thread and reads nothing of it; the log line reads the
  runtime id and class name (decision 7).
- Tests: in `call_counts.rs`, `uia_dialog_text_costs_exactly` with the
  dialog's element kept from a property event, no failed cached read; in
  `uia_tree.rs`, a node kept from a text event navigated remotely with no
  failed read. The kept paths' failed reads in `call_counts.rs` become
  zero, so every failed-read expectation there is now zero.
- Mockapp files: `call_counts.rs`, `uia_tree.rs`.

### Step 5. Close the API, and the CI check

- The old entry points go: `ElementExt`'s `cached_*` methods become
  private to the shape module; `cached_properties`, `cache_request_for`,
  `base_cache_request`, the property lists and the
  `snapshot_*_from_cached_element` functions leave the public API;
  `Part::new`, `Part::with_text_pattern` and `Part`'s public fields go;
  `id_for_element` on a raw element goes.
- The `cargo xtask ci` scan of section 3.5, item 7, with a unit test in
  `xtask` that a cached read outside the shape module fails it, as
  `waits.rs` is tested.
- Mockapp files: every remaining use. `base_cache_request` becomes
  `uia.request::<Node>(Node::all_on())` in `caret_watch.rs`,
  `focus_reports.rs`, `native_controls.rs`, `shutdown.rs`, `terminal.rs`,
  `terminal_reading.rs`, `text.rs`, `time_limits.rs`, and the rest of
  `events.rs`, `remote_ops.rs` and `instruction_limit.rs`; their
  `cached_string` and `cached_value` reads become reads of the shape's
  fields (`remote_ops.rs:164`, which reads a property by id, reads the
  field that holds it).
- No expected count changes.

## 10. Decisions

Every decision was settled by Dickson on 2026-10-13.

- Decision 1. Declarative macro or hand-written shapes. Decided: the
  macro, with the recording test as its check. Both forms carry the same
  typed handles (`Cached`, `Agile`, `Request`, `Fill`, sealed `Shape`,
  `Ungated`, `Covers`), the same scan and the same recording test; with
  the macro a shape's request and read come from one field list, so a
  drift fails the build, not only `cargo xtask ci`.
- Decision 2. The kept-element registry (section 6). Decided: one `Kept`
  shape that every kept kind covers, stored as `Agile<Kept>`, with readers
  that need more refreshing into a declared shape explicitly. Every current
  reader is served by it without a new call, given that the focus query's
  input is typed as `FocusInput` (section 7).
- Decision 3. `IsDataValidForForm` in remotely filled caches (section 7).
  Decided: the absent record, from the program's existing support test,
  the program changed by one result per element; it is part of the audited
  trust boundary (section 3.5, item 9).
- Decision 6. The listener's event role. Decided: widened. The listener's
  request gains `IsDialog` and `IsTogglePatternAvailable`, so
  `ListenerEvent` reads the role through `RoleCore`, refined for dialogs
  and toggle buttons as every other read is. This departs from NVDA's base
  request, which the listener matched until now (a decision of record it
  replaces), at the cost of two properties per desktop-wide event; the
  outpost's own reading of the role is unchanged. Lands in step 2 with
  `ListenerEvent`.
- Decision 7. The log line in `text_event_from` (bug 5). Decided: it logs
  the runtime id and class name only.
- Decision 8. `wants_selected_child`'s live fallback in
  `verbatim-uia-rops` (section 5.2). Decided: removed, together with the
  typed `Cached<FocusInput>` input that makes it safe, in step 3.


- Decision 4. The description of ancestors, and `is_layout` (section 5.4).
  The layout judgment always has ancestors' descriptions, whatever the
  theme, as NVDA's `presentationType` does: `Ancestor` holds an ungated
  `Description` group beside `Node`'s gated one, so the outpost judges
  ancestors as the listener does. It costs two properties per ancestor
  read by the outpost with descriptions off. Lands in step 3.
- Decision 5. Dialog text (section 5.4 and bug 4). Dialog text always
  includes each control's description, whatever the theme, as NVDA's
  `basicText` does, through an ungated `Description` group in
  `DialogText`; and nothing of the dialog's own element is read. Lands in
  step 3, with the kept dialog's typed hand-off in step 4.

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

Not part of this migration. Formatting is not element properties but text
range attributes: font name, size, weight, italic, underline and its
style, strikethrough, foreground and background colours, bullet style,
link, and the annotation types that carry spelling and grammar errors.
They are read by the caret's remote operation
(`verbatim-uia-rops/src/caret.rs`, `TextAttribute` at 74 and the attribute
set at 142) or classically with `GetAttributeValues`, one attribute at a
time where a provider lacks it (`verbatim-uia/src/text.rs:385` to 410).
Which to read is decided by `attributes_for(fetches)`
(`verbatim-outpost/src/text/uia.rs:807` to 836), and what they mean by
`attributes_of(run, fetches)` (845), the same gates applied in two places.

A read of an attribute a provider does not support returns UI Automation's
"not supported" object rather than an error (established, `text.rs:262` to
265 documentation), so the cost of section 2 does not arise here, which is
why it is outside this design's goal. What the same pattern would give is
agreement: one declaration, `uia_attributes!`, generating the attribute
list for gates (which feeds both the remote program and
`GetAttributeValues`) and the read of the results into a struct with
`Option` fields for gated attributes. Its gates would need the gates
header of section 4.1, since the annotation types are read for either
spelling or grammar errors (`verbatim-outpost/src/text/uia.rs:811`).
Three attribute shapes would cover today's reads:

- `AttributeSet`, gated by the document-formatting settings, for the
  caret's formatting.
- `Culture`, never gated, for the language of a unit (`text.rs:447`).
- `ConsoleProbe`, `FontName` alone, for the console's provider check
  (`verbatim-uia/src/checks.rs:81`).

The set of attributes a provider was found not to support
(`CaretAnswer::unsupported`, `caret.rs:335`, kept per node by
`text_reads.rs`) would stay a run-time subtraction from the request: it is
learned from answers, so no declaration can hold it. The work would be:
`uia_attributes!` and the three attribute shapes; `attributes_for` and the
matching half of `attributes_of` go; a unit test per gate (its attributes
in the list exactly when on); the caret formatting tests in mockapp
unchanged; the Notepad formatting scenarios. It deserves a design of its
own when taken up.

## 12. Review of 2026-10-13

An adversarial review checked about 30 of this document's citations
against the working tree (all accurate, as were the counts) and its
const-evaluation claims in a scratch crate. Each finding, and what the
document now does about it:

Blocking:

- B1, elements cross threads with no typed handle. Adopted: `Agile<S>`,
  `Send` and `Sync` with no `unsafe` through `PhantomData<fn() -> S>`, and
  each hand-off listed with the shape it carries; the registry stores
  `Agile<Kept>`. The single-threaded apartment case is stated as not
  verified. Sections 3.3 and 6.
- B2, `Fill<S>` lets any crate forge a `Cached<S>`. Adopted: `Fill<S>` is
  neither `Clone` nor `Copy`, `new` and `claim` are hidden and scanned to
  `verbatim-uia-rops`, and the program's two lists come from the token's
  `properties()` and `request()`. What remains is stated as an audited
  trust boundary. Sections 3.5 (item 8) and 7.
- B3, the guarantees depend on sealing and on where shapes live. Adopted:
  `Shape`, `Ungated` and `Covers` sealed, `uia_shape!` private, every shape
  in the shape module, `read_from` over a crate-private `Reader` (which
  also lets a group be read inside any shape), and other crates' mapping
  as free functions. Sections 3.1 and 3.2.
- B4, `Live` exposes the raw element. Adopted: the guarantee is stated as
  the scan's, with its patterns, the children read's exception, and
  `build_updated_cache` rejected outside `verbatim-uia`. Sections 3.3 and
  3.5 (item 7).
- B5, the registry step could not compile before the fetch step. Adopted:
  fetch shapes are step 3 and the registry step 4, old and new entry
  points coexist until step 5, and `verbatim-uia-rops` moves in one commit
  with its callers. Section 9.

Should fix:

- S1, gate subtraction wrong for a property both gated and ungated, nested
  gates unspecified. Adopted: the request is a run-time union, a group's
  gates nest in its parent's, and decision 4 uses an ungated description
  field. Sections 3.2 and 4.1.
- S2, macro claims not achievable as written. Adopted: two-pass constants,
  `&'static` lists, explicit read kinds with the Rust type following from
  the kind, a `PATTERNS` constant with read and prefetched patterns, and
  tree scope and filter on the fill call. The gates header is argued
  against for element shapes: with one gate per field, named after it, no
  identifier repeats and none needs removing; it is kept for attributes,
  should they be taken up. Sections 3.1, 3.2, 3.4, 4.1 and 11.
- S3, request and gates could disagree. Adopted: `Request<S>` keeps its
  gates and fills take it alone. Section 3.4.
- S4, `Part`'s public fields bypass shapes. Adopted: private fields,
  `Part::shaped::<S>` for `S: Ungated`, `.extended()` and the working
  tree's `.text_changes()` as builders, `Group::holds` and the new group
  untouched, and `CachedReads` kept as the reader's seam. Section 3.4.
- S5, `verbatim-uia-rops` reads its inputs' caches. Adopted: the focus
  query takes `Cached<FocusInput>` and the step query `Cached<Kept>`;
  decision 8 is restated as depending on it. Sections 7 and 10.
- S6, the failed-read count missed the callback and reading threads and
  could over-count. Adopted: per-thread counting gathered into counters
  owned by the registration, the listener and the outpost; only errors
  whose code is a failure count; a counter of its own, not a `CallCounts`
  field; every profile. Section 3.6.
- S7, the debug check was the "is it cached?" check by another name.
  Adopted: replaced by the recording test over every combination of gates.
  Sections 3.5 (item 6) and 3.6.
- S8, the migration did not mention the mockapp tests. Adopted: each step
  lists the test files it moves, and the public shapes serve tests in
  another crate. Section 9.
- S9, the summary overstated the compiler's guarantees. Adopted: the
  summary and section 3.5 attribute each guarantee to the compiler, a
  unit test, the scan or the audited boundary. Sections 1 and 3.5.

Minor:

- "Not fetched" collapses into "absent" through `Option`'s methods. Argued
  against: the two uses where that would matter now read ungated fields,
  so a `Gated<T>` type would add ceremony for no remaining case. Section
  4.1.
- `live()` without `unsafe`, and `Clone` by hand. Adopted. Section 3.3.
- Pattern conditions on `RangeValueValue` and `ValueIsReadOnly` must stay
  in the group's mapping. Adopted, stated. Section 3.1.
- The flood trace as a pass condition. Adopted: a recorded measurement.
  Section 9, step 2.
- Step 1's counts hard-coding today's failed reads. Adopted: each step
  says which expectations become zero, and all are zero after step 4.
  Section 9.
- Attribute reads have no failure cost, so they are outside the goal.
  Adopted: moved to a later note. Section 11.
- The dialog's root needs its own type. Adopted: a root type holding a
  `Live` and offering only `children`. Sections 5.6 and 9, step 3.
- Decision 2's claim holds given S5. Adopted, stated. Section 10.

The review's simpler alternative, hand-written shapes with the same typed
handles, recording test and scan, is weighed in decision 1.
