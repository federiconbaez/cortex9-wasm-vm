//! Binary decoder for WebAssembly 1.0 modules. Function bodies are only
//! split off here; their instructions are decoded by the validator.

use crate::error::{malformed, Error};
use crate::types::{FuncType, GlobalType, Limits, ValType, Value};

/// Byte reader with strict LEB128 rules.
#[derive(Debug, Clone)]
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    /// Over `buf`.
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }
    /// Current offset.
    pub fn pos(&self) -> usize {
        self.pos
    }
    /// Whether everything was consumed.
    pub fn is_empty(&self) -> bool {
        self.pos >= self.buf.len()
    }
    /// One byte.
    pub fn byte(&mut self) -> Result<u8, Error> {
        let b = *self
            .buf
            .get(self.pos)
            .ok_or_else(|| Error::Malformed("unexpected end".into()))?;
        self.pos += 1;
        Ok(b)
    }
    /// `n` bytes.
    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|e| *e <= self.buf.len())
            .ok_or_else(|| Error::Malformed("unexpected end".into()))?;
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn leb(&mut self, bits: u32, signed: bool) -> Result<u64, Error> {
        let max_bytes = bits.div_ceil(7);
        let mut result: u64 = 0;
        let mut shift = 0u32;
        for i in 0..max_bytes {
            let b = self.byte()?;
            let last = i == max_bytes - 1;
            if last {
                if b & 0x80 != 0 {
                    return malformed("integer representation too long");
                }
                // Bits of the final byte beyond the target width must be
                // zero (unsigned) or copies of the sign bit (signed).
                let used = bits - shift;
                let rest = (b & 0x7f) >> used;
                let ok = if signed {
                    let sign = (b >> (used - 1)) & 1;
                    let mask = (0x7f >> used) as u8;
                    rest == if sign == 1 { mask } else { 0 }
                } else {
                    rest == 0
                };
                if !ok {
                    return malformed("integer too large");
                }
            }
            result |= ((b & 0x7f) as u64) << shift;
            shift += 7;
            if b & 0x80 == 0 {
                if signed && shift < 64 && b & 0x40 != 0 {
                    result |= !0u64 << shift;
                }
                return Ok(result);
            }
        }
        unreachable!("loop returns on the last byte")
    }

    /// Unsigned 32-bit LEB128.
    pub fn u32(&mut self) -> Result<u32, Error> {
        Ok(self.leb(32, false)? as u32)
    }
    /// Signed 32-bit LEB128.
    pub fn s32(&mut self) -> Result<i32, Error> {
        Ok(self.leb(32, true)? as i32)
    }
    /// Signed 64-bit LEB128.
    pub fn s64(&mut self) -> Result<i64, Error> {
        Ok(self.leb(64, true)? as i64)
    }
    /// Little-endian `u32` (f32 bits).
    pub fn f32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.bytes(4)?.try_into().expect("4")))
    }
    /// Little-endian `u64` (f64 bits).
    pub fn f64(&mut self) -> Result<u64, Error> {
        Ok(u64::from_le_bytes(self.bytes(8)?.try_into().expect("8")))
    }
    /// UTF-8 name.
    pub fn name(&mut self) -> Result<String, Error> {
        let n = self.u32()? as usize;
        let b = self.bytes(n)?;
        std::str::from_utf8(b)
            .map(str::to_string)
            .map_err(|_| Error::Malformed("invalid UTF-8 encoding".into()))
    }
    /// Value type.
    pub fn valtype(&mut self) -> Result<ValType, Error> {
        let b = self.byte()?;
        ValType::from_byte(b).ok_or_else(|| Error::Malformed("invalid value type".into()))
    }
    fn limits(&mut self) -> Result<Limits, Error> {
        match self.byte()? {
            0 => Ok(Limits {
                min: self.u32()?,
                max: None,
            }),
            1 => Ok(Limits {
                min: self.u32()?,
                max: Some(self.u32()?),
            }),
            _ => malformed("integer too large"),
        }
    }
    fn table_type(&mut self) -> Result<Limits, Error> {
        if self.byte()? != 0x70 {
            return malformed("malformed element type");
        }
        self.limits()
    }
    fn global_type(&mut self) -> Result<GlobalType, Error> {
        let ty = self.valtype()?;
        let mutable = match self.byte()? {
            0 => false,
            1 => true,
            _ => return malformed("invalid mutability"),
        };
        Ok(GlobalType { ty, mutable })
    }
    fn vec_len(&mut self) -> Result<usize, Error> {
        let n = self.u32()? as usize;
        // Every element takes at least one byte.
        if n > self.buf.len() - self.pos {
            return malformed("unexpected end");
        }
        Ok(n)
    }
}

/// A constant expression as decoded (validated later).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConstExpr {
    /// `*.const`.
    Const(Value),
    /// `global.get`.
    Global(u32),
    /// Anything else (invalid in a constant expression), or more than one
    /// instruction.
    NotConst,
}

/// What an import provides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportKind {
    /// Function with this type index.
    Func(u32),
    /// Table.
    Table(Limits),
    /// Memory.
    Memory(Limits),
    /// Global.
    Global(GlobalType),
}

/// One import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    /// Module name.
    pub module: String,
    /// Field name.
    pub name: String,
    /// Kind and type.
    pub kind: ImportKind,
}

/// Export kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportKind {
    /// Function.
    Func,
    /// Table.
    Table,
    /// Memory.
    Memory,
    /// Global.
    Global,
}

/// One export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Export {
    /// Name.
    pub name: String,
    /// Kind.
    pub kind: ExportKind,
    /// Index in that kind's index space.
    pub index: u32,
}

/// Element segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Elem {
    /// Table index (must be 0).
    pub table: u32,
    /// Offset.
    pub offset: ConstExpr,
    /// Function indices.
    pub funcs: Vec<u32>,
}

/// Data segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Data {
    /// Memory index (must be 0).
    pub memory: u32,
    /// Offset.
    pub offset: ConstExpr,
    /// Bytes.
    pub bytes: Vec<u8>,
}

/// Function body before validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawBody {
    /// Declared locals (not including parameters).
    pub locals: Vec<ValType>,
    /// Instruction bytes, ending with `end`.
    pub code: Vec<u8>,
}

/// A decoded (not yet validated) module.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RawModule {
    /// Type section.
    pub types: Vec<FuncType>,
    /// Imports.
    pub imports: Vec<Import>,
    /// Type index of each defined function.
    pub funcs: Vec<u32>,
    /// Defined tables.
    pub tables: Vec<Limits>,
    /// Defined memories.
    pub memories: Vec<Limits>,
    /// Defined globals.
    pub globals: Vec<(GlobalType, ConstExpr)>,
    /// Exports.
    pub exports: Vec<Export>,
    /// Start function.
    pub start: Option<u32>,
    /// Element segments.
    pub elems: Vec<Elem>,
    /// Data segments.
    pub data: Vec<Data>,
    /// Bodies of defined functions.
    pub bodies: Vec<RawBody>,
}

fn const_expr(r: &mut Reader) -> Result<ConstExpr, Error> {
    let e = match r.byte()? {
        0x41 => ConstExpr::Const(Value::I32(r.s32()?)),
        0x42 => ConstExpr::Const(Value::I64(r.s64()?)),
        0x43 => ConstExpr::Const(Value::F32(r.f32()?)),
        0x44 => ConstExpr::Const(Value::F64(r.f64()?)),
        0x23 => ConstExpr::Global(r.u32()?),
        0x0b => return Ok(ConstExpr::NotConst),
        _ => {
            // Not a constant instruction: skip to the end of the
            // expression is not possible without a full decoder, and the
            // module is rejected either way.
            return skip_to_end(r, ConstExpr::NotConst);
        }
    };
    match r.byte()? {
        0x0b => Ok(e),
        _ => skip_to_end(r, ConstExpr::NotConst),
    }
}

/// Consume bytes up to and including the next `0x0b` (best effort, used
/// only for expressions that are invalid anyway).
fn skip_to_end(r: &mut Reader, e: ConstExpr) -> Result<ConstExpr, Error> {
    while r.byte()? != 0x0b {}
    Ok(e)
}

const MAGIC: &[u8] = b"\0asm";

/// Decode a module binary.
pub fn decode(bin: &[u8]) -> Result<RawModule, Error> {
    let mut r = Reader::new(bin);
    if r.bytes(4)? != MAGIC {
        return malformed("magic header not detected");
    }
    if r.bytes(4)? != [1, 0, 0, 0] {
        return malformed("unknown binary version");
    }
    let mut m = RawModule::default();
    let mut last_id = 0u8;
    let mut func_count: Option<usize> = None;
    let mut code_seen = false;
    while !r.is_empty() {
        let id = r.byte()?;
        let size = r.u32()? as usize;
        let body = r.bytes(size)?;
        let mut s = Reader::new(body);
        if id != 0 {
            if id > 11 {
                return malformed("invalid section id");
            }
            if id <= last_id {
                return malformed("unexpected content after last section");
            }
            last_id = id;
        }
        match id {
            0 => {
                s.name()?;
            }
            1 => {
                for _ in 0..s.vec_len()? {
                    if s.byte()? != 0x60 {
                        return malformed("malformed function type");
                    }
                    let mut params = Vec::new();
                    for _ in 0..s.vec_len()? {
                        params.push(s.valtype()?);
                    }
                    let mut results = Vec::new();
                    for _ in 0..s.vec_len()? {
                        results.push(s.valtype()?);
                    }
                    m.types.push(FuncType { params, results });
                }
            }
            2 => {
                for _ in 0..s.vec_len()? {
                    let module = s.name()?;
                    let name = s.name()?;
                    let kind = match s.byte()? {
                        0 => ImportKind::Func(s.u32()?),
                        1 => ImportKind::Table(s.table_type()?),
                        2 => ImportKind::Memory(s.limits()?),
                        3 => ImportKind::Global(s.global_type()?),
                        _ => return malformed("malformed import kind"),
                    };
                    m.imports.push(Import { module, name, kind });
                }
            }
            3 => {
                for _ in 0..s.vec_len()? {
                    m.funcs.push(s.u32()?);
                }
                func_count = Some(m.funcs.len());
            }
            4 => {
                for _ in 0..s.vec_len()? {
                    m.tables.push(s.table_type()?);
                }
            }
            5 => {
                for _ in 0..s.vec_len()? {
                    m.memories.push(s.limits()?);
                }
            }
            6 => {
                for _ in 0..s.vec_len()? {
                    let t = s.global_type()?;
                    m.globals.push((t, const_expr(&mut s)?));
                }
            }
            7 => {
                for _ in 0..s.vec_len()? {
                    let name = s.name()?;
                    let kind = match s.byte()? {
                        0 => ExportKind::Func,
                        1 => ExportKind::Table,
                        2 => ExportKind::Memory,
                        3 => ExportKind::Global,
                        _ => return malformed("malformed export kind"),
                    };
                    m.exports.push(Export {
                        name,
                        kind,
                        index: s.u32()?,
                    });
                }
            }
            8 => m.start = Some(s.u32()?),
            9 => {
                for _ in 0..s.vec_len()? {
                    let table = s.u32()?;
                    let offset = const_expr(&mut s)?;
                    let mut funcs = Vec::new();
                    for _ in 0..s.vec_len()? {
                        funcs.push(s.u32()?);
                    }
                    m.elems.push(Elem {
                        table,
                        offset,
                        funcs,
                    });
                }
            }
            10 => {
                code_seen = true;
                let n = s.vec_len()?;
                if n != func_count.unwrap_or(0) {
                    return malformed("function and code section have inconsistent lengths");
                }
                for _ in 0..n {
                    let size = s.u32()? as usize;
                    let mut b = Reader::new(s.bytes(size)?);
                    let mut locals = Vec::new();
                    let mut total: u64 = 0;
                    for _ in 0..b.vec_len()? {
                        let c = b.u32()?;
                        let t = b.valtype()?;
                        total += c as u64;
                        if total > u32::MAX as u64 {
                            return malformed("too many locals");
                        }
                        if total > 50_000 {
                            // Implementation limit, far above anything real.
                            return malformed("too many locals");
                        }
                        locals.extend(std::iter::repeat(t).take(c as usize));
                    }
                    let code = b.buf[b.pos..].to_vec();
                    if code.last() != Some(&0x0b) {
                        return malformed("END opcode expected");
                    }
                    m.bodies.push(RawBody { locals, code });
                }
            }
            11 => {
                for _ in 0..s.vec_len()? {
                    let memory = s.u32()?;
                    let offset = const_expr(&mut s)?;
                    let n = s.vec_len()?;
                    let bytes = s.bytes(n)?.to_vec();
                    m.data.push(Data {
                        memory,
                        offset,
                        bytes,
                    });
                }
            }
            _ => unreachable!(),
        }
        if id != 0 && !s.is_empty() {
            return malformed("section size mismatch");
        }
    }
    if func_count.unwrap_or(0) > 0 && !code_seen {
        return malformed("function and code section have inconsistent lengths");
    }
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leb_u32(b: &[u8]) -> Result<u32, Error> {
        Reader::new(b).u32()
    }
    fn leb_s32(b: &[u8]) -> Result<i32, Error> {
        Reader::new(b).s32()
    }
    fn leb_s64(b: &[u8]) -> Result<i64, Error> {
        Reader::new(b).s64()
    }

    #[test]
    fn leb128_limits() {
        assert_eq!(leb_u32(&[0xe5, 0x8e, 0x26]), Ok(624_485));
        assert_eq!(leb_u32(&[0xff, 0xff, 0xff, 0xff, 0x0f]), Ok(u32::MAX));
        assert!(
            leb_u32(&[0xff, 0xff, 0xff, 0xff, 0x1f]).is_err(),
            "too large"
        );
        assert!(
            leb_u32(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x00]).is_err(),
            "too long"
        );
        assert_eq!(
            leb_u32(&[0x80, 0x80, 0x80, 0x80, 0x00]),
            Ok(0),
            "padded zero ok"
        );
        assert_eq!(leb_s32(&[0x7f]), Ok(-1));
        assert_eq!(leb_s32(&[0x80, 0x80, 0x80, 0x80, 0x78]), Ok(i32::MIN));
        assert!(leb_s32(&[0x80, 0x80, 0x80, 0x80, 0x70]).is_err());
        assert_eq!(leb_s32(&[0xff, 0xff, 0xff, 0xff, 0x7f]), Ok(-1));
        assert_eq!(
            leb_s64(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x7f]),
            Ok(i64::MIN)
        );
        assert!(leb_s64(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x01]).is_err());
        assert!(leb_u32(&[0x80]).is_err(), "unexpected end");
    }

    #[test]
    fn rejects_bad_headers_and_section_order() {
        assert!(decode(b"\0asx\x01\0\0\0").is_err());
        assert!(decode(b"\0asm\x02\0\0\0").is_err());
        assert_eq!(decode(b"\0asm\x01\0\0\0"), Ok(RawModule::default()));
        // Two type sections.
        assert!(decode(b"\0asm\x01\0\0\0\x01\x01\x00\x01\x01\x00").is_err());
        // Section shorter than its contents.
        assert!(decode(b"\0asm\x01\0\0\0\x01\x02\x00\x00").is_err());
    }
}
