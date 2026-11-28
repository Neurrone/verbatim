# The rewrite's design records

The design of Verbatim's rewrite (`docs/roadmap.md`, "The rewrite") and the work it rests on, written on 2026-11-28. The two designs are the records; the rest is the evidence and the reviews behind them.

## The designs

- [design.md](design.md): the rewrite's skeleton: the protocol, the records in each process, Core's queue and async rules, the readers as reducers, held views, the simulator, the recorder and the verification against NVDA, with Dickson's decisions and what is provisional.
- [extensions-design.md](extensions-design.md): the extensions design, rewritten for the rewrite, with M5 in it from the start.

## What they rest on

- [patterns.md](patterns.md): the bad patterns found across today's code, which the rewrite removes, and the plan they came from.
- [options.md](options.md): the options weighed for the protocol, the queue, concurrency in the readers and element lifetime.
- [features-a.md](features-a.md) and [features-b.md](features-b.md): every feature of today's product worked through the design.
- [extensions-first.md](extensions-first.md): the line between Core and extensions, the hot path and the cost of crossing to an extension.
- [extension-api-additions.md](extension-api-additions.md): the first draft of the interfaces built-in extensions need.
- The NVDA inventory, every part of NVDA's source classed by what replaying it needs: [the core loop and input](nvda-inventory/core.md), [object classes and app modules](nvda-inventory/objects.md), [the accessibility handlers](nvda-inventory/handlers.md), [speech, braille and audio](nvda-inventory/speech.md), and [text, documents and terminals](nvda-inventory/text.md).
- The oracle experiments are in [experiments/nvda-oracle](../../../experiments/nvda-oracle/readme.md).

## The pre-mortem

- [premortem.md](premortem.md): the bug classes the design's own mechanisms invite, with their evidence and mitigations; [review-premortem-fable.md](review-premortem-fable.md), its adversarial review, fixed once in it.

## Reviews

- [review-fable.md](review-fable.md) and [review-fable-2.md](review-fable-2.md): the two adversarial reviews of the rewrite's design, each fixed once in it.
- [review-extensions-fable.md](review-extensions-fable.md): the adversarial review of the extensions design, fixed once in it.
