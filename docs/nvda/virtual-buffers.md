# Virtual buffers

A virtual buffer is NVDA's in-process cached flattening of a document —
the machinery that makes browse mode fast in Firefox, Chrome, Edge
(IA2 path), Adobe Reader, and a few legacy hosts. The C++ lives in
`nvdaHelper/vbufBase/` (storage and backend base) and
`nvdaHelper/vbufBackends/` (per-toolkit renderers), running inside the
target app via injection ([Process injection](process-injection.md)); the Python side is
`source/virtualBuffers/`. The in-tree `nvdaHelper/readme.md` gives the
official narrative; the load-bearing details:

## Storage: a text stream with a field tree

`vbufBase/storage.cpp` implements the buffer proper: one contiguous
text stream (the document, flattened in reading order) plus a tree of
*field nodes* over it — control fields (elements: their role, states,
attributes as key-value strings, and identity as docHandle+ID pairs)
and text fields (leaf runs). Every node knows its offset range.
Queries the storage answers: text in range, node at offset, offsets of
node, next/previous node matching attribute conditions (the quick-nav
primitive), plus line calculation. Everything downstream — browse-mode
reading, quick nav, the elements list — is offset arithmetic against
this structure.

## Backends: rendering inside the app

A backend (subclass of `VBufBackend_t`, `vbufBase/backend.h`) fills
storage by walking the app's accessibility tree *in-process*. The
contract:

- `render(buffer, docHandle, ID, oldNode)` — produce the subtree's
  content into a buffer. The gecko_ia2 backend
  (`vbufBackends/gecko_ia2/gecko_ia2.cpp`) walks IA2: for each node it
  reads role, states, attributes, and `IAccessibleText`, splitting
  text runs on formatting and embedded-object boundaries (U+FFFC
  resolved via `IAccessibleHypertext` into child control fields).
  Backends exist for gecko_ia2 (serving Gecko *and* Chromium), MSHTML,
  WebKit, Adobe Acrobat, and Lotus Notes rich text
  (`vbufBackends/`).
- Change tracking: the backend registers an in-context winevent hook
  on its render thread (`renderThread_winEventProcHook`); events mark
  nodes invalid (`pendingInvalidSubtreesList`), and a timer
  (`renderThread_timerProc`) batches re-renders of invalid subtrees
  into the storage, replacing the old nodes — incremental damage and
  repair, entirely inside the app process, on the app's own UI thread
  (where the accessibility calls are same-apartment and cheap).
- After an update, the backend notifies NVDA
  (`nvdaControllerInternal_vbufChangeNotify`), so the Python side can
  re-sync any open TextInfos and report live changes.

`vbufRemote.cpp` exports the RPC surface NVDA calls: create/destroy a
buffer for a root (docHandle = window, ID = IA2 uniqueID or
equivalent), fetch text ranges, resolve offsets to fields, search by
attributes, get selection, activate a node.

## The Python side

`virtualBuffers.VirtualBuffer` (subclass of
`browseMode.BrowseModeDocumentTreeInterceptor`;
`source/virtualBuffers/__init__.py`) owns a buffer handle:

- Created by the tree interceptor factory when focus lands in a
  document whose NVDAObject declares a buffer backend
  (`event_documentLoadComplete` and `treeInterceptorHandler`;
  [Browse mode](browse-mode.md)); `loadBuffer` runs the initial render in a
  background thread (`_loadBuffer`), announcing when ready.
- `VirtualBufferTextInfo` is an *offsets-based* TextInfo
  ([TextInfo](text-infos.md)) whose primitive operations are RPC calls into the
  in-process storage: text retrieval with embedded control-field
  markup, offset-to-node and node-to-offset resolution. Reading a line
  is thus one cross-process call against a cache, not a walk of the
  live tree.
- Mapping to and from the live tree: each control field carries
  docHandle+ID; `getNVDAObjectFromIdentifier` (per-buffer subclass)
  converts to a real NVDAObject (for focus mode handoff, activation,
  caret sync), and `getIdentifierFromNVDAObject` the reverse (for
  "where is focus in the buffer").
- Updates: `_handleUpdate` re-anchors state after the backend reports
  a change; quick nav searches (`_iterNodesByType`) map item types to
  attribute-condition searches executed in the storage.

## Properties that define the design

- The buffer is *stale by construction* between updates: NVDA reads
  the cache, not the DOM; the damage/re-render cycle bounds staleness
  but never eliminates it. Screen reader users experience this as the
  occasional "buffer says X, app says Y" — accepted as the price of
  instant navigation.
- Because rendering runs on the app's UI thread, a busy page slows
  its own buffer updates but not NVDA's responsiveness on the stale
  content — an intentional decoupling.
- Backends are per-engine, with engine-specific quirk handling baked
  in (gecko_ia2 contains years of Gecko/Chromium-specific
  interpretation); adding a browser means writing a backend.
- Chromium/Gecko UIA documents bypass all of this: UIA browse mode is
  bufferless ([Browse mode](browse-mode.md)), built directly on UIA text ranges.
