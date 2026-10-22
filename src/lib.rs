//! cortex9-wasm-vm: a WebAssembly 1.0 (MVP) interpreter.
//!
//! - [`decode`]: binary decoder with strict LEB128 and section rules.
//! - [`validate`]: type checking fused with compilation to a flat
//!   instruction array with resolved branch targets.
//! - [`exec`]: store, linking, instantiation and the interpreter loop.
//! - [`num`]: numeric instruction semantics, including float corner cases.
//! - [`fuzzing`]: the never-panic property shared by fuzz targets and tests.

#![warn(missing_docs)]

pub mod decode;
pub mod error;
pub mod exec;
pub mod fuzzing;
pub mod num;
pub mod types;
pub mod validate;

pub use error::{Error, Trap};
pub use exec::{Extern, HostFunc, Imports, Store};
pub use types::{FuncType, GlobalType, Limits, ValType, Value};
pub use validate::Module;
