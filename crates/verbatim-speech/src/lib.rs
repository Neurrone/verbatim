//! Speech pipeline (architecture section 6).
//!
//! Stages in order: utterance, dictionary and symbol processing, language
//! tagging, synth driver, PCM, audio sink. The speech manager owns priority
//! lanes (interrupt, next, queued), index marks with callbacks, and
//! per-language voice switching; utterances carry language tags end-to-end.
//! Synth drivers implement one trait — streaming PCM plus index-mark events
//! — regardless of origin (built-in, Wasm, or the sandboxed native host).
//!
//! Skeleton only in M0; the pipeline lands with milestone M1.
