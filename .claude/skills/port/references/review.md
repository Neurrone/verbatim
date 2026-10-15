# Semantic-review targets

Give this list to the review subagent in phase 9. Review the change as if it
is subtly wrong despite passing its tests. Look especially for:

- an omitted stage in the NVDA causal path;
- a source behavior accidentally mistaken for an implementation detail;
- an NVDA implementation detail accidentally mistaken for required behavior;
- stale-event races;
- incorrect event ordering;
- duplicate delivery;
- incorrect coalescing;
- identity confusion across processes;
- forbidden cross-process calls;
- a blocking call on the wrong thread;
- failure isolation weakened by the port;
- restart behavior that loses necessary state;
- backend arbitration differences;
- focus or foreground state observed at the wrong time;
- cancellation or supersession occurring too late;
- tests that pass without exercising the claimed production path;
- expectations derived from Verbatim rather than independently established;
- tests weakened during implementation;
- undocumented intentional divergence.

And for cost, which the design's costing and the ledger
(`docs/performance.md`) fix in advance:

- a call into the application that the costing did not predict, or one
  made from a process other than the one the design names;
- a sequence of UIA calls that a remote operation could have made in one
  round trip;
- a property fetched that the event's cache request already held, or a
  cached read the request did not fill (counted as a failed cached read);
- work done, or a thread woken, on every occurrence of an event that is
  only needed when the event is spoken;
- an allocation, copy, or parse on the per-event path that could be done
  once;
- a new wake-up source: a timer, a poll, or a hand-off between threads
  where the producing thread could do the work;
- a ledger entry or call-count test left unchanged by a change that
  altered the operation's calls.

For every suspected defect, give a concrete counterexample, a violated
invariant, or the missing piece of evidence. Leave out style-only
suggestions, because they bury the findings that matter.
