# Design records

Each page here explains how one piece of finished work works today, with no alternatives or history. Its decisions, with the problem, the options weighed and why each was taken or rejected, and the limits accepted, are a numbered record in [the decision records](../../reference/adr/readme.md), and its mechanism's details are in [the crate guides](../../reference/crates/readme.md), which are kept current with the code. The other pages still keep their alternatives and reasons beside the explanation, and each is split the same way when its work next changes.

Records of work still being designed or built are kept in the repository and are published here once that work is finished.

- [Ownership](ownership.md) — one owner for every fact, the rules every component follows, and the five builds that applied them: the one judge, input, messages, output, and the outpost store and reducer.
- [How focus works](focus.md) — how a focus change travels from an application to speech, with its decisions in [ADR 0001](../../reference/adr/0001-focus.md).
- [The listener without connections](listener-without-connections.md) — why only an application's own outpost connects to it.
- [UIA shapes](uia-shapes.md) — every cached UIA read declared with its request.
- [Wake-ups](wake-ups.md) — the sources of Verbatim's wake-ups and the reason for each fix.
- [The terminal model](terminal-model.md) — how terminal screens change, and the rules Verbatim reads them by.
- [Key holding](key-holding.md) — holding caret keys as NVDA does.
