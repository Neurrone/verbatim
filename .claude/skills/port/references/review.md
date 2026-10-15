# Semantic-review targets

Give this list to the review subagent in phase 9. Review the slice as if it
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

For every suspected defect, give a concrete counterexample, a violated
invariant, or the missing piece of evidence. Leave out style-only
suggestions, because they bury the findings that matter.
