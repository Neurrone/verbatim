# The vision framework: focus highlight, screen curtain, magnifier

NVDA's "vision enhancement" framework (`source/vision/`) is the
plugin layer for *visual* output — features for sighted and
low-vision users driven by the same event stream as speech and
braille. Providers live in `source/visionEnhancementProviders/` and
can come from add-ons.

## The framework

`vision.visionHandler.VisionHandler` manages provider instances
(`providerBase.VisionEnhancementProvider`, each with typed settings
in the config and a GUI panel contributed automatically). Providers
subscribe to *extension points*
(`vision/visionHandlerExtensionPoints.py`, `EventExtensionPoints`):
`post_focusChange`, `post_foregroundChange`, `post_caretMove`,
`post_browseModeMove`, `post_reviewMove`, `post_mouseMove`,
`post_objectUpdate` — notified from the corresponding places in
event handling and the core pump (`visionHandler.handleFocusChange`
et al.). A provider is thus a pure consumer: it gets told where
focus/caret/review went and draws accordingly.

## Focus highlight

`visionEnhancementProviders/NVDAHighlighter.py`: draws colored
rectangles around the current focus, navigator object, and browse
mode caret (distinct styles per role of highlight, blended when
they coincide). Implementation: a transparent, click-through,
topmost layered window covering the desktop, painted with GDI+ from
a dedicated window/timer thread; rectangle positions come from the
objects' `location` and TextInfo `boundingRects`. The injected
helper also reports focus rectangles for some legacy cases
(`nvdaControllerInternal_drawFocusRectNotify` from in-process code;
[Process injection](process-injection.md)).

## Screen curtain

`source/screenCurtain/_screenCurtain.py` (recently moved out of the
providers package): blacks out the entire display — privacy for
blind users — using the **Magnification API**: `MagInitialize`,
then `MagSetFullscreenColorEffect` with an all-zeros color matrix
(and careful `MagUninitialize` balancing, since the API is
refcounted process-wide). Because the magnification API affects
compositor output, the screen *content* is still rendered and
readable by NVDA; only the physical display is dark. Notable UX
detail: enabling it warns loudly (a confirmation dialog,
`WarnOnLoadDialog`) because a sighted user accidentally enabling it
sees a dead screen.

## The magnifier (in development)

`source/_magnifier/` (underscore: not yet stable API) is NVDA
growing a built-in screen magnifier — historically the domain of
separate products (ZoomText) or Windows Magnifier. Structure:
`magnifier.Magnifier` is the base (tracking the point of interest,
computing source rectangles, clamping to screen limits
(`_getScreenLimits`), applying transforms via the same
Magnification API); subclasses implement the classic presentation
modes — `fullscreenMagnifier` (`MagSetFullscreenTransform`),
`lensMagnifier` (a floating lens window), `dockedMagnifier` and
`fixedMagnifier` (a docked strip); `commands.py` binds
zoom/pan/mode gestures, `config.py` the settings. Tracking will
ride the same vision extension points (focus, caret, mouse), which
is exactly why the framework routes those events generically. As of
the pinned commit this is pre-release, feature-flagged work — treat
specifics as moving.
