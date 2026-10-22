//! Validation (WebAssembly 1.0 typing rules) fused with compilation of
//! each function body into a flat instruction array whose branches carry
//! their resolved target, the operand height to unwind to, and the
//! number of values to keep.

use crate::decode::{decode, ConstExpr, ExportKind, ImportKind, RawBody, RawModule, Reader};
use crate::error::{invalid, malformed, Error};
use crate::num;
use crate::types::{FuncType, GlobalType, Limits, ValType};
use std::collections::HashSet;
use std::rc::Rc;

/// Resolved branch target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    /// Instruction index to continue at.
    pub pc: u32,
    /// Operand stack height (above the locals) at the label.
    pub height: u32,
    /// Values carried by the branch.
    pub arity: u32,
}

/// Compiled instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// Trap.
    Unreachable,
    /// Unconditional jump with no stack change (end of a `then` arm).
    Jump(u32),
    /// Pop an i32; jump if zero (`if`).
    JumpIfZero(u32),
    /// `br`.
    Br(Target),
    /// `br_if`.
    BrIf(Target),
    /// `br_table`: `len` targets at `start` in [`Body::targets`], the last
    /// one being the default.
    BrTable {
        /// First target.
        start: u32,
        /// Number of targets including the default.
        len: u32,
    },
    /// Return from the function.
    Return,
    /// Direct call.
    Call(u32),
    /// Indirect call through table 0 with this type index.
    CallIndirect(u32),
    /// Drop.
    Drop,
    /// Select.
    Select,
    /// `local.get`.
    LocalGet(u32),
    /// `local.set`.
    LocalSet(u32),
    /// `local.tee`.
    LocalTee(u32),
    /// `global.get`.
    GlobalGet(u32),
    /// `global.set`.
    GlobalSet(u32),
    /// Load with opcode and offset.
    Load(u8, u32),
    /// Store with opcode and offset.
    Store(u8, u32),
    /// `memory.size`.
    MemorySize,
    /// `memory.grow`.
    MemoryGrow,
    /// Constant (raw bits).
    Const(u64),
    /// Numeric opcode 0x45..=0xbf.
    Num(u8),
}

/// A validated, compiled function body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Body {
    /// Signature.
    pub ty: FuncType,
    /// Parameters followed by declared locals.
    pub locals: Vec<ValType>,
    /// Instructions.
    pub ops: Vec<Op>,
    /// `br_table` targets.
    pub targets: Vec<Target>,
    /// Maximum operand stack height.
    pub max_height: u32,
}

/// A validated module, ready to instantiate.
#[derive(Debug, Clone)]
pub struct Module {
    /// Decoded sections.
    pub raw: RawModule,
    /// Type index of every function (imports first).
    pub func_types: Vec<u32>,
    /// Type of every global (imports first).
    pub global_types: Vec<GlobalType>,
    /// Number of imported functions.
    pub imported_funcs: usize,
    /// Number of imported globals.
    pub imported_globals: usize,
    /// Compiled bodies of defined functions.
    pub bodies: Vec<Rc<Body>>,
}

const MAX_PAGES: u32 = 65536;

impl Module {
    /// Decode and validate a binary module.
    pub fn new(bin: &[u8]) -> Result<Module, Error> {
        let raw = decode(bin)?;
        validate(raw)
    }
}

fn load_info(op: u8) -> Option<(ValType, u32)> {
    use ValType::*;
    Some(match op {
        0x28 => (I32, 2),
        0x29 => (I64, 3),
        0x2a => (F32, 2),
        0x2b => (F64, 3),
        0x2c | 0x2d => (I32, 0),
        0x2e | 0x2f => (I32, 1),
        0x30 | 0x31 => (I64, 0),
        0x32 | 0x33 => (I64, 1),
        0x34 | 0x35 => (I64, 2),
        _ => return None,
    })
}

fn store_info(op: u8) -> Option<(ValType, u32)> {
    use ValType::*;
    Some(match op {
        0x36 => (I32, 2),
        0x37 => (I64, 3),
        0x38 => (F32, 2),
        0x39 => (F64, 3),
        0x3a => (I32, 0),
        0x3b => (I32, 1),
        0x3c => (I64, 0),
        0x3d => (I64, 1),
        0x3e => (I64, 2),
        _ => return None,
    })
}

fn validate(raw: RawModule) -> Result<Module, Error> {
    for t in &raw.types {
        if t.results.len() > 1 {
            return invalid("invalid result arity");
        }
    }
    let ty = |i: u32| {
        raw.types
            .get(i as usize)
            .ok_or_else(|| Error::Invalid(format!("unknown type {i}")))
    };
    let mut func_types = Vec::new();
    let mut global_types = Vec::new();
    let (mut tables, mut memories) = (Vec::new(), Vec::new());
    for imp in &raw.imports {
        match &imp.kind {
            ImportKind::Func(t) => {
                ty(*t)?;
                func_types.push(*t);
            }
            ImportKind::Table(l) => tables.push(*l),
            ImportKind::Memory(l) => memories.push(*l),
            ImportKind::Global(g) => global_types.push(*g),
        }
    }
    let imported_funcs = func_types.len();
    let imported_globals = global_types.len();
    for t in &raw.funcs {
        ty(*t)?;
        func_types.push(*t);
    }
    tables.extend(raw.tables.iter().copied());
    memories.extend(raw.memories.iter().copied());
    if tables.len() > 1 {
        return invalid("multiple tables");
    }
    if memories.len() > 1 {
        return invalid("multiple memories");
    }
    let check_limits = |l: &Limits, cap: Option<u32>| -> Result<(), Error> {
        if let Some(c) = cap {
            if l.min > c || l.max.is_some_and(|m| m > c) {
                return invalid("memory size must be at most 65536 pages (4GiB)");
            }
        }
        if l.max.is_some_and(|m| m < l.min) {
            return invalid("size minimum must not be greater than maximum");
        }
        Ok(())
    };
    for l in &tables {
        check_limits(l, None)?;
    }
    for l in &memories {
        check_limits(l, Some(MAX_PAGES))?;
    }
    // Constant expressions may only read imported, immutable globals.
    let imported = global_types.clone();
    let const_ty = |e: &ConstExpr| -> Result<ValType, Error> {
        match e {
            ConstExpr::Const(v) => Ok(v.ty()),
            ConstExpr::Global(i) => {
                let g = imported
                    .get(*i as usize)
                    .ok_or_else(|| Error::Invalid(format!("unknown global {i}")))?;
                if g.mutable {
                    return invalid("constant expression required");
                }
                Ok(g.ty)
            }
            ConstExpr::NotConst => invalid("constant expression required"),
        }
    };
    for (g, init) in &raw.globals {
        if const_ty(init)? != g.ty {
            return invalid("type mismatch");
        }
    }
    global_types.extend(raw.globals.iter().map(|(g, _)| *g));
    let mut names = HashSet::new();
    for e in &raw.exports {
        if !names.insert(e.name.as_str()) {
            return invalid("duplicate export name");
        }
        let n = match e.kind {
            ExportKind::Func => func_types.len(),
            ExportKind::Table => tables.len(),
            ExportKind::Memory => memories.len(),
            ExportKind::Global => global_types.len(),
        };
        if e.index as usize >= n {
            let kind = match e.kind {
                ExportKind::Func => "function",
                ExportKind::Table => "table",
                ExportKind::Memory => "memory",
                ExportKind::Global => "global",
            };
            return invalid(format!("unknown {kind} {}", e.index));
        }
    }
    if let Some(s) = raw.start {
        let t = func_types
            .get(s as usize)
            .ok_or_else(|| Error::Invalid(format!("unknown function {s}")))?;
        if *ty(*t)? != FuncType::default() {
            return invalid("start function");
        }
    }
    for e in &raw.elems {
        if e.table as usize >= tables.len() {
            return invalid(format!("unknown table {}", e.table));
        }
        if const_ty(&e.offset)? != ValType::I32 {
            return invalid("type mismatch");
        }
        for f in &e.funcs {
            if *f as usize >= func_types.len() {
                return invalid(format!("unknown function {f}"));
            }
        }
    }
    for d in &raw.data {
        if d.memory as usize >= memories.len() {
            return invalid(format!("unknown memory {}", d.memory));
        }
        if const_ty(&d.offset)? != ValType::I32 {
            return invalid("type mismatch");
        }
    }
    let ctx = Ctx {
        types: &raw.types,
        funcs: &func_types,
        globals: &global_types,
        has_table: !tables.is_empty(),
        has_memory: !memories.is_empty(),
    };
    let mut bodies = Vec::with_capacity(raw.bodies.len());
    for (i, b) in raw.bodies.iter().enumerate() {
        let t = raw.types[raw.funcs[i] as usize].clone();
        bodies.push(Rc::new(compile(&ctx, t, b)?));
    }
    Ok(Module {
        raw,
        func_types,
        global_types,
        imported_funcs,
        imported_globals,
        bodies,
    })
}

struct Ctx<'a> {
    types: &'a [FuncType],
    funcs: &'a [u32],
    globals: &'a [GlobalType],
    has_table: bool,
    has_memory: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Block,
    Loop,
    If,
    Else,
    Func,
}

#[derive(Debug, Clone, Copy)]
enum Patch {
    Op(usize),
    Table(usize),
}

struct Ctrl {
    kind: Kind,
    result: Option<ValType>,
    height: usize,
    unreachable: bool,
    start: u32,
    patches: Vec<Patch>,
    if_jump: Option<usize>,
}

impl Ctrl {
    fn label_arity(&self) -> u32 {
        if self.kind == Kind::Loop {
            0
        } else {
            self.result.is_some() as u32
        }
    }
    fn label_type(&self) -> Option<ValType> {
        if self.kind == Kind::Loop {
            None
        } else {
            self.result
        }
    }
}

struct V {
    vals: Vec<Option<ValType>>,
    ctrls: Vec<Ctrl>,
    ops: Vec<Op>,
    targets: Vec<Target>,
    max: usize,
}

fn mismatch<T>() -> Result<T, Error> {
    invalid("type mismatch")
}

impl V {
    fn push(&mut self, t: Option<ValType>) {
        self.vals.push(t);
        self.max = self.max.max(self.vals.len());
    }
    fn pop(&mut self) -> Result<Option<ValType>, Error> {
        let c = self.ctrls.last().expect("control stack");
        if self.vals.len() == c.height {
            if c.unreachable {
                return Ok(None);
            }
            return mismatch();
        }
        Ok(self.vals.pop().expect("nonempty"))
    }
    fn pop_expect(&mut self, want: ValType) -> Result<(), Error> {
        match self.pop()? {
            Some(t) if t != want => mismatch(),
            _ => Ok(()),
        }
    }
    fn pop_opt(&mut self, want: Option<ValType>) -> Result<(), Error> {
        match want {
            Some(t) => self.pop_expect(t),
            None => Ok(()),
        }
    }
    fn set_unreachable(&mut self) {
        let c = self.ctrls.last_mut().expect("control stack");
        self.vals.truncate(c.height);
        c.unreachable = true;
    }
    fn label(&self, depth: u32) -> Result<&Ctrl, Error> {
        let n = self.ctrls.len();
        if depth as usize >= n {
            return invalid(format!("unknown label {depth}"));
        }
        Ok(&self.ctrls[n - 1 - depth as usize])
    }
    /// Target for a branch to `depth`; forward targets are patched at `end`.
    fn target(&mut self, depth: u32, patch: Patch) -> Result<Target, Error> {
        let n = self.ctrls.len();
        let c = self.label(depth)?;
        let t = Target {
            pc: if c.kind == Kind::Loop { c.start } else { 0 },
            height: c.height as u32,
            arity: c.label_arity(),
        };
        if c.kind != Kind::Loop {
            self.ctrls[n - 1 - depth as usize].patches.push(patch);
        }
        Ok(t)
    }
    fn emit(&mut self, op: Op) {
        self.ops.push(op);
    }
}

fn block_type(r: &mut Reader) -> Result<Option<ValType>, Error> {
    let b = r.byte()?;
    if b == 0x40 {
        return Ok(None);
    }
    match ValType::from_byte(b) {
        Some(t) => Ok(Some(t)),
        None => malformed("malformed block type"),
    }
}

fn memarg(r: &mut Reader, natural: u32) -> Result<u32, Error> {
    let align = r.u32()?;
    let offset = r.u32()?;
    if align > natural {
        return invalid("alignment must not be larger than natural");
    }
    Ok(offset)
}

fn compile(ctx: &Ctx, ty: FuncType, body: &RawBody) -> Result<Body, Error> {
    let mut locals = ty.params.clone();
    locals.extend(body.locals.iter().copied());
    let mut v = V {
        vals: Vec::new(),
        ctrls: vec![Ctrl {
            kind: Kind::Func,
            result: ty.results.first().copied(),
            height: 0,
            unreachable: false,
            start: 0,
            patches: Vec::new(),
            if_jump: None,
        }],
        ops: Vec::new(),
        targets: Vec::new(),
        max: 0,
    };
    let mut r = Reader::new(&body.code);
    let local = |i: u32| -> Result<ValType, Error> {
        locals
            .get(i as usize)
            .copied()
            .ok_or_else(|| Error::Invalid(format!("unknown local {i}")))
    };
    let need_mem = || {
        if ctx.has_memory {
            Ok(())
        } else {
            invalid("unknown memory 0")
        }
    };
    while !v.ctrls.is_empty() {
        let op = r.byte()?;
        match op {
            0x00 => {
                v.emit(Op::Unreachable);
                v.set_unreachable();
            }
            0x01 => {}
            0x02..=0x04 => {
                let bt = block_type(&mut r)?;
                let mut if_jump = None;
                if op == 0x04 {
                    v.pop_expect(ValType::I32)?;
                    if_jump = Some(v.ops.len());
                    v.emit(Op::JumpIfZero(0));
                }
                let kind = match op {
                    0x02 => Kind::Block,
                    0x03 => Kind::Loop,
                    _ => Kind::If,
                };
                v.ctrls.push(Ctrl {
                    kind,
                    result: bt,
                    height: v.vals.len(),
                    unreachable: false,
                    start: v.ops.len() as u32,
                    patches: Vec::new(),
                    if_jump,
                });
            }
            0x05 => {
                let c = v.ctrls.last().expect("control stack");
                if c.kind != Kind::If {
                    return malformed("else without if");
                }
                let res = c.result;
                v.pop_opt(res)?;
                if v.vals.len() != v.ctrls.last().expect("ctrl").height {
                    return mismatch();
                }
                let j = v.ops.len();
                v.emit(Op::Jump(0));
                let pc = v.ops.len() as u32;
                let c = v.ctrls.last_mut().expect("ctrl");
                c.patches.push(Patch::Op(j));
                if let Some(i) = c.if_jump.take() {
                    v.ops[i] = Op::JumpIfZero(pc);
                }
                c.kind = Kind::Else;
                c.unreachable = false;
            }
            0x0b => {
                let res = v.ctrls.last().expect("ctrl").result;
                v.pop_opt(res)?;
                let c = v.ctrls.last().expect("ctrl");
                if v.vals.len() != c.height {
                    return mismatch();
                }
                if c.kind == Kind::If && c.result.is_some() {
                    // An `if` without `else` must not produce a value.
                    return mismatch();
                }
                let c = v.ctrls.pop().expect("ctrl");
                // Branches to the function label land on its `Return`.
                let end = v.ops.len() as u32;
                if c.kind == Kind::Func {
                    v.emit(Op::Return);
                }
                if let Some(i) = c.if_jump {
                    v.ops[i] = Op::JumpIfZero(end);
                }
                for p in c.patches {
                    match p {
                        Patch::Op(i) => match &mut v.ops[i] {
                            Op::Br(t) | Op::BrIf(t) => t.pc = end,
                            Op::Jump(pc) => *pc = end,
                            _ => unreachable!("patch on a non-branch"),
                        },
                        Patch::Table(i) => v.targets[i].pc = end,
                    }
                }
                if c.kind != Kind::Func {
                    if let Some(t) = c.result {
                        v.push(Some(t));
                    }
                }
            }
            0x0c => {
                let d = r.u32()?;
                let i = v.ops.len();
                let t = v.target(d, Patch::Op(i))?;
                let lt = v.label(d)?.label_type();
                v.pop_opt(lt)?;
                v.emit(Op::Br(t));
                v.set_unreachable();
            }
            0x0d => {
                let d = r.u32()?;
                v.pop_expect(ValType::I32)?;
                let i = v.ops.len();
                let t = v.target(d, Patch::Op(i))?;
                let lt = v.label(d)?.label_type();
                v.pop_opt(lt)?;
                if let Some(t) = lt {
                    v.push(Some(t));
                }
                v.emit(Op::BrIf(t));
            }
            0x0e => {
                let n = r.u32()? as usize;
                if n > body.code.len() {
                    return malformed("unexpected end");
                }
                let mut depths = Vec::with_capacity(n + 1);
                for _ in 0..=n {
                    depths.push(r.u32()?);
                }
                v.pop_expect(ValType::I32)?;
                let default = *depths.last().expect("default");
                let arity = v.label(default)?.label_arity();
                let lt = v.label(default)?.label_type();
                let start = v.targets.len() as u32;
                for d in &depths {
                    let c = v.label(*d)?;
                    if c.label_arity() != arity || (c.label_type() != lt) {
                        return mismatch();
                    }
                    let i = v.targets.len();
                    let t = v.target(*d, Patch::Table(i))?;
                    v.targets.push(t);
                }
                v.pop_opt(lt)?;
                v.emit(Op::BrTable {
                    start,
                    len: depths.len() as u32,
                });
                v.set_unreachable();
            }
            0x0f => {
                v.pop_opt(ty.results.first().copied())?;
                v.emit(Op::Return);
                v.set_unreachable();
            }
            0x10 => {
                let f = r.u32()?;
                let t = ctx
                    .funcs
                    .get(f as usize)
                    .ok_or_else(|| Error::Invalid(format!("unknown function {f}")))?;
                let ft = &ctx.types[*t as usize];
                for p in ft.params.iter().rev() {
                    v.pop_expect(*p)?;
                }
                for t in &ft.results {
                    v.push(Some(*t));
                }
                v.emit(Op::Call(f));
            }
            0x11 => {
                let ti = r.u32()?;
                if r.byte()? != 0 {
                    return malformed("zero flag expected");
                }
                if !ctx.has_table {
                    return invalid("unknown table 0");
                }
                let ft = ctx
                    .types
                    .get(ti as usize)
                    .ok_or_else(|| Error::Invalid(format!("unknown type {ti}")))?;
                v.pop_expect(ValType::I32)?;
                for p in ft.params.iter().rev() {
                    v.pop_expect(*p)?;
                }
                for t in &ft.results {
                    v.push(Some(*t));
                }
                v.emit(Op::CallIndirect(ti));
            }
            0x1a => {
                v.pop()?;
                v.emit(Op::Drop);
            }
            0x1b => {
                v.pop_expect(ValType::I32)?;
                let a = v.pop()?;
                let b = v.pop()?;
                let t = match (a, b) {
                    (Some(x), Some(y)) if x != y => return mismatch(),
                    (Some(x), _) | (_, Some(x)) => Some(x),
                    _ => None,
                };
                v.push(t);
                v.emit(Op::Select);
            }
            0x20 => {
                let i = r.u32()?;
                let t = local(i)?;
                v.push(Some(t));
                v.emit(Op::LocalGet(i));
            }
            0x21 | 0x22 => {
                let i = r.u32()?;
                let t = local(i)?;
                v.pop_expect(t)?;
                if op == 0x22 {
                    v.push(Some(t));
                    v.emit(Op::LocalTee(i));
                } else {
                    v.emit(Op::LocalSet(i));
                }
            }
            0x23 | 0x24 => {
                let i = r.u32()?;
                let g = *ctx
                    .globals
                    .get(i as usize)
                    .ok_or_else(|| Error::Invalid(format!("unknown global {i}")))?;
                if op == 0x23 {
                    v.push(Some(g.ty));
                    v.emit(Op::GlobalGet(i));
                } else {
                    if !g.mutable {
                        return invalid("global is immutable");
                    }
                    v.pop_expect(g.ty)?;
                    v.emit(Op::GlobalSet(i));
                }
            }
            0x28..=0x35 => {
                let (t, nat) = load_info(op).expect("load opcode");
                let off = memarg(&mut r, nat)?;
                need_mem()?;
                v.pop_expect(ValType::I32)?;
                v.push(Some(t));
                v.emit(Op::Load(op, off));
            }
            0x36..=0x3e => {
                let (t, nat) = store_info(op).expect("store opcode");
                let off = memarg(&mut r, nat)?;
                need_mem()?;
                v.pop_expect(t)?;
                v.pop_expect(ValType::I32)?;
                v.emit(Op::Store(op, off));
            }
            0x3f | 0x40 => {
                if r.byte()? != 0 {
                    return malformed("zero flag expected");
                }
                need_mem()?;
                if op == 0x40 {
                    v.pop_expect(ValType::I32)?;
                    v.emit(Op::MemoryGrow);
                } else {
                    v.emit(Op::MemorySize);
                }
                v.push(Some(ValType::I32));
            }
            0x41 => {
                let c = r.s32()?;
                v.push(Some(ValType::I32));
                v.emit(Op::Const(c as u32 as u64));
            }
            0x42 => {
                let c = r.s64()?;
                v.push(Some(ValType::I64));
                v.emit(Op::Const(c as u64));
            }
            0x43 => {
                let c = r.f32()?;
                v.push(Some(ValType::F32));
                v.emit(Op::Const(c as u64));
            }
            0x44 => {
                let c = r.f64()?;
                v.push(Some(ValType::F64));
                v.emit(Op::Const(c));
            }
            0x45..=0xbf => {
                let (params, res) = num::signature(op).expect("numeric opcode");
                for p in params.iter().rev() {
                    v.pop_expect(*p)?;
                }
                v.push(Some(res));
                v.emit(Op::Num(op));
            }
            _ => return malformed(format!("illegal opcode {op:#04x}")),
        }
    }
    if !r.is_empty() {
        return malformed("operators remaining after end of function");
    }
    Ok(Body {
        ty,
        locals,
        ops: v.ops,
        targets: v.targets,
        max_height: v.max as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Module with one function `() -> i32` whose body is `code`.
    fn module(code: &[u8]) -> Vec<u8> {
        let mut body = vec![0u8]; // no locals
        body.extend_from_slice(code);
        let mut m = b"\0asm\x01\0\0\0".to_vec();
        m.extend([1, 5, 1, 0x60, 0, 1, 0x7f]); // type () -> i32
        m.extend([3, 2, 1, 0]); // func 0 : type 0
        let mut code_sec = vec![1, body.len() as u8];
        code_sec.extend(body);
        m.push(10);
        m.push(code_sec.len() as u8);
        m.extend(code_sec);
        m
    }

    #[test]
    fn accepts_well_typed_and_rejects_ill_typed() {
        assert!(Module::new(&module(&[0x41, 1, 0x0b])).is_ok());
        // i64 where i32 is expected.
        assert!(matches!(
            Module::new(&module(&[0x42, 1, 0x0b])),
            Err(Error::Invalid(_))
        ));
        // Empty stack.
        assert!(Module::new(&module(&[0x0b])).is_err());
        // Unreachable makes the stack polymorphic.
        assert!(Module::new(&module(&[0x00, 0x6a, 0x0b])).is_ok());
        // Branch to a label that does not exist.
        assert!(Module::new(&module(&[0x41, 1, 0x0c, 1, 0x0b])).is_err());
        // Illegal opcode.
        assert!(matches!(
            Module::new(&module(&[0xc0, 0x0b])),
            Err(Error::Malformed(_))
        ));
    }

    #[test]
    fn branches_are_resolved() {
        // block (result i32) i32.const 7 br 0 end
        let m = Module::new(&module(&[0x02, 0x7f, 0x41, 7, 0x0c, 0, 0x0b, 0x0b])).unwrap();
        let ops = &m.bodies[0].ops;
        assert_eq!(
            ops[1],
            Op::Br(Target {
                pc: 2,
                height: 0,
                arity: 1
            })
        );
        assert_eq!(ops[2], Op::Return);
    }
}
