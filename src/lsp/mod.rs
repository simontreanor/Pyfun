//! A small, dependency-free Language Server (`DESIGN.md` §9) — the "front-end-
//! first" tooling slice. It speaks LSP over stdio (JSON-RPC with `Content-Length`
//! framing) and offers exactly two features for now:
//!
//! - **Diagnostics**: the existing type/effect/unit errors ([`crate::analyze`])
//!   streamed as `textDocument/publishDiagnostics` on open/change.
//! - **Hover**: the inferred type (effects included, e.g. `string ->{io} unit`) of
//!   the narrowest expression under the cursor — the only way to *see* an inferred
//!   type, since Pyfun never writes them.
//!
//! The protocol plumbing is hand-rolled (see [`json`]) to keep the crate
//! dependency-free, the same choice we made for the lexer and parser. The
//! message-handling core ([`Server::handle`]) is pure — JSON in, JSON out — so it
//! is tested directly without spawning a process or touching stdio.

pub mod json;

use std::collections::HashMap;
use std::io::{self, BufRead, Write};

use json::{Json, int, obj, str};

/// Run the server: read framed JSON-RPC messages from stdin, dispatch them, and
/// write framed responses/notifications to stdout until an `exit` notification.
pub fn run() -> io::Result<()> {
    let stdin = io::stdin();
    let mut reader = stdin.lock();
    let stdout = io::stdout();
    let mut writer = stdout.lock();
    let mut server = Server::default();

    while let Some(text) = read_message(&mut reader)? {
        let Ok(msg) = json::parse(&text) else {
            continue; // ignore unparseable frames rather than crash the server
        };
        for out in server.handle(&msg) {
            write_message(&mut writer, &out.to_string())?;
        }
        if server.exit {
            break;
        }
    }
    Ok(())
}

/// The server state: open documents by URI, plus a flag set by `exit`.
#[derive(Default)]
pub struct Server {
    documents: HashMap<String, String>,
    pub exit: bool,
}

impl Server {
    /// Handle one incoming message, returning the messages to send back (a
    /// response, zero or more notifications, or nothing). Pure over `self` apart
    /// from the document store and the `exit` flag — the unit of testing.
    pub fn handle(&mut self, msg: &Json) -> Vec<Json> {
        let method = msg.get("method").and_then(Json::as_str);
        let id = msg.get("id").cloned();
        match method {
            Some("initialize") => vec![response(id, initialize_result())],
            Some("shutdown") => vec![response(id, Json::Null)],
            Some("exit") => {
                self.exit = true;
                vec![]
            }
            Some("initialized") => vec![],
            Some("textDocument/didOpen") => self.did_open(msg),
            Some("textDocument/didChange") => self.did_change(msg),
            Some("textDocument/didClose") => self.did_close(msg),
            Some("textDocument/hover") => {
                let result = self.hover(msg);
                vec![response(id, result)]
            }
            // Unknown *request* (has an id) → a proper error so the client isn't
            // left waiting; unknown notification → silently ignored, per spec.
            Some(_) if id.is_some() => vec![method_not_found(id)],
            _ => vec![],
        }
    }

    fn did_open(&mut self, msg: &Json) -> Vec<Json> {
        let doc = msg.get("params").and_then(|p| p.get("textDocument"));
        let (Some(uri), Some(text)) = (
            doc.and_then(|d| d.get("uri")).and_then(Json::as_str),
            doc.and_then(|d| d.get("text")).and_then(Json::as_str),
        ) else {
            return vec![];
        };
        self.documents.insert(uri.to_string(), text.to_string());
        vec![self.diagnostics_for(uri)]
    }

    fn did_change(&mut self, msg: &Json) -> Vec<Json> {
        let params = msg.get("params");
        let Some(uri) = params
            .and_then(|p| p.get("textDocument"))
            .and_then(|d| d.get("uri"))
            .and_then(Json::as_str)
        else {
            return vec![];
        };
        // Full-sync: the last content change carries the whole document.
        let Some(text) = params
            .and_then(|p| p.get("contentChanges"))
            .and_then(Json::as_array)
            .and_then(|changes| changes.last())
            .and_then(|c| c.get("text"))
            .and_then(Json::as_str)
        else {
            return vec![];
        };
        self.documents.insert(uri.to_string(), text.to_string());
        vec![self.diagnostics_for(uri)]
    }

    fn did_close(&mut self, msg: &Json) -> Vec<Json> {
        let Some(uri) = msg
            .get("params")
            .and_then(|p| p.get("textDocument"))
            .and_then(|d| d.get("uri"))
            .and_then(Json::as_str)
        else {
            return vec![];
        };
        self.documents.remove(uri);
        // Clear diagnostics for the closed file.
        vec![publish_diagnostics(uri, Json::Array(vec![]))]
    }

    /// Build a `publishDiagnostics` notification for `uri` from a fresh analysis.
    fn diagnostics_for(&self, uri: &str) -> Json {
        let Some(text) = self.documents.get(uri) else {
            return publish_diagnostics(uri, Json::Array(vec![]));
        };
        let (errors, _types) = crate::analyze(text);
        let items = errors
            .iter()
            .map(|e| {
                obj(vec![
                    ("range", span_range(text, e.span)),
                    ("severity", int(1)), // Error
                    ("source", str("pyfun")),
                    ("message", str(e.message.clone())),
                ])
            })
            .collect();
        publish_diagnostics(uri, Json::Array(items))
    }

    /// Compute the hover result (a `Hover` object, or `Null` when there is no type
    /// under the cursor): the narrowest recorded type span containing the position.
    fn hover(&self, msg: &Json) -> Json {
        let params = msg.get("params");
        let uri = params
            .and_then(|p| p.get("textDocument"))
            .and_then(|d| d.get("uri"))
            .and_then(Json::as_str);
        let position = params.and_then(|p| p.get("position"));
        let (Some(uri), Some(position)) = (uri, position) else {
            return Json::Null;
        };
        let Some(text) = self.documents.get(uri) else {
            return Json::Null;
        };
        let line = position.get("line").and_then(Json::as_i64).unwrap_or(0) as u32;
        let character = position
            .get("character")
            .and_then(Json::as_i64)
            .unwrap_or(0) as u32;
        let offset = position_to_byte(text, line, character);

        let (_errors, types) = crate::analyze(text);
        // The narrowest span containing the offset is the most specific node.
        let best = types
            .iter()
            .filter(|t| t.span.start <= offset && offset < t.span.end)
            .min_by_key(|t| t.span.end - t.span.start);
        match best {
            None => Json::Null,
            Some(t) => obj(vec![
                (
                    "contents",
                    obj(vec![
                        ("kind", str("markdown")),
                        ("value", str(format!("```pyfun\n{}\n```", t.ty))),
                    ]),
                ),
                ("range", span_range(text, t.span)),
            ]),
        }
    }
}

/// The `initialize` result advertising our capabilities (full document sync +
/// hover).
fn initialize_result() -> Json {
    obj(vec![
        (
            "capabilities",
            obj(vec![
                ("textDocumentSync", int(1)), // 1 = Full
                ("hoverProvider", Json::Bool(true)),
            ]),
        ),
        (
            "serverInfo",
            obj(vec![
                ("name", str("pyfun-lsp")),
                ("version", str(env!("CARGO_PKG_VERSION"))),
            ]),
        ),
    ])
}

/// Wrap a result in a JSON-RPC response, echoing the request id (or `null`).
fn response(id: Option<Json>, result: Json) -> Json {
    obj(vec![
        ("jsonrpc", str("2.0")),
        ("id", id.unwrap_or(Json::Null)),
        ("result", result),
    ])
}

/// A JSON-RPC `method not found` error response.
fn method_not_found(id: Option<Json>) -> Json {
    obj(vec![
        ("jsonrpc", str("2.0")),
        ("id", id.unwrap_or(Json::Null)),
        (
            "error",
            obj(vec![
                ("code", int(-32601)),
                ("message", str("method not found")),
            ]),
        ),
    ])
}

/// A `textDocument/publishDiagnostics` notification.
fn publish_diagnostics(uri: &str, diagnostics: Json) -> Json {
    obj(vec![
        ("jsonrpc", str("2.0")),
        ("method", str("textDocument/publishDiagnostics")),
        (
            "params",
            obj(vec![("uri", str(uri)), ("diagnostics", diagnostics)]),
        ),
    ])
}

/// An LSP `Range` for a byte span in `text` (positions are line + UTF-16 column).
fn span_range(text: &str, span: crate::lexer::Span) -> Json {
    let (sl, sc) = byte_to_position(text, span.start);
    let (el, ec) = byte_to_position(text, span.end);
    obj(vec![("start", position(sl, sc)), ("end", position(el, ec))])
}

fn position(line: u32, character: u32) -> Json {
    obj(vec![
        ("line", int(line as i64)),
        ("character", int(character as i64)),
    ])
}

/// Map a byte offset to an LSP `(line, character)`, both 0-based, with `character`
/// counted in UTF-16 code units (the LSP default encoding).
pub fn byte_to_position(text: &str, byte: usize) -> (u32, u32) {
    let byte = byte.min(text.len());
    let mut line = 0u32;
    let mut col = 0u32;
    let mut idx = 0;
    for ch in text.chars() {
        if idx >= byte {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += ch.len_utf16() as u32;
        }
        idx += ch.len_utf8();
    }
    (line, col)
}

/// Map an LSP `(line, character)` (UTF-16) back to a byte offset. A character
/// past the end of its line clamps to the line's end; a line past the end clamps
/// to the end of the document.
pub fn position_to_byte(text: &str, line: u32, character: u32) -> usize {
    let mut cur_line = 0u32;
    let mut col = 0u32;
    for (idx, ch) in text.char_indices() {
        if cur_line == line && col == character {
            return idx;
        }
        if cur_line < line {
            if ch == '\n' {
                cur_line += 1;
            }
            continue;
        }
        // On the target line: a newline means the requested column is past the
        // line's content, so clamp to the newline's offset.
        if ch == '\n' {
            return idx;
        }
        col += ch.len_utf16() as u32;
    }
    text.len()
}

/// Read one `Content-Length`-framed message body, or `None` at EOF.
fn read_message<R: BufRead>(reader: &mut R) -> io::Result<Option<String>> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Ok(None); // EOF
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break; // blank line ends the header block
        }
        if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
            content_length = rest.trim().parse().ok();
        }
    }
    let Some(len) = content_length else {
        return Ok(None);
    };
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf)?;
    Ok(Some(String::from_utf8_lossy(&buf).into_owned()))
}

/// Write one `Content-Length`-framed message body.
fn write_message<W: Write>(writer: &mut W, body: &str) -> io::Result<()> {
    write!(writer, "Content-Length: {}\r\n\r\n{body}", body.len())?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_mapping_roundtrips_with_unicode() {
        // "α" is 2 UTF-8 bytes but 1 UTF-16 unit; "😀" is 4 bytes / 2 units.
        let text = "let x = 1\nlet α = 😀\nlet y = 2";
        for byte in [0usize, 4, 10, 14] {
            let (l, c) = byte_to_position(text, byte);
            assert_eq!(position_to_byte(text, l, c), byte, "byte {byte}");
        }
    }

    #[test]
    fn hover_reports_inferred_type() {
        let mut server = Server::default();
        let uri = "file:///t.pyfun";
        server.handle(&json::parse(&open_msg(uri, "let n = 1 + 2")).unwrap());
        // Hover over `n` (line 0, char 4).
        let hover = json::parse(&hover_msg(uri, 0, 4)).unwrap();
        let out = server.handle(&hover);
        let value = out[0]
            .get("result")
            .unwrap()
            .get("contents")
            .unwrap()
            .get("value")
            .unwrap()
            .as_str()
            .unwrap();
        assert!(value.contains("int"), "hover value was {value:?}");
    }

    #[test]
    fn did_open_publishes_diagnostics_for_a_type_error() {
        let mut server = Server::default();
        let uri = "file:///bad.pyfun";
        let out = server.handle(&json::parse(&open_msg(uri, "let r = 1 + true")).unwrap());
        let diags = out[0]
            .get("params")
            .unwrap()
            .get("diagnostics")
            .unwrap()
            .as_array()
            .unwrap();
        assert_eq!(diags.len(), 1);
        assert_eq!(
            out[0].get("method").unwrap().as_str(),
            Some("textDocument/publishDiagnostics")
        );
    }

    #[test]
    fn clean_program_has_no_diagnostics() {
        let mut server = Server::default();
        let out = server.handle(&json::parse(&open_msg("file:///ok.pyfun", "let n = 1")).unwrap());
        let diags = out[0].get("params").unwrap().get("diagnostics").unwrap();
        assert_eq!(diags.as_array().unwrap().len(), 0);
    }

    #[test]
    fn exit_sets_the_flag() {
        let mut server = Server::default();
        server.handle(&json::parse(r#"{"jsonrpc":"2.0","method":"exit"}"#).unwrap());
        assert!(server.exit);
    }

    fn open_msg(uri: &str, text: &str) -> String {
        obj(vec![
            ("jsonrpc", str("2.0")),
            ("method", str("textDocument/didOpen")),
            (
                "params",
                obj(vec![(
                    "textDocument",
                    obj(vec![("uri", str(uri)), ("text", str(text))]),
                )]),
            ),
        ])
        .to_string()
    }

    fn hover_msg(uri: &str, line: i64, character: i64) -> String {
        obj(vec![
            ("jsonrpc", str("2.0")),
            ("id", int(1)),
            ("method", str("textDocument/hover")),
            (
                "params",
                obj(vec![
                    ("textDocument", obj(vec![("uri", str(uri))])),
                    (
                        "position",
                        obj(vec![("line", int(line)), ("character", int(character))]),
                    ),
                ]),
            ),
        ])
        .to_string()
    }
}
