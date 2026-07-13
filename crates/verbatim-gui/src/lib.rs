//! GUI (architecture section 11, decision D4).
//!
//! The Verbatim menu and settings UI, built on wxWidgets via wxDragon on a
//! dedicated thread in Core. wxWidgets accessibility is proven in exactly
//! this role: NVDA's own GUI is wxPython. The M1 prototype's defining test
//! is Verbatim reading this GUI through a real outpost process over ordinary
//! UIA — no self-voicing side channel.
//!
//! M0 carries the wxDragon dependency so CI compiles the workspace's
//! heaviest dependency (and its ARM64 release-profile workaround) from day
//! one; the actual UI lands with milestone M1.

// Reference the dependency so an accidental removal of the wxDragon build
// from the workspace cannot go unnoticed by CI.
pub use wxdragon;
