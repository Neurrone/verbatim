# Extension API additions: a draft for Dickson

Status: design draft, Claude, 2026-11-28. Nothing in the repository changes because of it. It proposes the interfaces the rewrite's built-in extensions need (`extensions-first.md` and `design.md` in this folder), in WIT with the Rust guest SDK as an author would write it. Its inputs are the Extensions guide's reference (`books/extensions/src/reference/`, settled 2026-11-09), `docs/design/extensions.md`, `docs/design/m5-extensions-work.md`, ADR 0003's extensions rulings, the read-interface spike and its review (not kept; their findings are in design.md), the request list (`crates/verbatim-model/src/batch.rs`) and NVDA at the pinned submodule.

Frame (Dickson, 2026-11-28): the extensions design will be rewritten whole, so the 2026-11-09 reference is input material, not a contract this adds to. Each section says where it departs from the reference, and why. No time bound on a handler is fixed: a handler may take seconds, as a translation add-on calling an online service does, so each interface gives the options for what Core speaks, what waits and what is cancelled while a long handler runs. Who calls hooks is open, and is the first question below.

Every type and function name here is a draft. Words used throughout:

- A crossing is one message from one process to another and its answer: two wakes.
- The context is the record Core sends with every hook call, holding the facts Core already has (below). A hook that needs only the context makes no read.
- A holding hook is one whose answer can change the speech of the event it is called for (`drop`, `rewrite`, `replace`), so the event's default handling waits for it. A following hook is one whose answer cannot, so nothing waits for it.

## Assessment first

- Every addition maps onto the request list as it stands: typed reads, lists, conditions and the text interface are built from `Read`, `Related`, `Ancestors`, `Descendants`, `Test`, `Caret`, `ReadText`, `ReadRange`, `Move`, `FindText`, `Select`, `Locate` and `Visible`, with `Condition` for "only if". No new kind of request is needed for items 1 and 2. Three small gaps in existing operations are listed (a range read with formatting, a case-insensitive find, a comparison of two points that answers rather than skips); each is a field or a kind, so each is Dickson's under CLAUDE.md item 4.
- The context removes most reads. Report title makes one read (a fresh name of the foreground) or none; Calculator's result handler makes one; Explorer's tab switch makes none. That matters most under the option where Core calls every hook, since there each read costs two crossings.
- The largest open choice is who calls hooks, which decides each read's cost, who sees the queue's facts, and how a long handler is contained. The second is what waits while a holding hook runs long. Both are set out as options; Claude recommends for each, and the interfaces below work under every option.
- Speech after Core's default announcement is best a separate, following registration rather than a verdict: then Core never holds the default for it, and its speech shares the fate of the announcement it follows.
- Typing echo and caret movement stay on the hot path with no hook: Calculator's typing echo is a declaration Core applies, bound to the module's own setting.

## The open question: who calls hooks

| Option | A hook call | Each read | Who sees the queue's facts | A long handler |
|---|---|---|---|---|
| Core calls every hook (ADR 0003, Dickson, 2026-11-12) | one crossing, Core and host | two: host to Core, Core to the outpost | Core, which sends them in the context as it sends the call | Core decides what waits, since its queue holds the entry |
| The reading process, the outpost, calls hooks (the 2026-11-09 reference and `extensions.md` section 4) | one crossing, outpost and host | one: host to outpost | the outpost has none of Core's facts (the pending focus, the keys, the lock state, the settings); Core would have to send copies, which the design principles forbid as a reader deciding from a copy | the outpost's reducer holds that application's events; Core cannot see why |
| Core calls, and the host reads from the outpost directly under a scope Core grants with the call | one crossing, Core and host | one: host to outpost | Core, as in the first row | as in the first row |

Claude's assessment:

- The second row puts Core's facts in a reader, which is the copied-state class the ownership refactor removed, and the outpost waiting on a host is what ADR 0003 ruled out. Claude does not recommend it.
- The third row halves each read's cost, but the outpost then serves two askers with no order between them, so ADR 0003's "Core's own requests to an outpost are served first" becomes the outpost's judgement, and the ids a host gets back are in no Core record, so the held view cannot name them; each needs a new rule.
- So Claude recommends the first row, with the context and typed reads keeping reads few, and the measurement M5's step 4 planned (a host crossing and guest time on both architectures) made first. If a built-in's measured read cost misses D15 on a hot-path event, the third row is the fallback, designed then.

Under every row a read is answered by the outpost's own readers from the shared request list, and each interface below states its cost as a count of reads, which each row prices differently.

## What waits while a handler runs long: the options

The same three choices recur for each kind of hook, so they are named once here, and each interface says which apply.

- Hold: the queue's front entry waits for the answer, and every later key and event waits behind it. This is NVDA's behaviour, since its handlers run on its one main thread: a handler that takes two seconds silences NVDA for two seconds, keys included.
- Hold, ended by causes: the entry waits as in Hold, but declares the end causes design-v2 already has (`LaterKey`, `PendingFocus`, `Committed`). When one holds, Core cancels the call (the lane ends the suspended guest), goes on with the default as if the hook had passed, and drops any late answer. A slow handler then delays only the event it was called for, and only until the user does something else.
- Do not hold: the entry goes on with the default at once, the call runs beside the queue, and its answer can only add speech, judged when it arrives (as a following hook's is). This is available only to a hook whose answer cannot change the event's speech.

A bound counted by a clock (the 2026-11-09 reference's 11-second handling deadline, and ADR 0003's "short bound counted from when Core sends the call") is a fourth option, deliberately left without a figure here, as the brief asks; it could be added to Hold or to Hold ended by causes as a user setting or a fixed safety net, and it is listed as a question.

Claude's recommendation: holding hooks take Hold ended by causes; commands and following hooks take Do not hold. A host that does not answer at all (stuck in native code) is a separate matter, the liveness rule of `extensions.md` section 4, which ends the host; it is not a bound on the handler's own work.

## The context: what every call carries

Sent by Core with each call, read from Core's records when the call is sent. It adds no crossing under the first and third rows. Each value is what Core held at the send, as NVDA's handler reads its facts when it runs, and Core still judges every effect when it arrives (section 5), so a stale value can cost a wasted read or a dropped utterance, never wrong speech.

```wit
interface context {
  use types.{node, snapshot, identity};

  /// Core's facts at the moment it sent the call.
  record context {
    /// The event's control, with the properties the tell and Core's own
    /// read already hold.
    subject: option<snapshot>,
    /// Core's committed focus, and its ancestors nearest first, as Core
    /// holds them.
    focus: option<snapshot>,
    focus-ancestors: list<snapshot>,
    /// The foreground window's object and its application.
    foreground: option<snapshot>,
    foreground-application: option<identity>,
    /// The queue, as NVDA's isPendingEvents and lastQueuedFocusObject
    /// are used by app modules (section 3).
    pending: pending,
    /// The keyboard, from Core's Keys record (section 3).
    keys: keys,
    /// The session: unlocked, locked, or on the secure desktop.
    session: session,
    /// The module's own settings and the Verbatim settings it declared
    /// it reads (section 7), as values.
    settings: list<setting-value>,
  }

  /// A control as Core holds it: a handle and the values already read.
  record snapshot {
    node: node,
    values: list<answered>,
  }
}
```

Departure from the reference: the reference had each hook take only a node and read the rest; the context replaces `focus()` and `foreground()` as imports that Core answers (`extensions.md` section 14) and adds the queue, keys, session and settings. Reason: under the first row each of those reads was a crossing for a fact Core already had.

The nodes in the context are named by the lane's record in Core, so the held view keeps them for the call; the instance keeps a node beyond the call only by holding its handle, which the lane's record then names until it is dropped.

## 1. Typed reads in one request

### The interface

WIT has no recursive types, so a matcher travels as its TOML text and Core parses it at the boundary; the SDK's `matcher!` macro parses it at compile time, so a guest's matcher never fails at run time.

```wit
interface read {
  use types.{node, property-id, property-value, relation, view, refusal, window-path};

  /// What a step reads: a node, Core's focus or foreground, a window by
  /// its class path, or an earlier step's result.
  variant subject {
    node(borrow<node>),
    focus,
    foreground,
    window(window-path),
    earlier(u8),
    item(earlier-item),
  }
  record earlier-item { step: u8, index: u32 }

  record walk {
    depth: u32,
    view: view,
    matcher: option<string>,
    prune: option<string>,
    include-subject: bool,
    limit: u32,
    nodes: u32,
  }

  variant op {
    properties(subject),
    related(tuple<subject, relation>),
    ancestors(tuple<subject, u32>),
    descendants(tuple<subject, walk>),
    test(tuple<subject, string>),
  }

  /// Run the step only when an earlier step matched or found something,
  /// or, negated, only when it did not.
  enum check { matched, found }
  record condition { on: u8, check: check, negated: bool }

  record step {
    op: op,
    /// The properties read of each node the step answers, in this order.
    set: list<property-id>,
    only-if: option<condition>,
  }

  /// A property's answer, in the order the set asked for.
  variant answered {
    value(property-value),
    /// The control has no value for it.
    absent,
    /// Left out of a snapshot Core read for itself, because a setting
    /// removed it from Core's own set; only in the context, never in a
    /// read the module asked for.
    not-asked,
  }

  record element { node: node, values: list<answered> }

  variant outcome {
    nodes(list<element>),
    matched(bool),
    skipped,
    not-there,
    failed,
    refused(refusal),
  }

  variant read-error {
    /// More than sixteen steps, a step naming a later step, a matcher
    /// that does not parse, or a property of the other backend.
    bad-request(string),
    /// A subject beyond the module's applications.
    outside,
    /// The call this read belongs to was cancelled.
    cancelled,
  }

  /// Every step, sent and answered together.
  read: func(steps: list<step>) -> result<list<outcome>, read-error>;
}
```

### The Rust SDK

The narrowed derive from the spike review: a struct is one property set for one element step, and the navigation is the request builder's, not the struct's. Each step returns a typed handle, so the answer is parsed into the struct its step asked for.

```rust
use verbatim_ext::prelude::*;

/// One property set, for one element step.
#[derive(Properties)]
struct Child {
    automation_id: Option<AutomationId>, // absent is None
    name: Name,                          // absent is an error
}

fn results(cx: &mut Cx, window: &Window) -> Result<Vec<Child>, ReadError> {
    let mut r = Request::new();
    let landmark = r.descendants(
        Subject::Window(window.clone()),
        Walk::any_depth().matching(matcher!(uia-class-name = "LandmarkTarget")).limit(1),
    );
    let children = r.children::<Child>(landmark.item(0)).only_if(landmark.found());
    let answer = cx.read(r)?; // one read
    answer.list(children)
}
```

### Semantics

- One answer path: `read` returns once, with one outcome per step, or with an error. A read whose call Core cancels (a cause, the instance's end) returns `cancelled`, and the guest's code after it runs only to return; the lane drops the effects it makes after that.
- Parsing, against the set asked: the host checks each element has exactly as many answers as the step's set, each of the kind its property names; a mismatch is the reader's defect, logged, and the read answers `failed`. The SDK then parses into the struct: a required field with `absent` is `Unsupported { field }`, an optional one `None`. A `not-asked` field of a context snapshot makes the SDK read that property live, one read, so a guest never takes "not asked" for "absent". The two kinds can be told apart only because the answer carries them, which is what the review asked for.
- The spike's compile-time step bound stays: a request built from more than sixteen steps does not compile when its size is known at compile time, and fails `bad-request` otherwise.
- Boundary: Core parses the steps into a checked batch against the module's applications and refuses, without sending, a step whose subject is outside them; a navigation that leaves them answers `not-there` for that node, as the reference's `outside` did.
- Focus and foreground as subjects are Core's committed focus and foreground, resolved by Core to their nodes, not the request list's `Subject::Focus` (the application's focus now), which a module can name as `focus-now` if a port needs it.

### Mapping to the request list

`properties` is `Op::Read`, `related` `Op::Related`, `ancestors` `Op::Ancestors` with no known ancestors, `descendants` `Op::Descendants` with its `Walk`, `test` `Op::Test`, `window` `Subject::Window` with `WindowOf::ClassPath`, `earlier` and `item` `Subject::Result` and `Subject::Item`, and `only-if` a `Condition` with `Test::Step`. It adds no kind of request. The property ids are the model's table (`crates/verbatim-model/src/property.rs`), named at run time, as ADR 0003 allows.

### What a long read does

A read is the application's time, not the handler's: while it runs, the call is waiting as the hook is, and the options above apply to it unchanged. A read of a hung application answers `failed` when Core's own foreground stop abandons the worker, as Core's own reads do.

### Departures from the reference

- `node.properties()`, the five navigation functions, `children()`, `find-descendant` and `find-ancestor` become steps of one `read`. Reason: each was a read of its own, and under the first row each read is two crossings.
- `find-window` becomes a subject. `node.window()`, `node.text()` and `node.process()` become properties (`NativeWindow`, `WindowClass`, `Process` exist; whole text is a text read, section 2).

## 2. Text

### The model

NVDA's add-ons use a `TextInfo` as a range they make from a position (`makeTextInfo` with caret, selection, first, last or all), then `expand`, `move`, `collapse`, `copy`, `compareEndPoints`, `setEndPoint`, `find`, `text`, `getTextWithFields`, `updateCaret` and `updateSelection` (`nvda/source/textInfos/__init__.py` 273 to 326 and 438 to 631). Each of those is a call into the application. Here the provider-touching operations are steps of one request, and the range is a local value of two points, so `collapse`, `copy` and `setEndPoint` cost nothing.

```wit
interface text {
  use types.{node, text-unit, format-attribute};

  /// A position Core holds in a node's text; dropping it lets it go.
  resource anchor;

  variant point {
    caret, selection-start, selection-end, start, end,
    at(position),
    /// A point an earlier step of this request answered.
    of(tuple<u8, point-of>),
  }
  record position { anchor: borrow<anchor>, offset: u32 }
  enum point-of { caret, selection-start, selection-end, chunk-start, moved }

  record details {
    formats: list<format-attribute>,
    spelling-errors: bool,
    grammar-errors: bool,
    links: bool,
  }

  variant text-op {
    caret,
    expand(expand),
    range(tuple<point, point>),
    move(tuple<point, text-unit, s32>),
    find(find),
    select(tuple<point, point>),
    locate(point),
    visible,
  }
  record expand { at: point, move-first: option<tuple<text-unit, s32>>, unit: text-unit, count: u16, details: details }
  record find { at: point, needle: string, backward: bool, limit: u8 }

  record text-step { subject: borrow<node>, op: text-op }

  record chunk {
    text: string,
    unit: text-unit,
    start: anchor,
    offset: u32,
    first: bool,
    last: bool,
    truncated: bool,
    formats: list<format-run>,
    languages: list<language-run>,
  }

  variant text-outcome {
    caret(caret-at), chunks(tuple<s32, list<chunk>>), text(string), moved(tuple<s32, anchor>),
    found(list<string>), selected, located(option<tuple<u32, u32>>), visible(visible-text),
    no-text, no-such-unit, outside-text, not-there, failed, skipped,
  }

  read-text: func(steps: list<text-step>) -> result<list<text-outcome>, read-error>;
}
```

The SDK gives authors the `TextInfo` shape:

```rust
let mut r = TextRequest::new(&edit);
let line = r.expand(Point::Caret, TextUnit::Line, 1).with(Details::formats([Bold, FontName]));
let words = r.expand(Point::Caret, TextUnit::Word, 3).moved(TextUnit::Word, -1);
let answer = cx.read_text(r)?;     // one read
let line: Chunk = answer.one(line)?;
let range: Range = line.range();   // a local value
let start = range.collapsed(Start); // no call
```

### Mapping to the request list

`caret` is `Op::Caret`; `expand` `Op::ReadText` (its `movement` and `details`); `range` `Op::ReadRange`; `move` `Op::Move`; `find` `Op::FindText`; `select` `Op::Select`, which is `updateCaret` with two equal points and `updateSelection` otherwise; `locate` `Op::Locate`; `visible` `Op::Visible`. Anchors are `TextAnchor`s, named by the lane's record so the held view keeps each while its resource lives. `Op::MoveBySentence`, `WaitForCaret`, `CompareSelection` and `ReadColours` stay Core's: they serve caret keys, selection speech and terminals, the hot path.

### Gaps, each Dickson's (CLAUDE.md item 4)

- A range read with formatting: `ReadRange` has no `details`, so `getTextWithFields` over an arbitrary range is read by units with `expand`. Adding `details` to `ReadRange` is a field, not a kind; Claude asks whether it counts.
- A case-insensitive find: `FindText` has no case flag, and NVDA's `find` defaults to case-insensitive (`textInfos/__init__.py` 582).
- `compareEndPoints` and `isOverlapping`: only `Test::NotAfter`, a condition that skips a step, compares two points. An answering comparison is a new result kind. Until then the SDK offers `compare` as two conditional `locate` steps, which answers only where the text counts by offsets.
- Units: `TextUnit` has character, word, line, sentence, paragraph, page and document; NVDA's table, row, column, cell, story, reading chunk and offset units are not offered.

### Semantics and costs

One `read-text` is one read, whatever its steps. `select` is the one write: it changes the application's selection, so it is a capability of its own, shown in the install dialog as "moves the caret in it" (new row). A long read behaves as section 1 says.

## 3. The pending focus, and lock and modifier state

```wit
interface context-types {
  record pending {
    /// A focus is in Core's queue behind this call's entry, as NVDA's
    /// isPendingEvents("gainFocus").
    focus: bool,
    /// An event of this call's kind is queued behind it, for any control,
    /// as NVDA's isPendingEvents(eventName) without an object.
    same-kind: bool,
    /// An event of this call's kind for this call's control is queued.
    same-kind-same-control: bool,
  }
  record keys {
    caps-lock: bool, num-lock: bool, scroll-lock: bool,
    /// Held when Core stamped this call's event at arrival.
    alt: bool, control: bool, shift: bool, windows: bool,
  }
  enum session { unlocked, locked, secure-desktop }
}
```

Semantics:

- Values, not reads: each is read from `Queue`, `Keys` and `Foreground` when Core sends the call. Under the second row of the open question, the outpost would have to be sent them, which is why that row is not recommended.
- The modifiers are the arrival stamp design-v2 already keeps (async rule 8, "Alt down at arrival"), so a hook judges the keyboard as it was when the event came, as the switcher rule does; the toggles are Core's newest.
- The decided Alt matcher property (2026-11-28) is the rule form of the same stamp, so a rule needs no hook for the switcher.
- NVDA's `lastQueuedFocusObject` comparison (`explorer.py` 56 to 72) is not offered: it is Core's duplicate-focus test, which already drops the duplicate.
- A stale value is safe: if a focus arrives after the send, the hook may speak, and Core then judges that speech (section 5); a handler that skipped work because `focus` was true when a later cancel took the focus away loses only speech NVDA would also have skipped.

Departure: the reference has none of these. Install dialog: nothing.

## 4. Spelling and the clipboard

```wit
interface speech {
  /// Speak text letter by letter, with Verbatim's capitals and
  /// character descriptions, as NVDA's speakSpelling.
  speak-spelling: func(text: string, descriptions: bool);
}

interface clipboard {
  enum copied { copied, failed }
  /// Put text on the clipboard. With announce, Verbatim says "Copied to
  /// clipboard: <text>", or "<n> characters" for 1,024 characters or
  /// more, or "Unable to copy", as NVDA's reportTextCopiedToClipboard.
  copy-text: func(text: string, announce: bool) -> copied;
}

interface clipboard-read {
  /// The clipboard's text, or none.
  clipboard-text: func() -> option<string>;
}
```

Semantics:

- `speak-spelling` is an effect, judged by Core as `speak` is, and spelled by Core's speech pipeline, so the spelling is Verbatim's own.
- `copy-text` is performed by Core on a shell thread, which checks the clipboard after writing as NVDA does (`api.py` 399 to 424); its result costs one crossing to Core and back under every row, since only Core touches the clipboard. Its announcement is Core's speech, with NVDA's wording (`ui.py` 219 to 243).
- `clipboard-text` is a separate interface because the clipboard is the user's content: with a network import it adds a warning "Can send what you copy to <host>". Install dialog: nothing for `copy-text`; nothing for `clipboard-text` alone.
- A long copy: the call waits on Core's shell thread; another application holding the clipboard open makes it `failed` after Windows' own refusal, not after a wait of Verbatim's.

Departure: the reference listed the clipboard as not settled.

## 5. Speaking after Core's default announcement

### The interface

A following registration, not a verdict:

```toml
[[hooks]]
hook = "after-focus"
backend = "uia"
match = { role = "list-item", window-class = "DirectUIHWND" }
```

```wit
interface hooks {
  /// Called once Core has queued its own announcement of the focus. Its
  /// speech is spoken after that announcement. Nothing waits for it.
  after-focus: func(ctx: context);
  /// The same for an event Core handles itself.
  after-event: func(ctx: context, event: event);
}
```

### Ordering and focus validity

- The append is part of the announcement it follows: Core speaks its default, the call is sent as that announcement is queued (a focus is committed before its hooks run, ADR 0003), and the hook's speech, when it arrives, joins the end of that announcement's speech.
- It shares that announcement's fate: whatever cut the announcement (the key fence, a newer focus's announcement, cancel speech) drops the append; if the announcement finished speaking with nothing newer queued, the append is spoken then.
- If a later entry's speech is already queued when the append arrives, two options: drop it (recommended, since speech about a control the user has left is the noise NVDA's own cancel avoids), or speak it after the later speech.
- A holding `on-focus` hook that speaks keeps its place before the default, as NVDA's handler that speaks then calls `nextHandler`.

### A long following hook

Nothing waits, by construction. The options are only for the late answer: dropped by the fate rule above, or, as an option for a translation add-on, spoken whenever it comes if the focus is still the one it followed, even after the default finished. Claude recommends the second, since the fate rule already drops it once the user has moved.

Departure: ADR 0003 left "a hook's speech after the default announcement" to M5 build B; the reference's verdicts had no form for it.

## 6. New event kinds, registered narrowly

### The survey

The events NVDA's app modules handle (`nvda/source/appModules`, counted by `def event_`): `NVDAObject_init` 26, `gainFocus` 20, `valueChange` 10, `nameChange` 9, `UIA_notification` 5, `UIA_elementSelected` 5, `stateChange` 4, `show` 3, `appModule_loseFocus` 3, `appModule_gainFocus` 2, `typedCharacter` 2, `selection` 2, `foreground` 2, `focusEntered` 2, `UIA_window_windowOpen` 2, and once each `caret`, `pageChange`, `liveRegionChange`, `selectionAdd`, `selectionRemove`, `selectionWithIn`, `suggestionsClosed`, `UIA_itemStatus`, `loseFocus` and `treeInterceptor_gainFocus`.

### The kinds proposed

| Kind | NVDA's event | Users in the inbox modules | Data besides the control |
|---|---|---|---|
| `window-opened` | `UIA_window_windowOpen` | Explorer, the emoji panel | none |
| `shown` | `show` | Explorer's tooltips, MMC, the emoji panel | none |
| `state-change` | `stateChange` | (iTunes, Outlook, LibreOffice; later tiers) | the states before and after, as Core holds them |
| `item-status` | `UIA_itemStatus` | Quick Settings | the new status, or none |
| `focus-entered` | `focusEntered` | Open With, the emoji panel | none |
| `application-gained`, `application-lost` | `appModule_gainFocus`, `appModule_loseFocus` | (Lock app, Word) | none |
| `live-region-change` | `liveRegionChange` | (Lync) | none |
| `selection-added`, `selection-removed`, `selection-within` | `selectionAdd`, `selectionRemove`, `selectionWithIn` | (LibreOffice) | none |

Not offered as hooks, because they are the hot path: `typedCharacter` (Calculator's override is a declaration, section 7) and `caret`. `NVDAObject_init` is `on-node`; `foreground` is the window rules and `application-gained`.

### Registration

A registration names its kind and a matcher over the tell's own properties: the application is the module's, and the matcher may name role, automation id, UIA class name, window class, control type, framework, and for notifications kind and activity id.

```toml
[[hooks]]
hook = "on-event"
kinds = ["window-opened"]
backend = "uia"
match = { uia-class-name = "ApplicationFrameWindow" }
```

Semantics:

- The matcher is compiled into the listener's part of the `Watch` filters where it names only cached properties, so an unmatched event reaches Core as an ordinary tell with no hook stage, and costs one table lookup: the hot-path budget is kept for every unhandled event.
- A matcher naming a property the tell does not cache is judged in Core on the read control, costing what Core's own read costs.
- A kind Verbatim does not otherwise listen for (`window-opened`, `item-status`, `live-region-change`) needs a UI Automation event registration. Options: the listener registers it desktop-wide while any active module registers the kind, filtering by process at once (one handler, events from every application cross into the listener); or it registers per window of the claimed application (fewer events, a registration per window). This is ADR 0003's "modules asking for extra events", and which process registers is Dickson's (CLAUDE.md item 3).
- A long `on-event` handler is a holding hook unless its registration says `follow = true`, which makes it a following one (section 5).

Departure: the reference had five kinds and "kinds are added as modules need them"; this adds eleven and allows `activity-id` in hook registrations, which the reference allowed only in event rules.

## 7. Module settings and Verbatim's settings

Decided (2026-11-28); the interface:

```toml
[[settings]]
id = "typing-echo-buttons"
label = "calculator-typing-echo-buttons"   # message id
kind = "toggle"                             # toggle, choice, number or text
default = true

[[settings]]
id = "voice"
label = "example-voice"
kind = "choice"
choices = [{ id = "a", label = "example-voice-a" }, { id = "b", label = "example-voice-b" }]
default = "a"

reads-settings = ["keyboard.typing-echo"]   # Verbatim's, by public name

[typing-echo]
edit-controls-only-means = "always"
when = { setting = "typing-echo-buttons" }
```

```wit
interface settings {
  variant setting-value-kind { toggle(bool), choice(string), number(s64), text(string) }
  record setting-value { id: string, value: setting-value-kind }
  /// Change one of the module's own settings, as a command that toggles
  /// it would; Core applies, saves and announces nothing.
  set-own: func(id: string, value: setting-value-kind) -> result<_, setting-error>;
  enum setting-error { unknown, wrong-kind, out-of-range }
}
```

Semantics:

- The settings dialog shows each module's settings on a page under its extension's name; the settings file keeps them under the extension and module ids. Core owns them; their values travel in the context (section "The context"), so a read costs nothing and is fresh at every call.
- `reads-settings` names Verbatim's settings by a public name list, which becomes part of the settings file format's contract; reading them is shown nothing in the install dialog.
- Declarations such as `[typing-echo]` are data Core applies on the hot path with no call, bound to a module setting by `when`.
- `set-own` is an effect, one crossing to Core; an extension never writes Verbatim's settings.
- No settings-changed hook: a module sees a change at its next call.

## Worked examples

### Report title, a built-in global module

NVDA's `script_title` (`globalCommands.py` 3210 to 3230) reads the foreground's name, falls back to the application's name, then "No title"; refuses with "Action unavailable while Windows is locked" (`gui/blockAction.py` 62 to 66) when the foreground is below the lock screen; speaks, spells on the second press and copies with notice on the third and later.

```toml
kind = "global"

[[commands]]
id = "title"
gestures = ["verbatim+t"]
description = "report-title-description"

[[commands]]
id = "title-spell"
gestures = [{ keys = "verbatim+t", presses = 2 }]
description = "report-title-description"

[[commands]]
id = "title-copy"
gestures = [{ keys = "verbatim+t", presses = 3 }]
description = "report-title-description"
```

A fourth press runs `title-copy` again, as NVDA's repeat count of 3 or more copies.

```rust
#[derive(Properties)]
struct Title { name: Option<Name> }

fn on_command(cx: &mut Cx, command: &str, _: Trigger) -> Result<(), Error> {
    let ctx = cx.context();
    let Some(foreground) = ctx.foreground.as_ref() else {
        if ctx.session != Session::Unlocked {
            cx.speak(msg!("windows-locked"));
        }
        return Ok(());
    };
    let title: Title = cx.read_one(&foreground.node)?; // one read, a fresh name
    let text = title.name.map(|n| n.0).filter(|t| !t.trim().is_empty())
        .or_else(|| ctx.foreground_application.as_ref().map(|a| a.name.clone()))
        .unwrap_or_else(|| cx.message(msg!("no-title")));
    match command {
        "title" => cx.speak(text),
        "title-spell" => cx.speak_spelling(&text, false),
        _ => { cx.copy_text(&text, true); }
    }
    Ok(())
}
```

Costs: one read (two crossings under the first row), or none if the foreground's name in the context is used. A long call: commands do not hold the queue; its speech is judged by the key fence on arrival. The lock-screen rule rests on a trust question: whether Core gives an extension no node below the lock screen while Windows is locked, so the module sees no foreground (above), or gives the node and a `below-lock-screen` property.

### File Explorer

NVDA's `explorer.py` has no handler that speaks after an item's default announcement. Its tab switch (499 to 517) speaks the selected tab as a focus and then calls `nextHandler`:

```toml
[[hooks]]
hook = "on-event"
kinds = ["selection"]
backend = "uia"
match = { role = "tab", states = "selected", parent-automation-id = "TabListView" }
```

```rust
fn on_event(cx: &mut Cx, _: &Event) -> Verdict {
    if cx.context().pending.same_kind {
        return Verdict::Pass; // a second selection is queued: NVDA's double-speech guard
    }
    if let Some(tab) = cx.context().subject.as_ref() {
        cx.speak_node(&tab.node);
    }
    Verdict::Pass
}
```

No read: the context holds the tab, its role, states and parent's automation id are in the tell's cached set or Core's read. A following form fits too, since the verdict is always `pass`; NVDA speaks before its default, so the holding form keeps NVDA's order.

The after-focus form, as an add-on would use it (illustration only, not NVDA behaviour): after a file list item's announcement, speak the status bar's selection summary.

```rust
fn after_focus(cx: &mut Cx) {
    let mut r = Request::new();
    let bar = r.descendants(Subject::Foreground,
        Walk::children().matching(matcher!(control-type = "StatusBar")).limit(1));
    let parts = r.children::<Part>(bar.item(0)).only_if(bar.found());
    if let Ok(parts) = cx.read(r).and_then(|a| a.list(parts)) {
        cx.speak(parts.iter().map(|p| p.name.0.as_str()).collect::<Vec<_>>().join(", "));
    }
}
```

Its speech follows the item's announcement and is dropped once the user arrows on.

### Calculator

NVDA's `calculator.py`: digit keys pass through and set a flag (185 to 190); Enter, Escape and Delete pass through (143 to 155); `DisplayUpdated` finds `LandmarkTarget`, says nothing when its first child is an entry field and the flag was set, else cancels speech and speaks the result child's name, ending the event (94 to 141); typing echo set to "edit controls only" counts as always (61 to 70).

```toml
[[commands]]
id = "digit"
gestures = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "numlocknumpad0", "numlocknumpad9"]
pass-through = true
description = "calculator-digit"

[[hooks]]
hook = "on-event"
kinds = ["notification"]
backend = "uia"
match = { activity-id = "DisplayUpdated" }
```

```rust
#[derive(Properties)]
struct Child { automation_id: Option<AutomationId>, name: Name }

struct Calculator { no_result_key: bool }

fn on_command(&mut self, _: &mut Cx, _: &str, _: Trigger) { self.no_result_key = true; }

fn on_event(&mut self, cx: &mut Cx, event: &Event) -> Verdict {
    let skip = std::mem::take(&mut self.no_result_key);
    let Some(window) = cx.context().subject.as_ref().map(|s| s.window()) else { return Verdict::Pass };
    let Ok(children) = results(cx, &window) else { return Verdict::Pass }; // one read (section 1)
    let entry = children.first().and_then(|c| c.automation_id.as_ref())
        .is_some_and(|id| NO_ENTRY_ANNOUNCEMENTS.contains(&id.0.as_str()));
    if entry && skip { return Verdict::Drop; }
    match children.iter().find(|c| matches!(c.automation_id.as_ref().map(|i| i.0.as_str()),
                                            Some("CalculatorResults" | "CalculatorAlwaysOnTopResults"))) {
        Some(result) => { cx.cancel_speech(); cx.speak(result.name.0.clone()); Verdict::Drop }
        None => Verdict::Pass,
    }
}
```

The digit commands do not hold the queue (decided), and their calls reach the lane before the notification's, so the flag is set first. The typing echo is the `[typing-echo]` declaration in section 7, with the module's own toggle, on by default to match NVDA. Which tree view NVDA's `children` walks for these children is checked against a capture before the port.

### Reading a status line from text

An illustration: an editor whose last line of text is its status line, with the mode in bold.

```rust
#[derive(Properties)]
struct Editor { automation_id: AutomationId }

fn on_command(cx: &mut Cx, _: &str, _: Trigger) -> Result<(), Error> {
    let Some(focus) = cx.context().focus.as_ref() else { return Ok(()) };
    let mut r = TextRequest::new(&focus.node);
    let last = r.expand(Point::End, TextUnit::Line, 1).with(Details::formats([FontAttributes]));
    let line: Chunk = cx.read_text(r)?.one(last)?; // one read
    let mode = line.runs().find(|run| run.attributes.bold()).map(|run| run.text());
    cx.speak(match mode { Some(m) => format!("{m}, {}", line.text.trim()), None => line.text.trim().to_owned() });
    Ok(())
}
```

`Point::End` expanded to a line reads the last line, as the request list defines `TextPoint::End`. One read.

## Changes to the 2026-11-09 reference, all of them

- The hooks page: the context argument on every hook; `after-focus` and `after-event`; `follow = true`; "Where hooks run" depends on the open question.
- The events page: eleven kinds; `activity-id` in hook registrations.
- The imports page: `read` and `read-text` replace the per-property and per-navigation functions, `find-window`, `node.text()`, `node.window()` and `node.process()`; `focus()` and `foreground()` move into the context; `speak-spelling`, `copy-text`, `clipboard-text` and `set-own` added.
- The manifests page: `[[settings]]`, `reads-settings`, `[typing-echo]`.
- The capabilities page: "moves the caret in it" for `select`; the clipboard-read warning; "Not settled yet" loses the clipboard.
- The limits page: the 11-second handling deadline replaced by the options under "What waits while a handler runs long".

## Questions for Dickson

1. Who calls hooks: Core (recommended), the outpost, or Core with direct reads.
2. While a holding hook runs long: Hold, Hold ended by causes (recommended), or with a clock bound added, and if a bound, a setting or fixed.
3. A late following hook's speech: drop once later speech is queued, or speak while its focus stands (recommended).
4. The request list's gaps: `details` on `ReadRange`, a case flag on `FindText`, an answering comparison of two points.
5. New event registrations: desktop-wide in the listener while a module registers the kind, or per window.
6. Whether an extension is given nodes below the lock screen while Windows is locked.
7. The public names of Verbatim's settings that modules may read, which joins the settings file's contract.
8. Whether `set-own` is allowed, or only the settings dialog changes a module's settings.
9. The capability rows: "moves the caret in it", and the clipboard-read warning.
