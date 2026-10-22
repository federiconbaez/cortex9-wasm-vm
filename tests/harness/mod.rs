//! Runs `.wast` scripts from the official spec test suite. The `wast`
//! crate parses the text format and encodes modules to binary; decoding,
//! validation, linking and execution are this crate's.

#![allow(dead_code)]

use cortex9_wasm_vm::exec::Extern;
use cortex9_wasm_vm::{Error, FuncType, Imports, Limits, Module, Store, Trap, ValType, Value};
use std::collections::HashMap;
use std::rc::Rc;
use wast::core::{WastArgCore, WastRetCore};
use wast::parser::{self, ParseBuffer};
use wast::{QuoteWat, Wast, WastArg, WastDirective, WastExecute, WastInvoke, WastRet, Wat};

#[derive(Debug, Default, Clone)]
pub struct Report {
    pub passed: usize,
    pub failed: Vec<String>,
    /// `assert_malformed` on text modules: about the text parser, which is
    /// not part of this crate.
    pub skipped_text: usize,
    /// Rejections (`assert_invalid`, binary `assert_malformed`) whose
    /// error message also starts with the expected text.
    pub same_message: usize,
    /// All rejections checked.
    pub rejections: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Legacy {
    Canonical,
    Arithmetic,
}

/// The 1.0 suite uses `assert_return_canonical_nan` and
/// `assert_return_arithmetic_nan`, which current parsers dropped in
/// favour of `nan:canonical` / `nan:arithmetic` results. Rewrite them in
/// place (same length, so offsets keep matching) and remember where.
fn preprocess(text: &str) -> (String, HashMap<usize, Legacy>) {
    let mut out = text.to_string();
    let mut marks = HashMap::new();
    for (kw, kind) in [
        ("(assert_return_canonical_nan", Legacy::Canonical),
        ("(assert_return_arithmetic_nan", Legacy::Arithmetic),
    ] {
        let repl = format!(
            "(assert_return{}",
            " ".repeat(kw.len() - "(assert_return".len())
        );
        let mut from = 0;
        while let Some(i) = out[from..].find(kw) {
            let at = from + i;
            out.replace_range(at..at + kw.len(), &repl);
            marks.insert(at + 1, kind);
            from = at + kw.len();
        }
    }
    (out, marks)
}

struct Ctx {
    store: Store,
    registered: HashMap<String, HashMap<String, Extern>>,
    named: HashMap<String, usize>,
    last: Option<usize>,
    instance_exports: Vec<HashMap<String, Extern>>,
}

fn spectest(store: &mut Store) -> HashMap<String, Extern> {
    use ValType::*;
    let mut m = HashMap::new();
    let mut host = |name: &str, params: Vec<ValType>| {
        let ext = store.host_func(
            FuncType {
                params,
                results: vec![],
            },
            Rc::new(|_args: &[Value]| Ok(vec![])),
        );
        (name.to_string(), ext)
    };
    for (n, p) in [
        ("print", vec![]),
        ("print_i32", vec![I32]),
        ("print_i64", vec![I64]),
        ("print_f32", vec![F32]),
        ("print_f64", vec![F64]),
        ("print_i32_f32", vec![I32, F32]),
        ("print_f64_f64", vec![F64, F64]),
    ] {
        let (k, v) = host(n, p);
        m.insert(k, v);
    }
    m.insert(
        "global_i32".into(),
        store.alloc_global(Value::I32(666), false),
    );
    m.insert(
        "global_i64".into(),
        store.alloc_global(Value::I64(666), false),
    );
    m.insert(
        "global_f32".into(),
        store.alloc_global(Value::F32(666.6f32.to_bits()), false),
    );
    m.insert(
        "global_f64".into(),
        store.alloc_global(Value::F64(666.6f64.to_bits()), false),
    );
    m.insert(
        "table".into(),
        store.alloc_table(Limits {
            min: 10,
            max: Some(20),
        }),
    );
    m.insert(
        "memory".into(),
        store.alloc_memory(Limits {
            min: 1,
            max: Some(2),
        }),
    );
    m
}

impl Ctx {
    fn new() -> Self {
        let mut store = Store::new();
        let st = spectest(&mut store);
        let mut registered = HashMap::new();
        registered.insert("spectest".to_string(), st);
        Ctx {
            store,
            registered,
            named: HashMap::new(),
            last: None,
            instance_exports: Vec::new(),
        }
    }

    fn imports(&self, m: &Module) -> Imports {
        let mut out = Imports::new();
        for imp in &m.raw.imports {
            if let Some(e) = self
                .registered
                .get(&imp.module)
                .and_then(|x| x.get(&imp.name))
            {
                out.insert((imp.module.clone(), imp.name.clone()), *e);
            }
        }
        out
    }

    fn instantiate(&mut self, bin: &[u8]) -> Result<usize, Error> {
        let m = Module::new(bin)?;
        let imports = self.imports(&m);
        let id = self.store.instantiate(&m, &imports)?;
        let exports: HashMap<String, Extern> = m
            .raw
            .exports
            .iter()
            .map(|e| {
                (
                    e.name.clone(),
                    self.store.export(id, &e.name).expect("export"),
                )
            })
            .collect();
        while self.instance_exports.len() <= id {
            self.instance_exports.push(HashMap::new());
        }
        self.instance_exports[id] = exports;
        Ok(id)
    }

    fn instance(&self, id: Option<wast::token::Id>) -> Option<usize> {
        match id {
            Some(i) => self.named.get(i.name()).copied(),
            None => self.last,
        }
    }

    fn invoke(&mut self, inv: &WastInvoke) -> Result<Result<Vec<Value>, Trap>, String> {
        let inst = self.instance(inv.module).ok_or("no module")?;
        let args: Vec<Value> = inv
            .args
            .iter()
            .map(|a| match a {
                WastArg::Core(WastArgCore::I32(v)) => Ok(Value::I32(*v)),
                WastArg::Core(WastArgCore::I64(v)) => Ok(Value::I64(*v)),
                WastArg::Core(WastArgCore::F32(v)) => Ok(Value::F32(v.bits)),
                WastArg::Core(WastArgCore::F64(v)) => Ok(Value::F64(v.bits)),
                _ => Err("unsupported argument".to_string()),
            })
            .collect::<Result<_, _>>()?;
        match self.store.export(inst, inv.name) {
            Some(Extern::Func(f)) => Ok(self.store.invoke(f, &args)),
            _ => Err(format!("no exported function {:?}", inv.name)),
        }
    }

    fn exec(&mut self, e: &mut WastExecute) -> Result<Result<Vec<Value>, Trap>, String> {
        match e {
            WastExecute::Invoke(inv) => self.invoke(inv),
            WastExecute::Get { module, global, .. } => {
                let inst = self.instance(*module).ok_or("no module")?;
                match self.store.export(inst, global) {
                    Some(Extern::Global(g)) => Ok(Ok(vec![self.store.global(g)])),
                    _ => Err(format!("no exported global {global:?}")),
                }
            }
            WastExecute::Wat(w) => {
                let bin = w.encode().map_err(|e| e.to_string())?;
                match self.instantiate(&bin) {
                    Ok(_) => Ok(Ok(vec![])),
                    Err(Error::Trap(t)) => Ok(Err(t)),
                    Err(e) => Err(format!("instantiation failed: {e}")),
                }
            }
        }
    }
}

fn is_canonical(v: &Value) -> bool {
    match v {
        Value::F32(b) => b & 0x7fff_ffff == 0x7fc0_0000,
        Value::F64(b) => b & 0x7fff_ffff_ffff_ffff == 0x7ff8_0000_0000_0000,
        _ => false,
    }
}

fn is_arithmetic(v: &Value) -> bool {
    match v {
        Value::F32(b) => b & 0x7fc0_0000 == 0x7fc0_0000,
        Value::F64(b) => b & 0x7ff8_0000_0000_0000 == 0x7ff8_0000_0000_0000,
        _ => false,
    }
}

fn matches(got: &Value, want: &WastRet) -> bool {
    use wast::core::NanPattern;
    match (got, want) {
        (Value::I32(a), WastRet::Core(WastRetCore::I32(b))) => a == b,
        (Value::I64(a), WastRet::Core(WastRetCore::I64(b))) => a == b,
        (Value::F32(a), WastRet::Core(WastRetCore::F32(p))) => match p {
            NanPattern::Value(b) => *a == b.bits,
            NanPattern::CanonicalNan => is_canonical(got),
            NanPattern::ArithmeticNan => is_arithmetic(got),
        },
        (Value::F64(a), WastRet::Core(WastRetCore::F64(p))) => match p {
            NanPattern::Value(b) => *a == b.bits,
            NanPattern::CanonicalNan => is_canonical(got),
            NanPattern::ArithmeticNan => is_arithmetic(got),
        },
        _ => false,
    }
}

fn trap_matches(t: &Trap, msg: &str) -> bool {
    let ours = t.to_string();
    msg.starts_with(&ours) || ours.starts_with(msg)
}

fn same_message(e: &Error, want: &str) -> bool {
    let got = match e {
        Error::Malformed(m) | Error::Invalid(m) | Error::Unlinkable(m) => m.clone(),
        Error::Trap(t) => t.to_string(),
        Error::Limit(m) => m.clone(),
    };
    let same = got.starts_with(want) || want.starts_with(&got);
    if !same && std::env::var_os("WASM_VM_MSGS").is_some() {
        eprintln!("MSG want={want:?} got={got:?}");
    }
    same
}

fn module_id(w: &QuoteWat) -> Option<String> {
    match w {
        QuoteWat::Wat(Wat::Module(m)) => m.id.map(|i| i.name().to_string()),
        _ => None,
    }
}

/// Run one script. Every directive counts as passed or failed, except
/// text-format `assert_malformed`, which is skipped.
pub fn run(name: &str, text: &str) -> Report {
    let (text, legacy) = preprocess(text);
    let mut rep = Report::default();
    // names.wast deliberately uses bidi control characters in names.
    let mut lexer = wast::lexer::Lexer::new(&text);
    lexer.allow_confusing_unicode(true);
    let buf = match ParseBuffer::new_with_lexer(lexer) {
        Ok(b) => b,
        Err(e) => {
            rep.failed.push(format!("{name}: lex error {e}"));
            return rep;
        }
    };
    let wast: Wast = match parser::parse(&buf) {
        Ok(w) => w,
        Err(e) => {
            rep.failed.push(format!("{name}: parse error {e}"));
            return rep;
        }
    };
    let mut cx = Ctx::new();
    for mut d in wast.directives {
        let span = d.span();
        let (line, _) = span.linecol_in(&text);
        let at = format!("{name}:{}", line + 1);
        let result: Result<(), String> = match &mut d {
            WastDirective::Wat(w) => {
                let id = module_id(w);
                match w.encode() {
                    Err(e) => Err(format!("encode: {e}")),
                    Ok(bin) => match cx.instantiate(&bin) {
                        Ok(i) => {
                            cx.last = Some(i);
                            if let Some(id) = id {
                                cx.named.insert(id, i);
                            }
                            Ok(())
                        }
                        Err(e) => Err(format!("module rejected: {e}")),
                    },
                }
            }
            WastDirective::AssertMalformed {
                module, message, ..
            } => match module {
                QuoteWat::QuoteModule(..) => {
                    rep.skipped_text += 1;
                    continue;
                }
                _ => match module.encode() {
                    Err(_) => {
                        rep.skipped_text += 1;
                        continue;
                    }
                    Ok(bin) => match Module::new(&bin) {
                        Ok(_) => Err("malformed module accepted".into()),
                        Err(e) => {
                            rep.rejections += 1;
                            rep.same_message += same_message(&e, message) as usize;
                            Ok(())
                        }
                    },
                },
            },
            WastDirective::AssertInvalid {
                module, message, ..
            } => match module.encode() {
                Err(e) => Err(format!("encode: {e}")),
                Ok(bin) => match Module::new(&bin) {
                    Ok(_) => Err(format!("invalid module accepted ({message})")),
                    Err(e) => {
                        rep.rejections += 1;
                        rep.same_message += same_message(&e, message) as usize;
                        Ok(())
                    }
                },
            },
            WastDirective::Register { name, module, .. } => match cx.instance(*module) {
                Some(i) => {
                    let ex = cx.instance_exports[i].clone();
                    cx.registered.insert(name.to_string(), ex);
                    Ok(())
                }
                None => Err("register: no module".into()),
            },
            WastDirective::Invoke(inv) => match cx.invoke(inv) {
                Ok(Ok(_)) => Ok(()),
                Ok(Err(t)) => Err(format!("unexpected trap {t}")),
                Err(e) => Err(e),
            },
            WastDirective::AssertTrap { exec, message, .. } => match cx.exec(exec) {
                Ok(Err(t)) if trap_matches(&t, message) => Ok(()),
                Ok(Err(t)) => Err(format!("trap {t:?}, expected {message:?}")),
                Ok(Ok(v)) => Err(format!("returned {v:?}, expected trap {message:?}")),
                Err(e) => Err(e),
            },
            WastDirective::AssertReturn { exec, results, .. } => match cx.exec(exec) {
                Ok(Ok(got)) => {
                    let ok = match legacy.get(&span.offset()) {
                        Some(Legacy::Canonical) => got.len() == 1 && is_canonical(&got[0]),
                        Some(Legacy::Arithmetic) => got.len() == 1 && is_arithmetic(&got[0]),
                        None => {
                            got.len() == results.len()
                                && got.iter().zip(results.iter()).all(|(g, w)| matches(g, w))
                        }
                    };
                    if ok {
                        Ok(())
                    } else {
                        Err(format!("got {got:?}, want {results:?}"))
                    }
                }
                Ok(Err(t)) => Err(format!("unexpected trap {t}")),
                Err(e) => Err(e),
            },
            WastDirective::AssertExhaustion { call, .. } => match cx.invoke(call) {
                Ok(Err(Trap::StackExhausted)) => Ok(()),
                Ok(other) => Err(format!("expected exhaustion, got {other:?}")),
                Err(e) => Err(e),
            },
            WastDirective::AssertUnlinkable { module, .. } => match module.encode() {
                Err(e) => Err(format!("encode: {e}")),
                Ok(bin) => match Module::new(&bin) {
                    Err(e) => Err(format!("rejected before linking: {e}")),
                    Ok(m) => {
                        let imports = cx.imports(&m);
                        match cx.store.instantiate(&m, &imports) {
                            Ok(_) => Err("unlinkable module linked".into()),
                            Err(Error::Unlinkable(_))
                            | Err(Error::Trap(Trap::ElemOob))
                            | Err(Error::Trap(Trap::DataOob)) => Ok(()),
                            Err(e) => Err(format!("wrong failure: {e}")),
                        }
                    }
                },
            },
            _ => Err("unsupported directive".into()),
        };
        match result {
            Ok(()) => rep.passed += 1,
            Err(e) => rep.failed.push(format!("{at}: {e}")),
        }
    }
    rep
}
