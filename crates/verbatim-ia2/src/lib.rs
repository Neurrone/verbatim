//! MSAA/IA2 client stack (architecture section 4).
//!
//! `WinEvent`s arrive via out-of-context hooks scoped to the outpost's
//! target process; `IAccessible2` is acquired from the event's `IAccessible`
//! through `IServiceProvider::QueryService`. Every property is a cross-process COM
//! round trip, so fetch discipline and outpost-side caching carry the cost
//! model (risk R1). The IA2 IDL is vendored under nvda/include/ia2; the
//! proxy/stub story is settled during the first IA2 implementation work.
//!
//! Skeleton only in M0; first real client code lands with milestone M3.
