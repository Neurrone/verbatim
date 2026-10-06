//! The remote operations opcode table and the small enumerations some
//! instructions carry as literals.
//!
//! Ported from NVDA's `source/UIAHandler/_remoteOps/lowLevel.py`
//! (copyright NV Access Limited and contributors, GPL version 2 or later,
//! used here under GPL-3.0-or-later). The same values appear in Microsoft's
//! `RemoteOperationInstructions.h` from `microsoft-ui-uiautomation`
//! (copyright Microsoft Corporation, MIT License), which also gives the
//! formula for pattern-method opcodes used for the text range methods.

/// One instruction's opcode: the little-endian `i32` that starts every
/// instruction in the bytecode.
///
/// The table is NVDA's: every general opcode Windows defines (`0x00` to
/// `0x54`) plus the text range methods. A text range method's opcode is
/// `(patternId << 16) | (relatedObject << 8) | vtableIndex`, with the Text
/// pattern's id (10014, `0x271E`), related object 1 (the text range), and
/// the method's index in `IUIAutomationTextRange`'s vtable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(i32)]
#[allow(missing_docs)] // Each name is the instruction it starts.
pub enum Opcode {
    Nop = 0x00,
    Set = 0x01,

    // Control flow.
    ForkIfTrue = 0x02,
    ForkIfFalse = 0x03,
    Fork = 0x04,
    Halt = 0x05,

    // Loops.
    NewLoopBlock = 0x06,
    EndLoopBlock = 0x07,
    BreakLoop = 0x08,
    ContinueLoop = 0x09,

    // Error handling.
    NewTryBlock = 0x0A,
    EndTryBlock = 0x0B,
    SetOperationStatus = 0x0C,
    GetOperationStatus = 0x0D,

    // Arithmetic.
    Add = 0x0E,
    Subtract = 0x0F,
    Multiply = 0x10,
    Divide = 0x11,
    BinaryAdd = 0x12,
    BinarySubtract = 0x13,
    BinaryMultiply = 0x14,
    BinaryDivide = 0x15,

    // Boolean operators.
    InPlaceBoolNot = 0x16,
    InPlaceBoolAnd = 0x17,
    InPlaceBoolOr = 0x18,
    BoolNot = 0x19,
    BoolAnd = 0x1A,
    BoolOr = 0x1B,

    // Generic comparison.
    Compare = 0x1C,

    // Constructors.
    NewInt = 0x1D,
    NewUint = 0x1E,
    NewBool = 0x1F,
    NewDouble = 0x20,
    NewChar = 0x21,
    NewString = 0x22,
    NewPoint = 0x23,
    NewRect = 0x24,
    NewArray = 0x25,
    NewStringMap = 0x26,
    NewNull = 0x27,

    // Point and rectangle methods.
    GetPointProperty = 0x28,
    GetRectProperty = 0x29,

    // Array methods.
    RemoteArrayAppend = 0x2A,
    RemoteArraySetAt = 0x2B,
    RemoteArrayRemoveAt = 0x2C,
    RemoteArrayGetAt = 0x2D,
    RemoteArraySize = 0x2E,

    // String map methods.
    RemoteStringMapInsert = 0x2F,
    RemoteStringMapRemove = 0x30,
    RemoteStringMapHasKey = 0x31,
    RemoteStringMapLookup = 0x32,
    RemoteStringMapSize = 0x33,

    // String methods.
    RemoteStringGetAt = 0x34,
    RemoteStringSubstr = 0x35,
    RemoteStringConcat = 0x36,
    RemoteStringSize = 0x37,

    // Element methods.
    GetPropertyValue = 0x38,
    Navigate = 0x39,

    // Type tests.
    IsNull = 0x3A,
    IsNotSupported = 0x3B,
    IsMixedAttribute = 0x3C,
    IsBool = 0x3D,
    IsInt = 0x3E,
    IsUint = 0x3F,
    IsDouble = 0x40,
    IsChar = 0x41,
    IsString = 0x42,
    IsPoint = 0x43,
    IsRect = 0x44,
    IsArray = 0x45,
    IsStringMap = 0x46,
    IsElement = 0x47,

    // GUIDs.
    NewGuid = 0x48,
    IsGuid = 0x49,
    LookupId = 0x4A,
    LookupGuid = 0x4B,

    // Cache requests.
    NewCacheRequest = 0x4C,
    IsCacheRequest = 0x4D,
    CacheRequestAddProperty = 0x4E,
    CacheRequestAddPattern = 0x4F,
    PopulateCache = 0x50,

    Stringify = 0x51,
    GetMetadataValue = 0x52,

    // Extensions.
    CallExtension = 0x53,
    IsExtensionSupported = 0x54,

    // Text range methods.
    TextRangeClone = 0x271E_0103,
    TextRangeCompare = 0x271E_0104,
    TextRangeCompareEndpoints = 0x271E_0105,
    TextRangeExpandToEnclosingUnit = 0x271E_0106,
    TextRangeFindAttribute = 0x271E_0107,
    TextRangeFindText = 0x271E_0108,
    TextRangeGetAttributeValue = 0x271E_0109,
    TextRangeGetBoundingRectangles = 0x271E_010A,
    TextRangeGetEnclosingElement = 0x271E_010B,
    TextRangeGetText = 0x271E_010C,
    TextRangeMove = 0x271E_010D,
    TextRangeMoveEndpointByUnit = 0x271E_010E,
    TextRangeMoveEndpointByRange = 0x271E_010F,
    TextRangeSelect = 0x271E_0110,
    TextRangeAddToSelection = 0x271E_0111,
    TextRangeRemoveFromSelection = 0x271E_0112,
    TextRangeScrollIntoView = 0x271E_0113,
    TextRangeGetChildren = 0x271E_0114,
    TextRangeShowContextMenu = 0x271E_0115,
}

impl Opcode {
    /// Every opcode in the table.
    pub const ALL: [Self; 104] = [
        Self::Nop,
        Self::Set,
        Self::ForkIfTrue,
        Self::ForkIfFalse,
        Self::Fork,
        Self::Halt,
        Self::NewLoopBlock,
        Self::EndLoopBlock,
        Self::BreakLoop,
        Self::ContinueLoop,
        Self::NewTryBlock,
        Self::EndTryBlock,
        Self::SetOperationStatus,
        Self::GetOperationStatus,
        Self::Add,
        Self::Subtract,
        Self::Multiply,
        Self::Divide,
        Self::BinaryAdd,
        Self::BinarySubtract,
        Self::BinaryMultiply,
        Self::BinaryDivide,
        Self::InPlaceBoolNot,
        Self::InPlaceBoolAnd,
        Self::InPlaceBoolOr,
        Self::BoolNot,
        Self::BoolAnd,
        Self::BoolOr,
        Self::Compare,
        Self::NewInt,
        Self::NewUint,
        Self::NewBool,
        Self::NewDouble,
        Self::NewChar,
        Self::NewString,
        Self::NewPoint,
        Self::NewRect,
        Self::NewArray,
        Self::NewStringMap,
        Self::NewNull,
        Self::GetPointProperty,
        Self::GetRectProperty,
        Self::RemoteArrayAppend,
        Self::RemoteArraySetAt,
        Self::RemoteArrayRemoveAt,
        Self::RemoteArrayGetAt,
        Self::RemoteArraySize,
        Self::RemoteStringMapInsert,
        Self::RemoteStringMapRemove,
        Self::RemoteStringMapHasKey,
        Self::RemoteStringMapLookup,
        Self::RemoteStringMapSize,
        Self::RemoteStringGetAt,
        Self::RemoteStringSubstr,
        Self::RemoteStringConcat,
        Self::RemoteStringSize,
        Self::GetPropertyValue,
        Self::Navigate,
        Self::IsNull,
        Self::IsNotSupported,
        Self::IsMixedAttribute,
        Self::IsBool,
        Self::IsInt,
        Self::IsUint,
        Self::IsDouble,
        Self::IsChar,
        Self::IsString,
        Self::IsPoint,
        Self::IsRect,
        Self::IsArray,
        Self::IsStringMap,
        Self::IsElement,
        Self::NewGuid,
        Self::IsGuid,
        Self::LookupId,
        Self::LookupGuid,
        Self::NewCacheRequest,
        Self::IsCacheRequest,
        Self::CacheRequestAddProperty,
        Self::CacheRequestAddPattern,
        Self::PopulateCache,
        Self::Stringify,
        Self::GetMetadataValue,
        Self::CallExtension,
        Self::IsExtensionSupported,
        Self::TextRangeClone,
        Self::TextRangeCompare,
        Self::TextRangeCompareEndpoints,
        Self::TextRangeExpandToEnclosingUnit,
        Self::TextRangeFindAttribute,
        Self::TextRangeFindText,
        Self::TextRangeGetAttributeValue,
        Self::TextRangeGetBoundingRectangles,
        Self::TextRangeGetEnclosingElement,
        Self::TextRangeGetText,
        Self::TextRangeMove,
        Self::TextRangeMoveEndpointByUnit,
        Self::TextRangeMoveEndpointByRange,
        Self::TextRangeSelect,
        Self::TextRangeAddToSelection,
        Self::TextRangeRemoveFromSelection,
        Self::TextRangeScrollIntoView,
        Self::TextRangeGetChildren,
        Self::TextRangeShowContextMenu,
    ];

    /// The opcode's value as it is written into the bytecode.
    #[must_use]
    pub const fn code(self) -> i32 {
        self as i32
    }
}

/// The opcode of a method on an object a pattern returns, such as a text
/// range method: `(patternId << 16) | (relatedObject << 8) | vtableIndex`.
#[must_use]
pub const fn pattern_related_object_method(
    pattern_id: i32,
    related_object: i32,
    vtable_index: i32,
) -> i32 {
    (pattern_id << 16) | (related_object << 8) | vtable_index
}

/// How [`Compare`](crate::Instruction::Compare) compares its operands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
#[allow(missing_docs)] // Each name is the comparison it makes.
pub enum Comparison {
    Equal = 0,
    NotEqual = 1,
    GreaterThan = 2,
    LessThan = 3,
    GreaterThanOrEqual = 4,
    LessThanOrEqual = 5,
}

/// The direction of a [`Navigate`](crate::Instruction::Navigate), UIA's
/// `NavigateDirection`. Navigation runs over the raw view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
#[allow(missing_docs)] // Each name is the direction it goes.
pub enum NavigationDirection {
    Parent = 0,
    NextSibling = 1,
    PreviousSibling = 2,
    FirstChild = 3,
    LastChild = 4,
}

/// Which coordinate [`GetPointProperty`](crate::Instruction::GetPointProperty)
/// reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
#[allow(missing_docs)]
pub enum PointProperty {
    X = 0,
    Y = 1,
}

/// Which part [`GetRectProperty`](crate::Instruction::GetRectProperty) reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
#[allow(missing_docs)]
pub enum RectProperty {
    Height = 0,
    Width = 1,
    X = 2,
    Y = 3,
}

/// How a run ended, from the result's `Status`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// The program ran to its end or to a `Halt`.
    Success,
    /// The bytecode could not be parsed.
    MalformedBytecode,
    /// The program ran more instructions than the platform allows.
    InstructionLimitExceeded,
    /// An instruction failed outside any try block.
    UnhandledException,
    /// The run could not happen, for example because the provider is gone.
    ExecutionFailure,
    /// A status this table does not know.
    Other(i32),
}

impl Status {
    /// The status for the platform's value.
    #[must_use]
    pub const fn from_value(value: i32) -> Self {
        match value {
            0 => Self::Success,
            1 => Self::MalformedBytecode,
            2 => Self::InstructionLimitExceeded,
            3 => Self::UnhandledException,
            4 => Self::ExecutionFailure,
            other => Self::Other(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Opcode, pattern_related_object_method};

    #[test]
    fn text_range_opcodes_follow_the_pattern_method_formula() {
        // The Text pattern's id, related object 1, vtable indices 3 to 21.
        let text_pattern = 10014;
        assert_eq!(
            Opcode::TextRangeClone.code(),
            pattern_related_object_method(text_pattern, 1, 3)
        );
        assert_eq!(
            Opcode::TextRangeShowContextMenu.code(),
            pattern_related_object_method(text_pattern, 1, 21)
        );
    }
}
