//! End-to-end test of the `pyfun lsp` server: spawn the real binary, speak
//! `Content-Length`-framed JSON-RPC over its stdio, and assert on the responses.
//! This exercises the framing and run loop that the in-crate unit tests
//! (`src/lsp/`) deliberately skip by calling `Server::handle` directly.

use std::io::{Read, Write};
use std::process::{Command, Stdio};

use pyfun::lsp::json::{self, Json};

/// Frame a JSON-RPC body the way LSP requires.
fn frame(body: &str) -> String {
    format!("Content-Length: {}\r\n\r\n{body}", body.len())
}

/// Split a stream of framed messages into their JSON bodies.
fn unframe(mut stream: &[u8]) -> Vec<Json> {
    let mut messages = Vec::new();
    while let Some(header_end) = find(stream, b"\r\n\r\n") {
        let header = std::str::from_utf8(&stream[..header_end]).unwrap();
        let len: usize = header
            .lines()
            .find_map(|l| l.strip_prefix("Content-Length:"))
            .and_then(|n| n.trim().parse().ok())
            .expect("Content-Length header");
        let body_start = header_end + 4;
        let body = &stream[body_start..body_start + len];
        messages.push(json::parse(std::str::from_utf8(body).unwrap()).unwrap());
        stream = &stream[body_start + len..];
    }
    messages
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[test]
fn serves_initialize_diagnostics_and_hover() {
    let exe = env!("CARGO_BIN_EXE_pyfun");
    let mut child = Command::new(exe)
        .arg("lsp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn `pyfun lsp`");

    // `add` is curried; hovering its name should show the inferred signature.
    let source = "let add a b = a + b";
    let session = [
        frame(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#),
        frame(r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#),
        frame(&format!(
            r#"{{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{{"textDocument":{{"uri":"file:///t.pyfun","text":"{source}"}}}}}}"#
        )),
        // Hover over `add` — line 0, character 4.
        frame(
            r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{"textDocument":{"uri":"file:///t.pyfun"},"position":{"line":0,"character":4}}}"#,
        ),
        frame(r#"{"jsonrpc":"2.0","id":3,"method":"shutdown"}"#),
        frame(r#"{"jsonrpc":"2.0","method":"exit"}"#),
    ]
    .concat();

    child
        .stdin
        .take()
        .unwrap()
        .write_all(session.as_bytes())
        .unwrap();

    let mut out = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut out).unwrap();
    child.wait().unwrap();

    let messages = unframe(&out);

    // initialize → capabilities with hover support.
    let init = messages
        .iter()
        .find(|m| m.get("id").and_then(Json::as_i64) == Some(1))
        .expect("initialize response");
    assert_eq!(
        init.get("result")
            .unwrap()
            .get("capabilities")
            .unwrap()
            .get("hoverProvider")
            .unwrap(),
        &Json::Bool(true)
    );

    // didOpen → a publishDiagnostics notification (no errors here).
    let diag = messages
        .iter()
        .find(|m| m.get("method").and_then(Json::as_str) == Some("textDocument/publishDiagnostics"))
        .expect("publishDiagnostics notification");
    assert_eq!(
        diag.get("params")
            .unwrap()
            .get("diagnostics")
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        0
    );

    // hover → the inferred curried signature (defaults `num` to `int`).
    let hover = messages
        .iter()
        .find(|m| m.get("id").and_then(Json::as_i64) == Some(2))
        .expect("hover response");
    let value = hover
        .get("result")
        .unwrap()
        .get("contents")
        .unwrap()
        .get("value")
        .unwrap()
        .as_str()
        .unwrap();
    // `add` is unit- and num-polymorphic: `int<'a> -> int<'a> -> int<'a>`
    // (an unresolved `num` defaults to `int`; `'a` is the shared unit variable).
    assert!(
        value.contains("int<'a> -> int<'a> -> int<'a>"),
        "hover value: {value:?}"
    );

    // shutdown → null result.
    let shutdown = messages
        .iter()
        .find(|m| m.get("id").and_then(Json::as_i64) == Some(3))
        .expect("shutdown response");
    assert_eq!(shutdown.get("result").unwrap(), &Json::Null);
}

#[test]
fn publishes_diagnostics_for_a_type_error() {
    let exe = env!("CARGO_BIN_EXE_pyfun");
    let mut child = Command::new(exe)
        .arg("lsp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn `pyfun lsp`");

    let session = [
        frame(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#),
        frame(
            r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///bad.pyfun","text":"let r = 1 + true"}}}"#,
        ),
        frame(r#"{"jsonrpc":"2.0","method":"exit"}"#),
    ]
    .concat();

    child
        .stdin
        .take()
        .unwrap()
        .write_all(session.as_bytes())
        .unwrap();
    let mut out = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut out).unwrap();
    child.wait().unwrap();

    let messages = unframe(&out);
    let diag = messages
        .iter()
        .find(|m| m.get("method").and_then(Json::as_str) == Some("textDocument/publishDiagnostics"))
        .expect("publishDiagnostics notification");
    let diags = diag
        .get("params")
        .unwrap()
        .get("diagnostics")
        .unwrap()
        .as_array()
        .unwrap();
    assert_eq!(diags.len(), 1);
    let message = diags[0].get("message").unwrap().as_str().unwrap();
    assert!(!message.is_empty());
}

#[test]
fn serves_go_to_definition_and_completion() {
    let exe = env!("CARGO_BIN_EXE_pyfun");
    let mut child = Command::new(exe)
        .arg("lsp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn `pyfun lsp`");

    // `one` defined on line 0, referenced on line 1 (character 10).
    let session = [
        frame(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#),
        frame(
            r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///t.pyfun","text":"let one = 1\nlet two = one"}}}"#,
        ),
        frame(
            r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/definition","params":{"textDocument":{"uri":"file:///t.pyfun"},"position":{"line":1,"character":11}}}"#,
        ),
        frame(
            r#"{"jsonrpc":"2.0","id":3,"method":"textDocument/completion","params":{"textDocument":{"uri":"file:///t.pyfun"},"position":{"line":1,"character":0}}}"#,
        ),
        frame(r#"{"jsonrpc":"2.0","method":"exit"}"#),
    ]
    .concat();

    child
        .stdin
        .take()
        .unwrap()
        .write_all(session.as_bytes())
        .unwrap();
    let mut out = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut out).unwrap();
    child.wait().unwrap();

    let messages = unframe(&out);

    // initialize advertises the new providers.
    let caps = messages
        .iter()
        .find(|m| m.get("id").and_then(Json::as_i64) == Some(1))
        .unwrap()
        .get("result")
        .unwrap()
        .get("capabilities")
        .unwrap();
    assert_eq!(caps.get("definitionProvider").unwrap(), &Json::Bool(true));
    assert!(caps.get("completionProvider").is_some());

    // definition → jumps to line 0, character 4 (the name span of `one`).
    let def = messages
        .iter()
        .find(|m| m.get("id").and_then(Json::as_i64) == Some(2))
        .expect("definition response");
    let start = def
        .get("result")
        .unwrap()
        .get("range")
        .unwrap()
        .get("start")
        .unwrap();
    assert_eq!(start.get("line").unwrap().as_i64(), Some(0));
    assert_eq!(start.get("character").unwrap().as_i64(), Some(4));

    // completion → includes the user symbol `one` and the prelude.
    let comp = messages
        .iter()
        .find(|m| m.get("id").and_then(Json::as_i64) == Some(3))
        .expect("completion response");
    let labels: Vec<&str> = comp
        .get("result")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|i| i.get("label").and_then(Json::as_str))
        .collect();
    assert!(labels.contains(&"one"), "labels: {labels:?}");
    assert!(labels.contains(&"print"));
}

#[test]
fn go_to_definition_resolves_a_parameter() {
    let exe = env!("CARGO_BIN_EXE_pyfun");
    let mut child = Command::new(exe)
        .arg("lsp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn `pyfun lsp`");

    // `let add a b = a + b` — the reference to `a` (character 14) jumps to the
    // parameter `a` (character 8). This is the locals slice: go-to-def into a
    // parameter, not just a module symbol.
    let session = [
        frame(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#),
        frame(
            r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///p.pyfun","text":"let add a b = a + b"}}}"#,
        ),
        frame(
            r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/definition","params":{"textDocument":{"uri":"file:///p.pyfun"},"position":{"line":0,"character":14}}}"#,
        ),
        frame(r#"{"jsonrpc":"2.0","method":"exit"}"#),
    ]
    .concat();

    child
        .stdin
        .take()
        .unwrap()
        .write_all(session.as_bytes())
        .unwrap();
    let mut out = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut out).unwrap();
    child.wait().unwrap();

    let def = unframe(&out)
        .into_iter()
        .find(|m| m.get("id").and_then(Json::as_i64) == Some(2))
        .expect("definition response");
    let start = def
        .get("result")
        .unwrap()
        .get("range")
        .unwrap()
        .get("start")
        .unwrap();
    assert_eq!(start.get("line").unwrap().as_i64(), Some(0));
    assert_eq!(start.get("character").unwrap().as_i64(), Some(8));
}

#[test]
fn serves_find_references() {
    let exe = env!("CARGO_BIN_EXE_pyfun");
    let mut child = Command::new(exe)
        .arg("lsp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn `pyfun lsp`");

    // `one` is defined on line 0 and used twice on line 1. Find-references from
    // the definition name (line 0, char 4) returns 2 uses + the declaration.
    let session = [
        frame(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#),
        frame(
            r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///r.pyfun","text":"let one = 1\nlet two = one + one"}}}"#,
        ),
        frame(
            r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/references","params":{"textDocument":{"uri":"file:///r.pyfun"},"position":{"line":0,"character":4},"context":{"includeDeclaration":true}}}"#,
        ),
        frame(r#"{"jsonrpc":"2.0","method":"exit"}"#),
    ]
    .concat();

    child
        .stdin
        .take()
        .unwrap()
        .write_all(session.as_bytes())
        .unwrap();
    let mut out = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut out).unwrap();
    child.wait().unwrap();

    let refs = unframe(&out)
        .into_iter()
        .find(|m| m.get("id").and_then(Json::as_i64) == Some(2))
        .expect("references response");
    let locations = refs.get("result").unwrap().as_array().unwrap();
    assert_eq!(locations.len(), 3, "locations: {locations:?}");
    // Capabilities advertised the provider.
    assert!(
        locations
            .iter()
            .all(|l| l.get("uri").is_some() && l.get("range").is_some())
    );
}

#[test]
fn serves_resilient_analysis_on_a_half_typed_file() {
    let exe = env!("CARGO_BIN_EXE_pyfun");
    let mut child = Command::new(exe)
        .arg("lsp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn `pyfun lsp`");

    // The middle `let bad =` is broken; `good` and `also` still parse, so the
    // server reports one syntax diagnostic *and* hover still works on `good`.
    let session = [
        frame(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#),
        frame(
            r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///p.pyfun","text":"let good = 1\nlet bad =\nlet also = 2"}}}"#,
        ),
        frame(
            r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{"textDocument":{"uri":"file:///p.pyfun"},"position":{"line":0,"character":4}}}"#,
        ),
        frame(r#"{"jsonrpc":"2.0","method":"exit"}"#),
    ]
    .concat();

    child
        .stdin
        .take()
        .unwrap()
        .write_all(session.as_bytes())
        .unwrap();
    let mut out = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut out).unwrap();
    child.wait().unwrap();

    let messages = unframe(&out);

    // A syntax diagnostic is still published despite the broken middle item.
    let diag = messages
        .iter()
        .find(|m| m.get("method").and_then(Json::as_str) == Some("textDocument/publishDiagnostics"))
        .expect("publishDiagnostics notification");
    assert_eq!(
        diag.get("params")
            .unwrap()
            .get("diagnostics")
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // Hover over `good` still resolves a type from the recovered items.
    let hover = messages
        .iter()
        .find(|m| m.get("id").and_then(Json::as_i64) == Some(2))
        .expect("hover response");
    let value = hover
        .get("result")
        .unwrap()
        .get("contents")
        .unwrap()
        .get("value")
        .unwrap()
        .as_str()
        .unwrap();
    assert!(value.contains("int"), "hover on partial file: {value:?}");
}

#[test]
fn serves_rename() {
    let exe = env!("CARGO_BIN_EXE_pyfun");
    let mut child = Command::new(exe)
        .arg("lsp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn `pyfun lsp`");

    let uri = "file:///rn.pyfun";
    // Rename `one` (definition name, line 0 char 4) to `uno`: declaration + 2 uses.
    let session = [
        frame(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#),
        frame(
            r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///rn.pyfun","text":"let one = 1\nlet two = one + one"}}}"#,
        ),
        frame(
            r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/rename","params":{"textDocument":{"uri":"file:///rn.pyfun"},"position":{"line":0,"character":4},"newName":"uno"}}"#,
        ),
        frame(r#"{"jsonrpc":"2.0","method":"exit"}"#),
    ]
    .concat();

    child
        .stdin
        .take()
        .unwrap()
        .write_all(session.as_bytes())
        .unwrap();
    let mut out = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut out).unwrap();
    child.wait().unwrap();

    let rename = unframe(&out)
        .into_iter()
        .find(|m| m.get("id").and_then(Json::as_i64) == Some(2))
        .expect("rename response");
    let edits = rename
        .get("result")
        .unwrap()
        .get("changes")
        .unwrap()
        .get(uri)
        .unwrap()
        .as_array()
        .unwrap();
    assert_eq!(edits.len(), 3, "edits: {edits:?}");
    assert!(
        edits
            .iter()
            .all(|e| e.get("newText").and_then(Json::as_str) == Some("uno"))
    );
}
