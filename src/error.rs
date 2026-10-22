//! Errors.

use std::fmt;

/// Why a module was rejected or an instantiation failed. The message
/// follows the wording of the reference interpreter where one exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Binary decoding failed.
    Malformed(String),
    /// The module decoded but does not validate.
    Invalid(String),
    /// Imports do not match.
    Unlinkable(String),
    /// A trap during instantiation (start function or segment bounds).
    Trap(Trap),
    /// The module needs more than the store allows (memory or table size).
    Limit(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Malformed(m) => write!(f, "malformed: {m}"),
            Error::Invalid(m) => write!(f, "invalid: {m}"),
            Error::Unlinkable(m) => write!(f, "unlinkable: {m}"),
            Error::Trap(t) => write!(f, "trap: {t}"),
            Error::Limit(m) => write!(f, "limit: {m}"),
        }
    }
}

impl std::error::Error for Error {}

/// Runtime traps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trap {
    /// `unreachable` executed.
    Unreachable,
    /// Integer division or remainder by zero.
    DivByZero,
    /// Signed division overflow, or float-to-int out of range.
    IntOverflow,
    /// Float-to-int of a NaN.
    InvalidConversion,
    /// Memory access out of bounds.
    MemoryOob,
    /// `call_indirect` index past the table.
    UndefinedElement,
    /// `call_indirect` on an empty table slot.
    UninitializedElement,
    /// `call_indirect` signature mismatch.
    IndirectTypeMismatch,
    /// Too many nested calls or values.
    StackExhausted,
    /// Element segment does not fit the table (instantiation).
    ElemOob,
    /// Data segment does not fit the memory (instantiation).
    DataOob,
    /// A fuel limit set by the embedder ran out.
    OutOfFuel,
}

impl fmt::Display for Trap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Trap::Unreachable => "unreachable",
            Trap::DivByZero => "integer divide by zero",
            Trap::IntOverflow => "integer overflow",
            Trap::InvalidConversion => "invalid conversion to integer",
            Trap::MemoryOob => "out of bounds memory access",
            Trap::UndefinedElement => "undefined element",
            Trap::UninitializedElement => "uninitialized element",
            Trap::IndirectTypeMismatch => "indirect call type mismatch",
            Trap::StackExhausted => "call stack exhausted",
            Trap::ElemOob => "elements segment does not fit",
            Trap::DataOob => "data segment does not fit",
            Trap::OutOfFuel => "out of fuel",
        })
    }
}

pub(crate) fn malformed<T>(m: impl Into<String>) -> Result<T, Error> {
    Err(Error::Malformed(m.into()))
}
