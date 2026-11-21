//! What this crate counts as a cross-process call, in the one per-thread
//! count `verbatim_win32::calls` keeps
//! (`books/dev/src/reference/performance.md`, "What counts as a call").
//!
//! A call counts when it reaches the application's process: an
//! `IAccessible` method (`accName`, `accValue`, `accRole`, `accState`,
//! `accDescription`, `accKeyboardShortcut`, `accLocation`, `accParent`,
//! `accChildCount`, `accNavigate`, `accFocus`, `accSelection`,
//! `accDefaultAction`, `accDoDefaultAction`), `IAccIdentity`'s identity
//! string, the acquisitions `AccessibleObjectFromEvent`,
//! `AccessibleObjectFromWindow`, `AccessibleChildren`, and
//! `WindowFromAccessibleObject`, a `QueryInterface` for any interface but
//! `IUnknown` on an object from another process, and a window message sent
//! to one of the application's windows (a list view's or tree view's `LVM_`
//! and `TVM_` messages). One API call counts once, however many round trips
//! it makes inside (`AccessibleChildren` and `WindowFromAccessibleObject`
//! each make several); the provider-side counters in `mockapp` show those.
//!
//! `QueryInterface` is counted every time because COM answers it from its
//! proxy only when the proxy already holds that interface, which Verbatim
//! cannot see; asking for `IUnknown`, the object's identity, is always
//! answered locally and is not counted. Nor are local window functions on
//! the application's window handles (`IsWindow`, `GetClassName`,
//! `GetAncestor`, `GetWindow`, `IsWindowVisible`, `IsChild`), which never
//! reach its message loop, nor the
//! reference counting COM does when an object is kept or released.
