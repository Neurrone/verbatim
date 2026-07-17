# ARIA, annotations, and compound documents

Three web-and-document semantics layers that sit above the raw
backends: the ARIA vocabulary mapping, the annotation/details
machinery, and compound documents. All become load-bearing with
browse-mode work (roadmap M6 for Verbatim); collected here so that
work has its reference.

## The ARIA mapping

`source/aria.py` is small and central: `ariaRolesToNVDARoles` maps
ARIA role strings to `controlTypes.Role` (application, search,
banner, dialog, group, …), `landmarkRoles` maps the landmark subset
to their spoken names (banner as "banner", contentinfo as "content
info", …), and `AriaLivePoliteness` names the live-region levels.
Consumers: the IA2 web overlays read role/landmark strings from IA2
object attributes ([IA2 usage](ia2.md)) and the UIA path from ARIA
properties UIA forwards ([The UIA client](uia.md)); landmarks
surface in browse-mode quick nav (D key), the elements list, and as
low-verbosity context in formatting announcements
([Document formatting reporting](document-formatting.md)). The
mapping's deliberate smallness is the lesson: most ARIA semantics
arrive already digested by the browser into roles, states, and
attributes; a screen reader maps vocabulary, it does not implement
ARIA.

## Annotations and details relations

`source/annotation.py` models the ARIA 1.3 annotations story
(`aria-details` with typed details: comments, footnotes, form-error
descriptions): an `AnnotationOrigin` on an object enumerates
`AnnotationTarget`s (each with a role and summary), reported when
reading the annotated content ("has comment"), with a command
cycling through details targets (`_AnnotationNavigation` state) and
jumping to them. Backend plumbing differs per API: IA2 uses
details/detailsFor relations; UIA uses annotation *pattern* objects
plus registered **custom annotation types**
(`source/UIAHandler/customAnnotations.py` — GUID-registered types,
e.g. Word's draft-comment type, resolved through UIA's extensibility
registrar). This is one of the few places NVDA consumes UIA's
custom-extension registration machinery — worth knowing it exists
before assuming the fixed UIA vocabulary is all there is.

## Compound documents

`source/compoundDocuments.py` solves "a document made of many
objects": editors whose text lives in a tree of separate accessible
objects (LibreOffice/Symphony-style embedded frames, text boxes)
rather than one flat text surface. `CompoundDocument` is a tree
interceptor ([Browse mode](browse-mode.md)) whose
`TreeCompoundTextInfo` implements the TextInfo contract *across*
object boundaries — movement units walk into and out of child text
objects, and `getTextWithFields` stitches per-object fields into one
stream ([TextInfo](text-infos.md)). It is the existence proof that
the TextInfo abstraction can span a forest, and the fallback pattern
for any app exposing document structure without a document-level
text API.
