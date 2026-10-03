# English resources, compiled into the binary as the permanent fallback.
#
# Keep every message on a single line: the pseudo-locale test parses this
# file line by line and multiline patterns would evade it.

startup-message = Verbatim is starting.

## The Verbatim menu and tray icon.

tray-tooltip = Verbatim
menu-settings = &Settings...
menu-exit = E&xit

## The settings dialog.

settings-title = Verbatim Settings
settings-title-with-category = Verbatim Settings: { $category }
settings-categories-label = &Categories:
settings-category-speech = Speech
button-ok = OK
button-cancel = Cancel
button-apply = &Apply

## The Speech settings page and the Select Synthesizer dialog.

speech-synthesizer-group = Synthesizer
speech-change-synth = C&hange...
select-synth-title = Select Synthesizer
select-synth-label = &Synthesizer:

## The system tray and taskbar list dialogs (the systrayList replica).

tray-list-title = System Tray Icons
tray-list-label = &Icons
taskbar-list-title = Taskbar Buttons
taskbar-list-label = &Buttons
tray-list-left-click = &Left Click
tray-list-left-double-click = Left &Double Click
tray-list-right-click = &Right Click

## Synthesizer display names.

synth-name-onecore = Windows OneCore voices

## Synthesizer setting labels, referenced by setting descriptors.

setting-voice = &Voice
setting-variant = V&ariant
setting-rate = &Rate
setting-rate-boost = Rate boos&t
setting-pitch = &Pitch
setting-inflection = &Inflection
setting-volume = V&olume

## Spoken role names.

role-window = window
role-dialog = dialog
role-pane = pane
role-property-page = property page
role-group = grouping
role-menu-bar = menu bar
role-menu = menu
role-menu-item = menu item
role-button = button
role-toggle-button = toggle button
role-check-box = check box
role-radio-button = radio button
role-combo-box = combo box
role-list = list
role-list-item = list item
role-slider = slider
role-spin-button = spin button
role-tab-control = tab control
role-tab = tab
role-static-text = text
role-editable-text = edit
role-link = link
role-tool-bar = tool bar
role-status-bar = status bar
role-tree = tree view
role-tree-item = tree view item
role-split-button = split button
role-drop-down-button = drop down button
role-menu-button = menu button
role-graphic = graphic
role-progress-bar = progress bar
role-scroll-bar = scroll bar
role-table = table
role-row = row
role-cell = cell
role-column-header = column header
role-row-header = row header
role-header = header
role-header-item = header item
role-data-grid = data grid
role-data-item = data item
role-calendar = calendar
role-tool-tip = tool tip
role-title-bar = title bar
role-separator = separator
role-document = document
role-application = application
role-alert = alert
role-hotkey-field = hot key field
role-thumb = thumb control
role-unknown = unknown

## Reader messages: fixed announcements that describe the reader's own
## outcome rather than any node's property. Wording matches NVDA's.

message-no-next-object = No next
message-no-previous-object = No previous
message-no-containing-object = No containing object
message-no-objects-inside = No objects inside
message-top = Top
message-bottom = Bottom
message-left = Left
message-right = Right
message-blank = blank
message-move-to-focus = Move to focus
message-no-navigator-object = No navigator object
message-activate = Activate
message-no-action = No action
message-space = space

## A lock key's new state, as NVDA announces it: the key, then on or off.

toggle-caps-lock = caps lock
toggle-num-lock = num lock
toggle-scroll-lock = scroll lock
toggle-state-on = { $key } on
toggle-state-off = { $key } off

## Spoken state names.

state-focused = focused
state-offscreen = off screen
state-selected = selected
state-not-selected = not selected
state-checked = checked
state-not-checked = not checked
state-mixed = half checked
state-disabled = unavailable
state-read-only = read only
state-expanded = expanded
state-collapsed = collapsed
state-pressed = pressed
state-not-pressed = not pressed
state-has-popup = submenu
state-busy = busy
state-protected = protected
state-required = required
state-invalid-entry = invalid entry

## Spoken object details.

object-position-in-set = { $position } of { $set_size }
object-level = level { $level }

## Clipboard.

clipboard-copied = Copied to clipboard: { $text }
clipboard-copy-failed = Unable to copy
# Used only for 1024 characters or more, so always plural.
clipboard-characters = { $count } characters
