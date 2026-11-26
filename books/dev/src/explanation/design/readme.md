# Design records

Each page here explains how one piece of finished work works today, with no alternatives or history. Its decisions, with the problem, the options weighed and why each was taken or rejected, and the limits accepted, are a numbered record in [the decision records](../../reference/adr/readme.md), and its mechanism's details are in [the crate guides](../../reference/crates/readme.md), which are kept current with the code. The other pages still keep their alternatives and reasons beside the explanation, and each is split the same way when its work next changes.

Records of work still being designed or built are kept in the repository and are published here once that work is finished.

- [Ownership](ownership.md) — one owner for every fact: who owns what and how each rule is held, with its decisions in [ADR 0006](../../reference/adr/0006-one-owner-per-fact.md).
- [How focus works](focus.md) — how a focus change travels from an application to speech, with its decisions in [ADR 0001](../../reference/adr/0001-focus.md).
- [Focus validity](focus-validity.md) — how every focus candidate, MSAA and UI Automation, is judged by one definition in Core, at its turn and while a caret key waits, with its decisions in [ADR 0004](../../reference/adr/0004-focus-validity.md).
- [The listener without connections](listener-without-connections.md) — why only an application's own outpost connects to it.
- [UIA shapes](uia-shapes.md) — every cached UIA read declared with its request.
- [Wake-ups](wake-ups.md) — the sources of Verbatim's wake-ups and the reason for each fix.
- [The terminal model](terminal-model.md) — how terminal screens change, and the rules Verbatim reads them by.
- [Key holding](key-holding.md) — holding caret keys as NVDA does.
