//! The official WebAssembly 1.0 spec tests (tag `wg-1.0` of
//! github.com/WebAssembly/spec, `test/core`, Apache-2.0), vendored in
//! `tests/spec/`.

mod harness;

use std::path::Path;

#[test]
fn spec_suite() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/spec");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "wast"))
        .collect();
    files.sort();
    let (mut pass, mut fail, mut skip, mut rej, mut same) = (0, 0, 0, 0, 0);
    let mut failures = Vec::new();
    for f in &files {
        let name = f.file_name().unwrap().to_string_lossy().into_owned();
        let bytes = std::fs::read(f).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        let r = harness::run(&name, &text);
        println!(
            "{name:32} pass {:6} fail {:4} skip(text) {:4}",
            r.passed,
            r.failed.len(),
            r.skipped_text
        );
        pass += r.passed;
        fail += r.failed.len();
        skip += r.skipped_text;
        rej += r.rejections;
        same += r.same_message;
        failures.extend(r.failed);
    }
    println!(
        "TOTAL files {} pass {pass} fail {fail} skip(text) {skip}",
        files.len()
    );
    println!("rejections {rej}, with the reference error message {same}");
    for f in failures.iter().take(60) {
        println!("FAIL {f}");
    }
    assert!(failures.is_empty(), "{} failures", failures.len());
}

/// The harness itself must notice wrong results, missing traps and
/// accepted invalid modules.
#[test]
fn harness_is_not_vacuous() {
    let cases = [
        r#"(module (func (export "f") (result i32) (i32.const 1)))
           (assert_return (invoke "f") (i32.const 2))"#,
        r#"(module (func (export "f") (result i32) (i32.const 1)))
           (assert_trap (invoke "f") "unreachable")"#,
        r#"(module (func (export "f") (result f32) (f32.const 1)))
           (assert_return (invoke "f") (f32.const nan:canonical))"#,
        r#"(assert_invalid (module (func (result i32) (i32.const 1))) "type mismatch")"#,
        r#"(assert_malformed (module binary "\00asm\01\00\00\00") "x")"#,
        r#"(module (func (export "f") (call 0)))
           (assert_return (invoke "f"))"#,
    ];
    for c in cases {
        let r = harness::run("neg", c);
        assert_eq!(r.failed.len(), 1, "harness accepted: {c}");
    }
}
