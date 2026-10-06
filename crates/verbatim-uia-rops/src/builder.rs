//! The typed program builder.
//!
//! Ported in design from NVDA's `source/UIAHandler/_remoteOps/builder.py`,
//! `remoteAPI.py`, and `remoteTypes/` (copyright NV Access Limited and
//! contributors, GPL version 2 or later, used here under
//! GPL-3.0-or-later): registers typed by what they hold, constants gathered
//! in a section ahead of the program, structured control flow that computes
//! its own jump offsets, and a record of where each instruction was
//! emitted. NVDA's local emulator and its Python operator overloading are
//! not ported.
//!
//! A program is written once per call, with that call's values as
//! constants. Each method emits instructions; nothing runs until the
//! finished [`Operation`] executes. Control flow takes closures, which run
//! once at build time to emit their bodies:
//!
//! ```no_run
//! # use verbatim_uia_rops::{Builder, NavigationDirection};
//! # fn example(element: &windows::Win32::UI::Accessibility::IUIAutomationElement) {
//! let mut b = Builder::new();
//! let current = b.import_element(element);
//! let count = b.new_int(0);
//! b.while_(
//!     |b| {
//!         let null = b.is_null(current);
//!         b.not(null)
//!     },
//!     |b| {
//!         let parent = b.navigate(current, NavigationDirection::Parent);
//!         b.set(current, parent);
//!         let one = b.int(1);
//!         b.add_assign(count, one);
//!     },
//! );
//! b.add_to_results(count);
//! let operation = b.finish();
//! # }
//! ```

use std::collections::HashMap;
use std::marker::PhantomData;
use std::panic::Location;

use windows::Win32::UI::Accessibility::{IUIAutomationElement, IUIAutomationTextRange};
use windows::core::GUID;

use crate::instruction::{Instruction, OperandId, TypeTest};
use crate::opcode::{Comparison, NavigationDirection, PointProperty, RectProperty};
use crate::operation::{Import, Operation};

/// The types a register can hold, as marker types for [`Reg`].
pub mod kind {
    /// A UIA element (or null).
    #[derive(Debug)]
    pub enum Element {}
    /// A UIA text range (or null).
    #[derive(Debug)]
    pub enum TextRange {}
    /// A signed 32-bit integer.
    #[derive(Debug)]
    pub enum Int {}
    /// An unsigned 32-bit integer.
    #[derive(Debug)]
    pub enum Uint {}
    /// A boolean.
    #[derive(Debug)]
    pub enum Bool {}
    /// A double.
    #[derive(Debug)]
    pub enum Double {}
    /// A UTF-16 code unit.
    #[derive(Debug)]
    pub enum Char {}
    /// A string.
    #[derive(Debug)]
    pub enum Str {}
    /// A point.
    #[derive(Debug)]
    pub enum Point {}
    /// A rectangle.
    #[derive(Debug)]
    pub enum Rect {}
    /// An array of values of any type.
    #[derive(Debug)]
    pub enum Array {}
    /// A map from strings to values of any type.
    #[derive(Debug)]
    pub enum StringMap {}
    /// A cache request, filled by `PopulateCache`.
    #[derive(Debug)]
    pub enum CacheRequest {}
    /// A GUID.
    #[derive(Debug)]
    pub enum Guid {}
    /// A value whose type is known only when the program runs: a property
    /// value, an array item, or a string map entry. [`Reg::assume`] names
    /// its type.
    #[derive(Debug)]
    pub enum Any {}
}

use kind::{
    Any, Array, Bool, CacheRequest, Char, Double, Element, Guid, Int, Point, Rect, Str, StringMap,
    TextRange, Uint,
};

/// The kinds that order: integers, doubles, and characters.
pub trait Ordered: sealed::Sealed {}
impl Ordered for Int {}
impl Ordered for Uint {}
impl Ordered for Double {}
impl Ordered for Char {}

/// The kinds arithmetic works on.
pub trait Numeric: sealed::Sealed {}
impl Numeric for Int {}
impl Numeric for Uint {}
impl Numeric for Double {}

/// The kinds an index can be.
pub trait Index: sealed::Sealed {}
impl Index for Int {}
impl Index for Uint {}

mod sealed {
    pub trait Sealed {}
    impl Sealed for super::Int {}
    impl Sealed for super::Uint {}
    impl Sealed for super::Double {}
    impl Sealed for super::Char {}
    impl Sealed for super::Any {}
}

/// A register holding a value of kind `K`. Copyable; it only names the
/// register.
pub struct Reg<K> {
    id: OperandId,
    kind: PhantomData<fn() -> K>,
}

impl<K> Clone for Reg<K> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K> Copy for Reg<K> {}

impl<K> std::fmt::Debug for Reg<K> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Reg({})", self.id.0)
    }
}

impl<K> Reg<K> {
    const fn new(id: OperandId) -> Self {
        Self {
            id,
            kind: PhantomData,
        }
    }

    /// The register's operand id.
    #[must_use]
    pub const fn id(self) -> OperandId {
        self.id
    }

    /// The same register, its type left to the run.
    #[must_use]
    pub const fn any(self) -> Reg<Any> {
        Reg::new(self.id)
    }
}

impl Reg<Any> {
    /// The same register, taken to hold a `K`. Nothing checks this while
    /// building; an instruction given a value of the wrong type fails when
    /// the program runs.
    #[must_use]
    pub const fn assume<K>(self) -> Reg<K> {
        Reg::new(self.id)
    }
}

/// An instruction with the source location of the call that emitted it.
pub(crate) struct Emitted {
    pub(crate) instruction: Instruction,
    pub(crate) location: &'static Location<'static>,
}

/// Builds one program. See the [module documentation](self).
pub struct Builder {
    constants: Vec<Emitted>,
    main: Vec<Emitted>,
    next_id: u32,
    int_constants: HashMap<i32, OperandId>,
    uint_constants: HashMap<u32, OperandId>,
    bool_constants: HashMap<bool, OperandId>,
    string_constants: HashMap<String, OperandId>,
    imports: Vec<(OperandId, Import)>,
    results: Vec<OperandId>,
    loop_depth: usize,
}

impl Default for Builder {
    fn default() -> Self {
        Self::new()
    }
}

impl Builder {
    /// An empty program.
    #[must_use]
    pub fn new() -> Self {
        Self {
            constants: Vec::new(),
            main: Vec::new(),
            next_id: 1,
            int_constants: HashMap::new(),
            uint_constants: HashMap::new(),
            bool_constants: HashMap::new(),
            string_constants: HashMap::new(),
            imports: Vec::new(),
            results: Vec::new(),
            loop_depth: 0,
        }
    }

    fn allocate<K>(&mut self) -> Reg<K> {
        let id = OperandId(self.next_id);
        self.next_id += 1;
        Reg::new(id)
    }

    /// Emits `instruction` into the program, recording the caller's source
    /// location, and returns its index in the program's main section.
    #[track_caller]
    fn emit(&mut self, instruction: Instruction) -> usize {
        self.main.push(Emitted {
            instruction,
            location: Location::caller(),
        });
        self.main.len() - 1
    }

    #[track_caller]
    fn emit_constant(&mut self, instruction: Instruction) {
        self.constants.push(Emitted {
            instruction,
            location: Location::caller(),
        });
    }

    /// Emits an instruction that writes a new register and returns it.
    #[track_caller]
    fn produce<K>(&mut self, make: impl FnOnce(OperandId) -> Instruction) -> Reg<K> {
        let result = self.allocate();
        self.emit(make(result.id));
        result
    }

    /// The offset from instruction `from` to the next one to be emitted.
    fn offset_to_next(&self, from: usize) -> i32 {
        i32::try_from(self.main.len() - from).unwrap_or(i32::MAX)
    }

    fn patch(&mut self, at: usize, offset: i32) {
        match &mut self.main[at].instruction {
            Instruction::ForkIfFalse { offset: o, .. }
            | Instruction::ForkIfTrue { offset: o, .. }
            | Instruction::Fork { offset: o }
            | Instruction::NewTryBlock { catch_offset: o }
            | Instruction::NewLoopBlock {
                break_offset: o, ..
            } => *o = offset,
            _ => unreachable!("only jumps are patched"),
        }
    }

    // Imports and results.

    /// Imports `element` into a new register. Every element and text range
    /// in one program must come from the same provider process.
    pub fn import_element(&mut self, element: &IUIAutomationElement) -> Reg<Element> {
        let reg = self.allocate();
        self.imports
            .push((reg.id, Import::Element(element.clone())));
        reg
    }

    /// Imports `range` into a new register.
    pub fn import_text_range(&mut self, range: &IUIAutomationTextRange) -> Reg<TextRange> {
        let reg = self.allocate();
        self.imports
            .push((reg.id, Import::TextRange(range.clone())));
        reg
    }

    /// Asks for `reg`'s value after the run, and returns it for reading
    /// with [`Outcome::get`](crate::Outcome::get).
    pub fn add_to_results<K>(&mut self, reg: Reg<K>) -> Reg<K> {
        if !self.results.contains(&reg.id) {
            self.results.push(reg.id);
        }
        reg
    }

    /// Finishes the program with a `Halt`, as NVDA ends every program.
    #[must_use]
    #[track_caller]
    pub fn finish(mut self) -> Operation {
        self.emit(Instruction::Halt);
        let mut emitted = self.constants;
        emitted.append(&mut self.main);
        Operation::new(emitted, self.imports, self.results)
    }

    // Constants and constructors.

    /// A constant integer, set once at the start of the program and shared
    /// by every use of the same value. Never change a constant's register.
    #[track_caller]
    pub fn int(&mut self, value: i32) -> Reg<Int> {
        if let Some(&id) = self.int_constants.get(&value) {
            return Reg::new(id);
        }
        let reg: Reg<Int> = self.allocate();
        self.emit_constant(Instruction::NewInt {
            result: reg.id,
            value,
        });
        self.int_constants.insert(value, reg.id);
        reg
    }

    /// A constant unsigned integer; see [`Builder::int`].
    #[track_caller]
    pub fn uint(&mut self, value: u32) -> Reg<Uint> {
        if let Some(&id) = self.uint_constants.get(&value) {
            return Reg::new(id);
        }
        let reg: Reg<Uint> = self.allocate();
        self.emit_constant(Instruction::NewUint {
            result: reg.id,
            value,
        });
        self.uint_constants.insert(value, reg.id);
        reg
    }

    /// A constant boolean; see [`Builder::int`].
    #[track_caller]
    pub fn bool(&mut self, value: bool) -> Reg<Bool> {
        if let Some(&id) = self.bool_constants.get(&value) {
            return Reg::new(id);
        }
        let reg: Reg<Bool> = self.allocate();
        self.emit_constant(Instruction::NewBool {
            result: reg.id,
            value,
        });
        self.bool_constants.insert(value, reg.id);
        reg
    }

    /// A constant string; see [`Builder::int`].
    #[track_caller]
    pub fn string(&mut self, value: &str) -> Reg<Str> {
        if let Some(&id) = self.string_constants.get(value) {
            return Reg::new(id);
        }
        let reg: Reg<Str> = self.allocate();
        self.emit_constant(Instruction::NewString {
            result: reg.id,
            value: value.to_owned(),
        });
        self.string_constants.insert(value.to_owned(), reg.id);
        reg
    }

    /// A new integer variable, set where this is emitted (each time a loop
    /// passes it).
    #[track_caller]
    pub fn new_int(&mut self, value: i32) -> Reg<Int> {
        self.produce(|result| Instruction::NewInt { result, value })
    }

    /// A new unsigned integer variable.
    #[track_caller]
    pub fn new_uint(&mut self, value: u32) -> Reg<Uint> {
        self.produce(|result| Instruction::NewUint { result, value })
    }

    /// A new boolean variable.
    #[track_caller]
    pub fn new_bool(&mut self, value: bool) -> Reg<Bool> {
        self.produce(|result| Instruction::NewBool { result, value })
    }

    /// A new double variable.
    #[track_caller]
    pub fn new_double(&mut self, value: f64) -> Reg<Double> {
        self.produce(|result| Instruction::NewDouble { result, value })
    }

    /// A new character variable.
    #[track_caller]
    pub fn new_char(&mut self, value: u16) -> Reg<Char> {
        self.produce(|result| Instruction::NewChar { result, value })
    }

    /// A new string variable.
    #[track_caller]
    pub fn new_string(&mut self, value: &str) -> Reg<Str> {
        let value = value.to_owned();
        self.produce(|result| Instruction::NewString { result, value })
    }

    /// A new point.
    #[track_caller]
    pub fn new_point(&mut self, x: f64, y: f64) -> Reg<Point> {
        self.produce(|result| Instruction::NewPoint { result, x, y })
    }

    /// A new rectangle.
    #[track_caller]
    pub fn new_rect(&mut self, left: f64, top: f64, width: f64, height: f64) -> Reg<Rect> {
        self.produce(|result| Instruction::NewRect {
            result,
            left,
            top,
            width,
            height,
        })
    }

    /// A new, empty array.
    #[track_caller]
    pub fn new_array(&mut self) -> Reg<Array> {
        self.produce(|result| Instruction::NewArray { result })
    }

    /// A new, empty string map.
    #[track_caller]
    pub fn new_string_map(&mut self) -> Reg<StringMap> {
        self.produce(|result| Instruction::NewStringMap { result })
    }

    /// A new null, which an element or text range register can be set to.
    #[track_caller]
    pub fn new_null(&mut self) -> Reg<Any> {
        self.produce(|result| Instruction::NewNull { result })
    }

    /// A new null element, for a register that may be given an element
    /// later.
    #[track_caller]
    pub fn new_null_element(&mut self) -> Reg<Element> {
        self.produce(|result| Instruction::NewNull { result })
    }

    /// A new GUID.
    #[track_caller]
    pub fn new_guid(&mut self, value: GUID) -> Reg<Guid> {
        self.produce(|result| Instruction::NewGuid { result, value })
    }

    /// A new, empty cache request.
    #[track_caller]
    pub fn new_cache_request(&mut self) -> Reg<CacheRequest> {
        self.produce(|result| Instruction::NewCacheRequest { result })
    }

    // General instructions.

    /// Sets `target` to `value`. Elements, text ranges, arrays, and string
    /// maps are held by reference, so both registers then name one object.
    #[track_caller]
    pub fn set<K>(&mut self, target: Reg<K>, value: Reg<K>) {
        self.emit(Instruction::Set {
            target: target.id,
            value: value.id,
        });
    }

    /// Whether `left` equals `right`.
    #[track_caller]
    pub fn equal<K>(&mut self, left: Reg<K>, right: Reg<K>) -> Reg<Bool> {
        self.compare_unchecked(left.id, right.id, Comparison::Equal)
    }

    /// Whether `left` differs from `right`.
    #[track_caller]
    pub fn not_equal<K>(&mut self, left: Reg<K>, right: Reg<K>) -> Reg<Bool> {
        self.compare_unchecked(left.id, right.id, Comparison::NotEqual)
    }

    /// Compares two ordered values.
    #[track_caller]
    pub fn compare<K: Ordered>(
        &mut self,
        left: Reg<K>,
        right: Reg<K>,
        comparison: Comparison,
    ) -> Reg<Bool> {
        self.compare_unchecked(left.id, right.id, comparison)
    }

    #[track_caller]
    fn compare_unchecked(
        &mut self,
        left: OperandId,
        right: OperandId,
        comparison: Comparison,
    ) -> Reg<Bool> {
        self.produce(|result| Instruction::Compare {
            result,
            left,
            right,
            comparison,
        })
    }

    /// Whether `reg` holds a value of the tested type.
    #[track_caller]
    pub fn is<K>(&mut self, test: TypeTest, reg: Reg<K>) -> Reg<Bool> {
        self.produce(|result| Instruction::Is {
            test,
            result,
            target: reg.id,
        })
    }

    /// Whether `reg` is null.
    #[track_caller]
    pub fn is_null<K>(&mut self, reg: Reg<K>) -> Reg<Bool> {
        self.is(TypeTest::Null, reg)
    }

    /// `reg`'s value as a string.
    #[track_caller]
    pub fn stringify<K>(&mut self, reg: Reg<K>) -> Reg<Str> {
        self.produce(|result| Instruction::Stringify {
            result,
            target: reg.id,
        })
    }

    /// The operation's status: zero, or the error an instruction raised.
    #[track_caller]
    pub fn get_operation_status(&mut self) -> Reg<Int> {
        self.produce(|result| Instruction::GetOperationStatus { result })
    }

    /// Sets the operation's status.
    #[track_caller]
    pub fn set_operation_status(&mut self, status: Reg<Int>) {
        self.emit(Instruction::SetOperationStatus { status: status.id });
    }

    // Arithmetic.

    /// `left + right`.
    #[track_caller]
    pub fn add<K: Numeric>(&mut self, left: Reg<K>, right: Reg<K>) -> Reg<K> {
        self.produce(|result| Instruction::BinaryAdd {
            result,
            left: left.id,
            right: right.id,
        })
    }

    /// `left - right`.
    #[track_caller]
    pub fn subtract<K: Numeric>(&mut self, left: Reg<K>, right: Reg<K>) -> Reg<K> {
        self.produce(|result| Instruction::BinarySubtract {
            result,
            left: left.id,
            right: right.id,
        })
    }

    /// `left * right`.
    #[track_caller]
    pub fn multiply<K: Numeric>(&mut self, left: Reg<K>, right: Reg<K>) -> Reg<K> {
        self.produce(|result| Instruction::BinaryMultiply {
            result,
            left: left.id,
            right: right.id,
        })
    }

    /// `left / right`.
    #[track_caller]
    pub fn divide<K: Numeric>(&mut self, left: Reg<K>, right: Reg<K>) -> Reg<K> {
        self.produce(|result| Instruction::BinaryDivide {
            result,
            left: left.id,
            right: right.id,
        })
    }

    /// `target += value`.
    #[track_caller]
    pub fn add_assign<K: Numeric>(&mut self, target: Reg<K>, value: Reg<K>) {
        self.emit(Instruction::Add {
            target: target.id,
            value: value.id,
        });
    }

    /// `target -= value`.
    #[track_caller]
    pub fn subtract_assign<K: Numeric>(&mut self, target: Reg<K>, value: Reg<K>) {
        self.emit(Instruction::Subtract {
            target: target.id,
            value: value.id,
        });
    }

    /// `target *= value`.
    #[track_caller]
    pub fn multiply_assign<K: Numeric>(&mut self, target: Reg<K>, value: Reg<K>) {
        self.emit(Instruction::Multiply {
            target: target.id,
            value: value.id,
        });
    }

    /// `target /= value`.
    #[track_caller]
    pub fn divide_assign<K: Numeric>(&mut self, target: Reg<K>, value: Reg<K>) {
        self.emit(Instruction::Divide {
            target: target.id,
            value: value.id,
        });
    }

    // Booleans.

    /// `!value`.
    #[track_caller]
    pub fn not(&mut self, value: Reg<Bool>) -> Reg<Bool> {
        self.produce(|result| Instruction::BoolNot {
            result,
            target: value.id,
        })
    }

    /// `left && right` (both are already evaluated).
    #[track_caller]
    pub fn and(&mut self, left: Reg<Bool>, right: Reg<Bool>) -> Reg<Bool> {
        self.produce(|result| Instruction::BoolAnd {
            result,
            left: left.id,
            right: right.id,
        })
    }

    /// `left || right` (both are already evaluated).
    #[track_caller]
    pub fn or(&mut self, left: Reg<Bool>, right: Reg<Bool>) -> Reg<Bool> {
        self.produce(|result| Instruction::BoolOr {
            result,
            left: left.id,
            right: right.id,
        })
    }

    /// Negates `target` in place.
    #[track_caller]
    pub fn not_assign(&mut self, target: Reg<Bool>) {
        self.emit(Instruction::InPlaceBoolNot { target: target.id });
    }

    /// `target = target && value`.
    #[track_caller]
    pub fn and_assign(&mut self, target: Reg<Bool>, value: Reg<Bool>) {
        self.emit(Instruction::InPlaceBoolAnd {
            target: target.id,
            value: value.id,
        });
    }

    /// `target = target || value`.
    #[track_caller]
    pub fn or_assign(&mut self, target: Reg<Bool>, value: Reg<Bool>) {
        self.emit(Instruction::InPlaceBoolOr {
            target: target.id,
            value: value.id,
        });
    }

    // Points and rectangles.

    /// One coordinate of a point.
    #[track_caller]
    pub fn point_property(&mut self, point: Reg<Point>, property: PointProperty) -> Reg<Double> {
        self.produce(|result| Instruction::GetPointProperty {
            result,
            target: point.id,
            property,
        })
    }

    /// One part of a rectangle.
    #[track_caller]
    pub fn rect_property(&mut self, rect: Reg<Rect>, property: RectProperty) -> Reg<Double> {
        self.produce(|result| Instruction::GetRectProperty {
            result,
            target: rect.id,
            property,
        })
    }

    // Arrays.

    /// Appends `value` to `array`.
    #[track_caller]
    pub fn array_append<K>(&mut self, array: Reg<Array>, value: Reg<K>) {
        self.emit(Instruction::ArrayAppend {
            target: array.id,
            value: value.id,
        });
    }

    /// Sets the item at `index`.
    #[track_caller]
    pub fn array_set_at<I: Index, K>(&mut self, array: Reg<Array>, index: Reg<I>, value: Reg<K>) {
        self.emit(Instruction::ArraySetAt {
            target: array.id,
            index: index.id,
            value: value.id,
        });
    }

    /// Removes the item at `index`, returning it.
    #[track_caller]
    pub fn array_remove_at<I: Index>(&mut self, array: Reg<Array>, index: Reg<I>) -> Reg<Any> {
        self.produce(|result| Instruction::ArrayRemoveAt {
            result,
            target: array.id,
            index: index.id,
        })
    }

    /// The item at `index`.
    #[track_caller]
    pub fn array_get_at<I: Index>(&mut self, array: Reg<Array>, index: Reg<I>) -> Reg<Any> {
        self.produce(|result| Instruction::ArrayGetAt {
            result,
            target: array.id,
            index: index.id,
        })
    }

    /// The number of items.
    #[track_caller]
    pub fn array_size(&mut self, array: Reg<Array>) -> Reg<Uint> {
        self.produce(|result| Instruction::ArraySize {
            result,
            target: array.id,
        })
    }

    // String maps.

    /// Inserts or replaces the entry for `key`.
    #[track_caller]
    pub fn map_insert<K>(&mut self, map: Reg<StringMap>, key: Reg<Str>, value: Reg<K>) {
        self.emit(Instruction::StringMapInsert {
            target: map.id,
            key: key.id,
            value: value.id,
        });
    }

    /// Removes the entry for `key`, returning what the platform reports.
    #[track_caller]
    pub fn map_remove(&mut self, map: Reg<StringMap>, key: Reg<Str>) -> Reg<Any> {
        self.produce(|result| Instruction::StringMapRemove {
            result,
            target: map.id,
            key: key.id,
        })
    }

    /// Whether the map has an entry for `key`.
    #[track_caller]
    pub fn map_has_key(&mut self, map: Reg<StringMap>, key: Reg<Str>) -> Reg<Bool> {
        self.produce(|result| Instruction::StringMapHasKey {
            result,
            target: map.id,
            key: key.id,
        })
    }

    /// The entry for `key`; the run fails if there is none.
    #[track_caller]
    pub fn map_lookup(&mut self, map: Reg<StringMap>, key: Reg<Str>) -> Reg<Any> {
        self.produce(|result| Instruction::StringMapLookup {
            result,
            target: map.id,
            key: key.id,
        })
    }

    /// The number of entries.
    #[track_caller]
    pub fn map_size(&mut self, map: Reg<StringMap>) -> Reg<Uint> {
        self.produce(|result| Instruction::StringMapSize {
            result,
            target: map.id,
        })
    }

    // Strings.

    /// The character at `index`.
    #[track_caller]
    pub fn string_get_at<I: Index>(&mut self, string: Reg<Str>, index: Reg<I>) -> Reg<Char> {
        self.produce(|result| Instruction::StringGetAt {
            result,
            target: string.id,
            index: index.id,
        })
    }

    /// `length` characters from `index`.
    #[track_caller]
    pub fn substring<I: Index>(
        &mut self,
        string: Reg<Str>,
        index: Reg<I>,
        length: Reg<I>,
    ) -> Reg<Str> {
        self.produce(|result| Instruction::StringSubstr {
            result,
            target: string.id,
            index: index.id,
            length: length.id,
        })
    }

    /// `left` followed by `right`.
    #[track_caller]
    pub fn concat(&mut self, left: Reg<Str>, right: Reg<Str>) -> Reg<Str> {
        self.produce(|result| Instruction::StringConcat {
            result,
            left: left.id,
            right: right.id,
        })
    }

    /// The string's length in UTF-16 code units.
    #[track_caller]
    pub fn string_size(&mut self, string: Reg<Str>) -> Reg<Uint> {
        self.produce(|result| Instruction::StringSize {
            result,
            target: string.id,
        })
    }

    // Elements and cache requests.

    /// An element's current property value. With `ignore_default`, a
    /// property the element does not support reads as UIA's reserved
    /// not-supported value rather than the property's default.
    #[track_caller]
    pub fn get_property_value(
        &mut self,
        element: Reg<Element>,
        property: Reg<Int>,
        ignore_default: Reg<Bool>,
    ) -> Reg<Any> {
        self.produce(|result| Instruction::GetPropertyValue {
            result,
            target: element.id,
            property: property.id,
            ignore_default: ignore_default.id,
        })
    }

    /// An element's current value for `property`, defaults included: the
    /// common case of [`Builder::get_property_value`].
    #[track_caller]
    pub fn property(&mut self, element: Reg<Element>, property: i32) -> Reg<Any> {
        let property = self.int(property);
        let ignore_default = self.bool(false);
        self.get_property_value(element, property, ignore_default)
    }

    /// The raw-view neighbor in `direction`, or null.
    #[track_caller]
    pub fn navigate(
        &mut self,
        element: Reg<Element>,
        direction: NavigationDirection,
    ) -> Reg<Element> {
        let direction = self.int(direction.into());
        self.produce(|result| Instruction::Navigate {
            result,
            target: element.id,
            direction: direction.id,
        })
    }

    /// Adds a property to a cache request.
    #[track_caller]
    pub fn cache_request_add_property(&mut self, request: Reg<CacheRequest>, property: i32) {
        let property = self.int(property);
        self.emit(Instruction::CacheRequestAddProperty {
            target: request.id,
            property: property.id,
        });
    }

    /// Adds a pattern to a cache request.
    #[track_caller]
    pub fn cache_request_add_pattern(&mut self, request: Reg<CacheRequest>, pattern: i32) {
        let pattern = self.int(pattern);
        self.emit(Instruction::CacheRequestAddPattern {
            target: request.id,
            pattern: pattern.id,
        });
    }

    /// Fills `element`'s cache from `request`, inside the provider; the
    /// element read back from the results carries the cache.
    #[track_caller]
    pub fn populate_cache(&mut self, element: Reg<Element>, request: Reg<CacheRequest>) {
        self.emit(Instruction::PopulateCache {
            target: element.id,
            cache_request: request.id,
        });
    }

    /// A metadata value of an element's property.
    #[track_caller]
    pub fn get_metadata_value(
        &mut self,
        element: Reg<Element>,
        property: Reg<Int>,
        metadata: Reg<Int>,
    ) -> Reg<Any> {
        self.produce(|result| Instruction::GetMetadataValue {
            result,
            target: element.id,
            property: property.id,
            metadata: metadata.id,
        })
    }

    // GUIDs and extensions.

    /// The integer id registered for a GUID of `identifier_type` (UIA's
    /// `AutomationIdentifierType`).
    #[track_caller]
    pub fn lookup_id(&mut self, guid: Reg<Guid>, identifier_type: i32) -> Reg<Int> {
        self.produce(|result| Instruction::LookupId {
            result,
            guid: guid.id,
            identifier_type,
        })
    }

    /// The GUID registered for an integer id of `identifier_type`.
    #[track_caller]
    pub fn lookup_guid(&mut self, id: Reg<Int>, identifier_type: i32) -> Reg<Guid> {
        self.produce(|result| Instruction::LookupGuid {
            result,
            id: id.id,
            identifier_type,
        })
    }

    /// Calls a provider extension on `target` with `arguments`, which the
    /// extension may write.
    #[track_caller]
    pub fn call_extension<K>(
        &mut self,
        target: Reg<K>,
        extension: Reg<Guid>,
        arguments: &[Reg<Any>],
    ) {
        self.emit(Instruction::CallExtension {
            target: target.id,
            extension: extension.id,
            arguments: arguments.iter().map(|argument| argument.id).collect(),
        });
    }

    /// Whether `target` supports a provider extension.
    #[track_caller]
    pub fn is_extension_supported<K>(&mut self, target: Reg<K>, extension: Reg<Guid>) -> Reg<Bool> {
        self.produce(|result| Instruction::IsExtensionSupported {
            result,
            target: target.id,
            extension: extension.id,
        })
    }

    // Text ranges.

    /// A copy of the range.
    #[track_caller]
    pub fn text_range_clone(&mut self, range: Reg<TextRange>) -> Reg<TextRange> {
        self.produce(|result| Instruction::TextRangeClone {
            result,
            target: range.id,
        })
    }

    /// Whether two ranges span the same text.
    #[track_caller]
    pub fn text_range_compare(
        &mut self,
        range: Reg<TextRange>,
        other: Reg<TextRange>,
    ) -> Reg<Bool> {
        self.produce(|result| Instruction::TextRangeCompare {
            result,
            target: range.id,
            other: other.id,
        })
    }

    /// Compares an endpoint of `range` with an endpoint of `other`.
    #[track_caller]
    pub fn text_range_compare_endpoints(
        &mut self,
        range: Reg<TextRange>,
        endpoint: Reg<Int>,
        other: Reg<TextRange>,
        other_endpoint: Reg<Int>,
    ) -> Reg<Int> {
        self.produce(|result| Instruction::TextRangeCompareEndpoints {
            result,
            target: range.id,
            endpoint: endpoint.id,
            other: other.id,
            other_endpoint: other_endpoint.id,
        })
    }

    /// Expands the range to the enclosing `unit`.
    #[track_caller]
    pub fn text_range_expand_to_enclosing_unit(&mut self, range: Reg<TextRange>, unit: Reg<Int>) {
        self.emit(Instruction::TextRangeExpandToEnclosingUnit {
            target: range.id,
            unit: unit.id,
        });
    }

    /// The first sub-range with `attribute` equal to `value`, or null.
    #[track_caller]
    pub fn text_range_find_attribute(
        &mut self,
        range: Reg<TextRange>,
        attribute: Reg<Int>,
        value: Reg<Any>,
        backward: Reg<Bool>,
    ) -> Reg<TextRange> {
        self.produce(|result| Instruction::TextRangeFindAttribute {
            result,
            target: range.id,
            attribute: attribute.id,
            value: value.id,
            backward: backward.id,
        })
    }

    /// The first sub-range containing `text`, or null.
    #[track_caller]
    pub fn text_range_find_text(
        &mut self,
        range: Reg<TextRange>,
        text: Reg<Str>,
        backward: Reg<Bool>,
        ignore_case: Reg<Bool>,
    ) -> Reg<TextRange> {
        self.produce(|result| Instruction::TextRangeFindText {
            result,
            target: range.id,
            text: text.id,
            backward: backward.id,
            ignore_case: ignore_case.id,
        })
    }

    /// A text attribute's value over the range.
    #[track_caller]
    pub fn text_range_get_attribute_value(
        &mut self,
        range: Reg<TextRange>,
        attribute: Reg<Int>,
    ) -> Reg<Any> {
        self.produce(|result| Instruction::TextRangeGetAttributeValue {
            result,
            target: range.id,
            attribute: attribute.id,
        })
    }

    /// The range's bounding rectangles.
    #[track_caller]
    pub fn text_range_get_bounding_rectangles(&mut self, range: Reg<TextRange>) -> Reg<Array> {
        self.produce(|result| Instruction::TextRangeGetBoundingRectangles {
            result,
            target: range.id,
        })
    }

    /// The element enclosing the range.
    #[track_caller]
    pub fn text_range_get_enclosing_element(&mut self, range: Reg<TextRange>) -> Reg<Element> {
        self.produce(|result| Instruction::TextRangeGetEnclosingElement {
            result,
            target: range.id,
        })
    }

    /// The range's text, at most `max_length` characters (-1 for all).
    #[track_caller]
    pub fn text_range_get_text(&mut self, range: Reg<TextRange>, max_length: Reg<Int>) -> Reg<Str> {
        self.produce(|result| Instruction::TextRangeGetText {
            result,
            target: range.id,
            max_length: max_length.id,
        })
    }

    /// Moves the range by `count` units; the number actually moved.
    #[track_caller]
    pub fn text_range_move(
        &mut self,
        range: Reg<TextRange>,
        unit: Reg<Int>,
        count: Reg<Int>,
    ) -> Reg<Int> {
        self.produce(|result| Instruction::TextRangeMove {
            result,
            target: range.id,
            unit: unit.id,
            count: count.id,
        })
    }

    /// Moves one endpoint by `count` units; the number actually moved.
    #[track_caller]
    pub fn text_range_move_endpoint_by_unit(
        &mut self,
        range: Reg<TextRange>,
        endpoint: Reg<Int>,
        unit: Reg<Int>,
        count: Reg<Int>,
    ) -> Reg<Int> {
        self.produce(|result| Instruction::TextRangeMoveEndpointByUnit {
            result,
            target: range.id,
            endpoint: endpoint.id,
            unit: unit.id,
            count: count.id,
        })
    }

    /// Moves one endpoint to an endpoint of `other`.
    #[track_caller]
    pub fn text_range_move_endpoint_by_range(
        &mut self,
        range: Reg<TextRange>,
        endpoint: Reg<Int>,
        other: Reg<TextRange>,
        other_endpoint: Reg<Int>,
    ) {
        self.emit(Instruction::TextRangeMoveEndpointByRange {
            target: range.id,
            endpoint: endpoint.id,
            other: other.id,
            other_endpoint: other_endpoint.id,
        });
    }

    /// Selects the range.
    #[track_caller]
    pub fn text_range_select(&mut self, range: Reg<TextRange>) {
        self.emit(Instruction::TextRangeSelect { target: range.id });
    }

    /// Adds the range to the selection.
    #[track_caller]
    pub fn text_range_add_to_selection(&mut self, range: Reg<TextRange>) {
        self.emit(Instruction::TextRangeAddToSelection { target: range.id });
    }

    /// Removes the range from the selection.
    #[track_caller]
    pub fn text_range_remove_from_selection(&mut self, range: Reg<TextRange>) {
        self.emit(Instruction::TextRangeRemoveFromSelection { target: range.id });
    }

    /// Scrolls the range into view.
    #[track_caller]
    pub fn text_range_scroll_into_view(&mut self, range: Reg<TextRange>, align_to_top: Reg<Bool>) {
        self.emit(Instruction::TextRangeScrollIntoView {
            target: range.id,
            align_to_top: align_to_top.id,
        });
    }

    /// The elements embedded in the range.
    #[track_caller]
    pub fn text_range_get_children(&mut self, range: Reg<TextRange>) -> Reg<Array> {
        self.produce(|result| Instruction::TextRangeGetChildren {
            result,
            target: range.id,
        })
    }

    /// Shows the range's context menu.
    #[track_caller]
    pub fn text_range_show_context_menu(&mut self, range: Reg<TextRange>) {
        self.emit(Instruction::TextRangeShowContextMenu { target: range.id });
    }

    // Control flow.

    /// Emits `body`, run when `condition` is true.
    #[track_caller]
    pub fn if_(&mut self, condition: Reg<Bool>, body: impl FnOnce(&mut Self)) {
        let fork = self.emit(Instruction::ForkIfFalse {
            condition: condition.id,
            offset: 1,
        });
        body(self);
        let offset = self.offset_to_next(fork);
        self.patch(fork, offset);
    }

    /// Emits `then`, run when `condition` is true, and `otherwise`, run
    /// when it is false.
    #[track_caller]
    pub fn if_else(
        &mut self,
        condition: Reg<Bool>,
        then: impl FnOnce(&mut Self),
        otherwise: impl FnOnce(&mut Self),
    ) {
        let fork = self.emit(Instruction::ForkIfFalse {
            condition: condition.id,
            offset: 1,
        });
        then(self);
        let skip = self.emit(Instruction::Fork { offset: 1 });
        let offset = self.offset_to_next(fork);
        self.patch(fork, offset);
        otherwise(self);
        let offset = self.offset_to_next(skip);
        self.patch(skip, offset);
    }

    /// Emits a loop that evaluates `condition` before each pass and runs
    /// `body` while it is true. `condition` emits the instructions that
    /// compute the boolean, so they run on every pass. Inside `body`,
    /// [`Builder::break_loop`] and [`Builder::continue_loop`] apply to this
    /// loop.
    #[track_caller]
    pub fn while_(
        &mut self,
        condition: impl FnOnce(&mut Self) -> Reg<Bool>,
        body: impl FnOnce(&mut Self),
    ) {
        // NVDA's layout: the loop block's continue target is the
        // condition, just after it; a false condition jumps to the end of
        // the loop block, and its break target is just past that.
        let block = self.emit(Instruction::NewLoopBlock {
            break_offset: 1,
            continue_offset: 1,
        });
        let condition = condition(self);
        let fork = self.emit(Instruction::ForkIfFalse {
            condition: condition.id,
            offset: 1,
        });
        self.loop_depth += 1;
        body(self);
        self.loop_depth -= 1;
        self.emit(Instruction::ContinueLoop);
        let offset = self.offset_to_next(fork);
        self.patch(fork, offset);
        self.emit(Instruction::EndLoopBlock);
        let offset = self.offset_to_next(block);
        self.patch(block, offset);
    }

    /// Leaves the innermost loop.
    ///
    /// # Panics
    ///
    /// When emitted outside a loop's body, a mistake in the program.
    #[track_caller]
    pub fn break_loop(&mut self) {
        assert!(self.loop_depth > 0, "break_loop outside a loop");
        self.emit(Instruction::BreakLoop);
    }

    /// Goes back to the innermost loop's condition.
    ///
    /// # Panics
    ///
    /// When emitted outside a loop's body, a mistake in the program.
    #[track_caller]
    pub fn continue_loop(&mut self) {
        assert!(self.loop_depth > 0, "continue_loop outside a loop");
        self.emit(Instruction::ContinueLoop);
    }

    /// Emits `body`, and `catch`, run instead of the rest of `body` when an
    /// instruction in it fails. `catch` is given the failure's status,
    /// after which the operation's status is reset to zero, as NVDA's catch
    /// blocks do.
    #[track_caller]
    pub fn try_catch(
        &mut self,
        body: impl FnOnce(&mut Self),
        catch: impl FnOnce(&mut Self, Reg<Int>),
    ) {
        let block = self.emit(Instruction::NewTryBlock { catch_offset: 1 });
        body(self);
        self.emit(Instruction::EndTryBlock);
        let skip = self.emit(Instruction::Fork { offset: 1 });
        let offset = self.offset_to_next(block);
        self.patch(block, offset);
        let status = self.get_operation_status();
        let zero = self.int(0);
        self.set_operation_status(zero);
        catch(self, status);
        let offset = self.offset_to_next(skip);
        self.patch(skip, offset);
    }

    /// Ends the run here.
    #[track_caller]
    pub fn halt(&mut self) {
        self.emit(Instruction::Halt);
    }
}

#[cfg(test)]
mod tests {
    use super::Builder;
    use crate::instruction::{Instruction, OperandId};
    use crate::opcode::Comparison;

    fn instructions(builder: Builder) -> Vec<Instruction> {
        builder
            .finish()
            .emitted()
            .iter()
            .map(|emitted| emitted.instruction.clone())
            .collect()
    }

    #[test]
    fn constants_come_first_and_are_shared() {
        let mut b = Builder::new();
        let a = b.new_int(7);
        let one = b.int(1);
        b.add_assign(a, one);
        let again = b.int(1);
        assert_eq!(one.id(), again.id());
        b.add_assign(a, again);
        assert_eq!(
            instructions(b),
            vec![
                Instruction::NewInt {
                    result: OperandId(2),
                    value: 1
                },
                Instruction::NewInt {
                    result: OperandId(1),
                    value: 7
                },
                Instruction::Add {
                    target: OperandId(1),
                    value: OperandId(2)
                },
                Instruction::Add {
                    target: OperandId(1),
                    value: OperandId(2)
                },
                Instruction::Halt,
            ]
        );
    }

    #[test]
    fn if_jumps_past_its_body() {
        let mut b = Builder::new();
        let condition = b.new_bool(true);
        let x = b.new_int(0);
        b.if_(condition, |b| {
            let one = b.new_int(1);
            b.set(x, one);
        });
        let program = instructions(b);
        // NewBool, NewInt, ForkIfFalse, NewInt, Set, Halt.
        assert_eq!(
            program[2],
            Instruction::ForkIfFalse {
                condition: OperandId(1),
                offset: 3
            }
        );
        assert_eq!(program[5], Instruction::Halt);
    }

    #[test]
    fn if_else_skips_the_other_branch() {
        let mut b = Builder::new();
        let condition = b.new_bool(true);
        let x = b.new_int(0);
        b.if_else(
            condition,
            |b| {
                let one = b.new_int(1);
                b.set(x, one);
            },
            |b| {
                let two = b.new_int(2);
                b.set(x, two);
            },
        );
        let program = instructions(b);
        // 0 NewBool, 1 NewInt, 2 ForkIfFalse, 3 NewInt, 4 Set, 5 Fork,
        // 6 NewInt, 7 Set, 8 Halt.
        assert_eq!(
            program[2],
            Instruction::ForkIfFalse {
                condition: OperandId(1),
                offset: 4
            }
        );
        assert_eq!(program[5], Instruction::Fork { offset: 3 });
        assert_eq!(program[8], Instruction::Halt);
    }

    #[test]
    fn while_loops_back_to_its_condition_and_breaks_past_its_end() {
        let mut b = Builder::new();
        let count = b.new_int(0);
        let limit = b.new_int(3);
        b.while_(
            |b| b.compare(count, limit, Comparison::LessThan),
            |b| {
                let flag = b.new_bool(false);
                b.if_(flag, Builder::break_loop);
                let one = b.new_int(1);
                b.add_assign(count, one);
            },
        );
        let program = instructions(b);
        // 0 NewInt, 1 NewInt, 2 NewLoopBlock, 3 Compare, 4 ForkIfFalse,
        // 5 NewBool, 6 ForkIfFalse, 7 BreakLoop, 8 NewInt, 9 Add,
        // 10 ContinueLoop, 11 EndLoopBlock, 12 Halt.
        assert_eq!(
            program[2],
            Instruction::NewLoopBlock {
                break_offset: 10,
                continue_offset: 1
            }
        );
        assert_eq!(
            program[4],
            Instruction::ForkIfFalse {
                condition: OperandId(3),
                offset: 7
            }
        );
        assert_eq!(
            program[6],
            Instruction::ForkIfFalse {
                condition: OperandId(4),
                offset: 2
            }
        );
        assert_eq!(program[7], Instruction::BreakLoop);
        assert_eq!(program[10], Instruction::ContinueLoop);
        assert_eq!(program[11], Instruction::EndLoopBlock);
        assert_eq!(program[12], Instruction::Halt);
    }

    #[test]
    fn try_catch_jumps_to_its_handler_and_resets_the_status() {
        let mut b = Builder::new();
        let x = b.new_int(0);
        let caught = b.new_int(0);
        b.try_catch(
            |b| {
                let one = b.new_int(1);
                b.set(x, one);
            },
            |b, status| b.set(caught, status),
        );
        let program = instructions(b);
        // Constants: 0 NewInt (zero). Main: 1 NewInt, 2 NewInt,
        // 3 NewTryBlock, 4 NewInt, 5 Set, 6 EndTryBlock, 7 Fork,
        // 8 GetOperationStatus, 9 SetOperationStatus, 10 Set, 11 Halt.
        assert_eq!(program[3], Instruction::NewTryBlock { catch_offset: 5 });
        assert_eq!(program[6], Instruction::EndTryBlock);
        assert_eq!(program[7], Instruction::Fork { offset: 4 });
        assert!(matches!(program[8], Instruction::GetOperationStatus { .. }));
        assert!(matches!(program[9], Instruction::SetOperationStatus { .. }));
        assert_eq!(program[11], Instruction::Halt);
    }

    #[test]
    fn each_instruction_records_the_line_that_emitted_it() {
        let mut b = Builder::new();
        let line = line!() + 1;
        let x = b.new_int(0);
        let one = b.int(1);
        b.add_assign(x, one);
        let operation = b.finish();
        // The constant first, then the variable, then the add.
        let lines: Vec<u32> = operation
            .emitted()
            .iter()
            .map(|emitted| emitted.location.line())
            .collect();
        assert_eq!(&lines[..3], &[line + 1, line, line + 2]);
        assert!(
            operation
                .location(0)
                .is_some_and(|location| location.file().ends_with("builder.rs"))
        );
    }

    #[test]
    #[should_panic(expected = "break_loop outside a loop")]
    fn break_outside_a_loop_is_a_mistake() {
        let mut b = Builder::new();
        b.break_loop();
    }
}
