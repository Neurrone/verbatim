# ADR 0005: Calls that reach another application

Status: decided, 2026-10-29, by Dickson; "The listener's start" stands in the form he gave it on 2026-10-30. [The architecture](../../explanation/architecture.md), section 1, and [the listener without connections](../../explanation/design/listener-without-connections.md) explain the rule these decisions keep, and the crate guides of [verbatim-outpost](../crates/verbatim-outpost.md), [verbatim-listener](../crates/verbatim-listener.md) and [verbatim-uia](../crates/verbatim-uia.md) describe the code.

## The problem

The architecture's rule is that an application's accessibility calls come only from its own outpost, the listener reads only elements it already holds, and a slow or hung application holds only the threads that serve it. Every remote operation and every call Verbatim makes by a window handle kept the rule, but blocking did not:

- a busy host window's application held the focus of the application in a child window below it, such as a Settings page inside its frame or web content inside its host;
- the foreground application held the listener's start, because UI Automation reads the system's focus while it makes the listener's desktop-wide registration;
- a search for a node by runtime id entered any child window another process serves below the application's window.

## Decisions

### Reading above the process boundary

Above the process boundary nothing is read through the other process's accessibility: the parent or owner window is described as a plain window from Windows' own state. This is [ADR 0001](0001-focus.md), "Object identity", which holds the options and the reason.

### The listener's start

- Options: register for UI Automation events before installing the `WinEvent` hooks, on one thread; or install the hooks first, make the registration on a thread of its own, and say the listener is ready once the registration returns.
- Decided: the second (Dickson, 2026-10-30).
- Reason: the user is never told Verbatim is ready before focus events can reach it, and the listener hears MSAA events and answers the supervisor while the foreground application is slow.

### A node whose element is gone

- Options:
  - search the application's windows as before;
  - search inside the application's own process by a remote operation, which cannot search a window UI Automation reads through its proxies and needs repeated runs for a large tree;
  - walk down by hand, pruning at each element another process serves, which makes more calls than the search it replaces;
  - or answer the node gone, as NVDA's object for a gone element is gone.
- Decided: answer it gone (Dickson, 2026-10-29).
- Reason: any search below the application's window can enter another process's child window and hold the user's next focus behind it, and the bounded searches cost more than the case they serve, while answering gone costs only a dead object the user has already left.

### Checking where a call landed

- Options: trust the result of a call whose target the system picks at call time, such as the focused element, MSAA's focused child or a step up the tree; or check which process serves the result before anything is done with it.
- Decided: check (Dickson, 2026-10-29).
- Reason: without the check, a focus that moves between Verbatim's own check and the call has Verbatim read and speak another application's object as this one's.

### Stated limits

- Options: remove the two collisions between Verbatim's own processes in a busy application, the listener's read beside the host's outpost and a replacement outpost beside the one it replaces; or state them as limits. Both are forms of UI Automation's fault where a new connection fails while the application is busy with another process's call; the listener's is the second form, in which two processes' new connections wait in the busy application while a third process has a call pending there ([Faults in UI Automation](../uia-bugs.md)).
- Decided: state them (Dickson, 2026-10-29).
- Reason: both need an application already busy, and both recover at the next event. Removing the first would move the wait to the host's outpost, and removing the second would leave the application with no outpost until its predecessor exits.

## Limits accepted

- Two Verbatim processes calling one busy application can fail a new connection, a replacement outpost's lookups included; the focus is then spoken at the application's next event.
- UI Automation's client side still sends a parent window across the process boundary the messages that begin a connection when the listener reads a focus below it, so a busy host still delays that read.
- A hook event can name a window another process serves, such as a console window, which its client owns and its console host serves. An outpost probes, connects to and walks up into only the windows its application serves, and reads any other window through MSAA.
- The listener's readiness waits on the foreground application at Verbatim's start and at each replacement of the listener.
- The check cannot prevent two calls: a step up the tree, which waits for the application of the window it reaches when the hierarchy changes between the check and the step, and the outpost's read of the focused element, the decided exception, which connects into the application the focus moved to.
- The walks down a subtree can enter another process's child window: the search for a selection in a list the focus controls, the gathering of a dialog's text when focus first enters it, the reading of the taskbar's and notification area's items, and the tree dump. Each is bounded or made only on the user's command.

## Evidence

The experiments, each with its readme and output: [the runtime-id search enters another process's child window](https://github.com/Neurrone/verbatim/blob/main/experiments/the-runtime-id-search-enters-another-processs-child-window/readme.md), [a process-scoped hook takes events by the window's owner](https://github.com/Neurrone/verbatim/blob/main/experiments/a-process-scoped-hook-takes-events-by-the-windows-owner/readme.md), [a walk up whose window changes after its check](https://github.com/Neurrone/verbatim/blob/main/experiments/a-walk-up-whose-window-changes-after-its-check/readme.md), [accFocus names no other process's child window](https://github.com/Neurrone/verbatim/blob/main/experiments/accfocus-names-no-other-processs-child-window/readme.md), [a replacement outpost's lookup beside its predecessor's operation](https://github.com/Neurrone/verbatim/blob/main/experiments/a-replacement-outposts-lookup-beside-its-predecessors-operation/readme.md), [the listener's registration waits on the foreground application](https://github.com/Neurrone/verbatim/blob/main/experiments/the-listeners-registration-waits-on-the-foreground-application/readme.md), [the MSAA continuation waits on the parent window's application](https://github.com/Neurrone/verbatim/blob/main/experiments/the-msaa-continuation-waits-on-the-parent-windows-application/readme.md) and [the listener's read below another process's window](https://github.com/Neurrone/verbatim/blob/main/experiments/the-listeners-read-below-another-processs-window/readme.md), with [the program they share](https://github.com/Neurrone/verbatim/blob/main/experiments/cross-process-calls-study-program/readme.md); and the UIA facts [a new connection fails while another process's remote operation is queued](https://github.com/Neurrone/verbatim/blob/main/uia-facts/a-new-connection-fails-while-another-processs-remote-operation-is-queued/readme.md), whose second form is the listener's collision, and [UIA answers some calls in the client](https://github.com/Neurrone/verbatim/blob/main/uia-facts/uia-answers-some-calls-in-the-client/readme.md).

## What would reopen this

A UI Automation client call that tells a window element's parent, or the process serving a result, without reaching the other process; or an application in use whose focus sits below a busy host often enough that the listener's delayed read is heard.
