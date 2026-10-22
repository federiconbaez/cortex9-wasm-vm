# cortex9-wasm-vm

[![CI](https://github.com/federiconbaez/cortex9-wasm-vm/actions/workflows/ci.yml/badge.svg)](https://github.com/federiconbaez/cortex9-wasm-vm/actions/workflows/ci.yml)
![License](https://img.shields.io/badge/license-Apache--2.0-blue)

Intérprete de **WebAssembly 1.0 (MVP)** en Rust, sin dependencias de runtime: decoder binario, validador, linker y máquina de pila. Pasa los **73 archivos de la suite oficial de tests de la especificación 1.0** (tag `wg-1.0`) dentro del subconjunto documentado abajo, y se fuzzea con cargo-fuzz.

**WebAssembly 1.0 (MVP)** interpreter in Rust with no runtime dependencies: binary decoder, validator, linker and stack machine. It passes **all 73 files of the official 1.0 spec test suite** (tag `wg-1.0`) within the subset documented below, and is fuzzed with cargo-fuzz.

Parte de la serie Cortex-9 / Part of the Cortex-9 series: [github.com/federiconbaez](https://github.com/federiconbaez#cortex-9).

> **ES** primero · **EN** below

---

## ES

### Diseño

- **`decode`**: secciones en orden, tamaños exactos, LEB128 estricto (largo máximo y bits sobrantes), nombres UTF-8 y conteos de funciones y código consistentes.
- **`validate`**: el algoritmo de validación de la especificación (pila de operandos con tipo desconocido después de `unreachable`/`br`, pila de control). En la misma pasada **compila** cada cuerpo a un arreglo plano de instrucciones: cada `br`/`br_if`/`br_table` ya trae su destino, la altura de pila a la que vuelve y cuántos valores conserva, así que en ejecución no hay búsqueda de etiquetas.
- **`exec`**: store con funciones, tablas, memorias y globales; linking con chequeo de tipos y límites de imports; instanciación con la semántica 1.0 (se verifican los límites de todos los segmentos antes de escribir). Una sola pila de `u64` para locales y operandos de todos los frames; las llamadas **no recursan en Rust**, así que la recursión profunda en wasm termina en la trap `call stack exhausted` y no en un stack overflow.
- **`num`**: los 123 opcodes numéricos, con los casos de punto flotante que exige la especificación: NaN aritméticos y canónicos, `min`/`max` con ±0, `nearest` con empate a par, conversiones que trapean con los límites exactos, y `abs`/`neg`/`copysign` sobre bits (preservan el payload del NaN).

### Cómo se prueba

| Test | Qué hace |
|---|---|
| `spec::spec_suite` | Corre los **73 archivos `.wast`** de `test/core` del tag `wg-1.0` de [WebAssembly/spec](https://github.com/WebAssembly/spec) (Apache-2.0, en `tests/spec/`). El crate `wast` solo parsea el formato de texto y codifica los módulos a binario; decodificar, validar, linkear y ejecutar es de este crate. |
| `spec::harness_is_not_vacuous` | El harness tiene que detectar un resultado incorrecto, una trap que falta, un NaN canónico que no lo es, un módulo inválido aceptado, un módulo mal formado aceptado y una recursión infinita. |
| `robust::*` | 60 000 módulos de la suite **mutados** (bits, bytes, inserciones, borrados) y 60 000 entradas aleatorias: nunca un panic; los que siguen siendo válidos se instancian y se llaman todos sus exports con combustible limitado. |
| `fuzz/decode_run` | cargo-fuzz (libFuzzer): bytes arbitrarios → decode → validate → instanciar con imports sustitutos → llamar cada export con combustible. Semillas: los 1 769 módulos de la suite. |
| unitarios | LEB128 (límites, largo, signo), encabezado y orden de secciones, tipado, resolución de saltos, bordes enteros y de punto flotante. |

CI en ubuntu + macOS (fmt, clippy `-D warnings`, tests en debug y release), 60 s de cargo-fuzz en nightly y build con MSRV 1.75.

### Subconjunto cubierto (documentado)

- **Directivas que cuentan**: `module`, `register`, `invoke`, `assert_return` (incluidas las formas viejas `assert_return_canonical_nan` / `assert_return_arithmetic_nan` de la 1.0), `assert_trap`, `assert_exhaustion`, `assert_invalid`, `assert_unlinkable` y `assert_malformed` sobre **módulos binarios**.
- **Excluido: 430 `assert_malformed` sobre módulos de texto** (`module quote`). Prueban el parser del formato de texto, que acá es el crate `wast`, no este intérprete. Se cuentan aparte y no se reportan como pasados.
- Para `assert_trap` se compara el mensaje. Para `assert_invalid`/`assert_malformed` alcanza con el rechazo; además se mide cuántos rechazos usan el mismo mensaje que el intérprete de referencia (ver números).
- Solo el MVP 1.0: sin multi-value, reference types, bulk memory, SIMD, threads, sign-extension ni conversiones saturadas (esos opcodes son `illegal opcode`).

### Números medidos

Máquina: VM Linux 6.12, Intel Xeon, 8 vCPU. Un hilo.

- Suite de la especificación: **18 840 directivas pasadas, 0 fallidas** en 73 archivos; 430 `assert_malformed` de texto excluidas. 1 638 de los 1 650 rechazos usan el mismo mensaje que el intérprete de referencia.
- Robustez: 60 000 mutantes, 4 791 siguen siendo válidos y se ejecutan (24 215 llamadas), sin panics.
- cargo-fuzz: **2 089 709 ejecuciones en 301 s** (~6 900/s), 2 971 bordes de cobertura, sin crashes.
- `cargo run --release --example bench` (comparado con el mismo algoritmo en Rust nativo):
  - `fib(30)` recursivo: 0,13 s (≈ 60× más lento que nativo).
  - Loop de 20 millones de iteraciones: 1,04 s (≈ 25×).
  - Criba sobre 1 MiB de memoria lineal: 0,14 s (≈ 50×).

### Límites (honestos)

- Intérprete sin JIT ni registro de registros; el costo por instrucción domina.
- Límites de implementación: 10 000 frames, 2²⁰ valores en pila, 50 000 locales por función, memoria y tablas acotadas por `Store` (configurable). Fuera de esos límites, trap o error, nunca panic.
- Sin WASI ni API de embedding estable más allá de `Store`/`Module`/`Value`.
- El formato de texto lo parsea el crate `wast` (solo en tests y ejemplos).

### Uso

```rust
use cortex9_wasm_vm::{Imports, Module, Store, Value};

let module = Module::new(&wasm_bytes)?;
let mut store = Store::new();
let inst = store.instantiate(&module, &Imports::new())?;
let out = store.invoke_export(inst, "fib", &[Value::I32(20)])?;
```

```sh
cargo test --release -- --nocapture        # suite + robustez
cargo run --release --example bench
cargo run --release --example dump_corpus -- fuzz/seeds
cd fuzz && cargo +nightly fuzz run decode_run corpus/decode_run seeds -- -max_total_time=60
```

### Licencia

Apache-2.0. Los `.wast` de `tests/spec/` son de WebAssembly/spec, Apache-2.0 (`tests/spec/LICENSE`).

---

## EN

### Design

- **`decode`**: section order, exact sizes, strict LEB128 (maximum length and unused bits), UTF-8 names, consistent function and code counts.
- **`validate`**: the spec's validation algorithm (operand stack with an unknown type after `unreachable`/`br`, control stack). The same pass **compiles** each body into a flat instruction array: every `br`/`br_if`/`br_table` carries its target, the stack height to unwind to and the number of values to keep, so execution never searches for labels.
- **`exec`**: store with functions, tables, memories and globals; linking with import type and limit checks; instantiation with 1.0 semantics (all segment bounds are checked before any write). One `u64` stack for locals and operands of all frames; calls **do not recurse in Rust**, so deep wasm recursion ends in the `call stack exhausted` trap, not a stack overflow.
- **`num`**: all 123 numeric opcodes, with the float corner cases the spec demands: arithmetic and canonical NaNs, `min`/`max` with ±0, ties-to-even `nearest`, trapping conversions with exact bounds, and bitwise `abs`/`neg`/`copysign` (NaN payloads preserved).

### How it is tested

| Test | What it does |
|---|---|
| `spec::spec_suite` | Runs the **73 `.wast` files** of `test/core` at tag `wg-1.0` of [WebAssembly/spec](https://github.com/WebAssembly/spec) (Apache-2.0, in `tests/spec/`). The `wast` crate only parses the text format and encodes modules to binary; decoding, validation, linking and execution are this crate's. |
| `spec::harness_is_not_vacuous` | The harness must catch a wrong result, a missing trap, a non-canonical NaN, an accepted invalid module, an accepted malformed module and infinite recursion. |
| `robust::*` | 60,000 **mutated** suite modules (bits, bytes, inserts, deletes) and 60,000 random inputs: never a panic; those still valid are instantiated and every export is called with limited fuel. |
| `fuzz/decode_run` | cargo-fuzz (libFuzzer): arbitrary bytes → decode → validate → instantiate with stand-in imports → call every export with fuel. Seeds: the suite's 1,769 modules. |
| unit tests | LEB128 (limits, length, sign), header and section order, typing, branch resolution, integer and float edge cases. |

CI on ubuntu + macOS (fmt, clippy `-D warnings`, debug and release tests), 60 s of cargo-fuzz on nightly, and an MSRV 1.75 build.

### Covered subset (documented)

- **Directives that count**: `module`, `register`, `invoke`, `assert_return` (including the 1.0-era `assert_return_canonical_nan` / `assert_return_arithmetic_nan`), `assert_trap`, `assert_exhaustion`, `assert_invalid`, `assert_unlinkable`, and `assert_malformed` on **binary modules**.
- **Excluded: 430 `assert_malformed` on text modules** (`module quote`). They test the text-format parser, which here is the `wast` crate, not this interpreter. They are counted separately and not reported as passed.
- `assert_trap` compares the message. For `assert_invalid`/`assert_malformed` the rejection is enough; how many rejections use the reference interpreter's message is measured too (see numbers).
- MVP 1.0 only: no multi-value, reference types, bulk memory, SIMD, threads, sign-extension or saturating conversions (those opcodes are `illegal opcode`).

### Measured numbers

Machine: Linux 6.12 VM, Intel Xeon, 8 vCPU. One thread.

- Spec suite: **18,840 directives passed, 0 failed** across 73 files; 430 text `assert_malformed` excluded. 1,638 of 1,650 rejections use the reference interpreter's message.
- Robustness: 60,000 mutants, 4,791 still valid and executed (24,215 calls), no panics.
- cargo-fuzz: **2,089,709 executions in 301 s** (~6,900/s), 2,971 coverage edges, no crashes.
- `cargo run --release --example bench` (vs the same algorithm in native Rust):
  - Recursive `fib(30)`: 0.13 s (≈ 60× slower than native).
  - 20-million-iteration loop: 1.04 s (≈ 25×).
  - Sieve over 1 MiB of linear memory: 0.14 s (≈ 50×).

### Limits (honest)

- An interpreter with no JIT or register allocation; per-instruction cost dominates.
- Implementation limits: 10,000 frames, 2²⁰ stack values, 50,000 locals per function, memory and tables capped by `Store` (configurable). Past them: a trap or an error, never a panic.
- No WASI and no stable embedding API beyond `Store`/`Module`/`Value`.
- The text format is parsed by the `wast` crate (tests and examples only).

### Usage

See the Spanish section above for the snippet and commands.

### License

Apache-2.0. The `.wast` files in `tests/spec/` come from WebAssembly/spec, Apache-2.0 (`tests/spec/LICENSE`).
