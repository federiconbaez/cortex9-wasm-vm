//! Write every module binary of the vendored spec tests to a directory,
//! as a seed corpus for the fuzzer.
//!
//! Usage: cargo run --example dump_corpus -- <out-dir>

use std::path::Path;
use wast::parser::{self, ParseBuffer};
use wast::{QuoteWat, Wast, WastDirective};

fn main() {
    let out = std::env::args()
        .nth(1)
        .expect("usage: dump_corpus <out-dir>");
    std::fs::create_dir_all(&out).expect("out dir");
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/spec");
    let mut n = 0;
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("spec dir")
        .map(|e| e.expect("entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "wast"))
        .collect();
    files.sort();
    for f in files {
        let text = String::from_utf8_lossy(&std::fs::read(&f).expect("read"))
            .replace("assert_return_canonical_nan", "assert_return")
            .replace("assert_return_arithmetic_nan", "assert_return");
        let mut lexer = wast::lexer::Lexer::new(&text);
        lexer.allow_confusing_unicode(true);
        let Ok(buf) = ParseBuffer::new_with_lexer(lexer) else {
            continue;
        };
        let Ok(wast) = parser::parse::<Wast>(&buf) else {
            continue;
        };
        for d in wast.directives {
            let mut m = match d {
                WastDirective::Wat(w @ QuoteWat::Wat(_)) => w,
                WastDirective::AssertInvalid { module, .. } => module,
                _ => continue,
            };
            if let Ok(b) = m.encode() {
                std::fs::write(Path::new(&out).join(format!("spec-{n:05}.wasm")), b)
                    .expect("write");
                n += 1;
            }
        }
    }
    println!("wrote {n} modules to {out}");
}
