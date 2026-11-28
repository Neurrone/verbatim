# NVDA text, editing, review, terminals and documents: replay inventory

Scope: `nvda/source` text modules, read at the pinned submodule on 2026-11-28. Line counts are `wc -l`. Every claim below was checked by reading the file named; anything unverified was left out.

Replay classes, as the brief defines them:

- R1: runs as-is under the existing harness (boot.py's stubs, standins.py's UIA tree and window stand-ins).
- R2: needs a named new stand-in.
- R3: needs a clock or thread seam.
- R4: needs Windows, meaning the application, to react to what NVDA does.
- R5: native or in-process code.
- R6: not needed, with the reason given.

A module often has parts in several classes. The class given for each row is the one that blocks it. The others are noted.

## Cross-cutting seams this area needs (none exist in the harness yet)

Checked in `experiments/nvda-oracle/uia/`: standins.py has no TextPattern or TextRange stand-in, `world_explorer.py` sets `IsTextPatternAvailable` False, and boot.py stubs `comInterfaces.tom` and `wx`.

- Clock (R3). `time.time` and `time.sleep` are used directly in the following places:
  - `editableText.py`, lines 89, 150 and 160: the caret watch.
  - `behaviors.py`, line 481: LiveText's `STABILIZE_DELAY`.
  - `speech/speech.py`, lines 1427 and 1477: the 0.1 s window for suppressing typed characters.
  - `browseMode.py`, lines 1975 and 2376: the 0.4 s window after a programmatic scroll.
  - `window/excel.py`, lines 1213 to 1241 and 1254 to 1262, and `window/winword.py`, lines 1746 to 1754 and 1864: waits for a value to change.

  `core.callLater` (`core.py`, 1206 to 1227) goes through `wx.CallLater`, which is stubbed, so every delayed call is lost today. The ones in this area:
  - The spelling-error check, 50 ms (`behaviors.py` 301).
  - Browse mode's expand or collapse, 100 ms (`browseMode.py` 2049).
  - The elements list's move, 100 ms (`browseMode.py` 1806).
  - Find, from `cursorManager.py` 158.

  The seam has to replace time.time, time.sleep, wx.CallLater, wx.CallAfter and wx.Yield with one simulated clock.
- Event delivery during polls (R3). `api.processPendingEvents` (`api.py`, 381 to 396) is called inside the caret watch and the Excel selection wait. It does four things:
  - calls `wx.Yield`;
  - pumps IAccessible winEvents;
  - invalidates every AutoPropertyObject cache;
  - optionally flushes the event queue.

  `eventHandler.isPendingEvents("gainFocus" | "caret" | "textChange")` and `scriptHandler.isScriptWaiting()` decide the outcome. The recorded world must therefore release each event and key onto NVDA's queues when its simulated time is reached, inside these polls.
- The key reaching the application (R4). Every caret, selection, Delete, Backspace and Enter script calls `gesture.send()` and then polls for the application's answer. Replay needs the world to apply the recorded effect of that key at the recorded delay after the send: caret moved, text changed, events raised. The order of "send, then world change, then events" is what the poll observes.
- Text models (R2). There are four text models, by interface:

  | Interface | Used by | Calls that must be answered |
  | --- | --- | --- |
  | UI Automation TextPattern and TextRange | `UIATextInfo`, `NVDAObjects/UIA/__init__.py` 92 to 1141, about 1,050 lines | Clone, CompareEndpoints, MoveEndpointByRange, MoveEndpointByUnit, Move, ExpandToEnclosingUnit, GetText, GetAttributeValue (including the mixed-attribute sentinel), GetChildren and GetEnclosingElement with cache requests (565 to 572, 755 to 771; `UIAHandler/utils.py` 174 to 218), GetSelection, GetVisibleRanges, RangeFromPoint, GetBoundingRectangles and Select |
  | IAccessibleText | `IA2TextTextInfo`, `NVDAObjects/IAccessible/__init__.py` from 233 | IAccessibleText calls; `IAccessible2Lib` is stubbed today |
  | Win32 edit window messages | `EditTextInfo`, `window/edit.py` 166 to 668 | EM_GETSEL, EM_EXGETSEL, EM_LINEINDEX, EM_POSFROMCHAR, EM_CHARFROMPOS, WM_GETTEXT and the others, all through `watchdog.cancellableSendMessage`, which calls `NVDAHelper.localLib.cancellableSendMessageTimeout`. A stand-in is needed for that local call. |
  | ITextDocument (TOM) | `ITextDocumentTextInfo`, `window/edit.py` 669 to end | TOM calls; `comInterfaces.tom` is stubbed today |

  Unit answers are provider-specific: the console host before the FORMATTED API level is end-inclusive, Windows Terminal's paragraph spans the buffer, and so on. A world should therefore record each call's answer, or model each real provider's rules. It must not model what NVDA expects.
- Watchdog. `watchdog.cancellableExecute` runs the call directly when the watchdog is not running (`watchdog.py` 417 to 420), so it is R1 in the harness.

## Module inventory

| Module (lines) | What it decides that users hear | Needs from outside Python | Class | Verbatim features |
| --- | --- | --- | --- | --- |
| `textInfos/__init__.py` (1091) | Field model (ControlField and FormatField, presentation category at 71 to 244), unit names, `getTextInChunks`, codepoint movement (797 to 987), link data at the caret (740 to 785), the clipboard text, and `getMathMl` | Nothing native; the subclasses supply the text | R1 | Every text feature; Document formatting; Line breaks |
| `textInfos/offsets.py` (952) | Line, word, character, sentence, paragraph and reading-chunk offsets over flat text; move, find, expand, compare | Word segmentation: `WordSegmenter` defaults to AUTO (configSpec 292). That uses ICU from Windows' own `icu.dll` (`winBindings/icu.py`), cppjieba for Chinese text, and Uniscribe as the fallback. Character segmentation defaults to UNISCRIBE (`offsets.py` 211), through `NVDAHelper.localLib.calculateCharacterOffsets` (451 to 453). Sentences use ICU (578). | R5 for Uniscribe, because nvdaHelperLocal is not built (`nvdaHelper/local/textUtils.cpp`, 167 lines, ScriptItemize and ScriptBreak). A small ctypes port over usp10.dll could replace it as an R2 stand-in. ICU runs as-is (R1), but its answers depend on the Windows build. cppjieba is R5, needed only for Chinese text. | Word and character segmentation; Review word and character commands; Caret keys; Moving by sentence |
| `textUtils/` (`__init__` 584, `icu` 209, `uniscribe` 39, `segFlag` 30, `_wordSeg` 648) | Offset converters between UTF-16 and str; `splitAtCharacterBoundaries` (`uniscribe.py`, using `calculateCharacterBoundaries`), which spelling uses (`speech/speech.py` 498 and 531) | Same native helpers as above | R5 or R2, as above | Spelling, capitals and character descriptions |
| `editableText.py` (514) | The caret watch `_hasCaretMoved` (70 to 163) and its outcomes: moved by bookmark; moved by event within `_useEvents_maxTimeoutSec`, 60 ms; for Delete, the word changed after 30 ms; a timeout after `caretMoveTimeoutMs` (default 100) times `_caretMovementTimeoutMultiplier`; or cut short by a key or a focus. Also what each caret key speaks (165 to 192), Enter's new-line text (208 to 238), sentence movement done by NVDA itself (240 to 265), Backspace and Delete (293 to 348), paragraph style (350 to 385), and the selection anchor and selection-change speech (417 to 514). | The clock; key delivery to the application; event polling; a text model | R3 and R4, with a text model (R2) | Caret keys; Holding caret keys; Backspace and Delete; Enter in a text field; Moving by sentence; Selecting with Shift; Editing chords |
| `NVDAObjects/behaviors.py`, EditableText part (239 to 370) | Enter bound only outside NVDA's own process. The spelling-error sound `textError.wav` plays when a typed character ends a word that is marked invalid-spelling, checked 50 ms later (260 to 314). Auto-select detection on caret events; text changes flag a "generalized" selection message. | `core.callLater`; nvwave for the sound | R3 | Typed character echo; Selecting with Shift; Spelling errors. No parity entry was found for the typing-time error sound (searched `docs/parity` for textError and alertForSpellingErrors). |
| `behaviors.py`, LiveText (372 to 509) | A monitor thread, started on focus. It waits on a `threading.Event`, sleeps `STABILIZE_DELAY`, reads `POSITION_ALL`, diffs, drops a single changed character, and queues the speech to the event queue with `_immediate`. | A real thread (400 to 407); the text model; `diffHandler` | R3, for deterministic monitor steps: run one monitor step at a chosen point after each textChange, not on a free thread | Terminals, verified against Verbatim's own model (row-by-row reading, `docs/parity/terminals.md`, "Terminal output read row by row"). It is kept for the typing parts it feeds. |
| `behaviors.py`, Terminal, EnhancedTermTypedCharSupport and KeyboardHandlerBasedTypedCharSupport (511 to 635) | Terminals never use caret events for the caret watch (527 to 530). Typed characters are queued until the next textChange, unless passwords are spoken. A short single new line is filtered as typed. Enter, Tab, Control+C, Control+D and Control+Pause flush the queue (595 to 619). | Same as LiveText | R3 | Typing in terminals. That entry differs from NVDA (rule 20), but the cases listed under "Matched" in `terminals.md` "Typing" are matched and still need NVDA. "Line keys and lists a program draws" is matched (verified). |
| `diffHandler.py` (135) | The character diff (DMP) keeps only inserted text, split into non-blank lines. Difflib is the line diff. | The `fast_diff_match_patch` C-extension package. The `python` on PATH lacks it (checked). | R1 once that package is installed | New output and floods, which differ from NVDA |
| `NVDAObjects/UIA/winConsoleUIA.py` (512) | The console host's text area overlay. Its TextInfo depends on the API level (390 to 401); the review stays within the visible text (`ConsoleUIATextInfo`, 39 to 128); word movement uses Uniscribe (`calculateWordOffsets`, 320); the end-inclusive workarounds are at 130 to 353. Caret multiplier 1 or 1.5 (374 to 376); Windows Terminal multiplier 3 (470, 486). Notifications are blocked (431 to 440, 474 to 476). The notifications-based Windows Terminal path sits behind `wtStrategy`, whose default is diffing (configSpec 320). | A UIA TextPattern stand-in for the console host and Windows Terminal, including the API-level probe (`UIAHandler/utils.py` 351, using GetVisibleRanges and GetAttributeValue for the font name); `consoleWindowsToThreadIDs` from the winEvent handler; Uniscribe | R2 (with R5 for Uniscribe) | Review cursor in terminals; Console windows; Windows and tabs (matched, verified); Line keys (matched); "The review cursor while output is written" (matched, 2026-11-28). `_NotificationsBasedWinTerminalUIA` is R6: it is off by default and Verbatim ignores the notifications. |
| `NVDAObjects/window/winConsole.py` (118) and `winConsoleHandler.py` (333) | The legacy console: console APIs, console winEvents, a dead-console timer, typed characters from EVENT_CONSOLE_UPDATE_SIMPLE | Console APIs, SetWinEventHook, `gui.NonReEntrantTimer` | R6. NVDA's legacy console is not ported or tested (Dickson, 2026-10-08, `terminals.md`), and every supported Windows has the UIA console. | none |
| `review.py` (190) | Review modes (object, document, screen) and their order. `handleCaretMove` moves the review cursor when "follow caret" is on. | `DisplayModelTextInfo` for screen review | R1, except that the screen mode needs the display model (R5) | Follow caret toggle; every review cursor row; Caret location and Review location |
| `globalCommands.py` review scripts (1862 to 2482, 2649 to 2661, 4139 to 4190) | Each review command's speech, edge messages, and two- and three-press behaviour | Text models | R1 with a text model. These scripts are in `globalCommands.py`; they are listed here so they are not overlooked. | Review cursor table; Say all from review; Set start marker; Select then copy |
| `speech/speech.py` typing and spelling parts (`speakSpelling` and `getSpellingSpeech` 328 to 675; `speakTypedCharacters` 1420 to 1492; `clearTypedWordBuffer` 3179) | The typed word ends on a character outside L, M and N; Backspace pops the word; a protected field gives "*"; the echo mode is Always or Edit controls (via `isFocusEditable`); the 0.1 s suppression | The clock (1427, 1477) | R3 | Typed character echo; Typed word echo; Typed characters (read-only); Protected fields; Speak typed characters and words toggles |
| `keyboardHandler.py` typed-character paths (`shouldUseToUnicodeEx` 178 to 200; the ToUnicodeEx block in `internal_keyDownEvent`, 320 to 356) | Which keys become typedCharacter events: ToUnicodeEx with flag 0x4 for UWP windows, for windows with no in-process helper, and for `KeyboardHandlerBasedTypedCharSupport` consoles. Otherwise the in-process WH_GETMESSAGE hook (`nvdaHelper/remote/typedCharacter.cpp`, 51 lines) sends WM_CHAR through `nvdaControllerInternal_typedCharacterNotify` (`NVDAHelper/__init__.py` 672 to 676), skipping ConsoleWindowClass. Caps Lock beep: `NVDAObjects/__init__.py` 1256 to 1265. | user32 GetKeyState, GetKeyboardLayout and ToUnicodeEx; the in-process hook | ToUnicodeEx is R2: record each key's characters, or call it against a pinned layout. The in-process WM_CHAR path is R5, best replaced by recording which characters the application received. | Typed character echo; Typing in terminals |
| `cursorManager.py` (774) | NVDA-driven caret movement in documents (a virtual caret): every unit, selection, find, and copy; `ReviewCursorManager` | `FindDialog` (43 to 170) is wx GUI; find runs through `core.callLater` | R1 for movement. Find needs R3, and the dialog itself is R6 because only its submitted text matters. | Not built yet: browse mode (M7) |
| `documentBase.py` (607) | Table navigation (next and previous row and column, first and last, say all of a row or column, layout tables) | Text fields carrying table ids and coordinates | R1 with a text model | Table cells (focus side); browse-mode tables (M7) |
| `documentNavigation/` (`paragraphHelper` 301, `sentenceHelper` 32) | The single and multiple line-break paragraph styles; the default is "application" (configSpec 293) | none | R1 | Caret keys (paragraph); Moving by sentence |
| `treeInterceptorHandler.py` (309) | Which tree interceptor owns an object; `RootProxyTextInfo` | none | R1 | M7 |
| `browseMode.py` (3012) | Quick navigation items (114 to 298). `BrowseModeTreeInterceptor` (299 to 1453): focus and browse mode, `shouldPassThrough` (366 onward) including automatic switching on focus and caret, the mode sounds and messages (63 to 75), and activation by mouse click via `setCursorPos` (709 to 712). The elements list (1454 to 1808, wx). `BrowseModeDocumentTreeInterceptor` (1809 to 3012): caret, focus and scroll handling, the 0.4 s scroll window, and say all. | Clock; `core.callLater`; the mouse; the application scrolling and focusing in response | R3 and R4, with R5 underneath (the virtual buffer text) | M7, browse mode |
| `virtualBuffers/` Python (`__init__` 922, `gecko_ia2` 809, `MSHTML` 550, `adobeAcrobat` 175, `webKit` 151, `lotusNotes` 127) | Buffer text, fields, lines, quick-nav searches, and the buffer's readiness | Every query is `NVDAHelper.localLib.VBuf_*` (the native backends belong to another agent). Loading runs on its own thread (559 to 590). | R5, plus R3 for the load thread | M7 |
| `displayModel.py` (790) and native (`nvdaHelper/remote/displayModel.cpp` 429, `displayModel.h` 203, `displayModelRemote.cpp` 136, `gdiHooks.cpp` 1274) | Screen review text and fields, the caret in display-model edits (found from the caret rectangle, set by moving the mouse cursor, 704 to 778), right-to-left normalization, and text-change notifications | GDI hooks inside the application, reached through `localLib.displayModel_*` | R5. The Python side would be R2 with a recorded `getWindowTextInRect` answer, the command list plus rectangles. | Not built: M17 (gated); screen review in M7 |
| `NVDAObjects/IAccessible/winword.py` (591) and `NVDAObjects/window/winword.py` (2239) | Word text and formatting. Lines come from `nvdaInProcUtils_winword_expandToLine` (998) and text with fields from `nvdaInProcUtils_winword_getTextInRange` (1079), which runs in Word's process (`nvdaHelper/remote/winword.cpp`, 1462 lines). Also moving by line (1397), quick navigation over Word's collections, and the value-change waits for formatting keys (1746 onward). `ProtectedDocumentPane` uses AttachThreadInput. | Word's object model through IDispatch (OBJID_NATIVEOM, 1707 to 1725); the in-process helper; the clock | R5 for the in-process text. The object-model parts are R2 (a Word object-model stand-in), with R3 for the waits. | Not built: M13 and Tier D |
| `NVDAObjects/UIA/wordDocument.py` (1202) | Word over UI Automation: TextInfo quirks, comments, revisions, headings, and line and page numbers through custom attributes (873 to 884). Sentence movement goes through `UIAHandler.remote` (624 to 660). | UIA TextPattern stand-in; remote operations. `UIAHandler/_remoteOps` has a Python `LocalExecutor` (`operation.py` 171 to 174, `localMode`), so these can run against the stand-in instead of the native `UIARemote`. | R2. `allowInMSWord` defaults to 0 (configSpec 309), meaning NVDA chooses. | M13 |
| `NVDAObjects/window/excel.py` (2663) and `NVDAObjects/UIA/excel.py` (602) | Cell text, cell states, formulas and comments; the selection-change wait (1205 to 1250); quick navigation | Excel's object model; `nvdaInProcUtils_excel_getCellInfos` (1577, 1620; `nvdaHelper/remote/excel.cpp`, 513 lines); the clock | The object-model parts are R2, the cell infos R5, and the wait R3. UIA in Excel is off by default (`useInMSExcelWhenAvailable=false`). | M13 |
| `mathPres/` (`__init__` 262, `_mathMlNode` 29, MathCAT 1696) | Speech for maths in text (`speech.py` 1524 to 1537) and interaction | `libmathcat_py`, native Rust | The hooks are R1. MathCAT itself is R5 and R6 for now: Verbatim's own maths is M14 and not scheduled, and the candidate library is the same MathCAT. | M14 |
| `speech/sayAll.py` (486) | Say all by line or sentence, reading ahead up to 10 lines, page turns | Speech-progress callbacks (`CallbackCommand`, 203 and 333), which fire when the synthesizer reaches an index | R3 (the order of synthesizer index callbacks) | Say all from caret; Say all from review; Say all settings |

## Terminal areas: NVDA versus Verbatim's own model

The following are verified against Verbatim's reference model, not NVDA: LiveText's whole-document diff, the flood policy, redrawn rows, the alternate screen, the single-character drop, and the typed-line filter.

Still verified against NVDA:

- the terminal caret watch, with its waits and the rule that the terminal's caret events are not used;
- the typing cases listed under "Matched" in `terminals.md` "Typing";
- notifications being blocked;
- switching windows and tabs;
- the review cursor following the caret;
- review over `ConsoleUIATextInfo` and `_WinTerminalUIATextInfo`.

These need the R2 console and Windows Terminal text models and the R3 clock, but not LiveText's thread.

## Hardest items

1. The caret watch, together with the key reaching the application (R3 with R4). The world has to react to the sent key at its recorded delay. Polls interleave time, event queues, cache invalidation and the next key waiting. Almost every editing feature goes through it, as do Excel's and Word's waits.
2. A faithful UIA TextPattern model (R2). It must cover units for each provider, the field walk through children and enclosing elements, attributes including mixed values, and the console host's API-level quirks. It is used by text fields, terminals and UIA Word.
3. LiveText's monitor thread (R3). Which snapshot it reads, and when, decides the diff. Replay needs one monitor step at a chosen point after each textChange.
4. In-process text (R5): Word's `winword.cpp`, Excel's cell infos, the virtual-buffer backends and the display model's GDI hooks. Each needs its own recorded answers or a native build.
5. Segmentation (R5 to R2). Character offsets default to Uniscribe through the unbuilt nvdaHelperLocal. The port is small: 167 lines over usp10.dll.

## Lines per class (approximate, by the blocking class)

| Class | Python lines | What is in it |
| --- | --- | --- |
| R1 | about 4,200 | `textInfos/__init__.py`, `documentBase.py`, `documentNavigation/`, `review.py`, `treeInterceptorHandler.py`, `diffHandler.py`, most of `textUtils/`, the mathPres hooks, and `offsets.py` apart from segmentation |
| R2 | about 6,000 | `UIATextInfo`, `winConsoleUIA.py`, `UIA/wordDocument.py`, `UIA/excel.py`, `window/edit.py`, `IA2TextTextInfo`, the object-model parts of Word and Excel, and the ToUnicodeEx path |
| R3 | about 1,500 | `editableText.py`, the EditableText and LiveText parts of `behaviors.py`, `sayAll.py`, speech typing echo, and browse mode's timers |
| R4 | none on its own | The key reaching the application inside every R3 caret script, browse-mode activation, and `updateCaret` and `updateSelection` |
| R5 | about 2,700 Python and about 4,200 native | Python: the virtualBuffers modules. Native: the display model (about 2,040), `winword.cpp` (1,462), `excel.cpp` (513) and `textUtils.cpp` (167). The in-process parts of `window/winword.py` and `window/excel.py` are counted under R2. MathCAT (1,696 Python) is counted under R6. |
| R6 | about 2,150 | `winConsole.py` and `winConsoleHandler.py` (451), the wx dialogs (about 485), and MathCAT for now (1,696) |
