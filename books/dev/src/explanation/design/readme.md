# Design records

A design record keeps, for one piece of finished work, the problem, the alternatives weighed, the reasons for the choice and its limitations. The mechanism it chose is described in [the crate guides](../../reference/crates/readme.md), which are kept current with the code; a design record changes only when its decision does.

Records of work still being designed or built are kept in the repository and are published here once that work is finished.

- [Ownership](ownership.md) — one owner for every fact, the rules every component follows, and the five builds that applied them: the one judge, input, messages, output, and the outpost store and reducer.
- [Communication](communication.md) — every pipe and thread from an event or a key to sound.
- [The focus pipeline](focus-pipeline.md) — how focus events reach Verbatim.
- [The listener without connections](listener-without-connections.md) — why only an application's own outpost connects to it.
- [UIA shapes](uia-shapes.md) — every cached UIA read declared with its request.
- [Wake-ups](wake-ups.md) — the sources of Verbatim's wake-ups and the reason for each fix.
- [The terminal model](terminal-model.md) — how terminal screens change, and the rules Verbatim reads them by.
- [Key holding](key-holding.md) — holding caret keys as NVDA does.
