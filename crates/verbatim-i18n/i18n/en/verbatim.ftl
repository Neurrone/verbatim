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
synth-name-espeak = eSpeak NG

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
role-terminal = terminal
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
message-invoke = invoke
message-space = space
message-start-marked = Start marked
message-no-start-marker = No start marker set
message-start-marker-elsewhere = The start marker must reside within the same object
message-caret-moves-review = caret moves review cursor
message-caret-does-not-move-review = caret doesn't move review cursor
message-not-supported = Not supported in this document
message-no-caret = No caret

## Reader messages with values in them. Wording matches NVDA's.

phrase-selected = selected { $text }
phrase-unselected = unselected { $text }
# Used only for 512 characters or more, so always plural.
phrase-characters = { $count } characters
phrase-positioned = Positioned at { $x }, { $y }
phrase-speak-typed-characters = speak typed characters { $mode }
phrase-speak-typed-words = speak typed words { $mode }
phrase-skipped-line = skipped { $count } line
phrase-skipped-lines = skipped { $count } lines
typing-echo-off = off
typing-echo-edit-controls = only in edit controls
typing-echo-always = always

## A lock key's new state, as NVDA announces it: the key, then on or off.

toggle-caps-lock = caps lock
toggle-num-lock = num lock
toggle-scroll-lock = scroll lock
toggle-state-on = { $key } on
toggle-state-off = { $key } off

## Themes (phase6-design.md, "Themes: one model for verbosity, speech, and
## sounds"): the indication catalogue's categories and entries as the theme
## panel lists them, how each is reported, the words for formatting and
## events, and the problems found loading a theme.

theme-default-name = Default
theme-default-description = Everything spoken as NVDA speaks it, with NVDA's sounds where NVDA plays them.
indication-category-roles = Roles
indication-category-states = States
indication-category-properties = Properties
indication-category-text-formatting = Text formatting
indication-category-structure = Structure
indication-category-events = Events
presentation-off = off
presentation-speech = speech
presentation-sound = sound
presentation-speech-and-sound = speech and sound
indication-description = description
indication-shortcut = shortcut key
indication-position = position
indication-level = level
indication-spelling-error = spelling error
indication-grammar-error = grammar error
indication-font-name = font name
indication-font-size = font size
indication-color = color
indication-capital = capital letter
indication-blank = blank
indication-skipped-lines = skipped lines
indication-app-not-responding = application not responding
indication-start = start
indication-exit = exit
indication-error = error
indication-browse-mode = browse mode
indication-focus-mode = focus mode
indication-suggestions-opened = suggestions opened
indication-suggestions-closed = suggestions closed
indication-progress = progress bar
format-spelling-error = spelling error
format-not-spelling-error = out of spelling error
format-grammar-error = grammar error
format-not-grammar-error = out of grammar error
earcon-app-not-responding = not responding
earcon-start = Verbatim started
earcon-exit = Exiting Verbatim
earcon-error = error
earcon-browse-mode = browse mode
earcon-focus-mode = focus mode
earcon-suggestions-opened = suggestions
earcon-suggestions-closed = suggestions closed
earcon-progress = { $percent } percent
theme-problem-unknown-indication = Unknown indication { $id }
theme-problem-sound-only-without-sound = { $indication } is reported by sound, but has no sound
theme-problem-missing-sound = { $indication }: the sound { $file } is missing
theme-problem-unreadable-sound = { $indication }: the sound { $file } cannot be played: { $reason }
theme-problem-invalid-sound-name = { $indication }: { $file } is not a sound file name
theme-problem-unknown-voice-style = { $indication }: there is no voice style named { $style }
theme-problem-gain-too-high = { $indication }: the gain is above the most allowed
theme-problem-theme-gain-too-high = The theme's gain is above the most allowed

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

## The character table: how one character is named when it is spoken on its
## own (caret and review movement by character, spelling), keyed by its
## code point in lowercase hexadecimal. NVDA's English symbol names, with
## its corrected ones ("superscript minus", "three eighths").

character-name-0009 = tab
character-name-000a = line feed
character-name-000d = carriage return
character-name-0020 = space
character-name-0021 = bang
character-name-0022 = quote
character-name-0023 = number
character-name-0024 = dollar
character-name-0025 = percent
character-name-0026 = and
character-name-0027 = tick
character-name-0028 = left paren
character-name-0029 = right paren
character-name-002a = star
character-name-002b = plus
character-name-002c = comma
character-name-002d = dash
character-name-002e = dot
character-name-002f = slash
character-name-003a = colon
character-name-003b = semi
character-name-003c = less
character-name-003d = equals
character-name-003e = greater
character-name-003f = question
character-name-0040 = at
character-name-005b = left bracket
character-name-005c = backslash
character-name-005d = right bracket
character-name-005e = caret
character-name-005f = line
character-name-0060 = graav
character-name-007b = left brace
character-name-007c = bar
character-name-007d = right brace
character-name-007e = tilda
character-name-00a0 = space
character-name-00a1 = inverted bang
character-name-00a2 = cents
character-name-00a3 = pound
character-name-00a5 = yen
character-name-00a6 = broken bar
character-name-00a7 = section
character-name-00a9 = copyright
character-name-00ab = double left pointing angle bracket
character-name-00ac = not
character-name-00ae = registered
character-name-00b0 = degrees
character-name-00b1 = plus or minus
character-name-00b2 = superscript 2
character-name-00b3 = superscript 3
character-name-00b5 = micro
character-name-00b6 = pilcrow
character-name-00b7 = middle dot
character-name-00b9 = superscript 1
character-name-00bb = double right pointing angle bracket
character-name-00bc = one quarter
character-name-00bd = one half
character-name-00be = three quarters
character-name-00bf = inverted question
character-name-00d7 = times
character-name-00f7 = divide by
character-name-2013 = en dash
character-name-2014 = em dash
character-name-2018 = left tick
character-name-2019 = right tick
character-name-201c = left quote
character-name-201d = right quote
character-name-2022 = bullet
character-name-2026 = dot dot dot
character-name-2028 = line separator
character-name-2029 = paragraph separator
character-name-2030 = per mille
character-name-207b = superscript minus
character-name-20ac = euro
character-name-2122 = trademark
character-name-215b = one eighth
character-name-215c = three eighths
character-name-215d = five eighths
character-name-215e = seven eighths
character-name-2190 = left arrow
character-name-2191 = up arrow
character-name-2192 = right arrow
character-name-2193 = down arrow
character-name-2713 = check
character-name-2714 = check

## Character descriptions, spoken when the current character is asked for
## twice and when text is spelled with descriptions; keyed like the names.
## A capital letter uses its small letter's description.

character-description-0061 = Alpha
character-description-0062 = Bravo
character-description-0063 = Charlie
character-description-0064 = Delta
character-description-0065 = Echo
character-description-0066 = Foxtrot
character-description-0067 = Golf
character-description-0068 = Hotel
character-description-0069 = India
character-description-006a = Juliet
character-description-006b = Kilo
character-description-006c = Lima
character-description-006d = Mike
character-description-006e = November
character-description-006f = Oscar
character-description-0070 = Papa
character-description-0071 = Quebec
character-description-0072 = Romeo
character-description-0073 = Sierra
character-description-0074 = Tango
character-description-0075 = Uniform
character-description-0076 = Victor
character-description-0077 = Whiskey
character-description-0078 = X-ray
character-description-0079 = Yankee
character-description-007a = Zulu
