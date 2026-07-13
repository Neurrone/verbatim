//! `OneCore` synth driver (architecture section 6).
//!
//! Streams PCM and index-mark events from `WinRT`'s
//! `Windows.Media.SpeechSynthesis` via windows-rs, behind the shared synth
//! driver trait. This is the first voice Verbatim speaks with (milestone
//! M1); eSpeak NG joins in M3 as the latency-budget reference synth.
//!
//! Skeleton only in M0.
