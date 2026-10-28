# Experiments: the terminals report no language, links or annotations

This survey, taken live on 2026-10-07 on Dickson's x64 machine (Windows 11, build 26200), reads every UIA text attribute and annotation in Windows Terminal (1.24), the console host, and Windows 11 Notepad: what each application reports, whether the value varies within the text, and what reading each attribute costs, in calls and in time, inside the caret's one remote operation and on the classic path. It is the material for choosing which attributes Verbatim fetches. The choice itself (Dickson, 2026-10-07) is stated in `docs/parity/speech-audio.md`, "Which text attributes are fetched", and the caret's read that applies it in `docs/crates/verbatim-uia-rops.md`. The probe was scratch code and is not kept.

## How it was taken

- The terminals ran a PowerShell script that printed six lines with virtual terminal sequences: plain text; colors (a basic red, a green background, a bright blue, a 256-color orange, and a 24-bit color); styles (bold, faint, italic, underline, double underline, curly underline, strikethrough, overline, inverse, and blink); an OSC 8 hyperlink beside a bare URL; a line in French, German, Chinese, and Arabic; and two underlines with underline colors (SGR 58).
- Notepad opened a plain text file with spelling errors, a URL, and a French sentence, and then a Markdown file, which Notepad shows formatted: a heading, bold, italic, strikethrough, a link, and a bullet.
- For each of the 44 attributes UIA defines (40000 to 40043, the list in Microsoft's "Text Attribute Identifiers"), the value was read over the whole document, over each line, and over each stretch the format unit gives within each line. "Not supported" is UIA's reserved not-supported value, and "mixed" its reserved mixed value.
- Times are medians of 30 reads, from a debug build of the probe on a 12-thread x64 desktop, so the client's own share is slightly inflated; the cross-process and provider share dominates. The machine had other builds running, so single numbers are good to about a third.

## What Windows Terminal reports

Supported, with the value varying by stretch where the output varies:

- ForegroundColor: the color of each stretch, as a BGR integer, the terminal's palette resolved (Campbell's red reads 2035653, its default foreground 13421772); bold text reads a brighter color, and inverse text reads the background color as its foreground. Mixed over a line with more than one color.
- BackgroundColor: likewise; the default background is 789516, a green background reads as its own color, and inverse text reads the foreground color.
- FontWeight: 400, and 700 for bold. Faint text reads 400 with a dimmer foreground color.
- IsItalic: true for italic text.
- UnderlineStyle: 0 none, 1 single, 3 double, and 8 wavy for curly underline (UIA's text decoration line styles).
- StrikethroughStyle: 0, and 1 for struck text.
- FontName: "Cascadia Mono", the same everywhere.
- IsReadOnly: false everywhere.

Not supported: everything else, including Culture, FontSize, Links, AnnotationTypes and AnnotationObjects, UnderlineColor, OverlineStyle and OverlineColor (an overline is not reported at all), StrikethroughColor, IsHidden, and all the paragraph, margin, indentation, style, and caret attributes. Blink is not reported. An OSC 8 hyperlink and a bare URL read as plain text: no Links attribute, no underline, and no color of their own.

The format unit gives a stretch per word, not per change of formatting: "plain text line" is three stretches, each with the same values.

## What the console host reports

The same attributes as Windows Terminal, with the same values for the same output: ForegroundColor, BackgroundColor, FontWeight, IsItalic, UnderlineStyle (1, 3, and 8), StrikethroughStyle, IsReadOnly, and FontName ("Consolas"). Nothing else is supported; Culture, Links, and the annotations are not.

One difference: the line with italic text reads as mixed for IsItalic, but no stretch the format unit gives reads as italic, so italics can be found by reading the line but not stretch by stretch.

## What Windows 11 Notepad reports

In a plain text file, uniform over the document: AnimationStyle 0, BackgroundColor (16382457), BulletStyle 0, CapStyle 0, Culture 1033 (the document's language, English, with the French line no different), FontName "Consolas", FontSize 11, FontWeight 400, ForegroundColor 0, HorizontalTextAlignment 0, the three indentations 0, IsHidden, IsItalic, IsReadOnly, IsSubscript and IsSuperscript false, OutlineStyles 0, StrikethroughStyle 0, Tabs (an empty array), TextFlowDirections 0, and UnderlineStyle 0. AnnotationTypes varies: each misspelled word reads [60001], the spelling error annotation, and the text between them reads not supported rather than an empty array. A URL in plain text is not a link.

In a Markdown file, shown formatted, more varies:

- FontSize: 28 for the heading, 11 elsewhere.
- FontWeight 700 for bold, IsItalic true for italic, StrikethroughStyle 1 (and StrikethroughColor 0) for struck text.
- BulletStyle 2 (a filled round bullet) for the list item, with IndentationLeading 20 and IndentationFirstLine -20.
- IsHidden is mixed over the link's line: the link's target is hidden text. The link itself reads no Links attribute, no underline, and no color of its own.

Not supported in either file: the margins, OverlineColor and OverlineStyle, UnderlineColor, AnnotationObjects (the spelling errors come as types only), StyleName and StyleId, Links, IsActive, SelectionActiveEnd, CaretPosition, CaretBidiMode, LineSpacing, the paragraph spacings, and SayAsInterpretAs.

## Annotations

Of UIA's annotation types (60000 to 60026), only the spelling error (60001) was seen, in Notepad; a grammar error (60002) is possible there but this text had none. Neither terminal supports AnnotationTypes or AnnotationObjects at all, and Notepad supports only AnnotationTypes.

## Language

Neither terminal reports a language: Culture is not supported in Windows Terminal or the console host, over the document, a line, or a stretch, whatever the text's script. So terminal output, which switches voice only when the terminal reports a language per line, is spoken in one language, as NVDA speaks it, and no Culture read is needed for it. Notepad reports one Culture for the whole document (the input language, 1033), the same on a French line, so it carries no per-line language either.

## What reading them costs

Each attribute's cost was taken on the line with the most stretches: in the terminals the styles line, 13 stretches; in Notepad the French line, 11 (each misspelled word is a stretch).

Windows Terminal:

- Classic, one GetAttributeValue over a line: about 0.1 milliseconds for a supported attribute and about 0.2 for one it does not support. All 44, one call each: 8.1 milliseconds and 44 calls.
- In a remote operation, per attribute, over all 13 stretches: 0.01 to 0.13 milliseconds for a supported attribute, and 0.6 to 0.9 for one it does not support. Every unsupported attribute is expensive in Windows Terminal: all 44 over the 13 stretches take 12.1 milliseconds in one call, against 0.25 for the same program reading none.
- IUIAutomationTextRange3 GetAttributeValues, all 44 at once: 1.8 milliseconds over the line in one call; over each of the 13 stretches, 26 milliseconds and 13 calls; against 113 milliseconds and 572 calls for one GetAttributeValue per attribute per stretch.

The console host:

- Classic: 0.04 to 0.1 milliseconds a call, supported or not; all 44, 3.0 milliseconds and 44 calls.
- In a remote operation, every attribute is within the measurement's noise over 13 stretches (under 0.1 milliseconds); all 44 over the 13 stretches, 1.1 milliseconds in one call, against 0.26 for none.
- GetAttributeValues: 0.15 milliseconds over the line; 1.6 milliseconds and 13 calls over each stretch; against 60 milliseconds and 572 calls one by one.

Notepad:

- Classic: 0.09 to 0.16 milliseconds a call; all 44, 5.1 milliseconds and 44 calls.
- In a remote operation, 0.01 to 0.1 milliseconds per attribute over 11 stretches; all 44, 0.83 milliseconds in one call, against 0.28 for none.
- GetAttributeValues: 0.24 milliseconds over the line; 2.2 milliseconds and 11 calls over each stretch; against 56 milliseconds and 484 calls one by one.

The caret's own read (`verbatim_uia_rops::caret_read`, the caret's line with its formatting, by the theme's four attribute groups as they were then), remote in one call against classic:

- Windows Terminal, the caret on a line of one stretch: 0.43 milliseconds with no attributes, 0.54 to 0.71 with one group, 0.86 with all four; classically 0.93 milliseconds and 8 calls with none, and 2.0 to 2.8 milliseconds and 17 to 23 calls with them.
- The console host, the same: 1.6 milliseconds with none and 3.0 to 4.2 with attributes, remote or classic alike (8 to 23 calls classically): the console host's provider, not the round trips, sets the cost, and most of it is finding the stretches.
- Notepad, the caret on a line of 5 stretches: 0.51 milliseconds with none, 0.65 to 0.91 with one group or all four (one measurement of 2.8 for the font attributes was an outlier); classically 1.1 milliseconds and 8 calls with none, and 5.2 to 13 milliseconds and 46 to 76 calls with them.

What this means for the choice:

- Inside the caret's remote operation, an attribute the provider supports costs almost nothing per stretch; the cost that matters is reading the stretches at all, which the formatting already pays. An attribute Windows Terminal does not support costs it about 0.05 milliseconds per stretch, so attributes a terminal never reports are best not asked of it.
- On the classic path, every attribute is one call per stretch; GetAttributeValues reads any number of attributes for a stretch in one call, at about the cost of two single reads, so for the classic path it is the way to read more than one or two.
- Remote operations have no GetAttributeValues instruction; inside a program, each attribute is its own GetAttributeValue instruction, which costs no round trip.

## Applied to the fetch as decided

Once support is learned, Windows Terminal and the console host are asked for none of FontSize, BulletStyle, or the Link attribute, which would each cost Windows Terminal about 0.05 milliseconds per stretch inside a remote operation, and Notepad is not asked for the Link attribute. With the default theme, which reads the annotation types and the link, a line is asked for its annotation types first and walked stretch by stretch only when it has some:

- Windows Terminal and the console host, which support no annotations: one read of the line in place of one per stretch, and no walk of the stretches, so the caret's read costs about what it costs with no attributes (0.43 milliseconds remotely in Windows Terminal, against 0.54 to 0.71 with one group read stretch by stretch; classically 9 calls in place of 17 to 23 for a line of one stretch, and about 88 for a line of 13).
- Notepad: a line without spelling errors costs one read and no walk (9 calls classically in place of 46 to 76 for a line of 5 stretches); a line with one costs one read more.

`docs/performance.md`, "The caret report after a focus, UIA", has the counts pinned against mockapp and the times measured there.
