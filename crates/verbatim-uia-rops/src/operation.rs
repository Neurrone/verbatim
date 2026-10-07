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
#[derive(Clone)]
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
    /// The first operand id the program does not use.
    next_id: u32,
}

impl Operation {
    pub(crate) fn new(
        emitted: Vec<Emitted>,
        imports: Vec<(OperandId, Import)>,
        results: Vec<OperandId>,
        next_id: u32,
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
            next_id,
        }
    }

    /// The same program counting what it executes: before each of its
    /// instructions, an `Add` of one to a counter register of its own, every
    /// jump's offset adjusted so it lands on the `Add` before its target,
    /// and the counter added to the results. Its registers, results, and
    /// effects are the program's; the counter ends as the number of the
    /// program's own instructions the run executed. The counting program
    /// executes twice as many and two more, so it reaches the platform's
    /// instruction limit at half the program's size.
    fn counting(&self) -> (Self, Reg<kind::Int>) {
        let counter = OperandId(self.next_id);
        let one = OperandId(self.next_id + 1);
        let at = Location::caller();
        let mut emitted = vec![
            Emitted {
                instruction: Instruction::NewInt {
                    result: counter,
                    value: 0,
                },
                location: at,
            },
            Emitted {
                instruction: Instruction::NewInt {
                    result: one,
                    value: 1,
                },
                location: at,
            },
        ];
        for item in &self.emitted {
            emitted.push(Emitted {
                instruction: Instruction::Add {
                    target: counter,
                    value: one,
                },
                location: item.location,
            });
            // An offset counts instructions from the jump to its target;
            // each now has an `Add` before it.
            let doubled = |offset: i32| 2 * offset - 1;
            let mut instruction = item.instruction.clone();
            match &mut instruction {
                Instruction::ForkIfFalse { offset, .. }
                | Instruction::ForkIfTrue { offset, .. }
                | Instruction::Fork { offset }
                | Instruction::NewTryBlock {
                    catch_offset: offset,
                } => *offset = doubled(*offset),
                Instruction::NewLoopBlock {
                    break_offset,
                    continue_offset,
                } => {
                    *break_offset = doubled(*break_offset);
                    *continue_offset = doubled(*continue_offset);
                }
                _ => {}
            }
            emitted.push(Emitted {
                instruction,
                location: item.location,
            });
        }
        let imports = self
            .imports
            .iter()
            .map(|(id, import)| (*id, import.clone()))
            .collect();
        let mut results = self.results.clone();
        results.push(counter);
        (
            Self::new(emitted, imports, results, self.next_id + 2),
            Reg::new(counter),
        )
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
    /// trip, which counts as one UIA call on this thread
    /// (`verbatim_uia::calls`); the support check is answered locally (see
    /// the crate guide).
    ///
    /// # Errors
    ///
    /// [`Error::Unavailable`] when Windows lacks the API,
    /// [`Error::Import`] when an import fails (a client-side proxy),
    /// [`Error::Unsupported`] when the provider lacks an instruction,
    /// [`Error::Execute`] when the call fails, and [`Error::Failed`] when
    /// the program stops with a failure status.
    ///
    /// While [`counting`] is on for this thread, the program runs as its
    /// counting form instead, which has the same results and effects, and
    /// how many of its instructions the run executed is recorded.
    pub fn execute(&self) -> Result<Outcome, Error> {
        if !counting::active() {
            return self.run();
        }
        let (program, counter) = self.counting();
        let result = program.run();
        let executed = match &result {
            Ok(outcome) => outcome.get(counter).ok(),
            Err(Error::Failed(failure)) => failure.partial.get(counter).ok(),
            Err(_) => None,
        };
        counting::record(executed.and_then(|count| u32::try_from(count).ok()));
        result
    }

    /// Runs the program as [`execute`](Self::execute) describes.
    fn run(&self) -> Result<Outcome, Error> {
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
        // One cross-process round trip, counted as one UIA call
        // (`docs/performance.md`).
        verbatim_uia::calls::count(verbatim_model::CallKind::Uia);
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

/// Counting the instructions remote programs execute, to measure them
/// against the platform's limit on one run (`docs/performance.md`, "The
/// instruction limit"). While counting is on for a thread, every program
/// that thread runs is run in its counting form, which has the same
/// results and effects, and how many of the program's own instructions it
/// executed is recorded. The counting form executes twice as many and two
/// more, so a program over half the limit fails with the limit's status
/// while counted; it is a measurement, never on in Verbatim itself.
pub mod counting {
    use std::cell::RefCell;

    thread_local! {
        static COUNTS: RefCell<Option<Vec<Option<u32>>>> = const { RefCell::new(None) };
    }

    /// Turns counting on for this thread, forgetting earlier counts.
    pub fn start() {
        COUNTS.with(|counts| *counts.borrow_mut() = Some(Vec::new()));
    }

    /// Turns counting off for this thread and returns, in order, how many
    /// instructions each program run since [`start`] executed: `None` for
    /// a run that gave no count (one that failed before it ran, or whose
    /// counter could not be read).
    #[must_use]
    pub fn stop() -> Vec<Option<u32>> {
        COUNTS.with(|counts| counts.borrow_mut().take().unwrap_or_default())
    }

    pub(super) fn active() -> bool {
        COUNTS.with(|counts| counts.borrow().is_some())
    }

    pub(super) fn record(executed: Option<u32>) {
        COUNTS.with(|counts| {
            if let Some(counts) = counts.borrow_mut().as_mut() {
                counts.push(executed);
            }
        });
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

#[cfg(test)]
mod tests {
    use crate::builder::Builder;
    use crate::instruction::{Instruction, OperandId};
    use crate::opcode::Comparison;

    /// The offsets of a jump instruction, none for another.
    fn offsets(instruction: &Instruction) -> Vec<i32> {
        match instruction {
            Instruction::ForkIfFalse { offset, .. }
            | Instruction::ForkIfTrue { offset, .. }
            | Instruction::Fork { offset }
            | Instruction::NewTryBlock {
                catch_offset: offset,
            } => vec![*offset],
            Instruction::NewLoopBlock {
                break_offset,
                continue_offset,
            } => vec![*break_offset, *continue_offset],
            _ => Vec::new(),
        }
    }

    #[test]
    fn the_counting_form_counts_before_every_instruction_and_keeps_its_jumps() {
        let mut b = Builder::new();
        let count = b.new_int(0);
        let limit = b.int(3);
        b.while_(
            |b| b.compare(count, limit, Comparison::LessThan),
            |b| {
                let flag = b.new_bool(false);
                b.if_(flag, Builder::break_loop);
                let one = b.int(1);
                b.add_assign(count, one);
            },
        );
        b.try_catch(|b| b.halt(), |_, _| {});
        let program = b.finish();
        let (counting, counter) = program.counting();
        let original: Vec<&Instruction> = program
            .emitted()
            .iter()
            .map(|emitted| &emitted.instruction)
            .collect();
        let instrumented: Vec<&Instruction> = counting
            .emitted()
            .iter()
            .map(|emitted| &emitted.instruction)
            .collect();
        // The counter and its step first, then an add before each
        // instruction.
        assert_eq!(instrumented.len(), 2 + 2 * original.len());
        let add = Instruction::Add {
            target: counter.id(),
            value: OperandId(counter.id().0 + 1),
        };
        let mut jumps = 0;
        for (index, instruction) in original.iter().enumerate() {
            assert_eq!(instrumented[2 + 2 * index], &add);
            let at = 3 + 2 * index;
            let (old, new) = (offsets(instruction), offsets(instrumented[at]));
            assert_eq!(old.len(), new.len());
            // Every jump lands on the add before its old target.
            for (old, new) in old.into_iter().zip(new) {
                let target = index.checked_add_signed(old as isize).expect("a target");
                let landed = at.checked_add_signed(new as isize).expect("a landing");
                assert_eq!(landed, 2 + 2 * target, "the jump at {index}");
                jumps += 1;
            }
        }
        // The loop block's two, its condition's fork, the break's fork, the
        // try block's catch, and the fork past the catch.
        assert_eq!(jumps, 6);
    }
}
