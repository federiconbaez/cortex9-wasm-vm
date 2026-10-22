//! Rough interpreter speed on three small programs, compared with the
//! same algorithm in native Rust. One run each; not a rigorous benchmark.
//!
//! Usage: cargo run --release --example bench

use cortex9_wasm_vm::{Imports, Module, Store, Value};
use std::time::Instant;

const WAT: &str = r#"
(module
  (memory 16)
  (func $fib (export "fib") (param i32) (result i32)
    (if (result i32) (i32.lt_u (local.get 0) (i32.const 2))
      (then (local.get 0))
      (else (i32.add (call $fib (i32.sub (local.get 0) (i32.const 1)))
                     (call $fib (i32.sub (local.get 0) (i32.const 2)))))))
  (func (export "sum") (param i32) (result i64) (local i64)
    (block $done
      (loop $l
        (br_if $done (i32.eqz (local.get 0)))
        (local.set 1 (i64.add (local.get 1) (i64.extend_i32_u (local.get 0))))
        (local.set 0 (i32.sub (local.get 0) (i32.const 1)))
        (br $l)))
    (local.get 1))
  ;; Sieve of Eratosthenes over bytes [0, n) of memory; returns the prime count.
  (func (export "sieve") (param $n i32) (result i32) (local $i i32) (local $j i32) (local $c i32)
    (local.set $i (i32.const 2))
    (block $out (loop $outer
      (br_if $out (i32.ge_u (local.get $i) (local.get $n)))
      (if (i32.eqz (i32.load8_u (local.get $i)))
        (then
          (local.set $c (i32.add (local.get $c) (i32.const 1)))
          ;; i*i must not wrap: only mark from primes below 46341.
          (local.set $j (select (i32.mul (local.get $i) (local.get $i)) (local.get $n)
                                (i32.lt_u (local.get $i) (i32.const 46341))))
          (block $done (loop $inner
            (br_if $done (i32.ge_u (local.get $j) (local.get $n)))
            (i32.store8 (local.get $j) (i32.const 1))
            (local.set $j (i32.add (local.get $j) (local.get $i)))
            (br $inner)))))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br $outer)))
    (local.get $c)))
"#;

fn fib(n: u32) -> u32 {
    if n < 2 {
        n
    } else {
        fib(n - 1) + fib(n - 2)
    }
}

fn sieve(n: usize) -> u32 {
    let mut m = vec![0u8; n];
    let mut c = 0;
    for i in 2..n {
        if m[i] == 0 {
            c += 1;
            let mut j = if i < 46341 { i * i } else { n };
            while j < n {
                m[j] = 1;
                j += i;
            }
        }
    }
    c
}

fn time<T>(f: impl FnOnce() -> T) -> (T, f64) {
    let t = Instant::now();
    let r = f();
    (r, t.elapsed().as_secs_f64())
}

fn main() {
    let buf = wast::parser::ParseBuffer::new(WAT).expect("lex");
    let mut wat: wast::Wat = wast::parser::parse(&buf).expect("parse");
    let bin = wat.encode().expect("encode");
    let m = Module::new(&bin).expect("valid");
    let mut s = Store::new();
    let i = s.instantiate(&m, &Imports::new()).expect("instantiate");
    let cases: [(&str, Value, f64); 3] = [
        ("fib", Value::I32(30), 0.0),
        ("sum", Value::I32(20_000_000), 0.0),
        ("sieve", Value::I32(65_536 * 16), 0.0),
    ];
    for (name, arg, _) in cases {
        let (got, dt) = time(|| s.invoke_export(i, name, &[arg]).expect("run"));
        let (native, ndt) = match (name, arg) {
            ("fib", Value::I32(n)) => {
                time(|| Value::I32(std::hint::black_box(fib(n as u32)) as i32))
            }
            ("sum", Value::I32(n)) => time(|| {
                let mut acc = 0i64;
                for k in 1..=n as i64 {
                    acc = std::hint::black_box(acc + k);
                }
                Value::I64(acc)
            }),
            (_, Value::I32(n)) => {
                time(|| Value::I32(std::hint::black_box(sieve(n as usize)) as i32))
            }
            _ => unreachable!(),
        };
        assert_eq!(got, vec![native], "{name}");
        println!(
            "{name:6} {arg:?} -> {:?}: interpreter {:.3} s, native {:.4} s ({:.0}x)",
            got[0],
            dt,
            ndt,
            dt / ndt.max(1e-9)
        );
    }
}
