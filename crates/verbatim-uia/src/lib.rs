//! UIA client stack (architecture section 4).
//!
//! Event registrations attach cache requests so events arrive with name,
//! role, states, and patterns prefetched in one round trip; callbacks arrive
//! on dedicated MTA threads separate from the threads that make UIA calls.
//! Runs on outpost threads and maps into the normalized model.
//!
//! Skeleton only in M0; the first real client code lands with milestone M1.
