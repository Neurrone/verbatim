//! Running a finished program: imports, execution, status, and results.
//!
//! Follows NVDA's `source/UIAHandler/_remoteOps/operation.py` and
//! `nvdaHelper/UIARemote/lowLevel.cpp` (copyright NV Access Limited and
//! contributors, GPL version 2 or later, used here under GPL-3.0-or-later),
//! calling Windows' `CoreAutomationRemoteOperation` directly.

use std::collections::BTreeSet;
use std::panic::Location;

use windows::Foundation::{IPropertyValue, PropertyType};
use windows::UI::UIAutomation::Core::{
    AutomationRemoteOperationOperandId, AutomationRemoteOperationResult,
    CoreAutomationRemoteOperation,
};
use windows::UI::UIAutomation::{AutomationElement, AutomationTextRange};
use windows::Win32::UI::Accessibility::{IUIAutomationElement, IUIAutomationTextRange};
use windows::core::{IInspectable, Interface};
use windows_collections::IVector;

use crate::builder::{Emitted, Reg, kind};
use crate::error::{Error, Failure};
use crate::instruction::{Instruction, OperandId};
use crate::opcode::{Opcode, Status};

/// An element or text range a program starts from.
pub(crate) enum Import {
    Element(IUIAutomationElement),
    TextRange(IUIAutomationTextRange),
}

/// A finished program with its imports and requested results, ready to
/// run. Running it is one cross-process round trip to the provider that
/// serves the imported elements.
pub struct Operation {
    emitted: Vec<Emitted>,
    bytecode: Vec<u8>,
    imports: Vec<(OperandId, Import)>,
    results: Vec<OperandId>,
    opcodes: BTreeSet<Opcode>,
}

impl Operation {
    pub(crate) fn new(
        emitted: Vec<Emitted>,
        imports: Vec<(OperandId, Import)>,
        results: Vec<OperandId>,
    ) -> Self {
        // The version, then every instruction.
        let mut bytecode = 0u32.to_le_bytes().to_vec();
        let mut opcodes = BTreeSet::new();
        for item in &emitted {
            item.instruction.encode(&mut bytecode);
            opcodes.insert(item.instruction.opcode());
        }
        Self {
            emitted,
            bytecode,
            imports,
            results,
            opcodes,
        }
    }

    #[cfg(test)]
    pub(crate) fn emitted(&self) -> &[Emitted] {
        &self.emitted
    }

    /// The program's bytes, as `Execute` receives them.
    #[must_use]
    pub fn bytecode(&self) -> &[u8] {
        &self.bytecode
    }

    /// The number of instructions.
    #[must_use]
    pub fn len(&self) -> usize {
        self.emitted.len()
    }

    /// Whether the program has no instructions (never true of a finished
    /// program, which ends with a `Halt`).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.emitted.is_empty()
    }

    /// The instruction at `index`, as a failure's location numbers it.
    #[must_use]
    pub fn instruction(&self, index: usize) -> Option<&Instruction> {
        self.emitted.get(index).map(|emitted| &emitted.instruction)
    }

    /// The Rust source location that emitted the instruction at `index`.
    #[must_use]
    pub fn location(&self, index: usize) -> Option<&'static Location<'static>> {
        self.emitted.get(index).map(|emitted| emitted.location)
    }

    /// Runs the program: imports its elements and text ranges, checks that
    /// the provider supports every instruction it uses, executes it, and
    /// returns the requested results. Blocks for one cross-process round
    /// trip (the support check is answered locally; see the crate guide).
    ///
    /// # Errors
    ///
    /// [`Error::Unavailable`] when Windows lacks the API,
    /// [`Error::Import`] when an import fails (a client-side proxy),
    /// [`Error::Unsupported`] when the provider lacks an instruction,
    /// [`Error::Execute`] when the call fails, and [`Error::Failed`] when
    /// the program stops with a failure status.
    pub fn execute(&self) -> Result<Outcome, Error> {
        let remote = CoreAutomationRemoteOperation::new().map_err(Error::Unavailable)?;
        for (id, import) in &self.imports {
            let operand = operand_id(*id);
            match import {
                Import::Element(element) => {
                    let element: AutomationElement = element.cast().map_err(Error::Import)?;
                    remote.ImportElement(operand, &element)
                }
                Import::TextRange(range) => {
                    let range: AutomationTextRange = range.cast().map_err(Error::Import)?;
                    remote.ImportTextRange(operand, &range)
                }
            }
            .map_err(Error::Import)?;
        }
        // Support is known only once something is imported, since it
        // depends on the provider's process.
        for &opcode in &self.opcodes {
            #[allow(clippy::cast_sign_loss)] // The opcode's bits, as the API takes them.
            let supported = remote
                .IsOpcodeSupported(opcode.code() as u32)
                .map_err(Error::Execute)?;
            if !supported {
                return Err(Error::Unsupported(opcode));
            }
        }
        for id in &self.results {
            remote
                .AddToResults(operand_id(*id))
                .map_err(Error::Execute)?;
        }
        let result = remote.Execute(&self.bytecode).map_err(Error::Execute)?;
        let outcome = Outcome { result };
        let status = Status::from_value(outcome.result.Status().map_err(Error::Execute)?.0);
        if status == Status::Success {
            return Ok(outcome);
        }
        let extended_error = outcome.result.ExtendedError().map_err(Error::Execute)?;
        let instruction = outcome
            .result
            .ErrorLocation()
            .ok()
            .and_then(|index| usize::try_from(index).ok());
        let opcode = instruction
            .and_then(|index| self.instruction(index))
            .map(Instruction::opcode);
        let location = instruction.and_then(|index| self.location(index));
        Err(Error::Failed(Box::new(Failure {
            status,
            extended_error,
            instruction,
            opcode,
            location,
            partial: outcome,
        })))
    }
}

fn operand_id(id: OperandId) -> AutomationRemoteOperationOperandId {
    // Operand ids are small, assigned from 1 upward.
    AutomationRemoteOperationOperandId {
        Value: i32::try_from(id.0).unwrap_or(i32::MAX),
    }
}

/// The results of one run.
pub struct Outcome {
    result: AutomationRemoteOperationResult,
}

impl std::fmt::Debug for Outcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Outcome")
    }
}

impl Outcome {
    /// Whether the run produced a value for `reg` (it was requested, and
    /// the run reached the instruction that set it).
    #[must_use]
    pub fn has<K>(&self, reg: Reg<K>) -> bool {
        self.result
            .HasOperand(operand_id(reg.id()))
            .unwrap_or(false)
    }

    /// `reg`'s value, converted to its Rust type.
    ///
    /// # Errors
    ///
    /// [`Error::MissingResult`] when the run produced no value for it, and
    /// [`Error::ResultType`] when the value is not of `K`'s type.
    pub fn get<K: Read>(&self, reg: Reg<K>) -> Result<K::Value, Error> {
        let operand = reg.id();
        if !self.has(reg) {
            return Err(Error::MissingResult(operand));
        }
        let raw = match self.result.GetOperand(operand_id(operand)) {
            Ok(raw) => Some(raw),
            // A null value comes back as a null pointer, which `windows`
            // reports as an error carrying no failure code.
            Err(error) if error.code().is_ok() => None,
            Err(error) => return Err(Error::ResultType { operand, error }),
        };
        K::read(raw).map_err(|error| Error::ResultType { operand, error })
    }
}

/// A result's value when its type is known only at run time.
#[derive(Clone, Debug)]
pub enum Value {
    /// Null.
    Null,
    /// A boolean.
    Bool(bool),
    /// A signed integer.
    Int(i32),
    /// An unsigned integer.
    Uint(u32),
    /// A 64-bit integer.
    Int64(i64),
    /// A double, or a single widened to one.
    Double(f64),
    /// A UTF-16 code unit.
    Char(u16),
    /// A string.
    String(String),
    /// A point.
    Point {
        /// The horizontal coordinate.
        x: f32,
        /// The vertical coordinate.
        y: f32,
    },
    /// A rectangle.
    Rect {
        /// The left edge.
        x: f32,
        /// The top edge.
        y: f32,
        /// The width.
        width: f32,
        /// The height.
        height: f32,
    },
    /// An integer array, such as a runtime id.
    IntArray(Vec<i32>),
    /// An element, with any cache the program populated.
    Element(IUIAutomationElement),
    /// A text range.
    TextRange(IUIAutomationTextRange),
    /// An array.
    Array(Vec<Value>),
    /// Something else, left as the object Windows returned.
    Other(IInspectable),
}

impl Value {
    /// Converts one returned object.
    ///
    /// # Errors
    ///
    /// The COM error from reading a property value or an array.
    pub fn from_inspectable(raw: Option<IInspectable>) -> windows::core::Result<Self> {
        let Some(raw) = raw else {
            return Ok(Self::Null);
        };
        if let Ok(value) = raw.cast::<IPropertyValue>() {
            return Ok(match value.Type()? {
                PropertyType::Empty => Self::Null,
                PropertyType::Boolean => Self::Bool(value.GetBoolean()?),
                PropertyType::Int32 => Self::Int(value.GetInt32()?),
                PropertyType::UInt32 => Self::Uint(value.GetUInt32()?),
                PropertyType::Int64 => Self::Int64(value.GetInt64()?),
                PropertyType::Double => Self::Double(value.GetDouble()?),
                PropertyType::Single => Self::Double(f64::from(value.GetSingle()?)),
                PropertyType::Char16 => Self::Char(value.GetChar16()?),
                PropertyType::String => Self::String(value.GetString()?.to_string()),
                PropertyType::Point => {
                    let point = value.GetPoint()?;
                    Self::Point {
                        x: point.X,
                        y: point.Y,
                    }
                }
                PropertyType::Rect => {
                    let rect = value.GetRect()?;
                    Self::Rect {
                        x: rect.X,
                        y: rect.Y,
                        width: rect.Width,
                        height: rect.Height,
                    }
                }
                PropertyType::Int32Array => {
                    let mut array = windows::core::Array::<i32>::new();
                    value.GetInt32Array(&mut array)?;
                    Self::IntArray(array.to_vec())
                }
                _ => Self::Other(raw),
            });
        }
        if let Ok(vector) = raw.cast::<IVector<IInspectable>>() {
            let size = vector.Size()?;
            let mut items = Vec::with_capacity(usize::try_from(size).unwrap_or(0));
            for index in 0..size {
                let item = match vector.GetAt(index) {
                    Ok(item) => Some(item),
                    Err(error) if error.code().is_ok() => None,
                    Err(error) => return Err(error),
                };
                items.push(Self::from_inspectable(item)?);
            }
            return Ok(Self::Array(items));
        }
        if let Ok(element) = raw.cast::<IUIAutomationElement>() {
            return Ok(Self::Element(element));
        }
        if let Ok(range) = raw.cast::<IUIAutomationTextRange>() {
            return Ok(Self::TextRange(range));
        }
        Ok(Self::Other(raw))
    }
}

/// How a register's kind is read back into Rust.
pub trait Read {
    /// The Rust type the value becomes.
    type Value;

    /// Converts the returned object (`None` for null).
    ///
    /// # Errors
    ///
    /// The COM error when the object is not of this kind.
    fn read(raw: Option<IInspectable>) -> windows::core::Result<Self::Value>;
}

fn property_value(raw: Option<IInspectable>) -> windows::core::Result<IPropertyValue> {
    raw.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_POINTER))?
        .cast()
}

impl Read for kind::Int {
    type Value = i32;
    fn read(raw: Option<IInspectable>) -> windows::core::Result<i32> {
        property_value(raw)?.GetInt32()
    }
}

impl Read for kind::Uint {
    type Value = u32;
    fn read(raw: Option<IInspectable>) -> windows::core::Result<u32> {
        property_value(raw)?.GetUInt32()
    }
}

impl Read for kind::Bool {
    type Value = bool;
    fn read(raw: Option<IInspectable>) -> windows::core::Result<bool> {
        property_value(raw)?.GetBoolean()
    }
}

impl Read for kind::Double {
    type Value = f64;
    fn read(raw: Option<IInspectable>) -> windows::core::Result<f64> {
        property_value(raw)?.GetDouble()
    }
}

impl Read for kind::Char {
    type Value = u16;
    fn read(raw: Option<IInspectable>) -> windows::core::Result<u16> {
        property_value(raw)?.GetChar16()
    }
}

impl Read for kind::Str {
    type Value = String;
    fn read(raw: Option<IInspectable>) -> windows::core::Result<String> {
        Ok(property_value(raw)?.GetString()?.to_string())
    }
}

impl Read for kind::Element {
    type Value = Option<IUIAutomationElement>;
    fn read(raw: Option<IInspectable>) -> windows::core::Result<Self::Value> {
        raw.map(|raw| raw.cast()).transpose()
    }
}

impl Read for kind::TextRange {
    type Value = Option<IUIAutomationTextRange>;
    fn read(raw: Option<IInspectable>) -> windows::core::Result<Self::Value> {
        raw.map(|raw| raw.cast()).transpose()
    }
}

impl Read for kind::Array {
    type Value = Vec<Value>;
    fn read(raw: Option<IInspectable>) -> windows::core::Result<Vec<Value>> {
        match Value::from_inspectable(raw)? {
            Value::Array(items) => Ok(items),
            _ => Err(windows::core::Error::from(
                windows::Win32::Foundation::E_NOINTERFACE,
            )),
        }
    }
}

impl Read for kind::Any {
    type Value = Value;
    fn read(raw: Option<IInspectable>) -> windows::core::Result<Value> {
        Value::from_inspectable(raw)
    }
}
