//! Audio output (architecture section 6, decision D5).
//!
//! The `AudioSink` trait decouples the speech pipeline from the audio backend;
//! the only initial implementation is WASAPI in event-driven shared mode
//! with small buffers, in service of the 50 ms keypress-to-audio budget.
//!
//! Skeleton only in M0; the sink lands with milestone M1.
