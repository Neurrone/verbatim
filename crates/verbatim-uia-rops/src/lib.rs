//! UIA Remote Operations wrapper (architecture section 4).
//!
//! Wraps the Microsoft.UI.UIAutomation remote-ops API so batched operations
//! execute inside the provider process in one cross-process round trip:
//! ancestor-chain retrieval on focus, bulk text attribute runs, terminal
//! text-range walking, browse-mode batch fetches. ARM64 behavior is risk R3,
//! verified alongside the first terminal work (milestone M4).
//!
//! Skeleton only in M0.
