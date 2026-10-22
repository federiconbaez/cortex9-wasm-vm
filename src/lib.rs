//! cortex9-wasm-vm: a WebAssembly 1.0 (MVP) interpreter.
//!
//! - [`decode`]: binary decoder with strict LEB128 and section rules.

#![warn(missing_docs)]

pub mod decode;
pub mod error;
pub mod types;

pub use error::{Error, Trap};
pub use types::{FuncType, GlobalType, Limits, ValType, Value};
