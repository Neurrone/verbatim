//! Input (architecture section 5).
//!
//! A low-level keyboard hook on a dedicated, never-blocking thread in Core.
//! The swallow-or-pass decision runs in microseconds against a read-only,
//! lock-free snapshot of the gesture map (Windows silently removes hooks
//! that exceed `LowLevelHooksTimeout`); the hook only decides and enqueues,
//! and gesture semantics run on the reducer thread. Gesture maps are
//! user-remappable and per-app-module overridable.
//!
//! Skeleton only in M0; the hook and the Verbatim modifier land with
//! milestone M1.
