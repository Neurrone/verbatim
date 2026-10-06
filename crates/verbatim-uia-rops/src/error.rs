//! The crate's error type.

use std::fmt;
use std::panic::Location;

use crate::instruction::OperandId;
use crate::opcode::{Opcode, Status};
use crate::operation::Outcome;

/// Why a remote operation, or its classic counterpart, gave no answer.
#[derive(Debug)]
pub enum Error {
    /// Windows could not create a remote operation: the API is missing.
    Unavailable(windows::core::Error),
    /// An element or text range could not be imported. A client-side proxy
    /// (an element UIA serves from MSAA inside the client's own process)
    /// fails with `E_UNEXPECTED`, since there is no provider process to run
    /// in; nothing about such an element will change, so the caller should
    /// not try again for its window.
    Import(windows::core::Error),
    /// The provider does not support an instruction the program uses.
    Unsupported(Opcode),
    /// The call to run the program failed before any status came back.
    Execute(windows::core::Error),
    /// The program ran and stopped with a failure status.
    Failed(Box<Failure>),
    /// A requested result is missing from the run's results.
    MissingResult(OperandId),
    /// A result is not of the type its register says.
    ResultType {
        /// The register read.
        operand: OperandId,
        /// The conversion's error.
        error: windows::core::Error,
    },
    /// A UIA call failed (the classic implementations, or a read from a
    /// returned element).
    Uia(windows::core::Error),
}

/// A run that stopped with a failure status.
#[derive(Debug)]
pub struct Failure {
    /// How the run ended.
    pub status: Status,
    /// The HRESULT of the instruction that failed.
    pub extended_error: windows::core::HRESULT,
    /// The failing instruction's index in the program, when the platform
    /// reported one.
    pub instruction: Option<usize>,
    /// That instruction's opcode.
    pub opcode: Option<Opcode>,
    /// The Rust source line that emitted that instruction.
    pub location: Option<&'static Location<'static>>,
    /// The results the run had computed before it stopped.
    pub partial: Outcome,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(error) => write!(f, "remote operations are unavailable: {error}"),
            Self::Import(error) => write!(f, "importing into a remote operation failed: {error}"),
            Self::Unsupported(opcode) => {
                write!(
                    f,
                    "the provider does not support the {opcode:?} instruction"
                )
            }
            Self::Execute(error) => write!(f, "running a remote operation failed: {error}"),
            Self::Failed(failure) => {
                write!(f, "remote operation ended with {:?}", failure.status)?;
                if let Some(index) = failure.instruction {
                    write!(f, " at instruction {index}")?;
                }
                if let Some(opcode) = failure.opcode {
                    write!(f, " ({opcode:?})")?;
                }
                if let Some(location) = failure.location {
                    write!(f, " emitted at {location}")?;
                }
                write!(f, ", extended error {}", failure.extended_error)
            }
            Self::MissingResult(operand) => {
                write!(f, "remote operation result {} is missing", operand.0)
            }
            Self::ResultType { operand, error } => {
                write!(
                    f,
                    "remote operation result {} has another type: {error}",
                    operand.0
                )
            }
            Self::Uia(error) => write!(f, "UIA call failed: {error}"),
        }
    }
}

impl std::error::Error for Error {}

impl Error {
    /// The HRESULT behind the error, where there is one: a COM error's
    /// code, or a failed run's extended error. A provider whose process has
    /// gone gives `UIA_E_ELEMENTNOTAVAILABLE`; one that did not answer
    /// within UIA's transaction timeout gives `UIA_E_TIMEOUT`.
    #[must_use]
    pub fn hresult(&self) -> Option<windows::core::HRESULT> {
        match self {
            Self::Unavailable(error)
            | Self::Import(error)
            | Self::Execute(error)
            | Self::Uia(error)
            | Self::ResultType { error, .. } => Some(error.code()),
            Self::Failed(failure) => Some(failure.extended_error),
            Self::Unsupported(_) | Self::MissingResult(_) => None,
        }
    }
}

impl From<windows::core::Error> for Error {
    fn from(error: windows::core::Error) -> Self {
        Self::Uia(error)
    }
}
