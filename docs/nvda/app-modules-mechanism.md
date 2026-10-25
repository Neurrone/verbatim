# App modules, global plugins, and add-ons: the mechanism

This file documents the *hook points* NVDA gives per-application and
global extension code — the machinery Verbatim's extension system and
the app-module porting track measure against. Individual app modules'
behavior is deliberately out of scope (per this folder's rules); the
apps whose support needed core/C++ assistance have their own files
([Office through COM](office-com.md), [Virtual buffers](virtual-buffers.md), [Process injection](process-injection.md)).

## App modules

An app module is a Python class bound to a *process*
(`source/appModuleHandler.py`):

- Binding is by executable name: `fetchAppModule(processID, appName)`
  imports `appModules.<name>` from NVDA's tree or an add-on
  (add-ons can also claim executables explicitly via
  `registerExecutableWithAppModule`). One `AppModule` instance exists
  per *running process*, created lazily the first time an event or
  object from that process appears
  (`getAppModuleFromProcessID`, cached in `runningTable`), and
  `terminate()`d when the process dies (`handleAppTerminate`). A
  default `AppModule` serves processes with no specific module.
- Every NVDAObject carries `appModule`
  (`getAppModuleForNVDAObject`), so all per-object machinery can
  consult it.
- A console window is the exception to binding by the window's owner.
  Windows names the console's client (such as `powershell.exe`) as the
  owner of a console window by `GetWindowThreadProcessId`, while the
  window, its UIA provider, and its text area are the console host's.
  NVDA gives the window and its text area the client's app module
  (`UIA._get_processID`, `NVDAObjects/UIA/__init__.py` 2336 to 2346;
  `getAppModuleForNVDAObject`, `appModuleHandler.py` 227 to 238), and
  learns the console host's real thread from the console's WinEvents
  (`consoleWindowsToThreadIDs`,
  `IAccessibleHandler/internalWinEventHandler.py` 266 to 273), falling
  back to the window's own thread.

The File Explorer app module leaves unsaid a focus on the nameless
windows that only stage a switch on Alt+Tab, Task View, and the
Windows+X menu (`ForegroundStaging`, `LauncherTipWnd`,
`ApplicationManager_DesktopShellWindow`; `appModules/explorer.py` lines
443 to 449). It does so after NVDA has made the window its focus and
run its foreground event, so speech is still cut there.

The same module gives an item of the task switcher's list, a UI
Automation list item whose parent has the automation id
`SwitchItemListControl` (Windows 10 from version 1809, and Windows 11,
where the list is named "Running applications"), or whose window's class
is `MultitaskingViewFrame` (Windows 10 before), a class whose container
is the desktop while Alt is held, read with `GetAsyncKeyState` when the
container is asked for, and its real parent otherwise
(`MultitaskingViewFrameListItem`, `appModules/explorer.py` 39 to 45 and
303 to 311). NVDA builds the focus ancestry by asking each object for its
container (`setFocusObject`, `api.py` 104 to 147), so with Alt held the
item's ancestry is the desktop alone, which the previous focus's ancestry
holds too: no container is entered, so none is spoken, and NVDA finds the
foreground window itself and runs its foreground event, which cuts speech
(`doPreGainFocus`, `eventHandler.py` 402 to 413). The item is then spoken
as any focus is, its name and its row, column and position ("Nvda Sw B
row 1 column 2 2 of 10"; [Speech](speech.md), "Table cell coordinates").
With Alt released, as in the switcher that Control+Alt+Tab keeps open,
the item's containers are entered and spoken as any focus's are.

What an app module can hook, in rough order of importance:

- `chooseNVDAObjectOverlayClasses(obj, clsList)` — inject overlay
  classes into any object from its process ([Object model](object-model.md)); the
  main way per-app rendering/behavior differences are implemented.
- `event_<name>(obj, nextHandler)` — see events for its process before
  tree interceptors and objects do ([Event handling](events.md)); may swallow them.
  Module-lifecycle pseudo-events `event_appModule_gainFocus` /
  `loseFocus` fire when focus enters/leaves the app
  (`appModuleHandler.handleAppSwitch`).
- Scripts and gesture bindings (`inputCore`; [Keyboard input](input.md)) scoped to the
  app while it has focus.
- `sleepMode` — setting it makes NVDA go silent in that app
  (self-voicing apps): events are dropped, scripts disabled.
- Process-level facts: `appModule.processID`, `appName`,
  `is64BitProcess`, `isWindowsStoreApp`, `appArchitecture`, helpers to
  read the process's environment — plus `injectionDone`-style
  interaction with the helper when in-process support is loaded.
- `terminate()` for cleanup, and `event_appModule_loseFocus` for
  focus-out state.

The critical scoping rule: app modules act on *their process's* objects
and only while relevant; they are instantiated per process, so state on
the module is per-app-instance state.

## Global plugins

`source/globalPluginHandler.py`: same shape, no process scoping — a
`GlobalPlugin` sees `chooseNVDAObjectOverlayClasses` for *every* object,
`event_*` for every event (first in the chain, before app modules), and
its scripts are active everywhere. Plugins come from add-ons (or the
scratchpad in developer mode) and are all loaded at startup
(`initialize` iterates `globalPlugins` packages).

## Add-ons, minimally

An add-on (`source/addonHandler/`) is a zip with a manifest and any of:
`appModules/`, `globalPlugins/`, `synthDrivers/`, `brailleDisplayDrivers/`,
`visionEnhancementProviders/` packages — dropped onto NVDA's import
paths. There is no sandboxing and no API surface restriction: add-on
code is arbitrary Python inside the NVDA process with monkey-patching
fully available and commonly used. Compatibility is managed socially,
by manifest-declared API version ranges
(`source/addonAPIVersion.py`) checked at install/load, with breaking
changes batched to yearly x.1 releases (`nvda/projectDocs/dev/deprecations.md`).
The practical consequence to note for any parity effort: a large
fraction of real-world NVDA behavior lives in this unrestricted add-on
layer, and *any* NVDA-internal symbol is potentially load-bearing for
someone's add-on.
