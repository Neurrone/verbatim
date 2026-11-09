# Decision records

Each record here keeps the decisions behind one part of Verbatim: the problem, the options weighed and why each was taken or rejected, each decision with its date and who took it, and the limits accepted. How that part works today is explained in [the design pages](../../explanation/design/readme.md), and its mechanism is in [the crate guides](../crates/readme.md).

Records are numbered in the order they were decided, because the order matters: a later record can rest on an earlier one, and never the other way round. A decision taken while Dickson was away is marked "pending Dickson's acceptance" until he accepts it. [The architecture](../../explanation/architecture.md) remains the index of every decision of record and links to the record that holds each one.

- [ADR 0001: Focus](0001-focus.md) — which process reads each focus, how a focus keeps its order, how Verbatim waits for a slow or hung application, how an earlier ancestry is reused safely, and what Verbatim+Tab reports.
