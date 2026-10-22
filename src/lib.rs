//! cortex9-wasm-vm: a WebAssembly 1.0 (MVP) interpreter.
//!
//! - [`decode`]: binary decoder with strict LEB128 and section rules.
//! - [`validate`]: type checking fused with compilation to a flat
//!   instruction array with resolved branch targets.
//! - [`num`]: numeric instruction semantics, including float corner cases.

#![warn(missing_docs)]

pub mod decode;
pub mod error;
pub mod num;
pub mod types;
pub mod validate;

pub use error::{Error, Trap};
pub use types::{FuncType, GlobalType, Limits, ValType, Value};
pub use validate::Module;
