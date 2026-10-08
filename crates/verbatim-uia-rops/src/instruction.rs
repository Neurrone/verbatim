//! Instructions and their exact encoding.
//!
//! Ported from NVDA's `source/UIAHandler/_remoteOps/instructions/` and
//! `builder.py` (copyright NV Access Limited and contributors, GPL version
//! 2 or later, used here under GPL-3.0-or-later). The parameter layouts of
//! the instructions NVDA does not define (such as the string map, string,
//! point, and rectangle methods), and of the two whose NVDA layout Windows
//! rejects (`RemoteArrayRemoveAt` and `RemoteStringMapRemove` both write a
//! result, which NVDA's definitions omit), follow Microsoft's
//! `RemoteOperationInstructions.h` and `RemoteOperationInstructionSerialization.cpp`
//! from `microsoft-ui-uiautomation`, which carry this notice:
//!
//! Copyright (c) Microsoft Corporation. Licensed under the MIT License.
//!
//! The encoding, verified against Windows 11 26200: a program is the `u32`
//! version 0 followed by its instructions, with no padding. An instruction
//! is its `i32` opcode followed by its parameters in order, little-endian:
//! an operand (a register) is a `u32` id, an offset or enumeration an
//! `i32`, a boolean one byte, a character two bytes, a double eight bytes,
//! and a string a `u32` length counting a terminating null followed by
//! that many UTF-16 code units, the null included. A jump offset counts
//! instructions, relative to the instruction that jumps.

use windows::core::GUID;

use crate::opcode::{Comparison, NavigationDirection, Opcode, PointProperty, RectProperty};

/// A register in the remote program's machine: the id an instruction names
/// it by, which is also the id a result is read back by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OperandId(pub u32);

/// A type test: an instruction that sets a boolean saying whether a
/// register holds a value of one type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)] // Each name is the type it tests for.
pub enum TypeTest {
    Null,
    NotSupported,
    MixedAttribute,
    Bool,
    Int,
    Uint,
    Double,
    Char,
    String,
    Point,
    Rect,
    Array,
    StringMap,
    Element,
    Guid,
    CacheRequest,
}

impl TypeTest {
    /// The opcode of this test's instruction.
    #[must_use]
    pub const fn opcode(self) -> Opcode {
        match self {
            Self::Null => Opcode::IsNull,
            Self::NotSupported => Opcode::IsNotSupported,
            Self::MixedAttribute => Opcode::IsMixedAttribute,
            Self::Bool => Opcode::IsBool,
            Self::Int => Opcode::IsInt,
            Self::Uint => Opcode::IsUint,
            Self::Double => Opcode::IsDouble,
            Self::Char => Opcode::IsChar,
            Self::String => Opcode::IsString,
            Self::Point => Opcode::IsPoint,
            Self::Rect => Opcode::IsRect,
            Self::Array => Opcode::IsArray,
            Self::StringMap => Opcode::IsStringMap,
            Self::Element => Opcode::IsElement,
            Self::Guid => Opcode::IsGuid,
            Self::CacheRequest => Opcode::IsCacheRequest,
        }
    }
}

/// One instruction with its parameters, in the order they are encoded.
///
/// `result` names the register an instruction writes; `target` the object
/// a method runs on (and, for the in-place operations, the register they
/// change). Offsets count instructions from this one.
#[derive(Clone, Debug, PartialEq)]
#[allow(missing_docs)] // Fields are named for the parameters they encode.
pub enum Instruction {
    Nop,
    Set {
        target: OperandId,
        value: OperandId,
    },
    ForkIfTrue {
        condition: OperandId,
        offset: i32,
    },
    ForkIfFalse {
        condition: OperandId,
        offset: i32,
    },
    Fork {
        offset: i32,
    },
    Halt,
    NewLoopBlock {
        break_offset: i32,
        continue_offset: i32,
    },
    EndLoopBlock,
    BreakLoop,
    ContinueLoop,
    NewTryBlock {
        catch_offset: i32,
    },
    EndTryBlock,
    SetOperationStatus {
        status: OperandId,
    },
    GetOperationStatus {
        result: OperandId,
    },
    Add {
        target: OperandId,
        value: OperandId,
    },
    Subtract {
        target: OperandId,
        value: OperandId,
    },
    Multiply {
        target: OperandId,
        value: OperandId,
    },
    Divide {
        target: OperandId,
        value: OperandId,
    },
    BinaryAdd {
        result: OperandId,
        left: OperandId,
        right: OperandId,
    },
    BinarySubtract {
        result: OperandId,
        left: OperandId,
        right: OperandId,
    },
    BinaryMultiply {
        result: OperandId,
        left: OperandId,
        right: OperandId,
    },
    BinaryDivide {
        result: OperandId,
        left: OperandId,
        right: OperandId,
    },
    InPlaceBoolNot {
        target: OperandId,
    },
    InPlaceBoolAnd {
        target: OperandId,
        value: OperandId,
    },
    InPlaceBoolOr {
        target: OperandId,
        value: OperandId,
    },
    BoolNot {
        result: OperandId,
        target: OperandId,
    },
    BoolAnd {
        result: OperandId,
        left: OperandId,
        right: OperandId,
    },
    BoolOr {
        result: OperandId,
        left: OperandId,
        right: OperandId,
    },
    Compare {
        result: OperandId,
        left: OperandId,
        right: OperandId,
        comparison: Comparison,
    },
    NewInt {
        result: OperandId,
        value: i32,
    },
    NewUint {
        result: OperandId,
        value: u32,
    },
    NewBool {
        result: OperandId,
        value: bool,
    },
    NewDouble {
        result: OperandId,
        value: f64,
    },
    NewChar {
        result: OperandId,
        value: u16,
    },
    NewString {
        result: OperandId,
        value: String,
    },
    NewPoint {
        result: OperandId,
        x: f64,
        y: f64,
    },
    NewRect {
        result: OperandId,
        left: f64,
        top: f64,
        width: f64,
        height: f64,
    },
    NewArray {
        result: OperandId,
    },
    NewStringMap {
        result: OperandId,
    },
    NewNull {
        result: OperandId,
    },
    GetPointProperty {
        result: OperandId,
        target: OperandId,
        property: PointProperty,
    },
    GetRectProperty {
        result: OperandId,
        target: OperandId,
        property: RectProperty,
    },
    ArrayAppend {
        target: OperandId,
        value: OperandId,
    },
    ArraySetAt {
        target: OperandId,
        index: OperandId,
        value: OperandId,
    },
    ArrayRemoveAt {
        result: OperandId,
        target: OperandId,
        index: OperandId,
    },
    ArrayGetAt {
        result: OperandId,
        target: OperandId,
        index: OperandId,
    },
    ArraySize {
        result: OperandId,
        target: OperandId,
    },
    StringMapInsert {
        target: OperandId,
        key: OperandId,
        value: OperandId,
    },
    StringMapRemove {
        result: OperandId,
        target: OperandId,
        key: OperandId,
    },
    StringMapHasKey {
        result: OperandId,
        target: OperandId,
        key: OperandId,
    },
    StringMapLookup {
        result: OperandId,
        target: OperandId,
        key: OperandId,
    },
    StringMapSize {
        result: OperandId,
        target: OperandId,
    },
    StringGetAt {
        result: OperandId,
        target: OperandId,
        index: OperandId,
    },
    StringSubstr {
        result: OperandId,
        target: OperandId,
        index: OperandId,
        length: OperandId,
    },
    StringConcat {
        result: OperandId,
        left: OperandId,
        right: OperandId,
    },
    StringSize {
        result: OperandId,
        target: OperandId,
    },
    GetPropertyValue {
        result: OperandId,
        target: OperandId,
        property: OperandId,
        ignore_default: OperandId,
    },
    Navigate {
        result: OperandId,
        target: OperandId,
        direction: OperandId,
    },
    Is {
        test: TypeTest,
        result: OperandId,
        target: OperandId,
    },
    NewGuid {
        result: OperandId,
        value: GUID,
    },
    LookupId {
        result: OperandId,
        guid: OperandId,
        identifier_type: i32,
    },
    LookupGuid {
        result: OperandId,
        id: OperandId,
        identifier_type: i32,
    },
    NewCacheRequest {
        result: OperandId,
    },
    CacheRequestAddProperty {
        target: OperandId,
        property: OperandId,
    },
    CacheRequestAddPattern {
        target: OperandId,
        pattern: OperandId,
    },
    PopulateCache {
        target: OperandId,
        cache_request: OperandId,
    },
    Stringify {
        result: OperandId,
        target: OperandId,
    },
    GetMetadataValue {
        result: OperandId,
        target: OperandId,
        property: OperandId,
        metadata: OperandId,
    },
    CallExtension {
        target: OperandId,
        extension: OperandId,
        arguments: Vec<OperandId>,
    },
    IsExtensionSupported {
        result: OperandId,
        target: OperandId,
        extension: OperandId,
    },
    TextRangeClone {
        result: OperandId,
        target: OperandId,
    },
    GetTextPattern {
        result: OperandId,
        target: OperandId,
    },
    GetTextPattern2 {
        result: OperandId,
        target: OperandId,
    },
    TextPatternGetSelection {
        result: OperandId,
        target: OperandId,
    },
    TextPatternGetVisibleRanges {
        result: OperandId,
        target: OperandId,
    },
    TextPatternGetDocumentRange {
        result: OperandId,
        target: OperandId,
    },
    TextPattern2GetCaretRange {
        result: OperandId,
        target: OperandId,
        is_active: OperandId,
    },
    TextRangeCompare {
        result: OperandId,
        target: OperandId,
        other: OperandId,
    },
    TextRangeCompareEndpoints {
        result: OperandId,
        target: OperandId,
        endpoint: OperandId,
        other: OperandId,
        other_endpoint: OperandId,
    },
    TextRangeExpandToEnclosingUnit {
        target: OperandId,
        unit: OperandId,
    },
    TextRangeFindAttribute {
        result: OperandId,
        target: OperandId,
        attribute: OperandId,
        value: OperandId,
        backward: OperandId,
    },
    TextRangeFindText {
        result: OperandId,
        target: OperandId,
        text: OperandId,
        backward: OperandId,
        ignore_case: OperandId,
    },
    TextRangeGetAttributeValue {
        result: OperandId,
        target: OperandId,
        attribute: OperandId,
    },
    TextRangeGetBoundingRectangles {
        result: OperandId,
        target: OperandId,
    },
    TextRangeGetEnclosingElement {
        result: OperandId,
        target: OperandId,
    },
    TextRangeGetText {
        result: OperandId,
        target: OperandId,
        max_length: OperandId,
    },
    TextRangeMove {
        result: OperandId,
        target: OperandId,
        unit: OperandId,
        count: OperandId,
    },
    TextRangeMoveEndpointByUnit {
        result: OperandId,
        target: OperandId,
        endpoint: OperandId,
        unit: OperandId,
        count: OperandId,
    },
    TextRangeMoveEndpointByRange {
        target: OperandId,
        endpoint: OperandId,
        other: OperandId,
        other_endpoint: OperandId,
    },
    TextRangeSelect {
        target: OperandId,
    },
    TextRangeAddToSelection {
        target: OperandId,
    },
    TextRangeRemoveFromSelection {
        target: OperandId,
    },
    TextRangeScrollIntoView {
        target: OperandId,
        align_to_top: OperandId,
    },
    TextRangeGetChildren {
        result: OperandId,
        target: OperandId,
    },
    TextRangeShowContextMenu {
        target: OperandId,
    },
}

impl Instruction {
    /// The instruction's opcode.
    #[must_use]
    #[allow(clippy::too_many_lines)] // One arm per instruction.
    pub fn opcode(&self) -> Opcode {
        match self {
            Self::Nop => Opcode::Nop,
            Self::Set { .. } => Opcode::Set,
            Self::ForkIfTrue { .. } => Opcode::ForkIfTrue,
            Self::ForkIfFalse { .. } => Opcode::ForkIfFalse,
            Self::Fork { .. } => Opcode::Fork,
            Self::Halt => Opcode::Halt,
            Self::NewLoopBlock { .. } => Opcode::NewLoopBlock,
            Self::EndLoopBlock => Opcode::EndLoopBlock,
            Self::BreakLoop => Opcode::BreakLoop,
            Self::ContinueLoop => Opcode::ContinueLoop,
            Self::NewTryBlock { .. } => Opcode::NewTryBlock,
            Self::EndTryBlock => Opcode::EndTryBlock,
            Self::SetOperationStatus { .. } => Opcode::SetOperationStatus,
            Self::GetOperationStatus { .. } => Opcode::GetOperationStatus,
            Self::Add { .. } => Opcode::Add,
            Self::Subtract { .. } => Opcode::Subtract,
            Self::Multiply { .. } => Opcode::Multiply,
            Self::Divide { .. } => Opcode::Divide,
            Self::BinaryAdd { .. } => Opcode::BinaryAdd,
            Self::BinarySubtract { .. } => Opcode::BinarySubtract,
            Self::BinaryMultiply { .. } => Opcode::BinaryMultiply,
            Self::BinaryDivide { .. } => Opcode::BinaryDivide,
            Self::InPlaceBoolNot { .. } => Opcode::InPlaceBoolNot,
            Self::InPlaceBoolAnd { .. } => Opcode::InPlaceBoolAnd,
            Self::InPlaceBoolOr { .. } => Opcode::InPlaceBoolOr,
            Self::BoolNot { .. } => Opcode::BoolNot,
            Self::BoolAnd { .. } => Opcode::BoolAnd,
            Self::BoolOr { .. } => Opcode::BoolOr,
            Self::Compare { .. } => Opcode::Compare,
            Self::NewInt { .. } => Opcode::NewInt,
            Self::NewUint { .. } => Opcode::NewUint,
            Self::NewBool { .. } => Opcode::NewBool,
            Self::NewDouble { .. } => Opcode::NewDouble,
            Self::NewChar { .. } => Opcode::NewChar,
            Self::NewString { .. } => Opcode::NewString,
            Self::NewPoint { .. } => Opcode::NewPoint,
            Self::NewRect { .. } => Opcode::NewRect,
            Self::NewArray { .. } => Opcode::NewArray,
            Self::NewStringMap { .. } => Opcode::NewStringMap,
            Self::NewNull { .. } => Opcode::NewNull,
            Self::GetPointProperty { .. } => Opcode::GetPointProperty,
            Self::GetRectProperty { .. } => Opcode::GetRectProperty,
            Self::ArrayAppend { .. } => Opcode::RemoteArrayAppend,
            Self::ArraySetAt { .. } => Opcode::RemoteArraySetAt,
            Self::ArrayRemoveAt { .. } => Opcode::RemoteArrayRemoveAt,
            Self::ArrayGetAt { .. } => Opcode::RemoteArrayGetAt,
            Self::ArraySize { .. } => Opcode::RemoteArraySize,
            Self::StringMapInsert { .. } => Opcode::RemoteStringMapInsert,
            Self::StringMapRemove { .. } => Opcode::RemoteStringMapRemove,
            Self::StringMapHasKey { .. } => Opcode::RemoteStringMapHasKey,
            Self::StringMapLookup { .. } => Opcode::RemoteStringMapLookup,
            Self::StringMapSize { .. } => Opcode::RemoteStringMapSize,
            Self::StringGetAt { .. } => Opcode::RemoteStringGetAt,
            Self::StringSubstr { .. } => Opcode::RemoteStringSubstr,
            Self::StringConcat { .. } => Opcode::RemoteStringConcat,
            Self::StringSize { .. } => Opcode::RemoteStringSize,
            Self::GetPropertyValue { .. } => Opcode::GetPropertyValue,
            Self::Navigate { .. } => Opcode::Navigate,
            Self::Is { test, .. } => test.opcode(),
            Self::NewGuid { .. } => Opcode::NewGuid,
            Self::LookupId { .. } => Opcode::LookupId,
            Self::LookupGuid { .. } => Opcode::LookupGuid,
            Self::NewCacheRequest { .. } => Opcode::NewCacheRequest,
            Self::CacheRequestAddProperty { .. } => Opcode::CacheRequestAddProperty,
            Self::CacheRequestAddPattern { .. } => Opcode::CacheRequestAddPattern,
            Self::PopulateCache { .. } => Opcode::PopulateCache,
            Self::Stringify { .. } => Opcode::Stringify,
            Self::GetMetadataValue { .. } => Opcode::GetMetadataValue,
            Self::CallExtension { .. } => Opcode::CallExtension,
            Self::IsExtensionSupported { .. } => Opcode::IsExtensionSupported,
            Self::TextRangeClone { .. } => Opcode::TextRangeClone,
            Self::GetTextPattern { .. } => Opcode::GetTextPattern,
            Self::GetTextPattern2 { .. } => Opcode::GetTextPattern2,
            Self::TextPatternGetSelection { .. } => Opcode::TextPatternGetSelection,
            Self::TextPatternGetVisibleRanges { .. } => Opcode::TextPatternGetVisibleRanges,
            Self::TextPatternGetDocumentRange { .. } => Opcode::TextPatternGetDocumentRange,
            Self::TextPattern2GetCaretRange { .. } => Opcode::TextPattern2GetCaretRange,
            Self::TextRangeCompare { .. } => Opcode::TextRangeCompare,
            Self::TextRangeCompareEndpoints { .. } => Opcode::TextRangeCompareEndpoints,
            Self::TextRangeExpandToEnclosingUnit { .. } => Opcode::TextRangeExpandToEnclosingUnit,
            Self::TextRangeFindAttribute { .. } => Opcode::TextRangeFindAttribute,
            Self::TextRangeFindText { .. } => Opcode::TextRangeFindText,
            Self::TextRangeGetAttributeValue { .. } => Opcode::TextRangeGetAttributeValue,
            Self::TextRangeGetBoundingRectangles { .. } => Opcode::TextRangeGetBoundingRectangles,
            Self::TextRangeGetEnclosingElement { .. } => Opcode::TextRangeGetEnclosingElement,
            Self::TextRangeGetText { .. } => Opcode::TextRangeGetText,
            Self::TextRangeMove { .. } => Opcode::TextRangeMove,
            Self::TextRangeMoveEndpointByUnit { .. } => Opcode::TextRangeMoveEndpointByUnit,
            Self::TextRangeMoveEndpointByRange { .. } => Opcode::TextRangeMoveEndpointByRange,
            Self::TextRangeSelect { .. } => Opcode::TextRangeSelect,
            Self::TextRangeAddToSelection { .. } => Opcode::TextRangeAddToSelection,
            Self::TextRangeRemoveFromSelection { .. } => Opcode::TextRangeRemoveFromSelection,
            Self::TextRangeScrollIntoView { .. } => Opcode::TextRangeScrollIntoView,
            Self::TextRangeGetChildren { .. } => Opcode::TextRangeGetChildren,
            Self::TextRangeShowContextMenu { .. } => Opcode::TextRangeShowContextMenu,
        }
    }

    /// Appends the instruction's bytes, opcode first.
    // One arm per parameter layout, binding the parameters `a` to `e` in
    // the order they are written.
    #[allow(clippy::too_many_lines, clippy::many_single_char_names)]
    pub fn encode(&self, out: &mut Vec<u8>) {
        use Instruction as I;
        let mut w = Writer(out);
        w.int(self.opcode().code());
        match self {
            I::Nop
            | I::Halt
            | I::EndLoopBlock
            | I::BreakLoop
            | I::ContinueLoop
            | I::EndTryBlock => {}
            I::Fork { offset } => w.int(*offset),
            I::NewTryBlock { catch_offset } => w.int(*catch_offset),
            I::NewLoopBlock {
                break_offset,
                continue_offset,
            } => {
                w.int(*break_offset);
                w.int(*continue_offset);
            }
            I::ForkIfTrue { condition, offset } | I::ForkIfFalse { condition, offset } => {
                w.ids(&[*condition]);
                w.int(*offset);
            }
            I::SetOperationStatus { status: a }
            | I::GetOperationStatus { result: a }
            | I::InPlaceBoolNot { target: a }
            | I::NewArray { result: a }
            | I::NewStringMap { result: a }
            | I::NewNull { result: a }
            | I::NewCacheRequest { result: a }
            | I::TextRangeSelect { target: a }
            | I::TextRangeAddToSelection { target: a }
            | I::TextRangeRemoveFromSelection { target: a }
            | I::TextRangeShowContextMenu { target: a } => w.ids(&[*a]),
            I::Set {
                target: a,
                value: b,
            }
            | I::Add {
                target: a,
                value: b,
            }
            | I::Subtract {
                target: a,
                value: b,
            }
            | I::Multiply {
                target: a,
                value: b,
            }
            | I::Divide {
                target: a,
                value: b,
            }
            | I::InPlaceBoolAnd {
                target: a,
                value: b,
            }
            | I::InPlaceBoolOr {
                target: a,
                value: b,
            }
            | I::BoolNot {
                result: a,
                target: b,
            }
            | I::ArrayAppend {
                target: a,
                value: b,
            }
            | I::ArraySize {
                result: a,
                target: b,
            }
            | I::StringMapSize {
                result: a,
                target: b,
            }
            | I::StringSize {
                result: a,
                target: b,
            }
            | I::Is {
                result: a,
                target: b,
                ..
            }
            | I::CacheRequestAddProperty {
                target: a,
                property: b,
            }
            | I::CacheRequestAddPattern {
                target: a,
                pattern: b,
            }
            | I::PopulateCache {
                target: a,
                cache_request: b,
            }
            | I::Stringify {
                result: a,
                target: b,
            }
            | I::TextRangeClone {
                result: a,
                target: b,
            }
            | I::GetTextPattern {
                result: a,
                target: b,
            }
            | I::GetTextPattern2 {
                result: a,
                target: b,
            }
            | I::TextPatternGetSelection {
                result: a,
                target: b,
            }
            | I::TextPatternGetVisibleRanges {
                result: a,
                target: b,
            }
            | I::TextPatternGetDocumentRange {
                result: a,
                target: b,
            }
            | I::TextRangeExpandToEnclosingUnit { target: a, unit: b }
            | I::TextRangeGetBoundingRectangles {
                result: a,
                target: b,
            }
            | I::TextRangeGetEnclosingElement {
                result: a,
                target: b,
            }
            | I::TextRangeScrollIntoView {
                target: a,
                align_to_top: b,
            }
            | I::TextRangeGetChildren {
                result: a,
                target: b,
            } => w.ids(&[*a, *b]),
            I::BinaryAdd {
                result: a,
                left: b,
                right: c,
            }
            | I::BinarySubtract {
                result: a,
                left: b,
                right: c,
            }
            | I::BinaryMultiply {
                result: a,
                left: b,
                right: c,
            }
            | I::BinaryDivide {
                result: a,
                left: b,
                right: c,
            }
            | I::BoolAnd {
                result: a,
                left: b,
                right: c,
            }
            | I::BoolOr {
                result: a,
                left: b,
                right: c,
            }
            | I::ArraySetAt {
                target: a,
                index: b,
                value: c,
            }
            | I::ArrayGetAt {
                result: a,
                target: b,
                index: c,
            }
            | I::ArrayRemoveAt {
                result: a,
                target: b,
                index: c,
            }
            | I::StringMapRemove {
                result: a,
                target: b,
                key: c,
            }
            | I::StringMapInsert {
                target: a,
                key: b,
                value: c,
            }
            | I::StringMapHasKey {
                result: a,
                target: b,
                key: c,
            }
            | I::StringMapLookup {
                result: a,
                target: b,
                key: c,
            }
            | I::StringGetAt {
                result: a,
                target: b,
                index: c,
            }
            | I::StringConcat {
                result: a,
                left: b,
                right: c,
            }
            | I::Navigate {
                result: a,
                target: b,
                direction: c,
            }
            | I::IsExtensionSupported {
                result: a,
                target: b,
                extension: c,
            }
            | I::TextRangeCompare {
                result: a,
                target: b,
                other: c,
            }
            | I::TextRangeGetAttributeValue {
                result: a,
                target: b,
                attribute: c,
            }
            | I::TextRangeGetText {
                result: a,
                target: b,
                max_length: c,
            }
            | I::TextPattern2GetCaretRange {
                result: a,
                target: b,
                is_active: c,
            } => w.ids(&[*a, *b, *c]),
            I::StringSubstr {
                result: a,
                target: b,
                index: c,
                length: d,
            }
            | I::GetPropertyValue {
                result: a,
                target: b,
                property: c,
                ignore_default: d,
            }
            | I::GetMetadataValue {
                result: a,
                target: b,
                property: c,
                metadata: d,
            }
            | I::TextRangeMove {
                result: a,
                target: b,
                unit: c,
                count: d,
            }
            | I::TextRangeMoveEndpointByRange {
                target: a,
                endpoint: b,
                other: c,
                other_endpoint: d,
            } => w.ids(&[*a, *b, *c, *d]),
            I::TextRangeCompareEndpoints {
                result: a,
                target: b,
                endpoint: c,
                other: d,
                other_endpoint: e,
            }
            | I::TextRangeFindAttribute {
                result: a,
                target: b,
                attribute: c,
                value: d,
                backward: e,
            }
            | I::TextRangeFindText {
                result: a,
                target: b,
                text: c,
                backward: d,
                ignore_case: e,
            }
            | I::TextRangeMoveEndpointByUnit {
                result: a,
                target: b,
                endpoint: c,
                unit: d,
                count: e,
            } => w.ids(&[*a, *b, *c, *d, *e]),
            I::Compare {
                result,
                left,
                right,
                comparison,
            } => {
                w.ids(&[*result, *left, *right]);
                w.int(*comparison as i32);
            }
            I::NewInt { result, value } => {
                w.ids(&[*result]);
                w.int(*value);
            }
            I::NewUint { result, value } => {
                w.ids(&[*result]);
                w.bytes(&value.to_le_bytes());
            }
            I::NewBool { result, value } => {
                w.ids(&[*result]);
                w.bytes(&[u8::from(*value)]);
            }
            I::NewDouble { result, value } => {
                w.ids(&[*result]);
                w.bytes(&value.to_le_bytes());
            }
            I::NewChar { result, value } => {
                w.ids(&[*result]);
                w.bytes(&value.to_le_bytes());
            }
            I::NewString { result, value } => {
                w.ids(&[*result]);
                let units: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
                w.bytes(&u32::try_from(units.len()).unwrap_or(u32::MAX).to_le_bytes());
                for unit in units {
                    w.bytes(&unit.to_le_bytes());
                }
            }
            I::NewPoint { result, x, y } => {
                w.ids(&[*result]);
                w.bytes(&x.to_le_bytes());
                w.bytes(&y.to_le_bytes());
            }
            I::NewRect {
                result,
                left,
                top,
                width,
                height,
            } => {
                w.ids(&[*result]);
                for part in [left, top, width, height] {
                    w.bytes(&part.to_le_bytes());
                }
            }
            I::GetPointProperty {
                result,
                target,
                property,
            } => {
                w.ids(&[*result, *target]);
                w.int(*property as i32);
            }
            I::GetRectProperty {
                result,
                target,
                property,
            } => {
                w.ids(&[*result, *target]);
                w.int(*property as i32);
            }
            I::NewGuid { result, value } => {
                w.ids(&[*result]);
                w.bytes(&value.data1.to_le_bytes());
                w.bytes(&value.data2.to_le_bytes());
                w.bytes(&value.data3.to_le_bytes());
                w.bytes(&value.data4);
            }
            I::LookupId {
                result,
                guid: source,
                identifier_type,
            }
            | I::LookupGuid {
                result,
                id: source,
                identifier_type,
            } => {
                w.ids(&[*result, *source]);
                w.int(*identifier_type);
            }
            I::CallExtension {
                target,
                extension,
                arguments,
            } => {
                w.ids(&[*target, *extension]);
                w.bytes(
                    &u32::try_from(arguments.len())
                        .unwrap_or(u32::MAX)
                        .to_le_bytes(),
                );
                w.ids(arguments);
            }
        }
    }
}

/// Appends little-endian values to an instruction's bytes.
struct Writer<'a>(&'a mut Vec<u8>);

impl Writer<'_> {
    fn int(&mut self, value: i32) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    fn ids(&mut self, ids: &[OperandId]) {
        for id in ids {
            self.0.extend_from_slice(&id.0.to_le_bytes());
        }
    }

    fn bytes(&mut self, bytes: &[u8]) {
        self.0.extend_from_slice(bytes);
    }
}

/// Converts a navigation direction to the integer a `Navigate` reads from
/// its direction register.
impl From<NavigationDirection> for i32 {
    fn from(direction: NavigationDirection) -> Self {
        direction as Self
    }
}

#[cfg(test)]
mod tests {
    //! One test of each instruction's exact bytes. The expected bytes are
    //! written out from the layout, not produced by the encoder.

    use std::collections::BTreeSet;

    use windows::core::GUID;

    use super::{Instruction as I, OperandId, TypeTest};
    use crate::opcode::{Comparison, Opcode, PointProperty, RectProperty};

    fn id(value: u32) -> OperandId {
        OperandId(value)
    }

    /// Little-endian 32-bit words, the shape of nearly every instruction.
    fn words(words: &[i64]) -> Vec<u8> {
        words
            .iter()
            .flat_map(|&word| {
                u32::try_from(word & 0xFFFF_FFFF)
                    .expect("a 32-bit word")
                    .to_le_bytes()
            })
            .collect()
    }

    fn encoded(instruction: &I) -> Vec<u8> {
        let mut out = Vec::new();
        instruction.encode(&mut out);
        out
    }

    /// Every instruction with operands 1, 2, 3... and its expected bytes.
    #[allow(clippy::too_many_lines)] // One entry per instruction.
    fn table() -> Vec<(I, Vec<u8>)> {
        let mut table = vec![
            (I::Nop, words(&[0x00])),
            (
                I::Set {
                    target: id(1),
                    value: id(2),
                },
                words(&[0x01, 1, 2]),
            ),
            (
                I::ForkIfTrue {
                    condition: id(1),
                    offset: -3,
                },
                words(&[0x02, 1, -3]),
            ),
            (
                I::ForkIfFalse {
                    condition: id(1),
                    offset: 4,
                },
                words(&[0x03, 1, 4]),
            ),
            (I::Fork { offset: -5 }, words(&[0x04, -5])),
            (I::Halt, words(&[0x05])),
            (
                I::NewLoopBlock {
                    break_offset: 7,
                    continue_offset: 1,
                },
                words(&[0x06, 7, 1]),
            ),
            (I::EndLoopBlock, words(&[0x07])),
            (I::BreakLoop, words(&[0x08])),
            (I::ContinueLoop, words(&[0x09])),
            (I::NewTryBlock { catch_offset: 6 }, words(&[0x0A, 6])),
            (I::EndTryBlock, words(&[0x0B])),
            (I::SetOperationStatus { status: id(1) }, words(&[0x0C, 1])),
            (I::GetOperationStatus { result: id(1) }, words(&[0x0D, 1])),
            (
                I::Add {
                    target: id(1),
                    value: id(2),
                },
                words(&[0x0E, 1, 2]),
            ),
            (
                I::Subtract {
                    target: id(1),
                    value: id(2),
                },
                words(&[0x0F, 1, 2]),
            ),
            (
                I::Multiply {
                    target: id(1),
                    value: id(2),
                },
                words(&[0x10, 1, 2]),
            ),
            (
                I::Divide {
                    target: id(1),
                    value: id(2),
                },
                words(&[0x11, 1, 2]),
            ),
            (
                I::BinaryAdd {
                    result: id(1),
                    left: id(2),
                    right: id(3),
                },
                words(&[0x12, 1, 2, 3]),
            ),
            (
                I::BinarySubtract {
                    result: id(1),
                    left: id(2),
                    right: id(3),
                },
                words(&[0x13, 1, 2, 3]),
            ),
            (
                I::BinaryMultiply {
                    result: id(1),
                    left: id(2),
                    right: id(3),
                },
                words(&[0x14, 1, 2, 3]),
            ),
            (
                I::BinaryDivide {
                    result: id(1),
                    left: id(2),
                    right: id(3),
                },
                words(&[0x15, 1, 2, 3]),
            ),
            (I::InPlaceBoolNot { target: id(1) }, words(&[0x16, 1])),
            (
                I::InPlaceBoolAnd {
                    target: id(1),
                    value: id(2),
                },
                words(&[0x17, 1, 2]),
            ),
            (
                I::InPlaceBoolOr {
                    target: id(1),
                    value: id(2),
                },
                words(&[0x18, 1, 2]),
            ),
            (
                I::BoolNot {
                    result: id(1),
                    target: id(2),
                },
                words(&[0x19, 1, 2]),
            ),
            (
                I::BoolAnd {
                    result: id(1),
                    left: id(2),
                    right: id(3),
                },
                words(&[0x1A, 1, 2, 3]),
            ),
            (
                I::BoolOr {
                    result: id(1),
                    left: id(2),
                    right: id(3),
                },
                words(&[0x1B, 1, 2, 3]),
            ),
            (
                I::Compare {
                    result: id(1),
                    left: id(2),
                    right: id(3),
                    comparison: Comparison::LessThan,
                },
                words(&[0x1C, 1, 2, 3, 3]),
            ),
            (
                I::NewInt {
                    result: id(1),
                    value: -2,
                },
                words(&[0x1D, 1, -2]),
            ),
            (
                I::NewUint {
                    result: id(1),
                    value: 0xFFFF_FFFE,
                },
                words(&[0x1E, 1, 0xFFFF_FFFE]),
            ),
            (I::NewArray { result: id(1) }, words(&[0x25, 1])),
            (I::NewStringMap { result: id(1) }, words(&[0x26, 1])),
            (I::NewNull { result: id(1) }, words(&[0x27, 1])),
            (
                I::GetPointProperty {
                    result: id(1),
                    target: id(2),
                    property: PointProperty::Y,
                },
                words(&[0x28, 1, 2, 1]),
            ),
            (
                I::GetRectProperty {
                    result: id(1),
                    target: id(2),
                    property: RectProperty::Width,
                },
                words(&[0x29, 1, 2, 1]),
            ),
            (
                I::ArrayAppend {
                    target: id(1),
                    value: id(2),
                },
                words(&[0x2A, 1, 2]),
            ),
            (
                I::ArraySetAt {
                    target: id(1),
                    index: id(2),
                    value: id(3),
                },
                words(&[0x2B, 1, 2, 3]),
            ),
            (
                I::ArrayRemoveAt {
                    result: id(1),
                    target: id(2),
                    index: id(3),
                },
                words(&[0x2C, 1, 2, 3]),
            ),
            (
                I::ArrayGetAt {
                    result: id(1),
                    target: id(2),
                    index: id(3),
                },
                words(&[0x2D, 1, 2, 3]),
            ),
            (
                I::ArraySize {
                    result: id(1),
                    target: id(2),
                },
                words(&[0x2E, 1, 2]),
            ),
            (
                I::StringMapInsert {
                    target: id(1),
                    key: id(2),
                    value: id(3),
                },
                words(&[0x2F, 1, 2, 3]),
            ),
            (
                I::StringMapRemove {
                    result: id(1),
                    target: id(2),
                    key: id(3),
                },
                words(&[0x30, 1, 2, 3]),
            ),
            (
                I::StringMapHasKey {
                    result: id(1),
                    target: id(2),
                    key: id(3),
                },
                words(&[0x31, 1, 2, 3]),
            ),
            (
                I::StringMapLookup {
                    result: id(1),
                    target: id(2),
                    key: id(3),
                },
                words(&[0x32, 1, 2, 3]),
            ),
            (
                I::StringMapSize {
                    result: id(1),
                    target: id(2),
                },
                words(&[0x33, 1, 2]),
            ),
            (
                I::StringGetAt {
                    result: id(1),
                    target: id(2),
                    index: id(3),
                },
                words(&[0x34, 1, 2, 3]),
            ),
            (
                I::StringSubstr {
                    result: id(1),
                    target: id(2),
                    index: id(3),
                    length: id(4),
                },
                words(&[0x35, 1, 2, 3, 4]),
            ),
            (
                I::StringConcat {
                    result: id(1),
                    left: id(2),
                    right: id(3),
                },
                words(&[0x36, 1, 2, 3]),
            ),
            (
                I::StringSize {
                    result: id(1),
                    target: id(2),
                },
                words(&[0x37, 1, 2]),
            ),
            (
                I::GetPropertyValue {
                    result: id(1),
                    target: id(2),
                    property: id(3),
                    ignore_default: id(4),
                },
                words(&[0x38, 1, 2, 3, 4]),
            ),
            (
                I::Navigate {
                    result: id(1),
                    target: id(2),
                    direction: id(3),
                },
                words(&[0x39, 1, 2, 3]),
            ),
            (
                I::LookupId {
                    result: id(1),
                    guid: id(2),
                    identifier_type: 3,
                },
                words(&[0x4A, 1, 2, 3]),
            ),
            (
                I::LookupGuid {
                    result: id(1),
                    id: id(2),
                    identifier_type: 0,
                },
                words(&[0x4B, 1, 2, 0]),
            ),
            (I::NewCacheRequest { result: id(1) }, words(&[0x4C, 1])),
            (
                I::CacheRequestAddProperty {
                    target: id(1),
                    property: id(2),
                },
                words(&[0x4E, 1, 2]),
            ),
            (
                I::CacheRequestAddPattern {
                    target: id(1),
                    pattern: id(2),
                },
                words(&[0x4F, 1, 2]),
            ),
            (
                I::PopulateCache {
                    target: id(1),
                    cache_request: id(2),
                },
                words(&[0x50, 1, 2]),
            ),
            (
                I::Stringify {
                    result: id(1),
                    target: id(2),
                },
                words(&[0x51, 1, 2]),
            ),
            (
                I::GetMetadataValue {
                    result: id(1),
                    target: id(2),
                    property: id(3),
                    metadata: id(4),
                },
                words(&[0x52, 1, 2, 3, 4]),
            ),
            (
                I::CallExtension {
                    target: id(1),
                    extension: id(2),
                    arguments: vec![id(3), id(4)],
                },
                words(&[0x53, 1, 2, 2, 3, 4]),
            ),
            (
                I::IsExtensionSupported {
                    result: id(1),
                    target: id(2),
                    extension: id(3),
                },
                words(&[0x54, 1, 2, 3]),
            ),
            (
                I::TextRangeClone {
                    result: id(1),
                    target: id(2),
                },
                words(&[0x271E_0103, 1, 2]),
            ),
            (
                I::TextRangeCompare {
                    result: id(1),
                    target: id(2),
                    other: id(3),
                },
                words(&[0x271E_0104, 1, 2, 3]),
            ),
            (
                I::TextRangeCompareEndpoints {
                    result: id(1),
                    target: id(2),
                    endpoint: id(3),
                    other: id(4),
                    other_endpoint: id(5),
                },
                words(&[0x271E_0105, 1, 2, 3, 4, 5]),
            ),
            (
                I::TextRangeExpandToEnclosingUnit {
                    target: id(1),
                    unit: id(2),
                },
                words(&[0x271E_0106, 1, 2]),
            ),
            (
                I::TextRangeFindAttribute {
                    result: id(1),
                    target: id(2),
                    attribute: id(3),
                    value: id(4),
                    backward: id(5),
                },
                words(&[0x271E_0107, 1, 2, 3, 4, 5]),
            ),
            (
                I::TextRangeFindText {
                    result: id(1),
                    target: id(2),
                    text: id(3),
                    backward: id(4),
                    ignore_case: id(5),
                },
                words(&[0x271E_0108, 1, 2, 3, 4, 5]),
            ),
            (
                I::TextRangeGetAttributeValue {
                    result: id(1),
                    target: id(2),
                    attribute: id(3),
                },
                words(&[0x271E_0109, 1, 2, 3]),
            ),
            (
                I::TextRangeGetBoundingRectangles {
                    result: id(1),
                    target: id(2),
                },
                words(&[0x271E_010A, 1, 2]),
            ),
            (
                I::TextRangeGetEnclosingElement {
                    result: id(1),
                    target: id(2),
                },
                words(&[0x271E_010B, 1, 2]),
            ),
            (
                I::TextRangeGetText {
                    result: id(1),
                    target: id(2),
                    max_length: id(3),
                },
                words(&[0x271E_010C, 1, 2, 3]),
            ),
            (
                I::TextRangeMove {
                    result: id(1),
                    target: id(2),
                    unit: id(3),
                    count: id(4),
                },
                words(&[0x271E_010D, 1, 2, 3, 4]),
            ),
            (
                I::TextRangeMoveEndpointByUnit {
                    result: id(1),
                    target: id(2),
                    endpoint: id(3),
                    unit: id(4),
                    count: id(5),
                },
                words(&[0x271E_010E, 1, 2, 3, 4, 5]),
            ),
            (
                I::TextRangeMoveEndpointByRange {
                    target: id(1),
                    endpoint: id(2),
                    other: id(3),
                    other_endpoint: id(4),
                },
                words(&[0x271E_010F, 1, 2, 3, 4]),
            ),
            (
                I::TextRangeSelect { target: id(1) },
                words(&[0x271E_0110, 1]),
            ),
            (
                I::TextRangeAddToSelection { target: id(1) },
                words(&[0x271E_0111, 1]),
            ),
            (
                I::TextRangeRemoveFromSelection { target: id(1) },
                words(&[0x271E_0112, 1]),
            ),
            (
                I::TextRangeScrollIntoView {
                    target: id(1),
                    align_to_top: id(2),
                },
                words(&[0x271E_0113, 1, 2]),
            ),
            (
                I::TextRangeGetChildren {
                    result: id(1),
                    target: id(2),
                },
                words(&[0x271E_0114, 1, 2]),
            ),
            (
                I::TextRangeShowContextMenu { target: id(1) },
                words(&[0x271E_0115, 1]),
            ),
        ];

        // The instructions with parameters that are not 32-bit words.
        let mut new_bool = words(&[0x1F, 1]);
        new_bool.push(1);
        table.push((
            I::NewBool {
                result: id(1),
                value: true,
            },
            new_bool,
        ));
        let mut new_double = words(&[0x20, 1]);
        new_double.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0xF8, 0x3F]); // 1.5
        table.push((
            I::NewDouble {
                result: id(1),
                value: 1.5,
            },
            new_double,
        ));
        let mut new_char = words(&[0x21, 1]);
        new_char.extend_from_slice(&[0x41, 0]);
        table.push((
            I::NewChar {
                result: id(1),
                value: 0x41,
            },
            new_char,
        ));
        // The length counts the terminating null, which follows the text.
        let mut new_string = words(&[0x22, 1, 3]);
        new_string.extend_from_slice(&[b'h', 0, b'i', 0, 0, 0]);
        table.push((
            I::NewString {
                result: id(1),
                value: "hi".to_owned(),
            },
            new_string,
        ));
        let mut new_point = words(&[0x23, 1]);
        new_point.extend_from_slice(&1.0f64.to_le_bytes());
        new_point.extend_from_slice(&2.0f64.to_le_bytes());
        table.push((
            I::NewPoint {
                result: id(1),
                x: 1.0,
                y: 2.0,
            },
            new_point,
        ));
        let mut new_rect = words(&[0x24, 1]);
        for part in [1.0f64, 2.0, 3.0, 4.0] {
            new_rect.extend_from_slice(&part.to_le_bytes());
        }
        table.push((
            I::NewRect {
                result: id(1),
                left: 1.0,
                top: 2.0,
                width: 3.0,
                height: 4.0,
            },
            new_rect,
        ));
        // A GUID in its in-memory layout: three little-endian fields, then
        // eight bytes in order.
        let mut new_guid = words(&[0x48, 1]);
        new_guid.extend_from_slice(&[
            0x1B, 0x92, 0xA6, 0xC3, 0x99, 0x4A, 0xF1, 0x44, 0xBC, 0xA6, 0x61, 0x18, 0x70, 0x52,
            0xC4, 0x31,
        ]);
        table.push((
            I::NewGuid {
                result: id(1),
                value: GUID::from_u128(0xC3A6_921B_4A99_44F1_BCA6_6118_7052_C431),
            },
            new_guid,
        ));

        // The text patterns.
        for (instruction, opcode) in [
            (
                I::GetTextPattern {
                    result: id(1),
                    target: id(2),
                },
                10014,
            ),
            (
                I::GetTextPattern2 {
                    result: id(1),
                    target: id(2),
                },
                10024,
            ),
            (
                I::TextPatternGetSelection {
                    result: id(1),
                    target: id(2),
                },
                (10014 << 10) | 5,
            ),
            (
                I::TextPatternGetVisibleRanges {
                    result: id(1),
                    target: id(2),
                },
                (10014 << 10) | 6,
            ),
            (
                I::TextPatternGetDocumentRange {
                    result: id(1),
                    target: id(2),
                },
                (10014 << 10) | 7,
            ),
        ] {
            table.push((instruction, words(&[opcode, 1, 2])));
        }
        table.push((
            I::TextPattern2GetCaretRange {
                result: id(1),
                target: id(2),
                is_active: id(3),
            },
            words(&[(10024 << 10) | 0xA, 1, 2, 3]),
        ));

        // The type tests share one layout.
        for (test, opcode) in [
            (TypeTest::Null, 0x3A),
            (TypeTest::NotSupported, 0x3B),
            (TypeTest::MixedAttribute, 0x3C),
            (TypeTest::Bool, 0x3D),
            (TypeTest::Int, 0x3E),
            (TypeTest::Uint, 0x3F),
            (TypeTest::Double, 0x40),
            (TypeTest::Char, 0x41),
            (TypeTest::String, 0x42),
            (TypeTest::Point, 0x43),
            (TypeTest::Rect, 0x44),
            (TypeTest::Array, 0x45),
            (TypeTest::StringMap, 0x46),
            (TypeTest::Element, 0x47),
            (TypeTest::Guid, 0x49),
            (TypeTest::CacheRequest, 0x4D),
        ] {
            table.push((
                I::Is {
                    test,
                    result: id(1),
                    target: id(2),
                },
                words(&[opcode, 1, 2]),
            ));
        }
        table
    }

    #[test]
    fn each_instruction_encodes_to_its_exact_bytes() {
        for (instruction, expected) in table() {
            assert_eq!(encoded(&instruction), expected, "{instruction:?}");
        }
    }

    #[test]
    fn the_tests_cover_every_opcode() {
        let covered: BTreeSet<Opcode> = table()
            .iter()
            .map(|(instruction, _)| instruction.opcode())
            .collect();
        let all: BTreeSet<Opcode> = Opcode::ALL.into_iter().collect();
        assert_eq!(covered, all);
    }
}
