# Reliability

A screen reader that goes silent leaves a blind user without a computer, so Verbatim bounds how long it waits for your code and turns off an extension that keeps failing. This page explains the rules and how to stay well inside them; the numbers are in [Limits](../reference/limits.md).

## Deadlines

Every call into your code has a deadline, made of two bounds:

- **Your own execution** is bounded by a compute deadline. Time your hook spends waiting for a read of the application does not count against it, so a slow application cannot make your code look slow. The bound is set at a generous multiple of the slowest legitimate hook among Verbatim's own modules.
- **The whole event** is bounded by the time Verbatim allows for handling it, the same bound its own reading meets. Once that is spent, further reads are refused, so a slow application stops your hook only where it would stop Verbatim.

When your hook overruns or traps, Verbatim carries on as if you had not been called: an event hook counts as passing the event on, a control hook as changing nothing, so the user hears what Verbatim would have said without you. Your instance is then replaced with a fresh one, and its state is lost.

## Answer in time, even with a failure

If your extension depends on something slow or unreliable, such as a network service, answer before your deadline with your own failure rather than waiting. A speech-text stage that cannot reach its service returns its failure indication, such as "Translation failed". The user then hears that something went wrong and which extension it was, and an answer in time is never counted as a failure to respond.

## The breaker

An extension that fails to respond three times within one minute is turned off until the user's next session, and Verbatim says once, naming it, "<extension> disabled after repeated failures". The user can turn it back on in the Extensions dialog, and the dialog shows why it was turned off.

A failure to respond is one of:

- a missed deadline, counting only time spent in your own code;
- a trap, such as a panic or an out-of-bounds access;
- your extension's process ending while one of its calls was in flight.

An answer in time that reports a failure is not counted, and neither is time spent waiting on the application. Turning off lasts only for the session, so a translator that failed because the network dropped is back once the user restarts Verbatim or turns it on again.

The breaker does not catch an extension that answers in time but answers badly, such as one that turns every utterance into nonsense. Users recover from that by starting Verbatim with every extension off and removing it, so test what your stage returns, not only that it returns.

## Resource limits

Your extension has generous limits on memory, processor time and the size of its data folder, and rates and sizes for what it speaks, shows and logs. They exist so that a runaway loop or a leak costs your extension, not the user's computer.

## Seeing what your extension costs

In developer mode, Verbatim shows for each extension:

- its processor time;
- each hook's duration and how close it came to its deadline;
- its peak memory and the size of its data folder;
- its network requests;
- which limit acted, whenever one did, and every failure the breaker counted.

Each event that passed through your hook carries its time in Verbatim's latency timeline, split into time in your code and time in the reads you made, so a slow event is attributed to the right place. Check these figures on a slow machine before every release.

## What a hook costs the user

An installed extension runs in a process of its own, so each hook costs a round trip between processes on the event's path, and each read your hook makes costs another. A module made only of rules costs nothing of the kind. Prefer rules where they can express what you need, register hooks only for the events and controls you handle, and read what you need in as few calls as you can.
