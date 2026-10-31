# Input

The Verbatim modifier, repeated presses, gestures, typed-character echo, IME, and the mouse.

### NVDA-modifier semantics

swallow, double-tap passthrough,
trapped-key release swallowing, unbound-chord fallthrough.
NVDA: `keyboardHandler` ([Keyboard input](../nvda/input.md)). Verbatim:
**partial**. The semantics are matched, verified by unit tests and
transcribed ([verbatim-input](../crates/verbatim-input.md)); no live
side-by-side session with NVDA has been made. Sticky and
locked modifier states are not yet built. Different, by decision
(Dickson, 2026-10-03): Caps Lock is a Verbatim key by default (see
"Spoken vocabulary and key layouts" in [the readme](readme.md)). NVDA
treats Num Lock as a modifier of the numpad operator keys, so a binding
of plain numpad plus (say all from the review cursor) does not take the
plus sign from a user with Num Lock on: the decision machine implements
this (`set_num_lock`), and the hook does not yet tell it the Num Lock
state, so numpad plus is always the command.

### Script repeat counting

NVDA: `scriptHandler` counts each run of the
same script within the multi-press timeout, and forgets the last script
when an unbound gesture comes between. Verbatim: **matched
(unverified)**, including the reset on an unbound key. Different,
deliberately: auto-repeat of a held key is not counted as presses, so
holding a key never turns into a double press; NVDA counts it.
### A command pressed while a caret key is held

NVDA runs scripts one at a time in the order queued
([Keyboard input](../nvda/input.md), "Scripts run one at a time"), and
its caret script says nothing once another script is queued. Verbatim:
a review command, or the time, menu or tray-list gesture, pressed while
a caret key is held for its caret read runs after that key has been
passed on and its watch answered, or 100 ms after the pass, whichever is
first; the caret key says nothing, and a reading command reads the line
the key moved to (`docs/design/ownership.md`; Core's `a_review_command_pressed_while_a_caret_key_is_held_runs_after_its_read_pass_and_watch`).
A command pressed while the watch of a caret key already passed on is
still open waits for it in the same way. Different, by decision
(Dickson, 2026-10-30): the wait for the watch is Verbatim's own, so the
command reads the moved line. NVDA, captured on 2026-10-30 (build
alpha-58022,732d1ce2, the pinned source's), in Windows 11 Notepad with
the lines "line one" and "line two" and the caret on line one, Down
Arrow then numpad 8 back to back: in each of 5 runs, speech is
cancelled and "line one" spoken 12 to 16 ms later; Down Arrow's own
caret speech is cancelled, and "line two" is never spoken. With 300 ms
between the two keys, in each of 2 runs, Down Arrow says "line two" and
numpad 8 says "line two". Verbatim says "line two" in both cases.

### Gesture map / rebindable input, input help mode

Verbatim: **partial**. The map exists; the user rebinding UI and input
help are not yet built (M8).

### Typed-character echo

NVDA: in-process reports; UIA textEdit
events where applicable. Verbatim: **matched (unverified)**, implemented
in Core: "Speak typed characters" (only in edit controls, by default)
and "Speak typed words" (off), each off, only in edit controls, or
always, toggled by Verbatim+2 and Verbatim+3; a word is spoken when a
character that is not a letter, mark, or number by its general category
ends it, before that character, so a virama or
a Thai tone mark continues the word, as in NVDA. The word typed so far
is forgotten when a key of the caret table that does not select is
pressed, whether or not it then moves the caret or deletes anything,
and when the focus moves, but not after a key that selects or a key
outside the caret table: "hel", Control+C, "lo" and a space says
"hello"; so does "hel", Shift+Left Arrow, "lo" typed over the "l"
selected, and a space; "hel", Left Arrow, Shift+Right Arrow, "lo" and a
space says "lo" (`notepad_typed_words`, from NVDA's captures
`05a-*` to `05c-*` in
`target/e2e-evidence/m42-text-captures-2026-10-23/`). Different,
deliberately: the zero-width non-joiner and joiner also continue a word,
which NVDA's rule does not, since Persian and Indic words are typed
with them. Different (Dickson, 2026-10-26):
NVDA also forgets the word for a movement key that moves nothing and for
Delete and Control+Delete, but not for a Backspace or Control+Backspace
that moves nothing, whose script returns before forgetting it when the
caret has not moved (`_backspaceScriptHelper`,
`nvda/source/editableText.py`); Verbatim forgets it for those too.
Different, and not yet decided: NVDA forgets the word when a terminal shows new
lines of output (`nvda/source/NVDAObjects/behaviors.py` 568), and
Verbatim does not. A protected field echoes only the protected character
("star") and no words; typing into a terminal waits until the
terminal's text changes unless "Speak passwords" is on, so a password
prompt speaks nothing, and Enter drops what was held. Different
(Dickson, 2026-10-06; `docs/crates/verbatim-input-windows.md`): the
characters come from the keyboard hook's own translation of each key,
not from inside the application; dead keys compose correctly, a letter
typed before the application has handled the dead key aside, against
[Keyboard input](../nvda/input.md).

### IME/composition reporting

Verbatim: **not yet (unscheduled)**; it needs a decision, since NVDA's
implementation is injection-dependent. Different: with an input
method's layout active, text committed from a composition is not
echoed; NVDA echoes it from inside the application. The same holds for
a keyboard text service outside Chinese, Japanese, and Korean
(Vietnamese Telex and VNI, the Indic Phonetic keyboards), found by the
active keyboard profile: Verbatim stays silent rather than echoing the
raw keys, until composed echo arrives with the injection helper (M7).

### Mouse tracking and touch interaction

Mouse tracking (text-unit speech, audio coordinates, injection
filtering) and touch interaction. NVDA:
[Mouse and touch](../nvda/mouse-and-touch.md). Verbatim: **not yet
(unscheduled)**; mouse tracking becomes cheap once point-to-node and
point-to-text resolution exist.
