//! Core WebAssembly 1.0 types.

use std::fmt;

/// Value types of the MVP.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ValType {
    /// 32-bit integer.
    I32,
    /// 64-bit integer.
    I64,
    /// 32-bit IEEE float.
    F32,
    /// 64-bit IEEE float.
    F64,
}

impl ValType {
    /// From the binary encoding.
    pub fn from_byte(b: u8) -> Option<ValType> {
        match b {
            0x7f => Some(ValType::I32),
            0x7e => Some(ValType::I64),
            0x7d => Some(ValType::F32),
            0x7c => Some(ValType::F64),
            _ => None,
        }
    }
}

/// Function signature. The MVP allows at most one result.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct FuncType {
    /// Parameter types.
    pub params: Vec<ValType>,
    /// Result types.
    pub results: Vec<ValType>,
}

/// Size limits of a table or memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Minimum (initial) size.
    pub min: u32,
    /// Optional maximum.
    pub max: Option<u32>,
}

impl Limits {
    /// Import matching: `self` (what is provided) fits `want`.
    pub fn matches(&self, want: &Limits) -> bool {
        self.min >= want.min
            && match (self.max, want.max) {
                (_, None) => true,
                (Some(a), Some(b)) => a <= b,
                (None, Some(_)) => false,
            }
    }
}

/// Type of a global.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlobalType {
    /// Value type.
    pub ty: ValType,
    /// Whether `global.set` is allowed.
    pub mutable: bool,
}

/// A runtime value. Floats are kept as raw bits so NaN payloads survive.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Value {
    /// `i32`.
    I32(i32),
    /// `i64`.
    I64(i64),
    /// `f32` bits.
    F32(u32),
    /// `f64` bits.
    F64(u64),
}

impl Value {
    /// Its type.
    pub fn ty(&self) -> ValType {
        match self {
            Value::I32(_) => ValType::I32,
            Value::I64(_) => ValType::I64,
            Value::F32(_) => ValType::F32,
            Value::F64(_) => ValType::F64,
        }
    }

    /// Raw 64-bit slot representation used by the interpreter.
    pub fn to_bits(self) -> u64 {
        match self {
            Value::I32(v) => v as u32 as u64,
            Value::I64(v) => v as u64,
            Value::F32(v) => v as u64,
            Value::F64(v) => v,
        }
    }

    /// Inverse of [`Value::to_bits`].
    pub fn from_bits(ty: ValType, b: u64) -> Value {
        match ty {
            ValType::I32 => Value::I32(b as u32 as i32),
            ValType::I64 => Value::I64(b as i64),
            ValType::F32 => Value::F32(b as u32),
            ValType::F64 => Value::F64(b),
        }
    }

    /// Zero of a type.
    pub fn zero(ty: ValType) -> Value {
        Value::from_bits(ty, 0)
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::I32(v) => write!(f, "i32:{v}"),
            Value::I64(v) => write!(f, "i64:{v}"),
            Value::F32(v) => write!(f, "f32:{}({v:#x})", f32::from_bits(*v)),
            Value::F64(v) => write!(f, "f64:{}({v:#x})", f64::from_bits(*v)),
        }
    }
}
