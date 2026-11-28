# NVDA core, input and Windows-binding modules: replay inventory

Scope: the modules in `nvda/source` named in the brief (core, events, queue, api, watchdog, keyboard, mouse, touch, input core, scripts, global commands, global plugins, app module handler, baseObject, winUser, winKernel, winBindings, NVDAHelper's Python side, config), and every top-level module or package no other agent covers. Line counts are `wc -l` of the pinned source. Braille hardware modules (bdDetect, brailleTables, brailleInput, brailleViewer, louisHelper, ftdi2, hwIo, hwPortUtils, tactile; 5,855 lines), `wasapi.py` and `_bridge` (the 32-bit SAPI bridge, used only by `synthDrivers/sapi4_32.py` and `sapi5_32.py`) are left to the braille and speech agents and are not counted here.

Replay classes: R1 replays as-is with the existing stand-ins; R2 needs a named new stand-in; R3 needs a simulated-clock or thread-order seam; R4 depends on Windows reacting to NVDA's own actions; R5 native or in-process code; R6 not needed for speech verification.

What the harness already replaces (from `boot.py`, `standins.py`, `run.py`): wx, gui and braille output are stub modules whose every attribute is a MagicMock; every function defined in `winUser` and `winKernel` is replaced, by a stand-in from the window world for 14 winUser functions (the stand-in table also names `getParent`, which winUser does not define) and `waitForSingleObject`, and by a raiser otherwise; every ctypes function in `winBindings.user32` is a raiser except five about NVDA's own desktop, which pass through; `UiaHasServerSideProvider` and `watchdog.cancellableSendMessage` (WM_GETTEXT and WM_GETTEXTLENGTH only) are stand-ins; `appModuleHandler.getAppNameFromProcessID` and `getProcessHandleFromProcessId` come from the world's process table; `UIAHandler.CoCreateInstance` returns the stand-in IUIAutomation; enhanced UIA event processing is off; `core_cycle` runs `queueHandler.pumpAll`, drains the UIA MTA thread, pumps again and invalidates property caches. `watchdog.initialize`, `keyboardHandler.initialize`, `mouseHandler.initialize`, `NVDAHelper.initialize` and `core.main` are never run, and `NVDAState._TrackNVDAInitialization.markInitializationComplete` is never called, so the lock state always reads as unlocked.

## Gaps in the harness found while reading

These are defects in the existing harness, not new stand-ins, and each silently changes speech:

1. `core.callLater` goes through `wx.CallLater` (core.py 1206 to 1221). With wx stubbed, `wx.GetApp()` is a truthy MagicMock and `wx.CallLater` a MagicMock, so every delayed call is dropped without an error. Speech lost this way: the lock key report 30 ms after Caps Lock, Num Lock or Scroll Lock (keyboardHandler 740 to 752), the virtual desktop name 250 ms after a `#32769` name change (eventHandler 347), `ui.delayedMessage`, and the other agents' timers (IAccessibleHandler's 50 ms fake focus and 200 ms user-desktop check, `NVDAObjects/behaviors.py` 50 ms detection, `NVDAObjects/window/winConsole.py` 200 ms reconnect, `appModules/excel.py`, `browseMode.py`, `cursorManager.py`). `mouseHandler` uses `gui.NonReEntrantTimer` for the cursor-shape report, also a MagicMock.
2. `gui.blockAction.when` is used as a decorator on 50 global commands (27 MODAL_DIALOG_OPEN, 9 BRAILLE_MODE_SPEECH_OUTPUT, 8 SECURE_MODE, 8 REMOTE_ACCESS_DISABLED by count of context names) and on `inputCore.GlobalGestureMap.save`. With gui stubbed, each such script is replaced by a MagicMock and does nothing, and the eight `gui.blockAction.Context.WINDOWS_LOCKED.translatedMessage` messages become MagicMocks that `run.py`'s speak capture drops as non-strings. `gui/blockAction.py` imports only config, globalVars, ui and utils.security at module level and `gui.message` lazily, so the real module can be loaded with a stand-in for `gui.message.isModalMessageBoxActive`.
3. The harness replaces every function in `winUser` whose `__module__` is winUser, including pure arithmetic: `LOBYTE`, `HIBYTE`, `MAKEWORD`, `LOWORD`, `HIWORD`, `GET_X_LPARAM`, `GET_Y_LPARAM`, `MAKELONG`. These are called by `NVDAHelper.nvdaControllerInternal_inputLangChangeNotify`, `winConsoleHandler.consoleWinEventHook` and `touchHandler`, and must pass through.
4. `eventHandler.shouldAcceptEvent` calls `winBindings.user32.GetWindowLong(hwnd, GWL_EXSTYLE)` directly (eventHandler 557 to 562), bypassing `winUser.getExtendedWindowStyle`; under the harness it raises for any event from a window that is not in the foreground's tree or root owner. It needs a user32 stand-in reading the world's `exstyle`.
5. `core_cycle` runs only the queue. NVDA's pump (core.py 1015 to 1048) runs, in order: touch handler, `JABHandler.pumpAll`, `IAccessibleHandler.pumpAll`, `queueHandler.pumpAll`, `mouseHandler.pumpAll`, `braille.pumpAll`, `vision.pumpAll`, `winAPI.sessionTracking.pumpAll`, then `AutoPropertyObject.invalidateCaches`. Any MSAA, mouse or lock-state replay needs the full order.

## Module inventory

### core.py (1,227 lines)

- Pump (`CorePump`, `requestPump`, `_PumpPending`; lines 63 to 70, 984 to 1052, 1180 to 1199). Decides when queued work runs: an immediate pump (focus events, input) runs as soon as possible and cancels a pending delayed one; a delayed pump waits `PUMP_MAX_DELAY` = 10 ms so events arriving together share one cycle; a pump requested during a pump re-queues itself. Because property caches are cleared once per cycle, which events share a cycle decides whether a property is read once or again. Needs wx.Timer, wx.CallAfter, the main thread id. R3: seam at `requestPump` and `CorePump.Notify`, driven by the simulated clock.
- `callLater` and `_callLaterExec` (1206 to 1227): a delayed call is a wx.CallLater whose callback queues the function on the event queue. R3: a simulated-clock timer list that queues through `queueHandler.queueFunction` when due.
- `_initializeObjectCaches` (592 to 613): the desktop window object becomes desktop, foreground, focus, navigator and mouse object. `_setInitialFocus` (422 to 435): if no focus was queued during start-up, queues gainFocus for `desktop.objectWithFocus()`. `_doLoseFocus` (616 to 625). R1. Bears on the startup announcement.
- `main` start-up and shutdown, restart, reset configuration, startup dialogs, wx app, message window, `_closeAllWindows` (the rest). The start sound, "Loading NVDA. Please wait..." spoken when start-up exceeds 5 seconds (809 to 813), and the order modules start in. R6: the harness's boot replaces start-up; the order it reproduces is listed above.

### eventHandler.py (599 lines)

- `queueEvent`, `_queueEventCallback`, `isPendingEvents` (48 to 106): counts pending events by name, object and both under an RLock, sets `lastQueuedFocusObject` for gainFocus, queues with immediate pump for gainFocus only. `isPendingEvents` answers from what is queued at the moment of asking, so its answer depends on when other threads enqueue. R1 for the logic; the enqueue time is the R3 seam under queueHandler.
- `_EventExecuter` (109 to 166): the chain global plugins, app module, tree interceptor (only when ready or the handler has `ignoreIsReady`), object. R1.
- `FocusLossCancellableSpeechCommand` and `_getFocusLossCancellableSpeechCommand` (186 to 309): a focus utterance is dropped once its object is no longer the focus, an ancestor of it, the foreground, or the menu-item case for IAccessible menus. R1 (reads api state only).
- `executeEvent`, `doPreGainFocus`, `doPreDocumentLoadComplete` (312 to 442): focus redirect, sleep mode, lock-screen filter, `api.setFocusObject`, removal of cancelled speech, foreground change when the focus difference level is 1 or less (`desktop.objectInForeground()`, falling back to the second focus ancestor), a `foreground` event, focusEntered for each new ancestor, tree-interceptor gain and loss. The virtual desktop name is held and spoken 250 ms after a `#32769` nameChange, or at the next gain focus, whichever first. R1, with the 250 ms timer under the core.callLater seam (R3).
- `requestEvents`, `handleAppTerminate`, `shouldAcceptEvent` (445 to 599): drops hide events, show events except six window classes, background events unless requested; accepts toasts (`alert` under `ToastChildWindowClass`), menuEnd, switchEnd, desktopSwitch, desktop-window events, `Windows.UI.Core` windows under the input thread's active window, windows sharing the foreground's root owner or under it, topmost windows or roots, Chromium documents reachable from the focused Chrome window in the UIA tree; accepts background valueChange when background progress bars are on. Needs winUser class name, process id, ancestors, GUI thread info, foreground, desktop, `user32.GetWindowLong`, and UIA `ElementFromHandle`, `CreatePropertyCondition`, `CreateTreeWalker`, `NormalizeElement`. R2: user32 `GetWindowLong` and the UIA tree walker on the stand-in automation object.

Bears on: Focus announcement, Focus ancestry and window on switching, Duplicate and ordered focus events, Menus, Toasts, A value change accepted by the background setting, The desktop's return and the virtual desktop's name, Alt+Tab switcher, The secure desktop and the lock state.

### queueHandler.py (102 lines)

One `SimpleQueue` (`eventQueue`) of function calls, put from any thread, run on the main thread; `flushQueue` runs only the items present when it starts (`qsize() + 1` iterations), so items queued during the flush wait for the next pump; registered generators advance one step per pump before the queue. No caller in the source registers a generator. R3: this queue is the thread-order seam. Every producer (UIA MTA thread, keyboard hook thread, NVDAHelper RPC threads, IAccessible's win-event thread through its own pump) must enqueue at a recorded point relative to pump cycles, and `isPendingEvents` reads its counts at that point.

### api.py (562 lines)

- Focus, foreground, navigator, mouse, desktop and review state (`setFocusObject` 80 to 194 and the setters and getters around it): walks the new focus's ancestors until it meets the old focus line, sets the focus difference level, runs app-module cleanup, tree-interceptor update, the browse-mode state notification, `appModuleHandler.handleAppSwitch` (profile triggers), navigator follow-focus, focusExited for the old ancestors. R1.
- `setNavigatorObject` (317 to 354): resets review position, switches review mode between object and document, fires becomeNavigatorObject. R1.
- `isTypingProtected`, `createStateList`, `isNVDAObject` and the like. R1.
- `copyToClip`, `getClipData` (399 to 435): needs `winUser.openClipboard(gui.mainFrame.Handle)`, empty, set and get clipboard data. R2: a clipboard stand-in. Bears on the third press of Report current object, Report status bar and the copy commands.
- `getStatusBar`, `getStatusBarText` (438 to 485): the app module's status bar, else the object at the bottom-left point of the foreground. R2: `objectFromPoint` hit-testing in the world. Bears on Report status bar.
- `processPendingEvents` (381 to 396): a nested pump inside a running script: `wx.Yield`, JAB and IAccessible pumps, cache invalidation and a queue flush. Called by `editableText` (96, 474), `browseMode` (768) and about a dozen app modules and NVDAObjects. R3: the seam must deliver the world events recorded during a script's wait at this call, not at the next cycle.
- `moveMouseToNVDAObject` (374 to 378): `winUser.setCursorPos`. R4.

### watchdog.py (452 lines)

- `alive`, `asleep`, `_watcher`, `_waitUntilNormalCoreAliveTimeout`, `waitForFreezeRecovery`, `_shouldRecoverAfterMinTimeout`, `_recoverAttempt` (76 to 240): a waitable timer marks the core dead after 0.5 s without `alive()`; the watcher thread waits up to 10 s (or 0.5 s when the foreground thread or window changed or a system menu opened), then sets the cancel event and repeatedly calls `CoCancelCall` on the main thread until the core revives. While recovering, keys only update modifiers and gestures pass through (keyboardHandler 307 to 309, inputCore 548 to 552). Needs kernel32 waitable timers and events, ole32 `CoCancelCall`, GUI thread info, a thread. R3: a seam where a recorded hang makes the stand-in call raise `exceptions.CallCancelled` or COM's cancellation at the simulated time recovery would begin. Bears on Hung application and Slow application.
- `CancellableCallThread`, `cancellableExecute`, `cancellableSendMessage`, `_notifySendMessageCancelled` (242 to 447): runs a call on a worker thread the watchdog can abandon; sends window messages through `nvdaHelperLocal.cancellableSendMessageTimeout`. With the watchdog not running, `cancellableExecute` runs inline. R2: the harness's `cancellableSendMessage` stand-in exists for two messages; each other message an NVDAObject sends (list view, tree view and edit messages) needs its answer from the world.
- Constants, `WatchdogObserver`, `Suspender`, `initialize`, `terminate`. R1.

### keyboardHandler.py (960 lines)

- `internal_keyDownEvent`, `internal_keyUpEvent` (205 to 416), called on winInputHook's thread: injected-key filtering, pass-next-key-through counting, the NVDA-key double tap (a second press within `multiPressTimeout` measured with `time.time()` bypasses the NVDA modifier), sticky-keys latching and locking with beeps, modifier tracking, `inputCore.manager.executeGesture` (which resolves the script and queues speech cancel, pause and the script itself, all from the hook thread against the main thread's focus), and, when the focus uses ToUnicodeEx typed characters, a typedCharacter event for each character `ToUnicodeEx(..., 0x4)` returns. The return value decides whether Windows delivers the key. R3: inject recorded keys by calling these two functions at recorded points between pump cycles, with `keyboardHandler.time` replaced by the simulated clock. Needs `user32.GetKeyState` for all 256 keys, `GetKeyboardLayout`, `ToUnicodeEx`, `winUser.getSystemStickyKeys` (R2).
- `shouldUseToUnicodeEx` (178 to 202): typed characters come from ToUnicodeEx when the focus is a Windows.UI.Core window, or when the app module has no `helperLocalBindingHandle` and the window is not a legacy console, or the object uses keyboard-handler typed characters. Whether the binding handle exists depends on nvdaHelperRemote having registered from inside the application (see NVDAHelper). R5 for the fact; the rule itself R1.
- `KeyboardInputGesture` naming and character (471 to 735): main key names from `vkCodes`, else `MapVirtualKeyEx`, else `GetKeyNameText`; display names; the produced character by `ToUnicodeEx` with forced modifier states; `shouldReportAsCommand`, `isCharacter`, `speechEffectWhenExecuted` (cancel for most keys, pause or resume for Shift, nothing for volume keys, for typed characters when "interrupt for characters" is off, and for Enter when "interrupt for Enter" is off). R2: keyboard layout stand-ins (`MapVirtualKeyEx`, `GetKeyNameText`, `ToUnicodeEx`, `GetKeyboardLayout`, `VkKeyScanEx`) and key state (`GetKeyState`), or pass-through to a host with the recorded layout.
- `reportExtra`, `_reportToggleKey` (740 to 752): 30 ms after a lock key, reads `getKeyState(vk) & 1` and says "caps lock on" or "off". R4: the toggle state is Windows' reaction to the key NVDA let through; the world must record it. Also behind the callLater gap.
- `executeScript`, `send`, `injectRawKeyboardInput` (754 to 817, 938 to 960): sends VK_NONE after a script consumes Alt or Windows, and re-injects gestures with `keybd_event`, then waits up to 10 ms for its own key-up. R4.
- Modifier key choice, `canModifiersPerformAction`, `fromName`, `getDisplayTextForIdentifier`, the rest. R1.

Bears on: Verbatim key double tap, Verbatim key choices, Lock keys, Cancel speech, Pause speech, Speech interrupt settings, Typed character echo, Menu guard, Repeated presses, Laptop layout.

### winInputHook.py (161 lines)

A thread with `SetWindowsHookEx` low-level keyboard and mouse hooks and a message loop, calling keyboardHandler and mouseHandler callbacks. R2: not run; recorded input is injected at the callbacks instead.

### mouseHandler.py (434 lines)

- `internal_mouseEvent`, `pumpAll`, `executeMouseMoveEvent`, `getMouseRestrictedToScreens`, `getTotalWidthAndHeightAndMinimumPosition` (106 to 247, 277 to 282): mouse moves set a flag and request a pump; the next pump hit-tests the desktop with `objectFromPoint`, skips objects transparent to the mouse, sets the mouse object and fires mouseMove; button presses queue a speech cancel. Needs `wx.Display` geometry and world hit-testing. R2.
- `updateMouseShape`, `_reportShape` (50 to 58, 272 to 274): called from IAccessibleHandler's cursor events, says "<shape> cursor" after 100 ms through a `gui.NonReEntrantTimer`. R3.
- Clicks, button locks, wheel (129 to 145, 298 to 434): `mouse_event`, `GetSystemMetrics(SWAP_BUTTON)`, `getKeyState(VK_LBUTTON)`. R4.
- `playAudioCoordinates` (74 to 103): beeps whose pitch and volume follow the position and, optionally, the screen brightness through `screenBitmap`. R5 (GDI capture); tones only.
- Bears on no Verbatim feature in the coverage table.

### touchHandler.py (664), touchTracker.py (777), screenExplorer.py (89)

A thread with a message-only window registered with `RegisterPointerInputTarget`, `touchTracker`'s gesture recogniser timed with `time.time()`, touch modes and touch scripts. R6: Verbatim has no touch features in the coverage table, and replay would need recorded pointer messages.

### inputCore.py (1,078 lines)

`InputGesture` (script lookup is cached per gesture through `scriptHandler.findScript`), `GlobalGestureMap` loading of locale and user `gestures.ini`, `InputManager.executeGesture` (sleep mode, say-all tracking, speech effect, idle prevention, input-help capture, speak command keys, `reportExtra`, intercepted-command ordering, `queueScript`), input help (`_handleInputHelp`: speaks the gesture's name with all punctuation, the character when its spoken form differs, and the script's description), gesture-source registration, `_AllGestureMappingsRetriever`. R1; braille messages go to the braille stub. Bears on Cancel speech, Pause speech, Repeated presses, the input-help gesture.

### scriptHandler.py (400 lines)

- `findScript`, `_findScript`, `_yieldObjectsForFindScript`, filters (101 to 211): order gesture's own object, global plugins, app module, braille display, vision providers, ready tree interceptor (honouring pass-through), focus, focus ancestors whose script `canPropagate`, configuration-profile commands, global commands; on the lock screen only safe scripts. Runs on the keyboard hook thread for keys. R1.
- `queueScript`, `_queueScriptCallback`, intercepted-command counting (230 to 253). R1.
- `executeScript`, `getLastScriptRepeatCount`, `clearLastScript` (264 to 323): a repeat within `multiPressTimeout` of the same script raises the count; say-all resumes after skim-reading scripts. Uses `time.time()`. R3: replace `scriptHandler.time` with the simulated clock. Bears on Repeated presses and every twice-or-three-times gesture (spell, copy, date).
- The `script` decorator and the rest. R1.

### globalCommands.py (5,714 lines, 267 scripts)

Lines per category are measured from each script's decorator to the next script.

| Group | Lines | Scripts | Class | Needs | Verbatim features |
|---|---|---|---|---|---|
| Text review | 838 | 28 | R1 | textInfos stand-ins (text agent) | Review cursor rows, Follow caret toggle, Say all from review, start marker, select then copy (copy needs the clipboard, R2) |
| Document formatting toggles | 579 | 35 | R1 | config | Document formatting |
| Object navigation | 478 | 16 | R1, except `navigatorObject_moveFocus` (`obj.setFocus()`) and `review_activate` (`obj.doAction()`), R4 | object tree stand-ins | Report current object, Parent, Next and Previous sibling, First child, To focus, Activate |
| System caret | 438 | 11 | R1 | text stand-ins | Say all from caret, Say all settings, current line and selection |
| Speech | 398 | 21 | R1 | synth settings ring (depends on the synth driver) | Speak typed characters, Speak typed words, symbol level |
| System focus | 264 | 10 | R1; the status-line scripts R2 (hit-testing, clipboard) | | Report focus, Report status bar |
| System status | 79 | 3 | R2 and R3: `GetTimeFormatEx`, `GetDateFormatEx`, the registry value for visible clock seconds, `GetSystemPowerStatus`, the clipboard, the simulated clock | | Speak time and date |
| Input | 54 | 3 | R1 | | input help, pass next key through, keyboard layout toggle |
| Mouse | 210 | 13 | R4 | `mouse_event`, `setCursorPos`, hit-testing | none |
| Configuration | 305 | 29 | R6, except toggling profile triggers and reporting the active profile, R1 | | Show menu has no counterpart here |
| Tools | 274 | 12 | R6 (consoles, log viewer, add-ons, speech viewer); `reportAppModuleInfo` R2 (product name and version); link destination R1 | | |
| Braille | 464 | 26 | R6 | | none |
| Magnifier | 273 | 20 | R6 | | none |
| Touch | 110 | 6 | R6 | | none |
| Remote | 95 | 8 | R6 | | none |
| Audio | 32 | 2 | R6 | | none |
| Math | 32 | 1 | R5 (MathCAT) | | none |
| Configuration profiles | 9 | 1 | R6 | | |
| No category | 438 | 22 | sleep mode and browse-mode pass-through R1; quit, restart, show GUI, braille modifier toggles R6; OCR and screen curtain R5 | | Starting and exiting |

The module header, `toggleBooleanValue`, `toggleIntegerValue` and `ConfigProfileActivationCommands` (about 350 lines) are R1. `script_title` reads the foreground object's name, falling back to the app name, then "No title"; twice spells, three times copies.

### globalPluginHandler.py (93 lines)

Imports plugins under `globalPlugins` (empty in the pinned source; add-ons are disabled in the harness) and lists them first in the event and script chains. R1.

### appModuleHandler.py (952 lines)

- `getAppModuleForNVDAObject`, `getAppModuleFromProcessID`, `fetchAppModule`, `_importAppModuleForExecutable`, executable-name mapping, `update`, `cleanup`, `reloadAppModules`, `handleAppSwitch` (227 to 457): one app module per process; an import error in an app module is spoken as "Error in appModule <name>"; `cleanup` drops modules whose process ended, firing `event_appLoseFocus` and `eventHandler.handleAppTerminate`; `handleAppSwitch` fires appModule lose and gain focus and exits and enters configuration-profile triggers inside one atomic profile switch, which can change speech settings. R1.
- Process facts: `getAppNameFromProcessID` (Toolhelp snapshot), `getProcessHandleFromProcessId` (`OpenProcess`, fallback through a top-level window and `oleacc.GetProcessHandleFromHwnd`), `AppModule.processExecutablePath` (`QueryFullProcessImageName`), product name and version (`GetPackageFullName`, `fileUtils.getFileVersionInfo`), `isAlive` (`waitForSingleObject(handle, 0)`), `is64BitProcess` and machine type (`IsWow64Process2`, `GetProcessInformation`), `getWmiProcessInfo`. R2: the harness covers the name, handle and liveness; a process that exits during the world, the product name and version, and the architecture need stand-ins. `appModuleHandler` binds `_getWindowThreadProcessID`, `_findTopLevelWindow` and `_getProcessHandleFromHwnd` at import, before the harness installs its stand-ins, so those three reach the real system unless patched on the module.
- `helperLocalBindingHandle`, `_inprocRegistrationHandle`, `dumpOnCrash` (`nvdaInProcUtils_dumpOnCrash`), RPC cleanup. R5.

Bears on: Focus ancestry and window on switching, Settings persistence (profiles), Outposts kept while idle (application exit).

### baseObject.py (319 lines)

`AutoPropertyObject` getters with per-instance caches cleared for every instance by `invalidateCaches` at the end of each pump; `ScriptableObject` gesture maps and `getScript`. R1; the clearing point is the R3 pump seam.

### winUser.py (943 lines, 67 functions)

Thin wrappers over user32: message loop, key state, window tree and facts, foreground and focus setting, win-event hooks, `sendMessage`, `keybd_event`, `mouse_event`, cursor and caret, GUI thread info, styles, keyboard layout and key names, sticky keys, `SendInput`, clipboard, `findTopLevelWindow`, the screen-reader flag. R2 per function from the world (14 done), R4 for `setForegroundWindow`, `setFocus`, `keybd_event`, `mouse_event`, `setCursorPos`, `SendInput`, `PostMessage`; the eight arithmetic helpers are R1 (see gaps).

### winKernel.py (603 lines, 33 functions)

Process handles and memory (`openProcess`, `readProcessMemory`, `virtualAllocEx` used by window-message buffers in other processes), waits, events and timers, `GetSystemPowerStatus`, time and date formatting, `GetThreadLocale`, `LCIDToLocaleName`, pipes and process creation. R2: `GetTimeFormatEx` and `GetDateFormatEx` against the simulated clock, `GetSystemPowerStatus`, process memory for cross-process messages (other agents' list and tree views), and the event and wait functions used by `keyboardHandler.send` (R4).

### winBindings/ (6,958 lines)

ctypes declarations only; their class is that of the function's caller.

| File | Lines | Class | Used for |
|---|---|---|---|
| user32 | 1,838 | R2 | window facts, `GetWindowLong`, key state and layout, `SystemParametersInfo`, clipboard, hooks, messages |
| kernel32 | 1,764 | R2 | processes, locale, waitable timers, power status, package names |
| advapi32 | 364 | R2 | tokens, process creation for the remote loaders |
| oleacc | 219 | R2 | `AccessibleObjectFromWindow`, `GetProcessHandleFromHwnd` |
| ole32 | 152 | R2 | `CoCancelCall`, COM initialisation |
| commCtrl | 109 | R2 | list and tree view message constants |
| version | 70 | R2 | file version info |
| wtsapi32 | 52 | R2 | lock state |
| mshtml | 41 | R2 | MSHTML from window |
| uiAutomationCore | 30 | R2 | `UiaHasServerSideProvider` (done) |
| shcore | 29 | R2 | DPI |
| gdi32 | 240 | R5 | display model, screen bitmap |
| icu | 111 | R5 | word segmentation |
| rpcrt4 | 59 | R5 | nvdaHelper RPC bindings |
| oleaut32, `__init__` | 39 | R1 | |
| hid, winusb, setupapi, bthprops, cfgmgr32 | 902 | R6 | braille hardware |
| gdiplus, magnification | 314 | R6 | magnifier, highlighter |
| shell32, shlwapi, urlmon, sas, crypt32, dbgHelp, jobapi2, winmm, mmeapi | 625 | R6 | shell, updates, crash dumps, audio |

### NVDAHelper (1,745 lines: `__init__.py` 958, `localLib.py` 689, `localWin10.py` 98)

- Controller API callbacks (`nvdaController_speakText`, `speakSsml`, `cancelSpeech`, `brailleMessage`, `isSpeaking`; 79 to 216): other programs ask NVDA to speak through nvdaHelperLocal's RPC server; each queues a speech call. R5 at the source; replay injects the recorded call into the Python callback.
- In-process callbacks from nvdaHelperRemote, which NVDA injects into every application: `requestRegistration` (sets the app module's `helperLocalBindingHandle`, which decides the typed-character source above), `typedCharacterNotify` (WM_CHAR seen inside the app becomes a typedCharacter event on the focus, except legacy consoles), `inputLangChangeNotify` (speaks "<language> - <layout>" or the layout alone, using `GetLocaleInfo` and the registry layout names, not during say-all, only for the focus thread), IME composition, candidate list, conversion mode and open status (speech mode toggled off around fake focus events), `reportLiveRegion`, `displayModelTextChangeNotify`, `drawFocusRectNotify`, `vbufChangeNotify`, add-on install, config directory, remote URL. These run on RPC threads and enqueue on the event queue. R5 (nvdaHelperRemote's in-process hooks); the Python bodies are R1 once the call is injected at a recorded queue point (R3).
- `initialize`, `terminate`, `_RemoteLoader`: loads `nvdaHelperRemote.dll` (`injection_initialize`, `installIA2Support`, `initInprocManagerThreadIfNeeded`) and starts `nvdaHelperRemoteLoader.exe` for the other architectures. R6 for replay (not run).
- `localLib.py`: entry points of `nvdaHelperLocal.dll`, loaded for real by the harness from `NVDA_LIB`: `cancellableSendMessageTimeout`, `nvdaInProcUtils_*` (Word, Excel, Outlook, list view, IA2 text, `getActiveObject`, `getTextFromIAccessible`, crash dump), `VBuf_*` (virtual buffers), `displayModel_*`, `registerUIAProperty`, `registerUIAAnnotationType`, `rateLimitedUIAEventHandler_create`, `wasPlay_*`, `wasSilence_*`, `calculateCharacterBoundaries`, `calculateCharacterOffsets`, `calculateWordOffsets`, `getOleClipboardText`, `findWindowWithClassInThread`, `audioDucking_shouldDelay`, `isScreenFullyBlack`. R5. The `calculate*` offsets functions compute from the string alone and can run for real; the `nvdaInProcUtils_*` and `VBuf_*` functions reach into the application and need world answers.
- `localWin10.py`: `uwpOcr_*`, `wgcCapture_*`. R5, no speech feature of Verbatim.

Bears on: Typed character echo, Typed characters (read-only), the IME and input-language reports.

### config/ (4,110 lines)

`configSpec` defaults, `ConfigManager` with base and profile sections, `atomicProfileSwitch`, triggers (`AppProfileTrigger` from appModuleHandler, say-all trigger), `post_configProfileSwitch`, feature flags and flag enums, aggregated sections, profile upgrade steps run on loading an old file. Behaviour reads it everywhere (report settings, interrupt settings, `multiPressTimeout`, `handleInjectedKeys`, background progress bars, focus-event cancellation). R1: the world must pin the capture's `nvda.ini` and profiles in the scratchpad config path. `isInstalledCopy`, `getInstalledUserConfigPath`, start on logon, system config copy and `config/registry.py` (about 375 lines) read and write the registry for paths and installation. R6.

### Other top-level modules in this area

| Module | Lines | Class | Behaviour, needs |
|---|---|---|---|
| ui.py | 243 | R1 (message, reviewMessage, delayedMessage through callLater R3); `browseableMessage` R6 | every spoken message |
| synthSettingsRing.py | 240 | R1 | synth setting ring speech |
| speechDictHandler/ | 782 | R1 | dictionary files from the config path |
| controlTypes/ | 1,640 | R1 | role, state and output-reason names |
| extensionPoints/ | 505 | R1 | actions and deciders |
| locationHelper, logHandler, garbageHandler, globalVars, NVDAState, autoSettingsUtils, monkeyPatches, aria, annotation, colors, tableUtils, XMLFormatting, keyLabels, vkCodes, msoAutoShapeTypes, small constant modules | 5,431 | R1 | pure Python |
| documentNavigation/ | 333 | R1 | paragraph and sentence navigation |
| compoundDocuments.py | 579 | R1 | compound text info (text stand-ins) |
| objidl, oleTypes, objbase | 1,449 | R1 | COM type definitions |
| languageHandler.py | 478 | R1 with real kernel32 locale calls about NVDA's own process | gettext, thread locale |
| textUtils/ | 1,578 | R1 for 682; R5 for `uniscribe`, `icu`, `_wordSeg` (cppjieba DLL, initialised on a thread) 896 | offsets, segmentation |
| utils/ | 1,686 | R1 730; R2 `security.py` 463 (lock screen, secure desktop, safe scripts); R6 `schedule`, `_crashHandler`, `mmdevice` 493 | |
| winAPI/ | 1,326 | R2: `sessionTracking` and `_wtsApi32` (lock state per pump from `WTSQuerySessionInformation`), `_powerTracking` (battery report, AC state change spoken from WM_POWERBROADCAST), `_displayTracking` (orientation spoken from WM_DISPLAYCHANGE), `messageWindow`, `secureDesktop`, 1,053; R6 `dpiAwareness` 99; R1 constants 174 | The secure desktop and the lock state |
| winVersion.py | 242 | R2 (Windows build from `sys.getwindowsversion` and the registry decides virtual desktop names, ToUnicodeEx typed characters, UIA choices) | |
| systemUtils.py | 310 | R2 (clock seconds visible from the registry, UIAccess, desktop name) | Speak time and date |
| winConsoleHandler.py, wincon.py | 482 | R2: `AttachConsole`, `GetConsoleScreenBufferInfo`, `ReadConsoleOutput*`, console win-event hooks, typed characters from console update events | Console windows (legacy console) |
| windowUtils.py | 385 | R2 (`EnumChildWindows` descendant search; custom windows) | |
| oleacc.py | 373 | R2 | MSAA entry points |
| fileUtils.py | 129 | R2 (version info) | |
| winGDI, screenBitmap | 238 | R5 (GDI) | |
| comHelper.py, mathType.py | 127 | R5 (`nvdaInProcUtils_getActiveObject`, MathType) | |
| mathPres/ | 1,987 | R5 (`libmathcat_py`) | none |
| contentRecog/ | 885 | R5 (UWP OCR) | none |
| vision/, visionEnhancementProviders/, screenCurtain/, _magnifier/ | 4,795 | R6: visual only; the screen curtain's spoken state comes from global commands | none |
| _remoteClient/ | 5,307 | R6: Remote Access sessions | none |
| addonHandler/, addonStore/ | 3,959 | R6: add-ons disabled in replay | |
| gui/ | 19,389 | R6, except `gui/blockAction.py` (see gaps) | Show menu, settings dialogs (Verbatim's own GUI) |
| speechViewer, pythonConsole, remotePythonConsole, installer, updateCheck, setup, l10nUtil, md2html, markdownTranslate, keyCommandsDoc, documentationUtils, argsParsing, easeOfAccess, audioDucking, audio/, fonts/, COMRegistrationFixes/, _asyncioEventLoop/, jobObject, hidpi, shlobj, shellapi | 9,804 | R6: tools, installation, build, audio routing, GUI scaling | |

## Hardest items

1. The pump and its clock: reproduce `CorePump` (immediate and 10 ms delayed pumps, re-pump after a pump, the pump order, cache invalidation per cycle) on a simulated clock, and route `core.callLater` and `gui.NonReEntrantTimer` through it. Which events share a cycle decides property reads and focus-event cancellation.
2. Thread order on the event queue: UIA's MTA thread, the keyboard hook thread (which resolves scripts and queues speech cancel against the main thread's focus), NVDAHelper's RPC threads and IAccessible's win-event thread all enqueue; `flushQueue` runs only what was present at its start and `isPendingEvents` counts at the moment of asking. Each recorded producer needs a recorded enqueue point.
3. Nested pumps inside scripts: `api.processPendingEvents` (with `wx.Yield`) in editable text, browse mode and app modules must receive the world events recorded during the script's wait.
4. Keys and Windows' reaction: whether NVDA passes or blocks a key changes what the application receives, the lock-key toggle state read 30 ms later, the caret move a caret script waits for, and WM_CHAR; key state, sticky keys and the keyboard layout are needed for every key.
5. The typed-character source: ToUnicodeEx from the hook versus WM_CHAR from nvdaHelperRemote inside the application, chosen by whether nvdaHelperRemote registered for that process; the world must record which.
6. Freeze recovery: a hung application's effect on speech appears only through the watchdog's cancellation timing and `CallCancelled`.
7. The harness defects listed first: dropped `callLater` timers, global commands turned into MagicMocks by the stubbed `gui.blockAction`, raised arithmetic helpers in winUser, `user32.GetWindowLong` in `shouldAcceptEvent`, and the partial pump.

## Lines per class

| Class | Lines | Largest parts |
|---|---|---|
| R1 | about 23,500 | config 3,735; globalCommands 3,250; controlTypes 1,640; objidl and oleTypes 1,449; inputCore 1,078 |
| R2 | about 11,100 | winBindings 4,670; winAPI 1,050; winUser 880; winKernel 600; appModuleHandler 400 |
| R3 | about 800 | watchdog 260; keyboardHandler key events 210; core pump and callLater 160; queueHandler 100 |
| R4 | about 550 | globalCommands mouse and activation 310; mouseHandler clicks 130; keyboardHandler send 100 |
| R5 | about 6,600 | mathPres 1,990; NVDAHelper 1,745; textUtils segmentation 900; contentRecog 885; winBindings gdi32, icu, rpcrt4 410 |
| R6 | about 48,300 | gui 19,390; _remoteClient 5,310; _magnifier 2,800; addonStore 2,510; globalCommands 1,760 |

Splits inside a module are counted by the line ranges given above; the figures are rounded and total about 90,800 lines of the area, excluding the braille-hardware, `wasapi` and `_bridge` modules (7,552 lines) left to other agents.
