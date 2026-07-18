# IAccessible2

IAccessible2 (IA2) is the open-standard extension that made MSAA rich enough
for web browsers and office suites. It is not a Microsoft API: it came out of
IBM's Lotus work in 2006 and is stewarded by the Linux Foundation. Its
serious implementors today are Gecko (Firefox), Chromium (Chrome, Edge,
Electron), and LibreOffice. The IDL is vendored in this repo under
`nvda/include/ia2`. NVDA's usage is documented in [IA2 usage](../nvda/ia2.md).

## How it attaches to MSAA

IA2 does not replace MSAA; it decorates it. Discovery is a two-step COM
dance:

1. Obtain a real MSAA `IAccessible` (not a simple child) the normal way.
2. `QueryInterface` it for `IServiceProvider`, then call
   `QueryService(IID_IAccessible, IID_IAccessible2, out)` — the *service*
   ID is `IID_IAccessible` (the service being asked about is the MSAA
   object), and the second argument names the interface wanted from it.
   NVDA's call is exactly this shape
   (`source/IAccessibleHandler/__init__.py`, `normalizeIAccessible`). The
   QueryService indirection (rather than plain QueryInterface) was chosen
   so implementations can hand out the IA2 view without every wrapper layer
   having to forward an unknown interface.

If either step fails, the node is MSAA-only. Because IA2 interfaces are not
system-provided, *marshaling them cross-process requires the IA2 proxy/stub
DLL to be present in both processes* ([COM](com.md)); a screen reader that
attaches to browsers must arrange this — which in practice ties IA2 use to
the injection machinery ([Process injection](../nvda/process-injection.md)).

## What it adds

The `IAccessible2` interface itself extends `IAccessible` with:

- A larger role set (`IA2_ROLE_*`: paragraph, heading, section, footnote,
  content deletion/insertion, …) layered on top of the MSAA role, plus
  extended states (`IA2_STATE_*`: editable, multi-line, required, invalid
  entry, …).
- **Object attributes**: arbitrary key-value string pairs
  (`attribute:value;` lists) — this is where HTML semantics that fit no
  role live (tag name, ARIA properties, display style, text formatting at
  the object level).
- **Relations** (`IAccessibleRelation`): typed links between nodes —
  label-for/labelled-by, controller-for, member-of, embeds, …
- `uniqueID`: a stable numeric identity per node within the process —
  fixing MSAA's worst structural gap. Negative IDs are used by Chromium and
  Gecko; the pair (window handle, uniqueID) identifies a node, and IA2
  events carry it.
- `windowHandle`, `indexInParent`, `groupPosition`, locale.

The companion interfaces, each obtained by QueryInterface from
`IAccessible2`, carry the substance:

- `IAccessibleText` — the text model: character/word/line retrieval by
  offset, caret offset, selections, character extents, offset-at-point, and
  *text attributes* (formatting runs as key-value strings over offset
  ranges). `IAccessibleEditableText` adds mutation.
- `IAccessibleHypertext` / `IAccessibleHyperlink` — embedded objects in
  text: a link or an image inside a paragraph appears as an *embedded
  object character* (U+FFFC) in the text stream, and hypertext maps that
  offset to the child object. This convention — text with embedded object
  markers — is the backbone of how browsers expose documents and how
  screen readers reconstruct them.
- `IAccessibleTable2` / `IAccessibleTableCell` — grids: row/column extents,
  headers, selection.
- `IAccessibleValue` — numeric value/min/max for ranges.
- `IAccessibleAction` — enumerable, named actions beyond MSAA's single
  default action.
- `IAccessibleApplication` — toolkit name and version (how a client learns
  "this is Gecko 128" and adapts).

## Events

IA2 keeps using WinEvents as the transport but extends the vocabulary with
`IA2_EVENT_*` values (registered in a reserved range): text-changed
(inserted/removed, with offsets), text-caret-moved, document-load-complete,
object-attribute-changed, page-changed, and — significant for screen
readers — the ARIA live-region machinery in browsers ([ARIA](aria.md))
is surfaced through text-inserted events plus object attributes
(`container-live`, `relevant`, `atomic`) that the client interprets. As with MSAA, events carry identity
(window, IA2 unique ID as the child parameter), not payloads beyond what
fits in the event; the client queries after the fact.

## Working characteristics

- IA2 inherits MSAA's transport and therefore all of [COM](com.md)'s costs:
  per-property blocking round trips into a (usually) STA server. Its rich
  text interfaces multiply the number of calls per node, which is why bulk
  document reading over raw IA2 is impractical cross-process and clients
  build in-process document buffers instead ([Virtual buffers](../nvda/virtual-buffers.md)).
- Because attributes and roles are stringly-typed and browser-defined,
  clients accumulate per-toolkit interpretation code; two browsers can
  express the same HTML differently and both be conformant.
- The `uniqueID`-based identity and offset-based text model are reliable in
  the two big implementations; the pre-IA2 MSAA weaknesses mostly do not
  apply within IA2 content.

## References

- [The IA2 specification and IDL (Linux Foundation)](https://accessibility.linuxfoundation.org/a11yspecs/ia2/docs/html/),
  and `nvda/include/ia2` in this repo.
- [Chromium accessibility overview](https://chromium.googlesource.com/chromium/src/+/main/docs/accessibility/overview.md) —
  their IA2 implementation notes.
