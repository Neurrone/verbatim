# Input

The Verbatim modifier, repeated presses, gestures, typed-character echo, IME, and the mouse.

### NVDA-modifier semantics

swallow, double-tap passthrough,
trapped-key release swallowing, unbound-chord fallthrough.
NVDA: `keyboardHandler` ([Keyboard input](../nvda/input.md)). Verbatim:
**matched (verified by unit tests)** — semantics transcribed
([verbatim-input](../crates/verbatim-input.md)); live side-by-side with NVDA still
worth one session (sticky/locked modifier states **not yet**).
**Different, deliberately:** Caps Lock is a Verbatim key by default (see
"Spoken vocabulary and key layouts"). NVDA treats Num Lock as a modifier
of the numpad operator keys, so a binding of plain numpad plus (say all
from the review cursor, bound since 2026-10-06) does not take the plus
sign from a user with Num Lock on: the decision machine **implements
this since 2026-10-06** (`set_num_lock`), and **not yet** is the hook
telling it the Num Lock state, until which numpad plus is always the
command.

### Script repeat counting

NVDA: `scriptHandler` counts each run of the
same script within the multi-press timeout, and forgets the last script
when an unbound gesture comes between. Verbatim: **matched since
2026-10-03** for the reset on an unbound key (it had carried the count
across). **Different, deliberately:** auto-repeat of a held key is not
counted as presses, so holding a key never turns into a double press;
NVDA counts it.

### Gesture map / rebindable input, input help mode

Verbatim: map
exists; user rebinding UI and input help **not yet (M8/M9)**.

### Typed-character echo

NVDA: in-process reports; UIA textEdit
events where applicable. Verbatim: the echo itself is **implemented in
Core since 2026-10-06**: "Speak typed characters" (only in edit controls, by default)
and "Speak typed words" (off), each off, only in edit controls, or
always, toggled by Verbatim+2 and Verbatim+3; a word is spoken when a
character that is not a letter, mark, or number by its general category
ends it, before that character (matched since 2026-10-08, so a virama or
a Thai tone mark continues the word, as in NVDA; before then any mark
ended it), and the zero-width non-joiner and joiner also continue it,
which NVDA's rule does not (**different**, deliberately: Persian and
Indic words are typed with them);
a protected field echoes only the protected character ("star") and no
words; typing into a terminal waits until the terminal's text changes
unless "Speak passwords" is on, so a password prompt speaks nothing, and
Enter drops what was held. **Different:** the characters come from the
keyboard hook's own translation of each key (decided 2026-10-06,
`docs/crates/verbatim-input-windows.md`), not from inside the
application; dead keys compose correctly, a letter typed before the
application has handled the dead key aside, against
[Keyboard input](../nvda/input.md).

### IME/composition reporting

Verbatim: **not yet** (unscheduled;
needs a decision — NVDA's implementation is injection-dependent).
**Different:** with an input method's layout active, text committed from
a composition is not echoed; NVDA echoes it from inside the application.
The same holds since 2026-10-08 for a keyboard text service outside
Chinese, Japanese, and Korean (Vietnamese Telex and VNI, the Indic
Phonetic keyboards), found by the active keyboard profile: Verbatim
stays silent rather than echoing the raw keys, until composed echo
arrives with the injection helper (M6).

### Mouse tracking and touch interaction

Mouse tracking (text-unit speech, audio coordinates, injection
filtering) and touch interaction. NVDA:
[Mouse and touch](../nvda/mouse-and-touch.md). Verbatim: **not yet**
(unscheduled; mouse tracking becomes cheap once point-to-node and
point-to-text resolution exist).
