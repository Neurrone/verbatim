# NVDA inventory: NVDAObjects, appModules, controlTypes

Scope: `nvda/source/NVDAObjects` (except `window/winConsole.py`, `IAccessible/winConsole.py`, `UIA/winConsoleUIA.py` and the terminal part of `behaviors.py`, lines 372 to 635: LiveText, Terminal, EnhancedTermTypedCharSupport, KeyboardHandlerBasedTypedCharSupport), every module in `nvda/source/appModules`, and `nvda/source/controlTypes`. Line counts are `wc -l`. Each claim was checked by reading or grepping the file named; where a fact could not be checked it is left out.

## What the harness already gives (the baseline for R1)

From `nvda-oracle/uia/boot.py`, `standins.py` and `run.py`:

- A real comtypes stand-in for `IUIAutomation6`: element property reads (current, cached, Ex), runtime id, `BuildUpdatedCache` with tree scope Element only, property, not, and, or and true conditions, raw, control and custom tree walkers (the BuildCache forms and `GetParentElement`), focused element, root element, `ElementFromHandleBuildCache`, `CompareElements`, the focus handler and event handler groups.
- Pattern requests answer only "not supported" (a world value of None). Any pattern object asked for is a miss.
- `winUser` replaced function by function: class name, thread and process ids, ancestor, foreground, desktop, window text, styles, control id, `isWindow`, visibility, hung, descendant, GUI thread info (active and focus only), parent. Every other `winUser` function and every `user32` ctypes function raises. `winKernel` raises except `waitForSingleObject`. `watchdog.cancellableSendMessage` answers WM_GETTEXT and WM_GETTEXTLENGTH only.
- `NVDAObjects.IAccessible` is replaced by an empty class shell, and `comInterfaces.IAccessible2Lib`, `comInterfaces.tom` and `comInterfaces.SpeechLib` are stubs. No MSAA or IA2 object, no JAB, no display model, no nvdaHelper functions, no application object model.
- No simulated clock: `queueHandler.pumpAll` and an MTA drain are run by hand; `time.time` is real.

## Replay classes

- R1 replays as-is with the existing stand-ins.
- R2 needs a named new stand-in.
- R3 needs a simulated-clock or thread-order seam.
- R4 depends on Windows reacting to NVDA's own actions (sent keys, `setFocus`, mouse clicks, cursor moves, default actions).
- R5 is native or in-process code (named).
- R6 is not needed for speech verification.

A module's primary class is the hardest one its speech needs; other classes it touches are listed beside it.

Stand-ins named below, defined once here:

- IA: an MSAA `IAccessible` stand-in (oleacc's interface) with `IAccessibleHandler`'s entry points that NVDAObjects call (`accessibleObjectFromEvent`, `accessibleObjectFromPoint`, `accessibleChildren`, `accNavigate`, `accFocus`, `accParent`, `accChild`, `accHitTest`, `getIAccIdentity`, `normalizeIAccessible`, `windowFromAccessibleObject`), and the real `IAccessible2Lib` comtypes module, which NVDA's build generates from the IDL in `nvda/include/ia2`.
- IA2: `IAccessible2`, `IAccessible2_2`, `IAccessibleText`, `IAccessibleHypertext`, `IAccessibleHyperlink`, `IAccessibleTable`, `IAccessibleTable2`, `IAccessibleTableCell`, `IAccessibleAction`, `IAccessibleRelation` objects.
- TP: `IUIAutomationTextPattern`, `IUIAutomationTextRange` (GetText, Move, MoveEndpointByUnit, ExpandToEnclosingUnit, Compare, GetAttributeValue, GetChildren, GetEnclosingElement, GetBoundingRectangles, RangeFromChild, RangeFromPoint, GetSelection, GetVisibleRanges).
- UP: UIA pattern objects other than text (Selection, Selection2, SelectionItem, Grid, TableItem, Invoke, Toggle, Value, RangeValue, LegacyIAccessible, TextEdit), children-scope caches (`GetCachedChildren`), and the plain `ElementFromHandle`, `GetFirstChildElement`, `ElementFromPointBuildCache` and `FindFirst`, which the harness lacks.
- WM: window messages through `watchdog.cancellableSendMessage` answered from a recording, together with the cross-process memory calls `winKernel.virtualAllocEx`, `readProcessMemory`, `writeProcessMemory`, `virtualFreeEx` and `oleacc.GetProcessHandleFromHwnd`.
- WIN: more window functions: `getWindow`, `getTopWindow`, `GetWindowRect`, the caret fields of GUI thread info, `IsWindowUnicode`, `WindowFromPoint`, `WindowFromPhysicalPoint`, `getKeyState`, `getAsyncKeyState`, `getCursorPos`, `SendMessageTimeout`, `GetMenu`, `GetMenuItemCount`, `GetMenuItemRect`, `GetComboBoxInfo`, `FindWindow`.
- OM(x): an application object model reached as late-bound `IDispatch` (`comtypes.client.dynamic.Dispatch`), through `oleacc.AccessibleObjectFromWindow(hwnd, OBJID_NATIVEOM)` or the running object table (`comHelper.getActiveObject`, which uses a helper subprocess, or `nvdaInProcUtils_getActiveObject` when an app module is passed).
- DM: the display model (`displayModel.DisplayModelTextInfo`, `requestTextChangeNotifications`), whose text comes from nvdaHelperRemote's GDI hooks injected into the application.
- INPROC(f): an `NVDAHelper.localLib` function that runs inside the application through the in-process helper (`helperLocalBindingHandle`).

## NVDAObjects

| Module | Lines | What the user hears that it decides | Outside Python | Class | Verbatim features |
|---|---|---|---|---|---|
| `__init__.py` | 1654 | The NVDAObject base: overlay class choice (`DynamicNVDAObjectType`), focus reporting, `reportFocus`, live region default (`event_liveRegionChange` speaks the name, assertive at NEXT priority), typed character echo (`event_typedCharacter`), lock screen confinement, flat review position, `basicText`, table navigation defaults | `winUser.getKeyState(VK_CAPITAL)` for the lowercase-with-Caps-Lock beep (a tone, not speech); `basicText` is re-read only after 0.5 s by `time.time()` | R3 (clock for `basicText`); R1 otherwise, already reached by `run.py` | Focus announcement, typed character echo, object navigation, review cursor |
| `behaviors.py` (in scope, 765 of 1029) | 765 | ProgressBar (percent spoken by interval, background bars by setting, cache keyed on screen position), Dialog (description from the dialog's static text), EditableTextBase (Enter and caret scripts, spelling error check after typing), InputFieldWithSuggestions, RowWithFakeNavigation and RowWithoutCellObjects (table cells of list rows), CandidateItem, ToolTip and Notification (spoken as focus on show and alert), FocusableUnfocusableContainer, WebDialog | `core.callLater(50)` for the spelling error check (a sound, not speech); `queueHandler.queueFunction` for progress speech; `self.location` and `isInForeground` | R3 (callLater, queue order); R4 for the caret scripts, which send the key and wait for the caret through `editableText` | Progress bars, help balloons, a dialog read once shown, caret keys, Enter in a text field, table cells, native controls' report views |
| `inputComposition.py` | 200 | IME composition, candidate list and candidate item speech | Driven by `NVDAHelper.handleInputComposition*` and `nvdaControllerInternal_inputCompositionUpdate`, `inputCandidateListUpdate`, called from nvdaHelperRemote's IME hooks in the application | R5 (nvdaHelperRemote IME monitoring); replayable by calling the recorded callbacks | None listed |
| `lockscreen.py` | 33 | Stops object navigation leaving the lock screen's application | None | R1 | The secure desktop and the lock state |
| `window/__init__.py` | 490 | Window base: name from window text, role, states from styles, parent and sibling walk by window order, foreground test, API class choice (UIA, MSAA, JAB), DisplayModelEditableText and DisplayModelLiveText | `winUser.getWindow`, `getTopWindow`, `getGUIThreadInfo` caret flags, `user32.GetWindowRect`, `IsWindowVisible`, `IsWindowUnicode`, `WindowFromPhysicalPoint`, `oleacc.GetProcessHandleFromHwnd`, `JABHandler.isJavaWindow`, EM_GETLINECOUNT | R2 (WIN, WM); R5 (DM) for the display model classes | Object navigation, focus ancestry, native controls |
| `window/_msOffice.py` | 16 | Hyperlink enum | None | R1 | None |
| `window/_msOfficeChart.py` | 1271 | Chart elements in Excel, Word and PowerPoint: series, points, axes, legend, trendlines | OM(Office chart) | R2 | None (no Office support in Verbatim's list) |
| `window/akelEdit.py` | 165 | AkelPad text | AkelEdit window messages, cross-process memory | R2 (WM) | Text and editing |
| `window/edit.py` | 1121 | Edit and RichEdit text by line, word, character, formatting (fonts, colours, alignment, links), embedded objects | EM_* and RichEdit messages with `virtualAllocEx`/`readProcessMemory`; TOM `ITextDocument` via `AccessibleObjectFromWindow(OBJID_NATIVEOM)` (`comInterfaces.tom`, stubbed today); INPROC `getOleClipboardText`, `getOleUserType` for embedded objects; `event_caret` calls `api.processPendingEvents` | R2 (WM, a TOM stand-in); R3 (pending events); R5 for embedded object labels | Caret keys, selecting with Shift, document formatting, protected fields, Notepad-style editing |
| `window/excel.py` | 2663 | Classic Excel: cell names, values, formulas, comments, merged cells, borders, selection, dropdowns, sheets, browse elements | OM(Excel) via OBJID_NATIVEOM; INPROC `nvdaInProcUtils_excel_getCellInfos`; `time.sleep` polling loops and `_WaitForValueChangeForAction`; `wx.CallLater(100)`; sends keys and waits for Excel's selection | R5; also R2 (OM), R3, R4 | Table cells (by analogy only) |
| `window/excelCellBorder.py` | 168 | Border description text from border values | None | R1 | None |
| `window/scintilla.py` | 339 | Scintilla text, caret, styles as formatting | SCI_* messages with cross-process memory | R2 (WM) | Text and editing |
| `window/winword.py` | 2239 | Classic Word: text, formatting, headings, tables, comments, revisions, line moves, browse elements | OM(Word) via OBJID_NATIVEOM; INPROC `nvdaInProcUtils_winword_getTextInRange`, `_expandToLine`, `_moveByLine`, `findWindowWithClassInThread`; `time.sleep` in waits; sends keys and waits for the value | R5; also R2 (OM), R3, R4 | None |
| `IAccessible/__init__.py` | 2917 | MSAA and IA2 base: name, role, states, value, description (and encoded descriptions), position info, relations (labelled by, controller for), group box labels, live regions, menu item locations, the `_staticMap` of window class and role to overlay class, IA2TextTextInfo | IA, IA2; WIN (`SendMessageTimeout` MN_GETHMENU, `GetMenu`, `GetMenuItemCount`, `GetMenuItemRect`, `WindowFromPoint`); INPROC `nvdaInProcUtils_getTextFromIAccessible` (annotation summaries); DM for ContentGenericClient | R2 (IA, IA2, WIN); R5 for ContentGenericClient and summaries | Focus announcement, menus, Alt+Tab switcher (TaskList), tray list, group boxes, help balloons, object navigation |
| `IAccessible/MSHTML.py` | 1271 | Internet Explorer engine content (HTML help, MSHTML hosts) | IA; `IServiceProvider.QueryService` to `IHTMLElement` and DOM over `IDispatch`; `api.processPendingEvents` | R2 (MSHTML DOM); R3 | None |
| `IAccessible/SysMonthCal32.py` | 40 | Month calendar name | IA | R2 (IA) | Native controls |
| `IAccessible/adobeAcrobat.py` | 354 | Acrobat nodes, math | IA; `AcrobatAccessLib` `IAccID`, `IGetPDDomNode`, `IPDDomElement` through `IServiceProvider` | R2 | None |
| `IAccessible/akelEdit.py` | 11 | Maps to window `akelEdit` | IA | R2 | Text and editing |
| `IAccessible/chromium.py` | 205 | Chromium document, combo box items, toggle buttons, presentational lists, editors | IA, IA2; `ChromeVBuf` is a native virtual buffer (browse mode) | R2; the virtual buffer is R5 and R6 for Verbatim, which has no browse mode | Focus announcement in Chromium applications |
| `IAccessible/delphi.py` | 37 | Delphi forms and rich views | IA | R2 | Native controls |
| `IAccessible/hh.py` | 54 | HTML Help keyword list | IA | R2 | None |
| `IAccessible/ia2TextMozilla.py` | 844 | Text in Firefox and Chromium editable content: caret, selection, lines through embedded objects | IA2 text, hypertext; INPROC `nvdaInProcUtils_IA2Text_findContentDescendant` for first, last and caret positions | R5; also R2 (IA2) | Caret keys and typed echo in web editors |
| `IAccessible/ia2Web.py` | 535 | Web roles (document, application, article, region, figure, switch, editor, math), annotations, details | IA2 attributes and relations; `ISimpleDOMNode` for MathML; INPROC summaries through annotations | R2 | Focus announcement, selection and controlled lists |
| `IAccessible/mozilla.py` | 299 | Gecko objects, documents, plugin windows, text leaves | IA, IA2 | R2 | Focus announcement |
| `IAccessible/msOffice.py` | 232 | Office SDM dialogs, command bars, ribbon, status bar | IA | R2 | None |
| `IAccessible/mscandui.py` | 302 | Microsoft IME candidate lists | IA; `winUser.setCursorPos` and `mouseHandler.doPrimaryClick`; `api.processPendingEvents` | R4; also R2, R3 | None |
| `IAccessible/qt.py` | 136 | Qt clients, containers, table rows and cells, tree items, menus | IA | R2 | Native controls |
| `IAccessible/scintilla.py` | 15 | Maps to window `scintilla` | IA | R2 | Text and editing |
| `IAccessible/sysListView32.py` | 840 | List view items: column content, headers, column order, locations, groups, item counts (position info) | IA; LVM_* messages with cross-process memory (out-of-process path); INPROC `nvdaInProcUtils_sysListView32_getColumnContent`, `getColumnHeader`, `getColumnOrderArray`, `getColumnLocation`, `getGroupInfo`, chosen whenever the app module has a binding handle | R5 (in-process path, the normal one); R2 (WM) for the fallback | Native controls (report views), File Explorer folder window, table cells |
| `IAccessible/sysTreeView32.py` | 335 | Tree view item states, levels, parent, child, sibling navigation, child counts | IA; TVM_* messages (`TVM_GETNEXTITEM`, `TVM_MAPACCIDTOHTREEITEM`, `TVM_MAPHTREEITEMTOACCID`, `TVM_GETITEMSTATE`) | R2 (WM) | Tree view navigation (SysTreeView32), native controls |
| `IAccessible/webKit.py` | 52 | WebKit nodes and documents | IA | R2 | None |
| `IAccessible/winword.py` | 591 | Word through MSAA: revisions, tables, bookmarks for headers, spelling errors, protected panes | IA; OM(Word); inherits window `winword` | R5 (inherited); R2, R4 | None |
| `IAccessible/wx.py` | 70 | wxWidgets web view | IA; `api.processPendingEvents` | R3; R2 | None |
| `JAB/__init__.py` | 805 | Java Access Bridge objects: roles, states, text, tables, relations, key bindings, actions | `JABHandler.JABContext` calls (`getAccessibleContextInfo`, `getAccessibleTextInfo`, `getAccessibleTextRange`, `getAccessibleChildFromContext`, table and relation calls), which call `windowsaccessbridge` through ctypes | R2 (a JABContext stand-in) | Focus announcement in Java applications |
| `UIA/__init__.py`, the UIA class and the rest (lines 1 to 91 and 1142 to 2950) | 1900 | UIA object: name, role, states, value, description, position info, overlay choice for Edge, Chromium, Word, Excel, Windows Forms, menus, tree items, list items, combo boxes, toasts, tooltips, suggestions, progress bars, live region dedupe | Element properties (the existing stand-in); UP for selection containers, tables, invoke and toggle; `user32.GetComboBoxInfo`; live region dedupe by `time.time()` within 0.5 s; INPROC `registerUIAProperty` for custom properties | R1 for focus (demonstrated by `run.py`); R2 (UP) for selection, tables and default action; R3 for live regions | Focus announcement, Settings app, File Explorer, UIA notifications, combo box opened, state changes, value changes and sliders, selection and controlled lists, progress bars, menus, activate |
| `UIA/__init__.py`, UIATextInfo (lines 92 to 1141) | 1050 | Text by unit, formatting attributes, annotations (spelling and grammar errors, comments, revisions), embedded children, caret and selection | TP | R2 (TP) | Text and editing, review cursor, caret keys, spelling errors, document formatting, say all |
| `UIA/VisualStudio.py` | 102 | IntelliSense items and live region, completion tooltips | `time.time()` dedupe | R3 | None |
| `UIA/anaheimEdge.py` | 14 | Edge Chromium alias | None | R1 | None |
| `UIA/chromium.py` | 89 | Chromium through UIA | TP | R2 | None |
| `UIA/excel.py` | 602 | Excel through UIA: cells, formulas, comments, notes, cell coordinates, overflow | Custom properties and annotation types (INPROC `registerUIAProperty`); TP (visible ranges) | R2; R5 for registration | None |
| `UIA/spartanEdge.py` | 402 | EdgeHTML content | TP | R2 | None |
| `UIA/sysListView32.py` | 119 | Windows Forms list views through UIA: item name from cells | UP (children-scope cache) | R2 | Native controls |
| `UIA/web.py` | 533 | UIA web content: landmarks, headings, lists, links | TP; UP (invoke) | R2 | None |
| `UIA/wordDocument.py` | 1202 | Word through UIA: sentence movement, line, page, section, column numbers, comments, revisions, headings collapsed | UIARemote remote operations (`msWord_textRange_moveBySentence`, `expandToEnclosingSentence`, `moveEndpointBySentence`, `msWord_getCustomAttributeValue`), which run in UIA core through nvdaHelperLocal; TP; OM(Word) for the view and selection | R5 (UIA remote operations); R2 | Moving by sentence (Alt+Up, Alt+Down) through UI Automation |

## controlTypes

All ten files (1640 lines: `role.py`, `state.py`, `processAndLabelStates.py`, `formatFields.py`, `isCurrent.py`, `descriptionFrom.py`, `outputReason.py`, `roleAndStateSpecialCases.py`, `deprecatedAliases.py`, `__init__.py`) are pure Python: the role and state names, which states are spoken positively or negatively (`processAndLabelStates`), current-item and description-source labels. No call leaves Python. R1; already reached by `run.py`. Bears on focus announcement, state changes and every spoken role.

## appModules

Grouped by what they need. Each is small unless its line count says otherwise.

### Pure decisions over properties and window classes (R1)

These decide by reading already-replayed properties, configuration and event order; the UIA ones need nothing new.

- `__init__.py` (73): the executable-to-module name map (for example `calculatorapp` to `calculator`, `searchhost` to `searchui`, `shellhost` to `shellexperiencehost`, `textinputhost` to the touch keyboard module).
- Sleep mode only: `cicero` (7), `doctts` (5), `dosvox` (5), `esysuite` (17), `obu` (13). NVDA says nothing in these applications.
- `csrss` (21): treats csrss as alive. `skype` (12), `whatsapp_root` (13): browse mode off. `zoom` (16): requests alert events from `zoom_acc_notify_wnd` windows.
- `1password` (21): drops name, item status and enabled property changes. `lockapphost` (15): silences the `LockAppHostFrameWindow` focus. `lockapp` (106): lock app container overlay and the safe-script input captor. `microsoftedge` (20): speaks Edge's UIA notifications when the focus is in Edge. `voiceaccess` (50): speaks Voice Access notifications from anywhere. `lync` (99): Skype for Business chat live regions. `taskmgr` (47): row children as content, SysListView32 kept on MSAA. `systemsettings` (30): no progress bar speech for the volume meters. `shellexperiencehost` (83): Action Center toggle status spoken on change, calendar day headers from names. `teamtalk4classic` (23), `webconferenceplugin` (19): no progress bar speech for named bars. `audacity` (19): strips ampersands from button names. `mplayerc` (14): a dialog as a status bar. `foobar2000` (210): elapsed, remaining and total time scripts from the status bar text.
- `windowsinternal_composableshell_experiences_textinput_inputapp` (448): emoji panel, clipboard history, IME candidates and the touch keyboard, by queuing `UIA_elementSelected`, `show` and `gainFocus` events on UIA children. Reads properties and children only.

Features: UIA notifications, the secure desktop and the lock state, Settings app, progress bars, shell staging windows, File Explorer.

### UIA calls the harness lacks (R2: UP)

- `notepad` (84): Windows 11 Notepad's status bar through `elementFromHandle`, a custom walker's `getFirstChildElement` and `productVersion`; drops the duplicate selection event. Features: Report status bar (Verbatim reads only terminal status rows today), focus announcement.
- `calculator` (190): speaks the result from the `DisplayUpdated` notification by walking to `CalculatorResults` through `elementFromHandle` and `getFirstChildElement`; result scripts send the key first (R4 on older versions); typed character echo forced on. Features: UIA notifications, typed character echo.
- `hxmail` (57): `NormalizeElement` and `FindFirst` with descendants scope to decide a read-only mail. `searchui` (98): Start search field description, Chromium objects, and a UIA window refusal. `msedgewebview2` (25) and `javaw` (53): the host application's name from the parent process or command line through `appModuleHandler.getWmiProcessInfo` (a WMI process stand-in). `wwahost` (43): the hosted application's name from `GetApplicationUserModelId` (a kernel32 stand-in).

### MSAA and IA2 applications (R2: IA, IA2)

- MSAA only: `aim` (8), `egui` (16), `instantbird` (25), `loudtalks` (42), `spotify` (42), `symphony` (21), `totalcmd` (49, speaks "left" or "right" panel), `itunes` (123), `mmc` (80), `msnmsgr` (104), `poedit` (297, scripts reading translator notes and warnings from child windows), `logonui` (129), `microsoftedgecp` (32), `dllhost` (22), `vipmud` (57, speaks new text appended to a read-only edit, from the window text), `notepadPlusPlus` (73, Scintilla text with WM).
- IA2 attributes: `teams` (33, ignores `menu` popups), `thunderbird` (58, popup menu items, document busy status), `code` (127, VS Code status bar found by IA2 id).
- `soffice` (653): LibreOffice text formatting from IA2 attributes, table cell selection through `IAccessibleTable2`, status bar from `IAccessibleText`, Backspace speech from `IAccessibleText`, formatting announcements gated by `time.time()` (R3).
- Window messages with cross-process memory (WM): `miranda32` (282), `winamp` (155, Winamp IPC messages and `FindWindow`).

### Display model applications (R5: DM)

`putty` (34), `mintty` (7), `securecrt` (31), `ttermpro` (27) (terminals through DisplayModelLiveText; the terminal behaviour is the other agent's), `mirc` (41), `fastlogentry` (25), `nlnotes` (60, also a Lotus Notes native virtual buffer and an object from point), `tween` (76), `utorrent` (74), `msimn` (165, Outlook Express unread state from the display model, list items with WM). The text these speak exists only as nvdaHelperRemote's captured GDI output.

### Depends on Windows reacting to NVDA (R4)

- `explorer` (551): mostly R1 decisions (start button, search box, tooltips, notification area, task switcher list items while Alt is held through `getAsyncKeyState`, duplicate list view focus, shell staging windows silenced, UIA notification filter, `isGoodUIAWindow`), but NotificationArea moves the mouse with `setCursorPos` and tooltips compare `mouseHandler.lastMouseEventTime` with `time.time()` (R3). Features: File Explorer folder window, shell staging windows, Alt+Tab switcher, tray list, help balloons, menus.
- `openwith` (54) and `bookshelf` (46): call `setFocus` on the focus target. `digitaleditions` (53): sends Page Down and compares the slider's range value. `azardi20` (44), `kindle` (464, also IA2 tables and hypertext): mouse clicks. `calc` (86): sends Enter and reads the display. `eclipse` (208): scripts send keys and read the documentation window. `wlmail` (74): sends Shift+Tab. `winword` (169): sends the key and polls Word's view, revision tracking or heading state through `_WaitForValueChangeForAction` (OM(Word), R3).

### Application object models (R2: OM, with R5 parts)

- `powerpnt` (1743): OM(PowerPoint) through OBJID_NATIVEOM on `paneClassDC` or the running object table (with `nvdaInProcUtils_getActiveObject` when `useRPC`), an `EApplication` connection-point sink (`SlideShowNextSlide`, `WindowSelectionChange`), and DM for some panes.
- `outlook` (791, R5): OM(Outlook) from `outlook.application`, appointments' dates through `winKernel.GetDateFormatEx` and `GetTimeFormatEx`, INPROC `nvdaInProcUtils_outlook_getMAPIProp` for a message's last verb (replied, forwarded), UIA grid rows, Word documents.
- `devenv` (250): Visual Studio's DTE from the running object table, per thread, for the text editor pane's text, caret and lines.
- `excel` (144, R3): Excel6 focus redirect polls with `time.sleep(0.05)`, closes a dropdown with `core.callLater(100)`, and edit fields through DM.

### Not needed (R6)

- `nvda` (257): NVDA's own dialogs, Python console and profile messages. The headless harness has no GUI, and Verbatim does not run NVDA's dialogs.

## The hardest items

- Word (window `winword.py`, IAccessible `winword.py`, UIA `wordDocument.py`, app modules `winword`, `outlook`, `hxmail`): the speech comes from in-process code walking Word's object model inside Word (`nvdaInProcUtils_winword_getTextInRange` returns formatted text; `_moveByLine` and `_expandToLine` decide line boundaries) and from UIA remote operations run in UIA core. A recorder would capture the in-process answers keyed by arguments, not the code.
- Excel (window `excel.py`, UIA `excel.py`, app module `excel`): INPROC `nvdaInProcUtils_excel_getCellInfos` plus OM(Excel), with sleeps and polling after sent keys.
- List views (`IAccessible/sysListView32.py`): the normal path for column text, headers and order is in-process; it is the only route to report-view speech that Verbatim's native controls and File Explorer features map to. The out-of-process WM fallback is a replayable alternative but is not the path NVDA takes when injected.
- Web editors (`ia2TextMozilla.py`): caret, first and last positions come from INPROC `nvdaInProcUtils_IA2Text_findContentDescendant`.
- The display model (window `DisplayModel*`, `ContentGenericClient`, nine app modules): text exists only as GDI output captured in the application.
- IME (`inputComposition.py`, `mscandui.py`): event source is nvdaHelperRemote; candidate selection clicks.
- The IA and IA2 stand-ins are the largest single gap: 9,140 lines of `IAccessible/*` plus every MSAA app module wait on them, and the harness stubs `IAccessible2Lib`.
- Everything that sends a key and waits (EditableText caret scripts in `editableText.py`, outside this area but used by `behaviors.py`; Word, Excel, PowerPoint, calc, eclipse, digitaleditions): the answer depends on the application's state after Windows delivers the key, so a recording must hold the state after each key.

App modules depending on application object models a recorder would have to capture: `powerpnt` (PowerPoint, plus its event sink), `outlook` (Outlook and MAPI), `winword` (Word), `excel` (Excel, through window `excel.py`), `devenv` (Visual Studio DTE), `hxmail` and `outlook` reading panes (Word, through `WordDocument`). Office charts (`_msOfficeChart.py`) reach the chart object model from Excel, Word and PowerPoint. Rich edit controls reach the TOM `ITextDocument` model (`window/edit.py`).

## Lines per class

Each module counted once, under its primary class.

| Class | NVDAObjects and controlTypes | appModules | Total |
|---|---|---|---|
| R1 | 3,771 | 1,386 | 5,157 |
| R2 | 13,479 | 4,318 | 17,797 |
| R3 | 2,591 | 797 | 3,388 |
| R4 | 302 | 1,749 | 2,051 |
| R5 | 8,579 | 1,331 | 9,910 |
| R6 | 0 | 257 | 257 |
| In scope | 28,722 | 9,838 | 38,560 |

Excluded and covered elsewhere: `window/winConsole.py` (118), `IAccessible/winConsole.py` (37), `UIA/winConsoleUIA.py` (512), and the terminal part of `behaviors.py` (264).
