# Design: the text provider, the details and the formatting memory (build 2 of ADR 0003)

Status: approved by Dickson on 2026-11-16. Nothing in it is built. It holds what remained of the keys-and-text design once Dickson's ruling of 2026-11-16 on the goals audit moved its caret key, stroke line, watch, held anchors, selection baseline, text-changed flag and sentence move into build 1 ([build 1's design](readers-tell-core-asks-build1.md)); none of it is needed for build 1 to work. Builds 1, 2 and 3 are built in number order and pushed together (Dickson, 2026-11-16). Written against main 1a0b4c0e, NVDA pinned at 732d1ce2f0f051da3ef1e6a6247c64cf04596337. Where it and ADR 0003 differ, ADR 0003 wins and the difference is a defect here.

Sources: [ADR 0003: Readers tell, Core asks](https://github.com/Neurrone/verbatim/blob/main/books/dev/src/reference/adr/0003-readers-tell-core-asks.md) ("the ADR"); `docs/design/readers-tell-core-asks.md` ("the build notes"), its "Build 2" and "Core's rules" (text, caret and keys); build 1's design, as designed, not as today's code. NVDA paths are under `nvda/source/`; Verbatim paths under `crates/` unless they start with `docs/` or `books/`.

## Terms

- Text provider: how the outpost reads a node's text: UI Automation, an edit control's window messages (with its edit API version), IAccessible2 text (M7), or none.
- Details: what a text read fetches beyond the text (formatting attributes, spelling errors, links).
- Formatting memory: the formatting last spoken in a focus record's text.
- Focus record: Core's focus object; each focus taken is a new record, named by its node and the trace of the event Core took it from.

## The decisions this design honours

- "Core holds every rule that decides what is spoken, written to read like NVDA's, and its thread never blocks on an application. An outpost does mechanical work on objects it holds, only on Core's request and with the objects named by id."
- "Every call into an application is made for a request from Core; the request carries what it needs and gets one answer."
- "One request list is shared by Core's own features and the extension surface ... A new command adds no kind of request."

## The model

Core names the text provider and the details in every text request, so the outpost keeps no settings and `CoreToOutpost::Fetches` goes. The formatting memory goes with the focus record, as ADR 0001 decided ("The formatting last reported goes with the focus object", Dickson, 2026-11-08), and a caret key's binding carries the record it was bound to, memory included. Until this build, the outpost chooses the provider by window class and reads the details `Fetches` names, as today, and build 1's Core sends `Fetches` as an effect.

## Rules, each with its reason

- Every text request names the provider Core chose and the details it fetches. Core chooses the provider from the focus answer: a UI Automation node reads through UI Automation; an MSAA node's client object whose window's normalized class is an edit control's reads by its window messages, with its edit API version (today in the outpost, `verbatim-ia2/src/edit.rs` 96 to 104, `text_reads.rs` 274 to 295); anything else has none. Reason: which interface reads a node is a rule, and rules are Core's. The outpost's two guesses that only cost calls stay: `TextSupport` (`text/uia.rs` 214 to 224) and the classic fallback (`text_reads.rs` 468 to 480).
- The details come from Core's settings, the unit and the reason: a character or word read takes the extra detail; a paragraph or cell read for a caret key leaves out spelling errors. NVDA: `speech/speech.py` 1586 to 1596. `CoreToOutpost::Fetches`, the theme broadcast (`verbatim-app/src/main.rs` 1911 to 1933) and `fetches_sent` (`verbatim-app/src/live.rs` 30) go; the MSAA backend's and the UI Automation gates' reading of the registry's `Fetches` (`verbatim-ia2/src/registry.rs` 226 to 236; `verbatim-outpost/src/outpost/mod.rs` 701 to 724) takes the request's set, which build 1 introduced for node reads.
- The formatting memory, per ADR 0001's decision, is part of the focus record, the committed focus object, and each focus taken, a silent re-take included, starts with none. A caret key's `CaretBinding` carries, from its press, the record it was bound to (its node and the trace the focus was taken from) and that record's memory by value. When the key speaks its unit it compares the formatting with that memory and updates it, then writes it back to the focus object only when that is still the same record. NVDA keeps the cache on the object (`speech/speech.py` 1503 to 1518, 1580 to 1583) and makes a new object for each focus event. Reason: a key run on its press-bound field (build 1, ruled 2026-11-16) compares formatting with that field's memory, as NVDA's script compares it with its own object's. The kind of line break stays with the focus's caret.
- Not built: writing the memory forward to later keys bound to the same record. A key that speaks has no caret key waiting behind it, since a later key withdraws its watch (build 1, Table 7, S4, column B), so the write could not happen.
- The details are a choice of set (build notes: "the property table's sets make the details a choice of set"), decided per request from Core's settings, the read's unit and its reason. A text read's `TextDetails` names the format attributes the settings ask for, one entry per attribute the theme switches (font name, font size, colour, background colour, font attributes, strikethrough, underline style, bullet style), beside spelling and grammar errors and links, in place of a single formatting flag. A node read, an MSAA read included, carries the property set the settings choose through the property table: description, keyboard shortcut, position in set, level, cell coordinates. The readers fetch exactly what a request names and nothing more, so a setting turned off leaves its property unread and its call unmade. Reason: the settings are Core's, and a copy of them in each outpost is a second representation of one fact.
- Control fields, when built, are compared by the provider's identity or by equality, never by an outpost node id. NVDA: `speech/speech.py` 1646 to 1657. Not built here.

## State table

### Table 1: a focus record's formatting memory

States: K, kept (the record is the committed focus, or a waiting key's binding carries it); G, gone.

Columns: A, a key bound to the record speaks its unit; B, a later focus is taken; C, its outpost is replaced; D, a silent re-take takes the focus.

| State | A | B | C | D |
| --- | --- | --- | --- | --- |
| K | compared with the binding's copy, which is updated, and written back to the focus object while it is the same record | the new record starts empty; a waiting key keeps its binding's copy | the re-take is a new record, so formatting is said afresh | the re-take starts empty |
| G | n/a | nothing | nothing | n/a |

## Decisions made outside Core, with their facts

- `TextSupport` and the classic fallback: guesses in the outpost that only cost calls, never a word.

The provider, the details and the memory are Core's.

## Waiting states, messages, timers and two-place lifecycles

Added: a copy of a focus record's memory in each caret key's binding, from its press until it runs: the record may be replaced meanwhile, as ADR 0001's decision has it.

Removed: `Fetches` in the outpost and the theme broadcast, a copy of Core's settings held in each outpost; the formatting memory on the focus object alone.

## Messages added and removed

- Every text request of the shared list (the caret read, the watch's final read, the sentence move, a line, word or character read, read ahead, a range, a point's location, `MoveCaret`, `Select`) names the provider and the details.
- Removed: `CoreToOutpost::Fetches`.

## What users hear

No sequence changes beyond the formatting memory's: a key bound to an earlier focus record compares formatting with that record's memory as it was at the press, and a record whose outpost was replaced says its formatting afresh. Pinned by, unchanged expectations: `spelling_errors`, `notepad_spelling_errors`, `theme_panel`, `notepad_editing`, `text_box_editing` and Core's formatting, spelling error and line break tests (`books/dev/src/reference/test-coverage.md`, "Spelling errors", "Document formatting" and "Line breaks").

## Tests

Hand-written, one per rule: every cell of Table 1 in Core; the provider chosen from each kind of focus answer; the details per unit and reason; the outpost reading the request's set with no `Fetches`; for each setting, that turning it off leaves its property unread, with a call-count test where the read it avoids is a call. Every existing test keeps its expected speech. Related scenarios for the parent: `spelling_errors`, `theme_panel`.

## How the code is organised

- `verbatim-core/src/text/`: `provider.rs` (the provider chosen from the focus answer), `details.rs` (details per request), `memory.rs` (Table 1).
- `verbatim-outpost`: the text reads take the provider and details from the request; `Fetches` goes from the outpost and the MSAA backend.
- `verbatim-app`: the theme broadcast and `fetches_sent` go.
- `verbatim-model/src/text.rs`: the provider and details fields on text requests; `Fetches` goes.

## Limits

- Control fields are not built here.
- Keys bound to an old focus record share its memory only through write-forward, which is not built: none speaks with another waiting.

## Performance figures

No added wake-up, message or call: the provider and details ride on requests already sent, and the theme broadcast to every outpost goes.
