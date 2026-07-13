//! WASM shim exposing the Pyfun compiler to the browser playground.
//!
//! A **separate crate** so the core `pyfun` crate stays dependency-free (`GUIDE.md`):
//! only this thin binding depends on `wasm-bindgen`. It reuses the real, pure library
//! entry points — [`pyfun::analyze`] (resilient diagnostics, the same engine the LSP
//! uses) and [`pyfun::compile`] (source → readable Python) — so what the browser shows
//! is byte-for-byte what the CLI emits. Neither path touches the filesystem or spawns a
//! process, which is exactly what makes them safe to run in WebAssembly.

use wasm_bindgen::prelude::*;

/// Compile `source` and return a JSON string:
///
/// ```json
/// { "ok": bool,
///   "python": string | null,
///   "diagnostics": [ { "start": int, "end": int, "severity": "error"|"info", "message": string } ] }
/// ```
///
/// `start`/`end` are **byte** offsets into `source`. `python` is the emitted Python
/// when the program is clean, otherwise `null` with the problems in `diagnostics`.
/// JSON is assembled by hand (with correct string escaping) to avoid pulling a
/// serializer into the build.
#[wasm_bindgen]
pub fn compile(source: &str) -> String {
    // Resilient analysis first: it reports every diagnostic (with spans) over a
    // partially-parsed file, so a half-typed program still gives useful feedback.
    let analysis = pyfun::analyze(source);
    let mut diagnostics: Vec<String> = Vec::new();

    for e in &analysis.diagnostics {
        diagnostics.push(diag(e.span.start, e.span.end, "error", &e.message));
    }
    // Typed holes (`?` / `?name`) are informational, not errors.
    for h in &analysis.holes {
        let label = match &h.name {
            Some(name) => format!("hole ?{name} : {}", h.ty),
            None => format!("hole ? : {}", h.ty),
        };
        diagnostics.push(diag(h.span.start, h.span.end, "info", &label));
    }

    // Emit Python only when the analysis is clean. A residual compile/lowering error
    // (rare once analysis passes) is surfaced as one more diagnostic.
    let python =
        if analysis.parse_ok && analysis.diagnostics.is_empty() && analysis.holes.is_empty() {
            match pyfun::compile(source) {
                Ok(py) => Some(py),
                Err(e) => {
                    let span = e.span();
                    diagnostics.push(diag(span.start, span.end, "error", &e.message()));
                    None
                }
            }
        } else {
            None
        };

    let python_field = match &python {
        Some(py) => format!("\"{}\"", json_escape(py)),
        None => "null".to_string(),
    };
    format!(
        "{{\"ok\":{},\"python\":{},\"diagnostics\":[{}]}}",
        python.is_some(),
        python_field,
        diagnostics.join(",")
    )
}

fn diag(start: usize, end: usize, severity: &str, message: &str) -> String {
    format!(
        "{{\"start\":{start},\"end\":{end},\"severity\":\"{severity}\",\"message\":\"{}\"}}",
        json_escape(message)
    )
}

/// Escape a string for embedding as a JSON string body (RFC 8259).
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}
