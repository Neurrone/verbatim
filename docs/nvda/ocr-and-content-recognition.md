# OCR and content recognition

`source/contentRecog/` is a small framework for "recognize what's on
screen and let the user read it": OCR today, designed generically
enough for other recognizers (image description) to slot in.

## The framework

- `contentRecog/__init__.py`: a `ContentRecognizer` receives a
  captured image (with coordinate conversion info,
  `RecogImageInfo`) and asynchronously produces a
  `RecognitionResult` — text *with per-word coordinates*
  (`LinesWordsResult`: lines of words, each word with a screen
  rectangle).
- Capture: the target is the navigator object's screen rectangle
  (`recognizeNavigatorObject` in `contentRecog/recogUi.py`),
  screenshotted via `source/screenBitmap.py` (GDI capture to RGB
  pixels), scaled per the recognizer's requested factor.
- `recogUi.py` presents the result as a *virtual result window*: a
  temporary NVDAObject (`RecogResultNVDAObject`) that takes fake
  focus, exposes the recognized text through a standard TextInfo
  (so all review/cursor commands work; [TextInfo](text-infos.md)), and routes
  activation — pressing Enter on a word clicks its screen
  coordinates. Escape dismisses and focus returns. Re-running
  recognition while one is displayed refreshes it; results are
  explicitly transient (nothing tracks the live screen).

## The OCR engine

`contentRecog/uwpOcr.py`: the Windows 10+ in-box OCR engine
(`Windows.Media.Ocr`), reached through NVDA's C++/WinRT helper
`nvdaHelperLocalWin10.dll` (`uwpOcr` functions) rather than Python
WinRT bindings. Language selection follows installed OCR language
packs (`getLanguages`); recognition runs asynchronously with the
result marshaled back via callback. The gesture NVDA+R runs it on
the navigator object.

Because recognition consumes a *screenshot*, it composes with the
rest of NVDA orthogonally: screen curtain ([The vision framework](vision.md)) does not
darken the captured content (capture happens before the
color-effect output stage), and the feature works on anything
visible regardless of accessibility support — its designed use is
exactly the apps everything else in this folder fails on.
