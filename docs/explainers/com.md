# COM for accessibility work

The Component Object Model is the substrate under MSAA, IA2, UIA, the Office
object models, and OneCore speech. You do not need COM's full generality
(class factories, the registry, OLE); you need the object model, the threading
rules, and marshaling, because those explain most accessibility bugs —
including hangs.

## The object model

A COM object is a set of *interfaces*, each a pointer to a vtable of function
pointers. Every interface derives from `IUnknown`, whose three methods carry
the whole model:

- `QueryInterface(iid, out ptr)` — ask the object for another of its
  interfaces by GUID. This is how capability discovery works everywhere in
  accessibility: you hold `IAccessible` and ask "do you also do
  `IAccessible2`?" (in IA2's case via an intermediary, `IServiceProvider` —
  see [IA2](ia2.md)).
- `AddRef` / `Release` — intrusive reference counting. In Rust, wrapper
  crates (`windows-rs`) manage this via `Clone`/`Drop`; the foot-gun that
  remains is *who owns a pointer you were handed* across an FFI or IPC
  boundary, and freeing buffers COM allocated (`SysFreeString` for BSTRs,
  `SafeArrayDestroy` for SAFEARRAYs, `CoTaskMemFree` for plain allocations).

Methods return an `HRESULT`: a 32-bit code where the high bit means failure.
Success is usually `S_OK` (0), sometimes `S_FALSE` (1) — *a success* that
means "nothing there," which callers regularly mishandle. Common failures in
this domain: `E_NOINTERFACE` (QueryInterface refusal), `RPC_E_DISCONNECTED` /
`CO_E_OBJNOTCONNECTED` (the remote object's process or apartment is gone —
routine when an app closes a window you hold a pointer into),
`RPC_E_SERVERCALL_RETRYLATER` (the server's thread is busy), and
`RPC_E_CALL_CANCELED` (someone cancelled the in-flight call — see
[Main loop and watchdog](../nvda/main-loop-and-watchdog.md) for who does that on purpose).

Data types to recognize: `BSTR` (length-prefixed wide string with mandated
allocator), `VARIANT` (a tagged union that can hold anything — 24 bytes on
64-bit, 16 on 32-bit, which matters when a structure crosses a bitness
boundary such as a 32-bit synth host; MSAA uses it for child IDs),
`SAFEARRAY` (a self-describing array — UIA returns them; ownership
mistakes here corrupt the heap).

One piece of the "full generality" is still needed: *activation*. Objects
mostly arrive in accessibility work by being handed to you (an event, a
`WM_GETOBJECT` answer), but system services are created by class ID:
`CoCreateInstance(clsid, iid, out ptr)` looks the CLSID up in the
registry, loads or launches the registered server, and returns the
requested interface. When [Speech APIs](speech-apis.md) says "`ISpVoice`
(CLSID `SpVoice`)", this is the call it is assuming; the UIA client object
(`CUIAutomation8`) is obtained the same way.

## Apartments: the threading model

Every thread that touches COM first calls `CoInitializeEx`, choosing:

- **STA** (single-threaded apartment): objects created by this thread may
  only be called on this thread. Calls from other threads are delivered as
  *window messages* to a hidden window, so an STA thread must pump a message
  loop, and an STA thread that stops pumping makes its objects unreachable —
  every caller blocks. GUI threads are STAs; most application UI objects,
  and therefore most MSAA/IA2 objects, live in STAs.
- **MTA** (multi-threaded apartment): one process-wide apartment; objects in
  it may be called from any MTA thread concurrently and must be thread-safe.
  No pumping requirement. UIA requires its client callbacks and is happiest
  entirely on MTA threads (see [UIA](uia.md)).

The apartment decision is per thread, sticky until `CoUninitialize`, and
wrong choices fail late and weirdly (marshaling errors, missing events,
deadlocks) rather than at the call site. When a pointer crosses an apartment
boundary it must be *marshaled* — passed via `CoMarshalInterface` or
mechanisms built on it — never smuggled as a raw pointer; a smuggled pointer
sometimes works (same process, thread-safe object) and sometimes corrupts or
deadlocks, which makes it a beloved source of heisenbugs.

## Marshaling and where hangs come from

When caller and callee are in different apartments or processes, the call
goes through a *proxy* (caller side) and *stub* (callee side): arguments are
serialized, shipped over COM's RPC channel, and the stub invokes the real
object. Two consequences dominate screen reader work:

1. **Every property fetch is a blocking round trip.** COM calls are
   synchronous; the calling thread waits for the reply. Cross-process, that
   is an RPC round trip (~tens of microseconds at best, unbounded at worst).
   Reading one accessible node the naive way is a dozen round trips.
2. **Calls into an STA wait for that STA's message loop.** The stub delivers
   the call by posting to the server thread's message queue. If that thread
   is hung, or busy not pumping, the caller blocks *indefinitely* — there is
   no default timeout on ordinary COM calls. This is the mechanism by which
   a frozen application freezes every screen reader that synchronously
   queries it, and it is proven, not folklore: see the evidence collected in
   [Main loop and watchdog](../nvda/main-loop-and-watchdog.md).

Mitigations that exist in the platform, all used by NVDA and documented in
`docs/nvda`:

- `CoEnableCallCancellation` plus `CoCancelCall(threadId)`: lets another
  thread cancel a COM call stuck on the named thread. Cancellation support
  is spotty per interface, but it works well enough to be NVDA's freeze
  recovery mechanism.
- Making risky calls on sacrificial worker threads and abandoning the thread
  if it never returns.
- `SendMessageTimeout` instead of `SendMessage` for raw window messages
  ([Windows and messages](windows-and-messages.md)).
- UIA's own timeout and asynchronicity machinery ([UIA](uia.md)).

## Re-entrancy, the subtle one

While an STA thread waits for an outgoing cross-apartment call, COM pumps
messages on its behalf so incoming calls can still be delivered — otherwise
two STAs calling each other would instantly deadlock. The price is that *your
code can be re-entered while blocked on an innocent-looking call*: an event
callback can fire, application state can change under you, and object graphs
you were mid-way through walking can be freed. Screen reader crashes that
look impossible ("this pointer was valid two lines ago") are often STA
re-entrancy. The defensive posture: treat every cross-process call as a
suspension point at which the world may change, exactly as you would treat
an `await`.

## Proxies need registration

Cross-process calls only work if both sides can find marshaling code for the
interface. System interfaces (MSAA, UIA) ship with Windows. Third-party
interfaces — IA2 above all — need their proxy/stub DLL registered or
activation-context-loaded in *both* processes, which is why NVDA's injected
DLL carries and registers the IA2 proxy ([Process injection](../nvda/process-injection.md)).
A missing proxy shows up as `E_NOINTERFACE` from QueryInterface for an
interface the object definitely implements — remember that failure shape.

## References

- [The COM Library (Microsoft Learn)](https://learn.microsoft.com/en-us/windows/win32/com/the-com-library)
  and [Processes, Threads, and Apartments (Microsoft Learn)](https://learn.microsoft.com/en-us/windows/win32/com/processes--threads--and-apartments)
- [CoCancelCall (Microsoft Learn)](https://learn.microsoft.com/en-us/windows/win32/api/combaseapi/nf-combaseapi-cocancelcall)
- [Don Box, *Essential COM*](https://www.informit.com/store/essential-com-9780201634464) —
  still the best conceptual treatment of apartments and marshaling.
