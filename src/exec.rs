//! Store, instantiation and the interpreter loop.
//!
//! The interpreter keeps one `Vec<u64>` value stack shared by all frames:
//! a frame's locals sit at `base..base + locals`, its operands above.
//! Calls do not recurse on the Rust stack, so deep wasm recursion is
//! bounded by [`Store::max_frames`] and reported as a trap.

use crate::decode::{ConstExpr, ExportKind, ImportKind};
use crate::error::{Error, Trap};
use crate::num;
use crate::types::{FuncType, GlobalType, Limits, Value};
use crate::validate::{Body, Module, Op};
use std::collections::HashMap;
use std::rc::Rc;

/// Page size.
pub const PAGE: usize = 65536;

/// Host function.
pub type HostFunc = Rc<dyn Fn(&[Value]) -> Result<Vec<Value>, Trap>>;

enum FuncInst {
    Wasm {
        ty: FuncType,
        inst: usize,
        body: Rc<Body>,
    },
    Host {
        ty: FuncType,
        f: HostFunc,
    },
}

impl FuncInst {
    fn ty(&self) -> &FuncType {
        match self {
            FuncInst::Wasm { ty, .. } | FuncInst::Host { ty, .. } => ty,
        }
    }
}

/// Table of function references.
#[derive(Debug, Clone)]
pub struct Table {
    /// Slots (function addresses).
    pub elems: Vec<Option<usize>>,
    /// Maximum size.
    pub max: Option<u32>,
}

/// Linear memory.
#[derive(Debug, Clone)]
pub struct Memory {
    /// Bytes (a multiple of [`PAGE`]).
    pub data: Vec<u8>,
    /// Maximum pages.
    pub max: Option<u32>,
}

/// Global variable.
#[derive(Debug, Clone, Copy)]
pub struct Global {
    /// Current value.
    pub value: Value,
    /// Mutability.
    pub mutable: bool,
}

/// Address of something in the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Extern {
    /// Function.
    Func(usize),
    /// Table.
    Table(usize),
    /// Memory.
    Memory(usize),
    /// Global.
    Global(usize),
}

struct Instance {
    types: Vec<FuncType>,
    funcs: Vec<usize>,
    tables: Vec<usize>,
    memories: Vec<usize>,
    globals: Vec<usize>,
    exports: HashMap<String, Extern>,
}

/// Imports by `(module, name)`.
pub type Imports = HashMap<(String, String), Extern>;

/// All runtime state.
pub struct Store {
    funcs: Vec<FuncInst>,
    tables: Vec<Table>,
    memories: Vec<Memory>,
    globals: Vec<Global>,
    instances: Vec<Instance>,
    /// Call depth limit.
    pub max_frames: usize,
    /// Value stack limit (locals + operands of all frames).
    pub max_values: usize,
    /// Memory cap in pages: `memory.grow` returns -1 beyond it, and
    /// instantiating a larger memory fails with [`Error::Limit`].
    pub max_pages: u32,
    /// Table size cap for instantiation.
    pub max_table: u32,
    /// If set, every branch and call costs one unit; at zero the call traps.
    pub fuel: Option<u64>,
}

impl Default for Store {
    fn default() -> Self {
        Self::new()
    }
}

struct Frame {
    body: Rc<Body>,
    inst: usize,
    pc: usize,
    base: usize,
}

impl Store {
    /// Empty store.
    pub fn new() -> Self {
        Store {
            funcs: Vec::new(),
            tables: Vec::new(),
            memories: Vec::new(),
            globals: Vec::new(),
            instances: Vec::new(),
            max_frames: 10_000,
            max_values: 1 << 20,
            max_pages: 65536,
            max_table: 10_000_000,
            fuel: None,
        }
    }

    /// Add a host function.
    pub fn host_func(&mut self, ty: FuncType, f: HostFunc) -> Extern {
        self.funcs.push(FuncInst::Host { ty, f });
        Extern::Func(self.funcs.len() - 1)
    }

    /// Add a table.
    pub fn alloc_table(&mut self, l: Limits) -> Extern {
        self.tables.push(Table {
            elems: vec![None; l.min as usize],
            max: l.max,
        });
        Extern::Table(self.tables.len() - 1)
    }

    /// Add a memory.
    pub fn alloc_memory(&mut self, l: Limits) -> Extern {
        self.memories.push(Memory {
            data: vec![0; l.min as usize * PAGE],
            max: l.max,
        });
        Extern::Memory(self.memories.len() - 1)
    }

    /// Add a global.
    pub fn alloc_global(&mut self, value: Value, mutable: bool) -> Extern {
        self.globals.push(Global { value, mutable });
        Extern::Global(self.globals.len() - 1)
    }

    /// Satisfy every import of `m` with a fresh stand-in: host functions
    /// that return zeros, tables and memories of the minimum size, and
    /// zero-valued globals. Used by the fuzz targets and robustness tests.
    pub fn stub_imports(&mut self, m: &Module) -> Imports {
        let mut out = Imports::new();
        for imp in &m.raw.imports {
            let ext = match &imp.kind {
                ImportKind::Func(t) => {
                    let ty = m.raw.types[*t as usize].clone();
                    let res = ty.results.clone();
                    self.host_func(
                        ty,
                        Rc::new(move |_: &[Value]| {
                            Ok(res.iter().map(|t| Value::zero(*t)).collect())
                        }),
                    )
                }
                ImportKind::Table(l) => self.alloc_table(Limits {
                    min: l.min.min(self.max_table),
                    max: l.max,
                }),
                ImportKind::Memory(l) => self.alloc_memory(Limits {
                    min: l.min.min(self.max_pages),
                    max: l.max,
                }),
                ImportKind::Global(g) => self.alloc_global(Value::zero(g.ty), g.mutable),
            };
            out.insert((imp.module.clone(), imp.name.clone()), ext);
        }
        out
    }

    /// Exported functions of an instance, by name.
    pub fn exported_funcs(&self, inst: usize) -> Vec<(String, usize)> {
        let mut v: Vec<_> = self.instances[inst]
            .exports
            .iter()
            .filter_map(|(n, e)| match e {
                Extern::Func(f) => Some((n.clone(), *f)),
                _ => None,
            })
            .collect();
        v.sort();
        v
    }

    /// Signature of a function.
    pub fn func_type(&self, f: usize) -> &FuncType {
        self.funcs[f].ty()
    }

    /// Look up an export of an instance.
    pub fn export(&self, inst: usize, name: &str) -> Option<Extern> {
        self.instances.get(inst)?.exports.get(name).copied()
    }

    /// Global value.
    pub fn global(&self, addr: usize) -> Value {
        self.globals[addr].value
    }

    /// Memory bytes.
    pub fn memory(&self, addr: usize) -> &Memory {
        &self.memories[addr]
    }

    /// Instantiate a validated module (WebAssembly 1.0 semantics: all
    /// segment bounds are checked before any segment is written).
    pub fn instantiate(&mut self, m: &Module, imports: &Imports) -> Result<usize, Error> {
        let raw = &m.raw;
        let id = self.instances.len();
        let mut inst = Instance {
            types: raw.types.clone(),
            funcs: Vec::new(),
            tables: Vec::new(),
            memories: Vec::new(),
            globals: Vec::new(),
            exports: HashMap::new(),
        };
        for imp in &raw.imports {
            let key = (imp.module.clone(), imp.name.clone());
            let ext = *imports
                .get(&key)
                .ok_or_else(|| Error::Unlinkable(format!("unknown import {}.{}", key.0, key.1)))?;
            let bad = || Error::Unlinkable("incompatible import type".into());
            match (&imp.kind, ext) {
                (ImportKind::Func(t), Extern::Func(a)) => {
                    if self.funcs[a].ty() != &raw.types[*t as usize] {
                        return Err(bad());
                    }
                    inst.funcs.push(a);
                }
                (ImportKind::Table(l), Extern::Table(a)) => {
                    let t = &self.tables[a];
                    let have = Limits {
                        min: t.elems.len() as u32,
                        max: t.max,
                    };
                    if !have.matches(l) {
                        return Err(bad());
                    }
                    inst.tables.push(a);
                }
                (ImportKind::Memory(l), Extern::Memory(a)) => {
                    let mm = &self.memories[a];
                    let have = Limits {
                        min: (mm.data.len() / PAGE) as u32,
                        max: mm.max,
                    };
                    if !have.matches(l) {
                        return Err(bad());
                    }
                    inst.memories.push(a);
                }
                (ImportKind::Global(g), Extern::Global(a)) => {
                    let have = self.globals[a];
                    if (GlobalType {
                        ty: have.value.ty(),
                        mutable: have.mutable,
                    }) != *g
                    {
                        return Err(bad());
                    }
                    inst.globals.push(a);
                }
                _ => return Err(bad()),
            }
        }
        // Allocate. Functions refer to the instance by id, which becomes
        // valid when it is pushed below.
        for (i, body) in m.bodies.iter().enumerate() {
            let ty = raw.types[raw.funcs[i] as usize].clone();
            self.funcs.push(FuncInst::Wasm {
                ty,
                inst: id,
                body: body.clone(),
            });
            inst.funcs.push(self.funcs.len() - 1);
        }
        for l in &raw.tables {
            if l.min > self.max_table {
                return Err(Error::Limit("table larger than the store allows".into()));
            }
            let Extern::Table(a) = self.alloc_table(*l) else {
                unreachable!()
            };
            inst.tables.push(a);
        }
        for l in &raw.memories {
            if l.min > self.max_pages {
                return Err(Error::Limit("memory larger than the store allows".into()));
            }
            let Extern::Memory(a) = self.alloc_memory(*l) else {
                unreachable!()
            };
            inst.memories.push(a);
        }
        let eval = |s: &Store, inst: &Instance, e: &ConstExpr| -> Value {
            match e {
                ConstExpr::Const(v) => *v,
                ConstExpr::Global(i) => s.globals[inst.globals[*i as usize]].value,
                ConstExpr::NotConst => unreachable!("validated"),
            }
        };
        for (g, init) in &raw.globals {
            let v = eval(self, &inst, init);
            let Extern::Global(a) = self.alloc_global(v, g.mutable) else {
                unreachable!()
            };
            inst.globals.push(a);
        }
        for e in &raw.exports {
            let ext = match e.kind {
                ExportKind::Func => Extern::Func(inst.funcs[e.index as usize]),
                ExportKind::Table => Extern::Table(inst.tables[e.index as usize]),
                ExportKind::Memory => Extern::Memory(inst.memories[e.index as usize]),
                ExportKind::Global => Extern::Global(inst.globals[e.index as usize]),
            };
            inst.exports.insert(e.name.clone(), ext);
        }
        let offset = |s: &Store, inst: &Instance, e: &ConstExpr| -> usize {
            match eval(s, inst, e) {
                Value::I32(v) => v as u32 as usize,
                _ => unreachable!("validated"),
            }
        };
        let mut elem_writes = Vec::new();
        for e in &raw.elems {
            let off = offset(self, &inst, &e.offset);
            let t = inst.tables[e.table as usize];
            if off + e.funcs.len() > self.tables[t].elems.len() {
                return Err(Error::Trap(Trap::ElemOob));
            }
            elem_writes.push((t, off));
        }
        let mut data_writes = Vec::new();
        for d in &raw.data {
            let off = offset(self, &inst, &d.offset);
            let mm = inst.memories[d.memory as usize];
            if off + d.bytes.len() > self.memories[mm].data.len() {
                return Err(Error::Trap(Trap::DataOob));
            }
            data_writes.push((mm, off));
        }
        for (e, (t, off)) in raw.elems.iter().zip(elem_writes) {
            for (i, f) in e.funcs.iter().enumerate() {
                self.tables[t].elems[off + i] = Some(inst.funcs[*f as usize]);
            }
        }
        for (d, (mm, off)) in raw.data.iter().zip(data_writes) {
            self.memories[mm].data[off..off + d.bytes.len()].copy_from_slice(&d.bytes);
        }
        let start = raw.start.map(|s| inst.funcs[s as usize]);
        self.instances.push(inst);
        if let Some(f) = start {
            self.invoke(f, &[]).map_err(Error::Trap)?;
        }
        Ok(id)
    }

    /// Call a function by address with type-checked arguments.
    pub fn invoke(&mut self, f: usize, args: &[Value]) -> Result<Vec<Value>, Trap> {
        let ty = self.funcs[f].ty().clone();
        assert_eq!(
            args.iter().map(|a| a.ty()).collect::<Vec<_>>(),
            ty.params,
            "argument types"
        );
        let mut stack: Vec<u64> = args.iter().map(|a| a.to_bits()).collect();
        self.run(f, &mut stack)?;
        let n = ty.results.len();
        let out = stack[stack.len() - n..]
            .iter()
            .zip(&ty.results)
            .map(|(b, t)| Value::from_bits(*t, *b))
            .collect();
        Ok(out)
    }

    /// Call an exported function.
    pub fn invoke_export(
        &mut self,
        inst: usize,
        name: &str,
        args: &[Value],
    ) -> Result<Vec<Value>, Trap> {
        match self.export(inst, name) {
            Some(Extern::Func(f)) => self.invoke(f, args),
            other => panic!("export {name:?} is not a function: {other:?}"),
        }
    }

    fn call_host(f: &HostFunc, ty: &FuncType, stack: &mut Vec<u64>) -> Result<(), Trap> {
        let n = ty.params.len();
        let args: Vec<Value> = stack[stack.len() - n..]
            .iter()
            .zip(&ty.params)
            .map(|(b, t)| Value::from_bits(*t, *b))
            .collect();
        stack.truncate(stack.len() - n);
        for v in f(&args)? {
            stack.push(v.to_bits());
        }
        Ok(())
    }

    fn burn(&mut self) -> Result<(), Trap> {
        if let Some(f) = &mut self.fuel {
            if *f == 0 {
                return Err(Trap::OutOfFuel);
            }
            *f -= 1;
        }
        Ok(())
    }

    fn run(&mut self, entry: usize, stack: &mut Vec<u64>) -> Result<(), Trap> {
        let (body, inst) = match &self.funcs[entry] {
            FuncInst::Host { f, ty } => {
                let (f, ty) = (f.clone(), ty.clone());
                return Self::call_host(&f, &ty, stack);
            }
            FuncInst::Wasm { body, inst, .. } => (body.clone(), *inst),
        };
        let mut frames: Vec<Frame> = Vec::new();
        let mut cur = Frame {
            base: stack.len() - body.ty.params.len(),
            body,
            inst,
            pc: 0,
        };
        stack.resize(cur.base + cur.body.locals.len(), 0);
        let mut obase = cur.base + cur.body.locals.len();
        macro_rules! pop {
            () => {
                stack.pop().expect("validated stack")
            };
        }
        macro_rules! branch {
            ($t:expr) => {{
                let t = $t;
                let dst = obase + t.height as usize;
                let n = t.arity as usize;
                let src = stack.len() - n;
                if src != dst {
                    stack.copy_within(src.., dst);
                }
                stack.truncate(dst + n);
                cur.pc = t.pc as usize;
            }};
        }
        loop {
            let op = cur.body.ops[cur.pc];
            cur.pc += 1;
            match op {
                Op::Unreachable => return Err(Trap::Unreachable),
                Op::Jump(pc) => cur.pc = pc as usize,
                Op::JumpIfZero(pc) => {
                    if pop!() as u32 == 0 {
                        cur.pc = pc as usize;
                    }
                }
                Op::Br(t) => {
                    self.burn()?;
                    branch!(t)
                }
                Op::BrIf(t) => {
                    if pop!() as u32 != 0 {
                        self.burn()?;
                        branch!(t)
                    }
                }
                Op::BrTable { start, len } => {
                    self.burn()?;
                    let i = (pop!() as u32).min(len - 1);
                    let t = cur.body.targets[(start + i) as usize];
                    branch!(t)
                }
                Op::Return => {
                    let n = cur.body.ty.results.len();
                    let src = stack.len() - n;
                    stack.copy_within(src.., cur.base);
                    stack.truncate(cur.base + n);
                    match frames.pop() {
                        None => return Ok(()),
                        Some(f) => {
                            cur = f;
                            obase = cur.base + cur.body.locals.len();
                        }
                    }
                }
                Op::Call(_) | Op::CallIndirect(_) => {
                    self.burn()?;
                    let addr = match op {
                        Op::Call(i) => self.instances[cur.inst].funcs[i as usize],
                        Op::CallIndirect(ti) => {
                            let i = pop!() as u32 as usize;
                            let inst = &self.instances[cur.inst];
                            let t = &self.tables[inst.tables[0]];
                            let a = t
                                .elems
                                .get(i)
                                .ok_or(Trap::UndefinedElement)?
                                .ok_or(Trap::UninitializedElement)?;
                            if self.funcs[a].ty() != &inst.types[ti as usize] {
                                return Err(Trap::IndirectTypeMismatch);
                            }
                            a
                        }
                        _ => unreachable!(),
                    };
                    match &self.funcs[addr] {
                        FuncInst::Host { f, ty } => {
                            let (f, ty) = (f.clone(), ty.clone());
                            Self::call_host(&f, &ty, stack)?;
                        }
                        FuncInst::Wasm { body, inst, .. } => {
                            if frames.len() + 1 >= self.max_frames
                                || stack.len() + body.locals.len() + body.max_height as usize
                                    > self.max_values
                            {
                                return Err(Trap::StackExhausted);
                            }
                            let next = Frame {
                                base: stack.len() - body.ty.params.len(),
                                body: body.clone(),
                                inst: *inst,
                                pc: 0,
                            };
                            stack.resize(next.base + next.body.locals.len(), 0);
                            obase = next.base + next.body.locals.len();
                            frames.push(std::mem::replace(&mut cur, next));
                        }
                    }
                }
                Op::Drop => {
                    pop!();
                }
                Op::Select => {
                    let c = pop!() as u32;
                    let b = pop!();
                    if c == 0 {
                        *stack.last_mut().expect("validated") = b;
                    }
                }
                Op::LocalGet(i) => stack.push(stack[cur.base + i as usize]),
                Op::LocalSet(i) => stack[cur.base + i as usize] = pop!(),
                Op::LocalTee(i) => stack[cur.base + i as usize] = *stack.last().expect("validated"),
                Op::GlobalGet(i) => {
                    let a = self.instances[cur.inst].globals[i as usize];
                    stack.push(self.globals[a].value.to_bits());
                }
                Op::GlobalSet(i) => {
                    let a = self.instances[cur.inst].globals[i as usize];
                    let g = &mut self.globals[a];
                    g.value = Value::from_bits(g.value.ty(), pop!());
                }
                Op::Load(code, off) => {
                    let m = self.instances[cur.inst].memories[0];
                    let mem = &self.memories[m].data;
                    let ea = pop!() as u32 as usize + off as usize;
                    let n = match code {
                        0x29 | 0x2b => 8,
                        0x28 | 0x2a | 0x34 | 0x35 => 4,
                        0x2e | 0x2f | 0x32 | 0x33 => 2,
                        _ => 1,
                    };
                    let bytes = mem.get(ea..ea + n).ok_or(Trap::MemoryOob)?;
                    let mut raw = [0u8; 8];
                    raw[..n].copy_from_slice(bytes);
                    let u = u64::from_le_bytes(raw);
                    let v = match code {
                        0x28 | 0x2a | 0x29 | 0x2b | 0x2d | 0x2f | 0x31 | 0x33 | 0x35 => u,
                        0x2c => u as u8 as i8 as i32 as u32 as u64,
                        0x2e => u as u16 as i16 as i32 as u32 as u64,
                        0x30 => u as u8 as i8 as i64 as u64,
                        0x32 => u as u16 as i16 as i64 as u64,
                        0x34 => u as u32 as i32 as i64 as u64,
                        _ => unreachable!(),
                    };
                    stack.push(v);
                }
                Op::Store(code, off) => {
                    let v = pop!();
                    let m = self.instances[cur.inst].memories[0];
                    let mem = &mut self.memories[m].data;
                    let ea = pop!() as u32 as usize + off as usize;
                    let n = match code {
                        0x37 | 0x39 => 8,
                        0x36 | 0x38 | 0x3e => 4,
                        0x3b | 0x3d => 2,
                        _ => 1,
                    };
                    let dst = mem.get_mut(ea..ea + n).ok_or(Trap::MemoryOob)?;
                    dst.copy_from_slice(&v.to_le_bytes()[..n]);
                }
                Op::MemorySize => {
                    let m = self.instances[cur.inst].memories[0];
                    stack.push((self.memories[m].data.len() / PAGE) as u64);
                }
                Op::MemoryGrow => {
                    let delta = pop!() as u32 as u64;
                    let m = self.instances[cur.inst].memories[0];
                    let cap = self.max_pages;
                    let mem = &mut self.memories[m];
                    let old = (mem.data.len() / PAGE) as u64;
                    let limit = mem.max.unwrap_or(65536).min(cap) as u64;
                    if old + delta > limit {
                        stack.push(u32::MAX as u64);
                    } else {
                        mem.data.resize((old + delta) as usize * PAGE, 0);
                        stack.push(old);
                    }
                }
                Op::Const(c) => stack.push(c),
                Op::Num(code) => num::exec(code, stack)?,
            }
        }
    }
}
