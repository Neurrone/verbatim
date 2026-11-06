# Verbatim Extensions Development Guide

This guide is for people writing extensions for Verbatim: support for an application, a voice, a braille display, or a change to what Verbatim says, such as translation.

Extensions are being built. App modules, global modules and synthesizers come first; speech-text stages, recognisers, the network and the data folder follow; braille drivers come last. The pages describe extensions as they are designed, and say so where something is not settled yet.

## How this guide is organized

The guide has four parts, each answering one kind of question:

- [Tutorials](tutorials/readme.md) teach by building something from start to finish.
- [How-to guides](how-to/readme.md) give the steps for one task you already understand.
- [Reference](reference/readme.md) states the facts: what each capability gives, every limit.
- [Explanation](explanation/readme.md) gives the reasons: how extensions work and why they work that way.

Within every part, pages are grouped by the same topics, in the same order, so the tutorial, the how-to guides, the reference and the explanation for one topic are found under the same heading in each part. A part lists only the topics it has pages for.

1. Fundamentals: extensions, modules, kinds and where code runs.
2. App modules.
3. Global modules.
4. Speech-text stages.
5. Synthesizers.
6. Recognisers.
7. Braille drivers.
8. Security and permissions.
9. Reliability and performance.
10. Packaging and distribution.
11. Testing and debugging.

If you are new, read [Extensions, modules and kinds](explanation/concepts.md) and [The security model](explanation/security-model.md) first.
