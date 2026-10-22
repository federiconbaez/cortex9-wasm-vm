//! Arbitrary bytes through decode → validate → instantiate → call every
//! export with fuel. Any panic, overflow or out-of-memory is a bug.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = cortex9_wasm_vm::fuzzing::run(data);
});
