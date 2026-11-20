# Merged review: settings, the GUI, localization and the extension crates

Checked against `main` at ba89dca7 (2026-11-20). Reviewer A's notes: `settings-gui-A.md`; reviewer B's: `settings-gui-B.md`. Every quotation below was compared with the file at the line given; line numbers are today's.

## 1. Size

Product lines exclude `#[cfg(test)]` modules (counted with `grep -n '^#\[cfg(test)\]'` and `wc -l`). The two reviewers' totals agree within a tenth (A about 10,200 product; recount 10,130), so the table is settled from the recount.

| Behaviour | Verbatim product | Verbatim tests | NVDA counterpart (pinned) | NVDA lines | Ratio | What the extra lines buy |
|---|---|---|---|---|---|---|
| Settings file: schema, load, save, saver thread | 1,000: config `lib.rs` 579, app `saver.rs` 191, `speech_settings.rs` 67, `terminal_settings.rs` 76, `main.rs` load and hand-off about 90 | 450: config `lib.rs` 317, `saver.rs` 133 | `config/configSpec.py` (575, for 242 settings) and the base load `config/__init__.py` 536-581, `_loadConfig` 597-630; the rest of its 1,570 is profiles and triggers | about 180 for the parts Verbatim has | about 5.5x (A said 6x, B 5x per setting; both right within their counting) | the saver thread with one writer per section (ownership.md); `ActiveConfig`/`Profile` machinery for profiles that do not exist (finding P2) |
| Settings dialog frame, keys and Speech page | about 1,520: `settings.rs` 308, `plan.rs` 184, `keys.rs` 117, `lib.rs` routing and speech about 210, bridge about 260, `gui.cpp` `SpeechPanel` and `SettingsDialogWindow` (227-437, 1011-1247) about 450 | 610 unit (`settings.rs` 303, `plan.rs` 143, `keys.rs` 123, `lib.rs` 41) + scenarios `settings_dialog_keys` 152, `menu_and_settings_dialog` 66 | `gui/settingsDialogs.py` 117-803 (dialog and panel frame), 1179-1653 (Speech panel, synthesizer dialog, `DriverSettingChanger`, `AutoSettingsMixin`), 3676-3687, 6666-6746 | about 1,255 | 1.2x (A 1.4x combined; B 0.85x frame and 1.8x Speech page) | the snap-back of refused values (Dickson, 2026-10-31, per the crate guide), the non-blocking synthesizer switch; near target |
| Terminal page (4 controls) | about 440: `terminal_panel.rs` 190, `gui.cpp` 940-1003 (64), bridge `TerminalPage` and five externs (about 40), `lib.rs` 656-682 (27), i18n `TerminalLabels` 22 + 4 Fluent lines, app `terminal_settings.rs` 76 | 167 unit + scenario `terminal_settings_page` 72 | none (a Terminal page is a ruled difference, Dickson 2026-10-06, `docs/parity/system.md` 241-242); a 4-control NVDA panel for scale: `ReviewCursorPanel`, `settingsDialogs.py` 2303-2340 | 38 | about 11x | a diff-apply nobody ruled (Q1) and a hand-written bridge per control (P1) |
| Theme page and theme files | about 2,900: config `themes.rs` 660, `package.rs` 125, `theme_panel.rs` 942, `gui.cpp` `ThemePanel` about 430, bridge about 200, `lib.rs` about 100, app `themes.rs` 168, i18n theme parts about 250 | 691 unit + scenario `theme_panel` 287 | none; themes are Verbatim's (D12) | - | - | judged on duplication only (R4, R10, D3) |
| Menu, tray icon, hidden frame, popups | about 1,000: `lifecycle.rs` 175, `foreground.rs` 143, `hidden_frame.rs` 44, `lib.rs` about 250, bridge about 150, `gui.cpp` about 250 | 110 | `gui/__init__.py` 142-911 (`MainFrame`, `SysTrayIcon`, `prePopup` 170-188) with far more menu items | 770 | about 1.3x | the forced foreground for control-plane input (`foreground.rs`, Q3) |
| Tray and taskbar list | about 520: `tray_list.rs` 229, `shell_items.rs` 104, `list_dialog.rs` 116, `gui.cpp` about 75 | 290 | the systrayList add-on, not in `nvda/` | - | - | M5 makes it a global module |
| Localization | 2,023: i18n `lib.rs` 1,461, Fluent file 562 | 692 | `languageHandler.py` 478; strings inline as `_()` | 478 | about 4x by lines (A's 1.8x counted NVDA's role and state label tables, 970 lines, which is fairer: 2.1x) | about 470 lines of one-function-per-message wrappers (R1) |
| wxWidgets build | 366: `build.rs` 53, `build/wx.rs` 313 | 0 | none (wxPython from pip) | - | - | a build that stays one cargo command (D4) |
| Extension crates | 57 (skeletons) | 0 | `addonHandler/` | - | - | nothing yet (M5) |
| Manual example | `examples/settings_preview.rs` 259 | - | - | - | - | nothing a routine run executes (R12) |

Area total: about 10,130 product lines (Rust, C++, Fluent, build script), 3,010 unit test lines (config 643, gui 1,542, i18n 692, saver 133), 577 lines of the four dialog scenarios, and the 259-line example. Where NVDA has a counterpart, the area is about 1.6x NVDA's lines overall; the outliers are the settings file (5.5x) and the Terminal page (11x).

Deletion target, from the verified findings:

| Part | Product lines | Test lines |
|---|---|---|
| Remove now | about 500 (R1 250, R2 100, R5 40, R6 25, R4 20, R10 20, R11 20, R3 15, R7 15, small items) | about 560 (the 259-line example, about 300 unit test lines in section 3) |
| If Dickson rules for the removal | about 60 (Q1 45, Q3 15) | about 40 (Q1) |
| Planned phase | about 290 (P1 M4.3 160, P2 M8.3 120, P3 M5 and M7 10) | about 215 (P1 100, P2 90, P3 25) |

Defects add about 70 product lines and 60 test lines (D1, D2, D3, D4).

## 2. Findings

| id | title | class | found by | file and line | what goes | product lines saved | test lines saved | speech changes |
|---|---|---|---|---|---|---|---|---|
| D1 | A settings file that fails to load is overwritten with defaults at the first saved change, with no backup and no word to the user; for that session themes and sounds are looked up under the temporary folder | Defect | both (the temporary-folder roots: found in verification) | `crates/verbatim-app/src/main.rs` 565-584, 126, 268-269; `saver.rs` 46-49, 133-158 | the load from `%TEMP%\verbatim-defaults` and its panic | -15 (adds about 20, removes 5) | -25 (one test added) | yes: a message box at start, in NVDA's words; needs a capture |
| P1 | Each Terminal page setting is plumbed by hand through seven files; the Speech page's generic controls would serve every page | Planned phase (M4.3) | both | `bridge.rs` 237-263, 701-710; `lib.rs` 656-682; `terminal_panel.rs` 136-173; `gui.cpp` 940-1003; i18n `lib.rs` 589-610 | `TerminalPage`, five externs, five `GuiCore` methods, four setters, C++ `TerminalPanel`, `TerminalLabels` | about 160 now; about 15 touches per M4.3 setting | about 100 | no |
| R1 | i18n wraps each message id in its own function | Remove now | both | `crates/verbatim-i18n/src/lib.rs` 248-720 | about 50 one-line accessors and their doc comments | about 250 | none directly | no |
| R2 | `ControlPlan` is a third copy of each Speech setting, with a fallback only a driver bug reaches | Remove now | both | `crates/verbatim-gui/src/plan.rs` 12-135 | `ControlPlan`, `id()`, `label_key()`, `plan_for` | about 100 | about 60 | no |
| D2 | The hidden frame's title is the localized tray tooltip, but a new instance looks for the fixed title "Verbatim" | Defect | A only | `crates/verbatim-gui/src/lib.rs` 211-213; `crates/verbatim-app/src/single_instance.rs` 49-52, 220-232 | `ShellText.title` | 2 | -5 (one assertion added) | no |
| R3 | Seven matches in i18n end in a silent wildcard because the model's enums are `#[non_exhaustive]` | Remove now | both (A seven enums, B four) | i18n `lib.rs` 765, 910, 1096, 1261, 1327, 1398, 1458 | `#[non_exhaustive]` on seven enums and the seven wildcard arms | about 15 | 0 | no |
| Q1 | The Terminal page applies only what changed, unlike NVDA's panels, and no ruling asks for it | Question for Dickson | A only | `crates/verbatim-gui/src/terminal_panel.rs` 11-16, 35-106, 121, 175-183 | `TerminalChange`'s options, `change_from`, `is_empty`, `committed` | about 45 | about 40 | yes (see Q1) |
| R5 | The modifier-key fields, their defaults and the keyboard layout are declared twice | Remove now | B only | config `lib.rs` 98-107, 108-122, 144-152; `verbatim-input/src/state.rs` 49-80; `scripts.rs` 22-38; app `main.rs` 228-231, 756-768 | the input crate's copies and the two translations | about 40 | 0 | no |
| R6 | `ConfigValue` mirrors `SettingValue`, and a saved number out of `i32` range is dropped silently | Remove now | A only | config `lib.rs` 214-223; `verbatim-speech/src/settings.rs` 62-69; app `speech_settings.rs` 34-38, 59-61 | `ConfigValue` and both conversions | about 25 | 0 | no |
| R4 | A sound's gain is computed in two crates, and the Events earcon mapping is written both ways | Remove now | both (earcon mapping A only) | `theme_panel.rs` 627-632, 927-941; `verbatim-speech/src/theme.rs` 197-203; model `theme.rs` 348-362 | one copy of the gain and `earcon_of` | about 20 | 0 | no |
| R7 | GUI lifecycle facts held twice | Remove now | A only | `lifecycle.rs` 64-80; `lib.rs` 242-262, 464-479 | the three `RefCell<Option<...>>` fields beside `Lifecycle` | about 15 | 0 | no |
| R8 | Bounded settings are plain integers clamped at every reader | Remove now | both | model `settings.rs` 87, 92, 118-126; `terminal_panel.rs` 89-90, 110-112; config `lib.rs` 378; `theme_panel.rs` 202, 630; speech `theme.rs` 201 | five read-side clamps and two clamp tests | about 0 net (newtypes added) | about 20 | no |
| D3 | Theme file errors are shown to the user in hard-coded English | Defect | B only | config `themes.rs` 133-151; `theme_panel.rs` 481, 679, 698, 711, 720, 739, 750 | nothing; adds a Fluent mapping | -30 | 0 | no (same English shown) |
| D4 | A runtime message lookup shows Fluent's "No localization for id" text in a control's label | Defect | A only | i18n `lib.rs` 241-243; callers `gui/settings.rs` 204, app `main.rs` 656, `verbatim-synth-onecore/src/lib.rs` 49 | nothing; adds a check | -10 | -15 | no |
| R9 | The settings dialog's key enums are declared on both sides of the bridge and the mapping is tested twice | Remove now | both | `bridge.rs` 556-611; `keys.rs` 21-93; `lib.rs` 741-796, 802-838 | the bridge test (product change optional) | 0 to 50 | 41 | no |
| R10 | Three copies of the atomic write; `ThemeHost::persist` returns an error it never has | Remove now | both | config `lib.rs` 571-578, `themes.rs` 652-659, `package.rs` 98-110; `theme_panel.rs` 62, 755-757; app `themes.rs` 70-78 | two helpers, the `Result` and its dead branch | about 20 | 0 | no |
| R11 | Dead and string-typed i18n wrappers | Remove now | B only | i18n `lib.rs` 232, 287-299, 323 | `settings_title`, `startup_message`, the string key of `toggle_key_state` | about 20 | about 8 | no |
| R12 | The settings preview example is run by nothing | Remove now | both (B as a question) | `crates/verbatim-gui/examples/settings_preview.rs` | the file | 0 | 259 | no |
| P2 | Profile layering with no profiles | Planned phase (M8.3) | both | config `lib.rs` 255-403, 445, 475, 526-527, 553 | `Profile`, `ProfileTheme`, `ActiveConfig`'s overlay walk, the `profiles` folder | about 120 | about 90 | no |
| R13 | Stale comments and an unused default | Remove now | both | see paragraph | text only, and one derive | about 3 | 0 | no |
| Q3 | `foreground.rs` keeps an `AttachThreadInput` fallback with no recorded case that needs it | Question for Dickson | A only | `crates/verbatim-gui/src/foreground.rs` 42, 120-133 | the fallback | about 15 | 0 | no; needs a measurement |
| P3 | Tray list: offscreen checked twice, dead `KeepOpen`, clamped default button | Planned phase (M5, M7) | B only | `tray_list.rs` 184; `shell_items.rs` 87-89; `list_dialog.rs` 30-37, 103-105 | the duplicate check and test overlap | about 10 | about 25 | no |

### D1. A settings file that fails to load is overwritten at the first saved change

Verified, step by step:

1. `main.rs` 125-126: `let exe_dir = exe_dir(); let config = load_config(&exe_dir);` runs before `init_tracing` (127), so nothing that follows here reaches the log.
2. `load_config` (`main.rs` 565-584), documented "falling back to defaults (without overwriting the files) when they are unreadable or corrupt". `ConfigStore::load` (config `lib.rs` 470-477) returns `ConfigError::Parse` for any file serde refuses: a TOML syntax error, or one bad value such as `layout = "Laptop"` (config `lib.rs` 116, `rename_all = "lowercase"`) or `terminal_full_lines = "30"`; and `ConfigError::Io` for an unreadable file. Either goes to the `Err` arm:
   ```
   report_error!("verbatim: config error, continuing with defaults: {error}");
   ConfigStore::load(&std::env::temp_dir().join("verbatim-defaults"))
       .unwrap_or_else(|fallback| panic!("default config must load: {fallback}"))
   ```
   `report_error!` (`main.rs` 110-115) writes one line to standard error and nothing else. The fallback reads `%TEMP%\verbatim-defaults\settings.toml` if one exists (defaults if not), and panics, before the panic hook is installed, if that file is itself bad.
3. The store returned is rooted at `%TEMP%\verbatim-defaults`, and `themes_dir()` and `sounds_dir()` (config `lib.rs` 503-511) are `self.root.join(...)`. `run` uses them for the starting theme (`main.rs` 258-260, `themes::configured` in app `themes.rs` 110-119) and keeps them for the Theme page (`main.rs` 262). So for that session the user's installed themes are not found, and the shared sounds the built-in theme plays are looked for under the temporary folder. Neither reviewer reported this.
4. `main.rs` 268-269: `saver::Saver::start(saver::DiskFile(exe_dir()), config.settings().clone())?` gives the saver the real program folder (`exe_dir()`, not the store's root) and the default `Settings`.
5. The first saved change of any section rewrites the whole file: the saver keeps the newest of each section over the loaded settings and calls `file.write(&settings)` (`saver.rs` 133-158), which is `ConfigStore::from_settings(&self.0, settings.clone()).save_settings()` (`saver.rs` 46-49), a temporary file renamed over `settings.toml` (config `lib.rs` 571-578). Saved changes are: a toggle key (Verbatim+2, 3, 5, 6: `Effect::SettingsChanged` from `reduce.rs` 1128, dispatched `main.rs` 1591 to `terminal_settings.rs` 48-51), OK or Apply on the Speech page (the synth thread's `commit`, `verbatim-speech/src/manager.rs` 1352-1358, through `speech_settings.rs` 48-66), and a theme choice (app `themes.rs` 70-78). Nothing saves without a user action.

What the user is told: nothing they can hear or see. Verbatim starts and speaks normally, on defaults; the one line goes to standard error before logging starts. After step 5 every saved voice, rate, theme choice and modifier key is gone, with no copy. Two documents say otherwise: the `load_config` comment and the config guide's "including corrupt ones, which `load` surfaces as errors rather than" silently replacing (`books/dev/src/reference/crates/verbatim-config.md` 206).

NVDA (`nvda/source/config/__init__.py` 536-562), when `_loadConfig` raises:
```
backupFileName = fn + ".corrupted.bak"
log.error(
    "Error loading base configuration; the base configuration file will be reinitialized."
    f" A copy of your previous configuration file will be saved at {backupFileName}",
...
    if os.path.exists(backupFileName):
        os.unlink(backupFileName)
    os.rename(fn, backupFileName)
...
self.baseConfigError = True
return self._initBaseConf(factoryDefaults=True)
```
and once the GUI is up (`nvda/source/core.py` 156-170) shows a message box titled "Configuration File Error": "Your configuration file contains errors. Your configuration has been reset to factory defaults.\nMore details about the errors can be found in the log file." (B quoted the first two sentences only.) One difference neither reviewer named: NVDA's reset is for a file ConfigObj cannot parse or upgrade; a single value that fails its spec does not fail the load, it is checked when read (`_cacheLeaf`, `__init__.py` 1268-1273). In Verbatim one bad value fails the whole file; keeping the other values is M8.3's ("every value used, clamped or refused with a warning (Dickson, 2026-11-05)", `docs/roadmap.md` 1692-1695).

Fix (both reviewers, merged): on a load error, rename `settings.toml` to `settings.toml.corrupted.bak` (replacing an older one), start from `ConfigStore::from_settings(exe_dir, Settings::default())`, drop the temporary-folder load and its panic, log after tracing starts, and show NVDA's message box in NVDA's words through Fluent once the GUI is up. The box is new speech at start that matches NVDA's; its exact speech needs a capture. Add one app test: a bad file is kept as the backup and the saver's first write does not touch it.

### P1. The Terminal page's per-control plumbing (M4.3)

Verified: the Speech page is generic (`ffi::SettingControl`, `bridge.rs` 465-489; callbacks `slider_changed`, `choice_changed`, `toggle_changed`, `bridge.rs` 631-635; one C++ builder `BuildControls`, `gui.cpp` 288-346). The Terminal page writes the same widgets by hand (`gui.cpp` 945-1003). Both reviewers' estimates differ (A 140, B 300 product); recounted: `TerminalPage` 26 lines, five externs 12, `with_terminal` and five `GuiCore` methods 27, C++ class and comment 69, `TerminalLabels` 22, `page()` and four setters 38: about 195, less about 35 for the rows and a page kind, so about 160 net. Commit 643e25cc (`git show --stat`: 21 files, 894 insertions; B's "17 files, about 880" is wrong) added the page. The per-setting list and the proposed shape are in section 5. Class: M4.3 adds the dialog controls this would serve, so it belongs at M4.3's start.

### R1. One function per message in i18n

`crates/verbatim-i18n/src/lib.rs` 248-720 is `pub mod messages`, 50 functions (counted with awk) of the form
```
pub fn tray_tooltip() -> String {
    fl!(loader(), "tray-tooltip")
}
```
plus `ThemeLabels` and `TerminalLabels`, copied again field by field into bridge structs. A proposes a `t!("id")` macro expanding to `fl!` so callers name the id. Correction from the macro's source (`i18n-embed-fl` 0.10.0, `src/lib.rs` 402-424): `fl!` finds `i18n.toml` from the manifest of the crate being compiled, so a macro called from `verbatim-gui` or `verbatim-app` checks ids only if each calling crate gets its own `i18n.toml` with `assets_dir` pointing at `../verbatim-i18n/i18n`. With that three-line file per caller, the compile-time check is kept and about 250 product lines go (A said 300, B 470 including the structs). Functions with logic stay (`clipboard_copied`, `theme_indication_summary`, the enum mappings).

### R2. `ControlPlan`

`plan.rs` 12-56 restates `SettingDescriptor` with an initial value; `plan_for` 86-135 builds it, and `settings.rs` 203-252 copies it into `ffi::SettingControl`; `settings.rs` 123-134 and 190-199 match the plan against a value's shape. `plan.rs` 81-84: "falls back to a safe default (the slider minimum, no selection, or unchecked) rather than panicking", pinned by `mismatched_value_shape_falls_back_instead_of_panicking` (`plan.rs` 278). B's line numbers for these (257-316, 321-324, 337-340, 372, 517-526) do not exist in a 327-line file; the quotation is at 81-84. Grep `ControlPlan|plan_for` over `crates/`, `xtask/`, `uia-facts/`: used only in `plan.rs`, `settings.rs` and the re-export at `lib.rs` 45. Fix: `SpeechControls` keeps `(SettingDescriptor, value)` and builds `SettingControl` directly, logging a wrong-shaped value as the driver's error. `plan.rs` 7 also names `crate::dialog`, which does not exist (`grep '^mod' lib.rs`).

### D2. The single-instance title (A only; confirmed, but A's consequence is wrong)

`crates/verbatim-gui/src/lib.rs` 211-213: `title: messages::tray_tooltip(), tooltip: messages::tray_tooltip(),`; the frame is created with it (`gui.cpp` 1360, `new HiddenFrame(nullptr, wxID_ANY, shell_.title, ...)`). `single_instance.rs` 49-52: "Deliberately not localized: it must be stable across locales for `FindWindowW` to work between differently configured builds. `const WINDOW_TITLE: &str = "Verbatim";`", searched at 220-232.

A says two Verbatims would then run. They would not: the running instance holds the startup mutex `Local\Verbatim` for its whole life (`InstanceGuard`, `single_instance.rs` 66-69, "Holds the startup mutex for the life of this instance"). The sequence: a translation names `tray-tooltip` anything but "Verbatim"; Verbatim starts with that title; a second `verbatim.exe` finds no window titled "Verbatim", so `replace_running_instance` returns `Gone`; it waits 2,000 ms on the mutex (`MUTEX_WAIT_MS`, 65), fails with "another Verbatim instance is still running and did not exit in time" (`acquire_replacing`, 193-198), writes that to standard error and exits. The old instance keeps running; no message box is shown (`show_not_ended` runs only for `StillRunning`). So a restart or relaunch silently does nothing.

Reachable today only by a user-made translation: the repository ships one Fluent file, `crates/verbatim-i18n/i18n/en/verbatim.ftl` (find over `crates/`: one `.ftl`), where line 14 is `tray-tooltip = Verbatim`; translations load only from a `locale` folder beside the executable (`main.rs` 612-625, i18n `lib.rs` 118-146), and nothing in `xtask/` or the repository provides one. It becomes reachable for everyone with M8.3's "localized UI shipped in at least two languages" (`docs/roadmap.md` 1690-1691) if that translation changes the tooltip. Fix: one constant beside `HIDDEN_FRAME_WINDOW_PROP` (`crates/verbatim-model/src/lib.rs` 174) for the frame title and `single_instance`; `ShellText.title` goes; one assertion.

### R3. Silent wildcards behind `#[non_exhaustive]`

Quoted at i18n `lib.rs` 763-765: "`Message` is non_exhaustive; an unmapped future message speaks nothing rather than crashing the pipeline. `_ => String::new(),`"; the same at 908-910 (`Phrase`), 1325-1327 (`TextFormat`), 1396-1398 (`Earcon`), and with a fallback text at 1094-1096 (`Role`, `role-unknown`), 1259-1261 (`Indication`, its id) and 1455-1458 (`ThemeProblem`, its English). The attributes: model `speech.rs` 141, 213, 298; `event.rs` 1201; `theme.rs` 94, 775; `tree.rs` 24. A script listing each enum's variants against its match found every variant mapped except `Role::Unknown`, which only the wildcard covers, so it needs its own arm. Precedent: `books/dev/src/explanation/design/ownership.md` 96, "`#[non_exhaustive]` was removed from the moved enums, since the processes are one build." Sequence: a builder adds a `Phrase` variant in Core and no arm; it compiles; the key that reports it says nothing. Fix: drop the attribute from these seven enums and the arms (wildcards on them in other crates then fail Clippy and go too).

### Q1. The Terminal page's diff-apply (A only; confirmed)

`terminal_panel.rs` 11-16: "Apply sends Core only the settings changed since the page opened or was last applied, so a toggle key pressed while the dialog is open is not undone by applying another setting." The mechanism is `TerminalChange` (35-73), `Values::change_from` (95-106), `committed` (121) and `apply` (175-183); the app merges it on the reducer thread (`main.rs` 1428-1433). Commit 643e25cc's message gives the reason; `docs/parity/system.md` 235-250 rules the page itself (Dickson, 2026-10-06) but not this; the crate guide (`verbatim-gui.md` 309-311) describes it without a ruling. NVDA's panels write every control on save, for example `settingsDialogs.py` 2594, `config.conf["presentation"]["reportDynamicContentChanges"] = self.dynamicContentCheckBox.IsChecked()`. See section 4 for the sequence.

### R5. Modifier keys and layout declared twice (B only; confirmed)

Config `lib.rs` 98-107 defaults `caps_lock`, `insert`, `numpad_insert` true and `share_modifier` false; `verbatim-input/src/state.rs` 49-80 (`DecisionConfig`) repeats the fields and defaults; `scripts.rs` 22-38 redeclares `KeyboardLayout` "so this crate stays decoupled from configuration"; app `main.rs` 228-231 and 756-768 translate. Both crates already depend on `verbatim-model` (`Cargo.toml`), where `ReaderSettings` shows the pattern. The "decoupled" pattern is recorded only in the crate guides (`verbatim-config.md` 66, `verbatim-input.md` 76), not as a decision. `DecisionConfig` keeps its own `multi_press_timeout`.

### R6. `ConfigValue` and `SettingValue` (A only; confirmed, lines corrected)

Config `lib.rs` 214-223 `ConfigValue { Flag(bool), Integer(i64), Text(String) }`; `verbatim-speech/src/settings.rs` 62-69 `SettingValue { Number(i32), Choice(String), Toggle(bool) }`, which already derives `Serialize`/`Deserialize`. Converted at app `speech_settings.rs` 34-38 and 59-61 (A gave 109-115 and 134-138, past the end of the 67-line file). Line 35, `SettingValue::Number(i32::try_from(*number).ok()?)`, drops an out-of-range saved value without a word.

### R4. Gain and earcon mapping

`theme_panel.rs` 627-632 ("The gain a sound of `setting` plays at, as speech plays it.") and `verbatim-speech/src/theme.rs` 197-203 hold the same expression, `percent(theme.gain) * percent(setting.gain) * f32::from(self.options.sound_volume.min(100)) / 100.0`. `theme_panel.rs` 927-941 `earcon_of` inverts model `theme.rs` 348-362 `Indication::of_earcon`. One `gain` in the model and `earcon_of` derived from `of_earcon`.

### R7. GUI lifecycle facts held twice (A only; confirmed)

`Lifecycle` (`lifecycle.rs` 73-80) holds `settings_open` and `shell_list: ShellList` (`Closed`, `Enumerating`, `Open`, 64-70); `GuiCore` holds beside it `theme: RefCell<Option<ThemePanel>>`, `terminal: RefCell<Option<TerminalPanel>>` and `list_buttons: RefCell<Option<ListDialogButtons>>` (`lib.rs` 242-262), cleared by hand in `dialog_closed` (`lib.rs` 464-479). Fold the panels into `Lifecycle`'s open variants.

### R8. Bounded values

Model `settings.rs` 87 and 92 are `u16`, clamped in `full_lines`/`last_lines` (118-126), again in `terminal_panel.rs` 89-90 and `line_limit` 110-112 (with an unreachable `unwrap_or(MAX_TERMINAL_LINES)`). `sound_volume: u8` (model `theme.rs` 895) is clamped by `.min(100)` at config `lib.rs` 378, `theme_panel.rs` 202 and 630, and speech `theme.rs` 201 (`theme_panel.rs` 637 clamps slider input, a boundary, and stays). Newtypes clamping on deserialize make the out-of-range value unrepresentable; the reviewers' line savings (A 15, B 40) ignore the newtypes' own code, so the product saving is about nil and the gain is the invalid state removed plus about 20 test lines.

### D3. Theme errors in English (B only; confirmed)

`ThemeError`'s `Display` (config `themes.rs` 133-151) holds English such as "a theme with the id `{id}` is installed"; `theme_panel.rs` returns `error.to_string()` at 481, 679, 698, 711, 720, 739 and 750, and the C++ shows it in a message box. D10 (`architecture.md` 84-88): "every lookup goes through `fl!`, and no user-visible string is hardcoded". Fix: a `theme_error_text` beside `theme_problem_text` (i18n `lib.rs` 1405). The English shown is unchanged.

### D4. Runtime lookup shows Fluent's error (A only; confirmed)

i18n `lib.rs` 241-243 `pub fn message(id: &str) -> String { loader().get(id) }`; `i18n-embed` 0.16.0 `src/fluent.rs` 179-187 logs an error and returns `format!("No localization for id: \"{}\"", message_id)`. Callers pass ids that arrive as data: `gui/settings.rs` 204 (descriptor label keys), app `main.rs` 656, `verbatim-synth-onecore/src/lib.rs` 49. A mistyped key becomes a control's spoken label. Fix: return `Option<String>` and log the missing id, plus one test that every registered synthesizer's label keys resolve.

### R9. Key enums across the bridge

`bridge.rs` 556, 571 and 592 declare `SettingsKey`, `SettingsFocus`, `SettingsKeyAction`; `keys.rs` declares `Key`, `FocusedControl`, `DialogButton`, `KeyAction`; `lib.rs` 741-790 maps between them, with `_ =>` arms because cxx's shared enums are open; `lib.rs` 802-838 (`the_bridge_routes_keys_as_the_router_does`, 41 lines) repeats cases `keys.rs`'s tests cover. B's "route_key can take and return the ffi enums directly" (about 75 product lines) would put the open-enum wildcards inside the router and lose its exhaustive match; A's narrower change (delete the bridge test, optionally return a small struct) is the one recorded. `next_category` (`lib.rs` 794-796) is a one-line wrapper.

### R10. Atomic writes and an infallible `persist`

Config `lib.rs` 571-578 `write_toml_atomic` and `themes.rs` 652-659 `write_atomic` have the same body; `package.rs` 98-110 is a third variant. `ThemeHost::persist` (`theme_panel.rs` 62) returns `Result<(), String>`; grep `fn persist` finds the app's (`themes.rs` 70-78), the test host's (`theme_panel.rs` 1028-1031) and the example's, all `Ok(())`, so `theme_panel.rs` 755-757 cannot run. B's second dead site, `lib.rs` 533-537, is the theme apply's other failures (file writes), which can happen; not dead.

### R11. Dead i18n wrappers (B only; confirmed)

Grep `settings_title\b` over `crates/`, `xtask/`, `uia-facts/`: 1 hit, the definition (`lib.rs` 323). `startup_message`: the definition (232) and one test (1468). `toggle_key_state(key: &str, on)` (287-299) receives the string `phrase_text` just made from `LockKey` (897-904) and maps it back with `_ => fl!(loader, "toggle-caps-lock")`; take `LockKey`.

### R12. The example (both; A as removal, B as a question)

Grep `settings_preview` over the repository outside `target/` and `nvda/`: 0 references (scripts, documents, workflows). Its header: "the lead session runs it manually". `git log` on the file: 11 commits. It is a developer tool, not a feature, and changes no speech or decision, so it is Remove now; B's question is answered by that.

### P2. Profiles (M8.3)

`overlays` is `Vec::new()` in both constructors (config `lib.rs` 475, 553); grep `ProfileTheme|Profile \{` outside `verbatim-config`: 0 hits. `ActiveConfig` is still read (`main.rs` 749, app `themes.rs` 112) over an empty overlay list, while the Theme page saves the base section directly ("until M8.3 activates profiles", app `themes.rs` 66-67). `ensure_files_exist` creates a `profiles` folder nothing reads (526-527). M8.3, "Configuration profiles (manual and triggered)" (`docs/roadmap.md` 1690), has no design. Planned phase; deleting the scaffold now is the reviewers' proposal and is listed under section 4.

### R13. Stale text and an unused default

- Model `settings.rs` 62: "Speak typed characters (NVDA's default: always)"; the default (132) and NVDA's are "only in edit controls" (`configSpec.py` 203-204, `# 0: Off, 1: Only in edit controls, 2: Always` and `speakTypedCharacters = integer(default=1,min=0,max=2)`).
- `TypingEcho`'s `#[default] Always` (model `settings.rs` 23): grep `TypingEcho::default` over the three trees, 0 hits; `ReaderSettings` uses its own `Default`. Remove the derive.
- `gui.cpp` 920-922: "a line limit's range is 1 to 100, so 30 read as 29"; 10,000 since 15715b57 (`git show --stat`: 7 files).
- `main.rs` 571-572 names "profiles/base.toml"; `load_config`'s "without overwriting the files" (D1).
- Config `lib.rs` 281-282: "`None` follows the Windows preferred languages"; `main.rs` 613-615 returns on `None`, so it is embedded English. Fix the comment; following Windows is M8.3's.
- `verbatim-ext/src/lib.rs` 3-5: "on dedicated in-process threads ... Capabilities are deny-by-default and manifest-declared" against D6 (`architecture.md` 49-56): "capabilities derived from a component's imports, never declared ... no Wasm runtime is in Core".

### Q3. The `AttachThreadInput` fallback (A only; confirmed as stated)

`foreground.rs` 42 and the crate guide (`verbatim-gui.md` 196-197) record the Control tap's measurement ("roughly 150 to 450 ms, versus roughly two seconds for the `AttachThreadInput` fallback the code still keeps for the case even the tap does not help"); no case where the tap fails is recorded. NVDA's `prePopup` raises its frame (`gui/__init__.py` 186-188). Removing it needs a measurement that the tap alone always succeeds; a question, low value.

### P3. Tray list details (B only; confirmed, planned)

`tray_list.rs` 184 `.filter(|candidate| !candidate.offscreen)` makes `usable_position`'s own offscreen check (`shell_items.rs` 87-89) unreachable from this caller; `KeepOpen` (`list_dialog.rs` 30-37) is dead outside tests by its own `expect(dead_code, reason = "... M7's elements list will keep its dialog open")`; `list_dialog.rs` 103-105 clamps a bad `default_button`. The list becomes a global module in M5; the elements list is M7.

## 3. Test findings

| Crate | Tests, lines | Removable | Replaceable | To add |
|---|---|---|---|---|
| verbatim-config | 25 tests, 643 lines | 4 tests, about 35 lines: `keyboard_layout_defaults_to_desktop` (restates `#[default]`), `terminal_settings_read_from_the_reader_section` (repeats the model's clamp test and serde), and in `typing_and_reading_settings_default_to_nvdas` the tautology `assert_eq!(settings.reader, ReaderSettings::default())` (726) | `keyboard_layout_round_trips_through_toml`, `reader_settings_round_trip_through_toml`, `the_theme_section_round_trips_through_toml` (52 lines) by one `save_and_reload_round_trips` over a `Settings` with every value changed: about 37 saved | none |
| verbatim-config, M8.3 (P2) | | `a_profile_file_cannot_carry_global_settings`, `a_profile_file_cannot_carry_a_keyboard_section`, `layered_resolution_prefers_overlays_over_the_base`, `the_theme_resolves_from_the_profile_then_the_base_then_the_default`: 103 lines | | |
| verbatim-gui | 72 tests, 1,542 lines | `the_bridge_routes_keys_as_the_router_does` (41, R9); with R8, `terminal_panel.rs`'s limits test (about 20) | `plan.rs`'s six `ControlPlan` tests (191-288, about 98 lines) by two builder tests in `settings.rs` (about 35): about 60 saved | the frame title equals the single-instance constant (D2), 5 lines |
| verbatim-gui, planned | | M5 and M7 (P3): `the_default_button_is_clamped_into_range`, the `KeepOpen` half of `callbacks_stay_with_their_buttons`, the tray list's offscreen test overlapping `shell_items`': about 25 | M4.3 (P1): `terminal_panel.rs`'s six tests (167) by two over the row table: about 100 saved | |
| verbatim-gui example | 259 lines | `examples/settings_preview.rs` (R12) | | |
| verbatim-i18n | 18 tests, 692 lines | `the_failed_replacement_box_is_nvdas` (9), `tray_list_messages_resolve` (22), the `startup_message` assertion: about 33 | `every_indication_and_category_has_a_name` (86, its own table of English) by iteration over the catalogue checking each id resolves: about 70 saved | every registered synthesizer's label keys resolve (D4), about 15 |
| verbatim-app (settings) | 2 tests, 133 lines | none | none | a bad `settings.toml` is kept as `.corrupted.bak` and the first save does not touch it (D1), about 25 |
| verbatim-ext, verbatim-ext-api | none | | | |

B's i18n figure (about 380 removable) is contradicted for most of its list: `every_new_format_is_worded` tests `underline_text` and `bullet_text`'s collapsing (for example `LineStyle::Wavy` to "strikethrough"), `theme_words_are_worded` tests `format_text` and `earcon_text(Earcon::Progress(40))` to "40 percent", and the phrase tests check which message each variant maps to, which `fl!`'s compile-time id check cannot catch. They stay. B's removal of `cycling_wraps_in_both_directions` is also dropped: `cycle_index` is live (`lib.rs` 795) and has wrap logic.

No generated tests in the area. `demo_settings_dialog_keys` reuses the body of `settings_dialog_keys` (8 lines) and is skipped by `--skip demo_`. The dialog scenarios cannot take expectations from NVDA captures for Verbatim's own pages, which is correct. The gap that matters is D1's: no test starts over a bad file and then saves.

Totals: removable about 380 unit lines plus the 259-line example; replaceable about 170 lines saved; to add about 70.

## 4. Questions for Dickson

### Q1. Should the Terminal page write all four values on OK, as NVDA's panels do?

Sequence: open the settings dialog's Terminal page with "Report new output" checked; press Verbatim+5, heard "report new output off" (`message-report-new-output-off`, Fluent line 239; the "on" form is pinned by `terminal_settings_page`, `crates/verbatim-e2e/src/scenarios/terminal_settings_page.rs` 63); move "Lines spoken in full" to 60; press OK.
- Today: only the limit is sent; "Report new output" stays off; the next terminal output is not spoken.
- NVDA's model: the page's check box, still checked, is written back; the setting is on again, silently; the next terminal output is spoken.
No NVDA capture can apply, since NVDA has no Terminal page; nothing is spoken at OK under either option. Choosing NVDA's model removes about 45 product and 40 test lines and the reducer-side merge's reason.

### Q2. Delete the profile scaffold now, or keep it for M8.3? (P2)

It changes no speech or feature; it is in Q only because M8.3 is a planned phase. About 120 product and 103 test lines.

### Q3. Remove the `AttachThreadInput` fallback once a measurement shows the Control tap always brings the frame forward?

No speech change; about 15 lines.

## 5. The cost of one behaviour here, and the shared shape

### Every place one setting is declared, defaulted, validated, stored, shown, applied and sent

Merged from both traces and checked at each line.

`speak_typed_characters` (changes speech; toggled by a key; not in the dialog):
- declared: model `settings.rs` 63, with `TypingEcho` 17-25 and its toggle order `next` 27-37;
- defaulted: model `settings.rs` 132 (and the unused `#[default] Always` at 23, R13);
- validated: serde only, inside `Settings.reader`, `#[serde(default)]` (model `settings.rs` 56); one bad word fails the whole file (D1);
- stored: `settings.toml` `[reader]` through the saver (`saver.rs` 74-78, 142-145), Core's `state.settings`, and the `ReaderView` copy (`terminal_settings.rs` 22-35);
- shown: not in the dialog (M4.3); spoken when toggled through `Phrase::SpeakTypedCharacters` (model `speech.rs` 255), i18n `lib.rs` 857-861, `typing_echo_name` 941-950, Fluent 256 and 273-275;
- bound: input `scripts.rs` 121, 181, 241 to `ReviewCommand::ToggleTypedCharacters` (model `event.rs` 1169);
- applied: read at app `main.rs` 239, given as the first `Input::Settings` (`main.rs` 2038), used at core `editing.rs` 1004, toggled at `reduce.rs` 1089 and 1113-1117, saved through `Effect::SettingsChanged` (`reduce.rs` 1128), `main.rs` 1591, `terminal_settings.rs` 48-51;
- sent to another process: never.

`report_background_progress_bars` (a reader process needs it):
- declared and defaulted: model `settings.rs` 93-99, 141; validated: a serde bool;
- read at start: `main.rs` 239; passed at launch: `main.rs` 313 to `Supervisor::new` (supervisor `lib.rs` 515, 524, 567, 586), kept by the owner (`owner.rs` 94, 156, 398, 435, 457), written as `--background-bars` (`owner.rs` 1241; `verbatim-process/src/arguments.rs` 45, 156, 171, 216), parsed by the listener (`listener/src/main.rs` 4, 32; `listener.rs` 2023-2026) into `BackgroundBars` (`configuration.rs` 36-38);
- sent again from Core at every `Input::Settings`: `reduce.rs` 117-125 `Effect::ListenerConfiguration`, `main.rs` 1592-1597, supervisor `lib.rs` 745-747, `owner.rs` 794-798, `CoreToListener::Configuration`, applied `listener.rs` 891-895;
- applied: Core `changes.rs` 325, 645, `entries.rs` 444, 871, `reduce.rs` 1490; the listener's read gate (`configuration.rs` 104-123, `listener.rs` 1528, 1602);
- shown: not in the dialog (M4.3). About 20 places in 11 files for one boolean. Both paths are ruled: "Core sends it at the reader's launch and on every change" (`architecture.md` 680-685, Dickson, 2026-11-19), so neither reviewer proposes a change; commit ccdb2a7a (`git show --stat`: 52 files; B said 45) built it with the read gate.

`terminal_full_lines` (the GUI edits it):
- declared: model `settings.rs` 87, bounds `MAX_TERMINAL_LINES` 108 and default `DEFAULT_TERMINAL_LINES` 113; defaulted 139;
- validated: never at parse; clamped at each read (model 118-120; `terminal_panel.rs` 89, 110-112) (R8);
- stored: as above;
- shown: `TerminalChange.full_lines` and its `apply_to` arm (`terminal_panel.rs` 43, 63-65), `Values.full_lines`, `Values::of`, `change_from` (79, 89, 102), `page()` (143-148), `set_full_lines` (160-163); bridge `TerminalPage` fields (`bridge.rs` 245-256) and extern (705); `GuiCore::terminal_full_lines_changed` (`lib.rs` 672-674); C++ label, slider, accessible, binding and member (`gui.cpp` 960-967, 987-989, 1000); `TerminalLabels.full_lines` (i18n `lib.rs` 593, 605); Fluent line 91;
- applied: Apply sends a `TerminalChange` (app `terminal_settings.rs` 67-75) as `ShellCommand::TerminalSettings` (`main.rs` 895), merged at `main.rs` 1428-1433 into `Input::Settings` (`reduce.rs` 110-125), used by Core's terminal reading; saved through `reader.changed`;
- sent to another process: only inside Core's reads.

Speech rate (a driver setting on the generic page): declared by each driver's descriptor (`SettingDescriptor::standard_numeric("rate", "setting-rate")`, `verbatim-synth-capture/src/lib.rs` 125; `verbatim-speech/src/hosting.rs` 360), labelled Fluent 128, stored as `ConfigValue::Integer` in `synth_settings` (config `lib.rs` 233) and converted both ways (`speech_settings.rs` 34-38, 59-61), planned `plan.rs` 86-110, described `settings.rs` 203-232, built by the generic `gui.cpp` 297-308, applied through `SpeechSettingsHost::set_setting` (`lib.rs` 498) and, for a hosted synthesizer, `ToHost::SetSetting` (`verbatim-synth-hosted/src/lib.rs` 448; `verbatim-synth-host/src/lib.rs` 397), answered and snapped back (`settings.rs` 159-185), saved on commit (`manager.rs` 1352-1358).

Counted: a reader setting edited on a hand-written page touches about 20 places in 7 files (A's count for `speak_terminal_passwords`, 20 places; B's 15 for `terminal_full_lines` without the model and Fluent); a driver setting on the generic page touches only its descriptor and its Fluent label. NVDA needs a `configSpec.py` line, about 10 lines in a panel's `makeSettings` and `onSave`, and the use (`ReviewCursorPanel`, `settingsDialogs.py` 2303-2340).

### The shape both propose

One table of setting rows driving the existing generic page code. Each row: the setting's id, its Fluent label id, its kind with range and steps (check box, slider, choice), and a read and a write on `ReaderSettings`. A page is the list of `ffi::SettingControl`s built from its rows, sent through the existing `slider_changed`, `choice_changed`, `toggle_changed` callbacks with a page kind, and built by the existing `BuildControls`; a flag on the page makes it apply on OK or Apply rather than live; `SliderPositionAccessible` (`gui.cpp` 924-938) becomes a flag on `SettingControl`.

What it removes now (P1): `ffi::TerminalPage`, five externs, `with_terminal` and five `GuiCore` methods, `page()` and four setters, the C++ `TerminalPanel`, `TerminalLabels` and `terminal_labels()`: about 160 product lines net, and about 100 test lines when the six panel tests become two over the table. With Q1 answered for NVDA's model, `TerminalChange`'s options, `change_from` and `committed` go too.

What it saves for M4.3. The entry (`docs/roadmap.md` 664-678) plans "the missing settings for behaviour Verbatim already has (capital pitch change, OneCore's pause after punctuation, the audio output device, sounds' volume following the voice, handling keys from other applications, the multiple key press timeout, the review cursor following focus, reporting tooltips, reporting notifications, the word segmentation standard, cancelling speech for expired focus events, trimming leading silence); dialog controls for the nine settings that exist only in `settings.toml`; and three small features (say "cap" before capitals, delayed character descriptions, the spelling-error sound while typing)", plus "The review cursor follows focus and the caret by default, each with NVDA's setting to turn it off", and "Background progress bar reporting with its setting". That is 12 named settings, 9 dialog controls for existing ones, and 3 features each with a setting: about 24 controls (B's 21 left out the three features). At today's rate, each reader setting on a hand-written page is about 15 touches across the page, bridge, C++ and labels; with the table it is the `ReaderSettings` field and default, the Fluent label and one row. Saving about 12 touches and, at the Terminal page's measured cost (about 440 product lines for 4 controls, about 110 each, against about 10 for a row and a label), roughly 100 product lines per setting: on the order of 2,000 to 2,400 lines across M4.3's 24 controls, and the per-control tests become rows in two table tests. Settings that belong to a driver (OneCore's pause, capital pitch change) already take the generic path and gain nothing.

Two further cuts, merged: bounded values parsed once (R8) remove each setting's clamps and clamp tests; and for settings a reader process needs, one configuration message carrying the reader processes' settings would replace a flag, a field and a handler per setting, a question under the 2026-11-19 ruling with no changed fact, so not proposed.

## 6. Dropped

- A, consequence of D2: "two instances hook the keyboard and both speak": contradicted by `single_instance.rs` 66-69 and 180-198 (the first instance holds the startup mutex; the second exits). The finding stays with the corrected sequence.
- B, F7's product claim that `route_key` can take the ffi enums directly for about 75 lines: it would move cxx's open-enum wildcards into the router; only the test removal is kept.
- B, F8's line numbers (`plan.rs` 257-316, 321-324, 337-340, 372, 517-526): not in the 327-line file; the finding stands from A's lines.
- B, F14's `lib.rs` 533-537 as a dead failure path: it handles theme file write failures, which can occur.
- B, test findings: `every_new_format_is_worded`, `theme_words_are_worded`, `shell_phrases_are_worded`, `theme_phrases_are_worded`, `role_and_state_names_resolve` and the phrase part of `selection_and_toggle_phrases_are_nvdas` as removable: they test mappings and formatting logic `fl!` cannot check; `cycling_wraps_in_both_directions`: tests live wrap logic.
- A, finding 5's "keeps that check": incorrect as stated (`fl!` reads the calling crate's `i18n.toml`); kept with the needed `i18n.toml` per caller.
- A, finding 7's runtime `format!("indication-{}")` lookup: it would trade R3's compile-time check for a runtime one; only its test replacement is kept (section 3).
- A, R6's lines `speech_settings.rs` 109-115 and 134-138: past the file's end; corrected to 34-38 and 59-61.
- A, finding 18's `report_background_progress_bars` launch-parameter question and B's F15: ruled on 2026-11-19 with no changed fact; recorded as cost in section 5 only.
- A, the claim that dd625bda added the background-bars setting in "6 lines" of `settings.rs`: not needed for any finding and not checked further; omitted.
- Commit counts corrected: 643e25cc is 21 files and 894 insertions (B said 17 and about 880); ccdb2a7a is 52 files (B said 45).
