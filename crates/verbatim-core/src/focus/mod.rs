//! Core's focus rules under ADR 0003 ("Readers tell, Core asks"), each in
//! its own module.

pub(crate) mod acceptance;
pub(crate) mod backend;
pub(crate) mod duplicate;
pub(crate) mod execute;
pub(crate) mod follow;
#[cfg(test)]
mod identity_tests;
pub(crate) mod menus;
pub(crate) mod msaa_run;
pub(crate) mod narrow;
pub(crate) mod plain;
pub(crate) mod retake;
pub(crate) mod shown;
pub(crate) mod uia_focus;
pub(crate) mod validity;
pub(crate) mod versions;

use verbatim_model::property::PropertyId;

/// The properties a focus read takes of each node: everything the focus
/// announcement speaks or Core's rules judge.
pub(crate) const FOCUS_SET: &[PropertyId] = &[
    PropertyId::Name,
    PropertyId::Role,
    PropertyId::Value,
    PropertyId::States,
    PropertyId::Description,
    PropertyId::KeyboardShortcut,
    PropertyId::PositionInSet,
    PropertyId::SetSize,
    PropertyId::Level,
    PropertyId::Rect,
    PropertyId::ClassName,
    PropertyId::AutomationId,
    PropertyId::NativeWindow,
    PropertyId::Process,
    PropertyId::RuntimeId,
    PropertyId::HasTextPattern,
    PropertyId::RangeValue,
    PropertyId::Cell,
    PropertyId::TableCounts,
    PropertyId::ValueFromSelection,
];
