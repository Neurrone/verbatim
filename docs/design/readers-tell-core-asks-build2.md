# Design: the text provider, the details and the formatting memory (build 2 of ADR 0003)

Status: approved by Dickson on 2026-11-16. Nothing in it is built. It holds what remained of the keys-and-text design once Dickson's ruling of 2026-11-16 on the goals audit moved its caret key, stroke line, watch, held anchors, selection baseline, text-changed flag and sentence move into build 1 ([build 1's design](readers-tell-core-asks-build1.md)); none of it is needed for build 1 to work. Builds 1, 2 and 3 are built in number order and pushed together (Dickson, 2026-11-16). Written against main 1a0b4c0e, NVDA pinned at 732d1ce2f0f051da3ef1e6a6247c64cf04596337. Where it and ADR 0003 differ, ADR 0003 wins and the difference is a defect here.

Sources: [ADR 0003: Readers tell, Core asks](https://github.com/Neurrone/verbatim/blob/main/books/dev/src/reference/adr/0003-readers-tell-core-asks.md) ("the ADR"); `docs/design/readers-tell-core-asks.md` ("the build notes"), its "Build 2" and "Core's rules" (text, caret and keys); build 1's design, as designed, not as today's code. NVDA paths are under `nvda/source/`; Verbatim paths under `crates/` unless they start with `docs/` or `books/`.

## Terms

- Text provider: how the outpost reads a node's text: UI Automation, an edit control's window messages (with its edit API version), IAccessible2 text (M7), or none.
- Details: what a text read fetches beyond the text (formatting attributes, spelling errors, links).
- Text memory: the formatting last spoken and the kind of line break, kept for one focus record.

## The decisions this design honours

- "Core holds every rule that decides what is spoken, written to read like NVDA's, and its thread never blocks on an application. An outpost does mechanical work on objects it holds, only on Core's request and with the objects named by id."
- "Every call into an application is made for a request from Core; the request carries what it needs and gets one answer."
- "One request list is shared by Core's own features and the extension surface ... A new command adds no kind of request."

## The model

Core names the text provider and the details in every text request, so the outpost keeps no settings and `CoreToOutpost::Fetches` goes. The text memory lives on the focus record a key or entry was bound to, not on the focus object alone. Until this build, the outpost chooses the provider by window class and reads the details `Fetches` names, as today, and build 1's Core sends `Fetches` as an effect.

## Rules, each with its reason

- Every text request names the provider Core chose and the details it fetches. Core chooses the provider from the focus answer: a UI Automation node reads through UI Automation; an MSAA node's client object whose window's normalized class is an edit control's reads by its window messages, with its edit API version (today in the outpost, `verbatim-ia2/src/edit.rs` 96 to 104, `text_reads.rs` 274 to 295); anything else has none. Reason: which interface reads a node is a rule, and rules are Core's. The outpost's two guesses that only cost calls stay: `TextSupport` (`text/uia.rs` 214 to 224) and the classic fallback (`text_reads.rs` 468 to 480).
- The details come from Core's settings, the unit and the reason: a character or word read takes the extra detail; a paragraph or cell read for a caret key leaves out spelling errors. NVDA: `speech/speech.py` 1586 to 1596. `CoreToOutpost::Fetches`, the theme broadcast (`verbatim-app/src/main.rs` 1911 to 1933) and `fetches_sent` (`verbatim-app/src/live.rs` 30) go; the MSAA backend's and the UI Automation gates' reading of the registry's `Fetches` (`verbatim-ia2/src/registry.rs` 226 to 236; `verbatim-outpost/src/outpost/mod.rs` 701 to 724) takes the request's set, which build 1 introduced for node reads.
- The formatting last spoken and the kind of line break live on each focus record a key or entry names, while the committed focus or a waiting entry names it, cleared when its outpost is replaced and not restored by a silent re-take. NVDA keeps them on the object (`speech/speech.py` 1504 to 1518, 1580 to 1583). Today they live only on the focus object (`editing.rs` 693 to 715). Reason: a key run on its press-bound field (build 1, ruled 2026-11-16) compares formatting with that field's memory, as NVDA's script compares it with its own object's.
- Control fields, when built, are compared by the provider's identity or by equality, never by an outpost node id. NVDA: `speech/speech.py` 1646 to 1657. Not built here.

## State table

### Table 1: a focus record's text memory (formatting last spoken, kind of line break)

States: K, kept (the record is the committed focus or a waiting entry names it); G, gone.

Columns: A, a key or entry bound to the record speaks text; B, the committed focus moves on and no waiting entry names the record; C, its outpost is replaced; D, a silent re-take takes it.

| State | A | B | C | D |
| --- | --- | --- | --- | --- |
| K | its formatting compared with and written to this record's memory | to G | cleared, so formatting is said afresh | not restored: the re-take starts empty |
| G | n/a: an entry naming the record keeps it | nothing | nothing | n/a |

## Decisions made outside Core, with their facts

- `TextSupport` and the classic fallback: guesses in the outpost that only cost calls, never a word.

The provider, the details and the memory are Core's.

## Waiting states, messages, timers and two-place lifecycles

Added: none. The text memory is one fact in Core, on each focus record.

Removed: `Fetches` in the outpost and the theme broadcast, a copy of Core's settings held in each outpost; the formatting memory on the focus object alone.

## Messages added and removed

- Every text request of the shared list (the caret read, the watch's final read, the sentence move, a line, word or character read, read ahead, a range, a point's location, `MoveCaret`, `Select`) names the provider and the details.
- Removed: `CoreToOutpost::Fetches`.

## What users hear

No sequence changes beyond the formatting memory's: a key bound to an earlier focus record compares formatting with that record's memory, and a record whose outpost was replaced says its formatting afresh. Pinned by, unchanged expectations: `spelling_errors`, `notepad_spelling_errors`, `theme_panel`, `notepad_editing`, `text_box_editing` and Core's formatting, spelling error and line break tests (`books/dev/src/reference/test-coverage.md`, "Spelling errors", "Document formatting" and "Line breaks").

## Tests

Hand-written, one per rule: every cell of Table 1 in Core; the provider chosen from each kind of focus answer; the details per unit and reason; the outpost reading the request's set with no `Fetches`. Related scenarios for the parent: `spelling_errors`, `theme_panel`.

## How the code is organised

- `verbatim-core/src/text/`: `provider.rs` (the provider chosen from the focus answer), `details.rs` (details per request), `memory.rs` (Table 1).
- `verbatim-outpost`: the text reads take the provider and details from the request; `Fetches` goes from the outpost and the MSAA backend.
- `verbatim-app`: the theme broadcast and `fetches_sent` go.
- `verbatim-model/src/text.rs`: the provider and details fields on text requests; `Fetches` goes.

## Limits

- Control fields are not built here.

## Performance figures

No added wake-up, message or call: the provider and details ride on requests already sent, and the theme broadcast to every outpost goes.
