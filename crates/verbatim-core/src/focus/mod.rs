//! Core's focus rules under ADR 0003 ("Readers tell, Core asks"), each in
//! its own module (`docs/design/readers-tell-core-asks-build1.md`, "How
//! the code is organised"). Built beside the reducer's focus path, which
//! the event queue replaces in a later step of build 1.

#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "built beside the reducer; the focus entries switch over to it in a later step of build 1"
    )
)]

pub(crate) mod acceptance;
pub(crate) mod backend;
pub(crate) mod duplicate;
pub(crate) mod pending;
pub(crate) mod versions;
