//! Shared body of the fuzz targets and the robustness tests: decode and
//! validate arbitrary bytes and, if they form a valid module, instantiate
//! it with stand-in imports and call every export under a fuel limit.
//! Nothing here may panic, whatever the input.

use crate::exec::Store;
use crate::types::Value;
use crate::validate::Module;

/// What happened to one input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Outcome {
    /// The bytes decoded and validated.
    pub valid: bool,
    /// Instantiation succeeded.
    pub instantiated: bool,
    /// Exported functions called.
    pub calls: u32,
    /// Calls that trapped (including running out of fuel).
    pub traps: u32,
}

/// Run one input.
pub fn run(data: &[u8]) -> Outcome {
    let mut out = Outcome::default();
    let Ok(m) = Module::new(data) else {
        return out;
    };
    out.valid = true;
    let mut store = Store::new();
    store.max_pages = 16;
    store.max_table = 1 << 16;
    store.max_frames = 500;
    store.fuel = Some(20_000);
    let imports = store.stub_imports(&m);
    let Ok(inst) = store.instantiate(&m, &imports) else {
        return out;
    };
    out.instantiated = true;
    for (_, f) in store.exported_funcs(inst) {
        let args: Vec<Value> = store
            .func_type(f)
            .params
            .iter()
            .map(|t| Value::zero(*t))
            .collect();
        store.fuel = Some(20_000);
        out.calls += 1;
        if store.invoke(f, &args).is_err() {
            out.traps += 1;
        }
    }
    out
}
