//! Robustness: random and mutated binaries never panic the decoder,
//! validator, linker or interpreter. A stable-Rust complement to the
//! cargo-fuzz targets in `fuzz/`.

use cortex9_wasm_vm::fuzzing;
use std::path::Path;
use wast::parser::{self, ParseBuffer};
use wast::{QuoteWat, Wast, WastDirective};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Valid module binaries from a few spec files.
fn corpus() -> Vec<Vec<u8>> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/spec");
    let mut out = Vec::new();
    for f in [
        "br_table.wast",
        "call_indirect.wast",
        "memory_grow.wast",
        "loop.wast",
        "i64.wast",
        "conversions.wast",
        "fac.wast",
        "float_exprs.wast",
        "int_exprs.wast",
        "memory_trap.wast",
        "traps.wast",
        "unwind.wast",
        "func.wast",
    ] {
        // Only the modules are used, so legacy assertion names can be
        // mapped to the current one.
        let text = std::fs::read_to_string(dir.join(f))
            .unwrap()
            .replace("assert_return_canonical_nan", "assert_return")
            .replace("assert_return_arithmetic_nan", "assert_return");
        let buf = ParseBuffer::new(&text).unwrap();
        let wast: Wast = parser::parse(&buf).unwrap();
        for d in wast.directives {
            if let WastDirective::Wat(mut w @ QuoteWat::Wat(_)) = d {
                if let Ok(b) = w.encode() {
                    out.push(b);
                }
            }
        }
    }
    out
}

fn iterations() -> usize {
    if cfg!(debug_assertions) {
        3_000
    } else {
        60_000
    }
}

#[test]
fn mutated_spec_modules_never_panic() {
    let corpus = corpus();
    assert!(corpus.len() > 20);
    let mut r = Rng(1);
    let (mut valid, mut ran, mut calls) = (0, 0, 0);
    for _ in 0..iterations() {
        let mut b = corpus[r.below(corpus.len())].clone();
        for _ in 0..1 + r.below(4) {
            let i = 8 + r.below(b.len().max(9) - 8);
            match r.below(4) {
                0 if i < b.len() => b[i] ^= 1 << r.below(8),
                1 if i < b.len() => b[i] = r.next() as u8,
                2 if i < b.len() => {
                    b.remove(i);
                }
                _ => b.insert(i.min(b.len()), r.next() as u8),
            }
        }
        let o = fuzzing::run(&b);
        valid += o.valid as usize;
        ran += o.instantiated as usize;
        calls += o.calls as usize;
    }
    println!(
        "{} mutants: {valid} still valid, {ran} instantiated, {calls} export calls",
        iterations()
    );
    assert!(
        valid > 0 && calls > 0,
        "mutations too destructive to test execution"
    );
}

#[test]
fn random_bytes_never_panic() {
    let mut r = Rng(2);
    for _ in 0..iterations() {
        let n = r.below(64);
        let mut b: Vec<u8> = b"\0asm\x01\0\0\0".to_vec();
        b.extend((0..n).map(|_| r.next() as u8));
        fuzzing::run(&b);
    }
}

#[test]
fn unmodified_corpus_runs() {
    let mut calls = 0;
    for b in corpus() {
        let o = fuzzing::run(&b);
        assert!(o.valid, "spec module rejected");
        calls += o.calls;
    }
    assert!(calls > 100);
}
