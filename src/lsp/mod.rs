//! A small, dependency-free Language Server (`DESIGN.md` §9) — the "front-end-
//! first" tooling slice. It speaks LSP over stdio (JSON-RPC with `Content-Length`
//! framing) and offers these features:
//!
//! - **Diagnostics**: the existing type/effect/unit errors ([`crate::analyze`])
//!   streamed as `textDocument/publishDiagnostics` on open/change.
//! - **Hover**: the inferred type (effects included, e.g. `string ->{io} unit`) of
//!   the narrowest expression under the cursor — the only way to *see* an inferred
//!   type, since Pyfun never writes them.
//! - **Go-to-definition** and **find-references**: navigate between a symbol and its
//!   uses (module-level or local), via the name resolver in [`resolve`].
//! - **Rename**: rewrite every occurrence of a local or top-level `let` value.
//! - **Completion**: in-scope module symbols plus prelude, builtins, and keywords.
//! - **Document symbols**: the module outline, from [`resolve::definitions`].
//!
//! All of them run over a **resilient, version-cached analysis** ([`crate::analyze`]):
//! the lexer and parser recover from errors, so a half-typed file still produces
//! results, and an unchanged document is analyzed once and reused.
//!
//! The protocol plumbing is hand-rolled (see [`json`]) to keep the crate
//! dependency-free, the same choice we made for the lexer and parser. The
//! message-handling core ([`Server::handle`]) is pure — JSON in, JSON out — so it
//! is tested directly without spawning a process or touching stdio.

pub mod json;
pub mod resolve;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::rc::Rc;

use crate::Analysis;
use json::{Json, int, obj, str};

/// Convert a `file:` document URI to a filesystem path, for resolving sibling
/// modules during import-aware analysis (`DESIGN.md` §6.1). Returns `None` for a
/// non-`file:` URI. Percent-escapes are decoded; on Windows a leading slash
/// before a drive letter (`/C:/…`) is dropped.
fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    // `file:///path` (empty authority) → `/path`; tolerate a `localhost` authority.
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let decoded = percent_decode(rest);
    let decoded = if cfg!(windows) {
        decoded
            .strip_prefix('/')
            .filter(|s| s.as_bytes().get(1) == Some(&b':'))
            .map(str::to_string)
            .unwrap_or(decoded)
    } else {
        decoded
    };
    Some(PathBuf::from(decoded))
}

/// Decode `%XX` percent-escapes in a URI path component (e.g. `%20` → space,
/// `%3A` → `:`). Invalid escapes are left as-is.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let (Some(h), Some(l)) = (
                bytes.get(i + 1).and_then(|b| (*b as char).to_digit(16)),
                bytes.get(i + 2).and_then(|b| (*b as char).to_digit(16)),
            )
        {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

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

/// One open document: its text and a monotonic version stamp (assigned by the
/// server, so it is independent of any `version` the client sends). The version
/// keys the analysis cache.
struct Doc {
    text: String,
    version: u64,
}

/// The server state: open documents by URI, a per-document **analysis cache**
/// (so repeated requests on an unchanged document reuse one parse + type-check —
/// the "incremental" half), plus a flag set by `exit`.
#[derive(Default)]
pub struct Server {
    documents: HashMap<String, Doc>,
    /// Cached analysis per URI, tagged with the document version it was computed
    /// from; a stale entry (older version) is recomputed on demand. `RefCell`
    /// because the read handlers take `&self`.
    cache: RefCell<HashMap<String, (u64, Rc<Analysis>)>>,
    /// Monotonic clock stamping each document edit with a fresh version.
    clock: u64,
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
            Some("textDocument/definition") => {
                let result = self.definition(msg);
                vec![response(id, result)]
            }
            Some("textDocument/references") => {
                let result = self.references(msg);
                vec![response(id, result)]
            }
            Some("textDocument/prepareRename") => {
                let result = self.prepare_rename(msg);
                vec![response(id, result)]
            }
            Some("textDocument/rename") => {
                let result = self.rename(msg);
                vec![response(id, result)]
            }
            Some("textDocument/completion") => {
                let result = self.completion(msg);
                vec![response(id, result)]
            }
            Some("textDocument/documentSymbol") => {
                let result = self.document_symbols(msg);
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
        self.set_document(uri, text);
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
        self.set_document(uri, text);
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
        self.cache.borrow_mut().remove(uri);
        // Clear diagnostics for the closed file.
        vec![publish_diagnostics(uri, Json::Array(vec![]))]
    }

    /// Store `text` for `uri` under a fresh version (which invalidates the cache
    /// entry lazily, on the next [`analysis`](Self::analysis) call).
    fn set_document(&mut self, uri: &str, text: &str) {
        self.clock += 1;
        self.documents.insert(
            uri.to_string(),
            Doc {
                text: text.to_string(),
                version: self.clock,
            },
        );
    }

    /// The source text of an open document.
    fn text(&self, uri: &str) -> Option<&str> {
        self.documents.get(uri).map(|d| d.text.as_str())
    }

    /// The analysis of `uri`, served from the cache when the document is
    /// unchanged since it was computed, otherwise analyzed fresh and cached. This
    /// is what makes repeated requests (hover, then go-to-def, then references) on
    /// one document version share a single parse + type-check.
    fn analysis(&self, uri: &str) -> Option<Rc<Analysis>> {
        let doc = self.documents.get(uri)?;
        if let Some((version, analysis)) = self.cache.borrow().get(uri)
            && *version == doc.version
        {
            return Some(analysis.clone());
        }
        // Resolve `import`s relative to the document's own directory, so a
        // multi-module file checks cleanly (`DESIGN.md` §6.1). A non-`file:` URI
        // (or a path without a parent) just analyzes without imports.
        let path = uri_to_path(uri);
        let dir = path.as_deref().and_then(std::path::Path::parent);
        let analysis = Rc::new(crate::analyze_in_dir(&doc.text, dir));
        self.cache
            .borrow_mut()
            .insert(uri.to_string(), (doc.version, analysis.clone()));
        Some(analysis)
    }

    /// Build a `publishDiagnostics` notification for `uri` from its analysis.
    fn diagnostics_for(&self, uri: &str) -> Json {
        let (Some(text), Some(analysis)) = (self.text(uri), self.analysis(uri)) else {
            return publish_diagnostics(uri, Json::Array(vec![]));
        };
        let items = analysis
            .diagnostics
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
        let (Some(text), Some(analysis)) = (self.text(uri), self.analysis(uri)) else {
            return Json::Null;
        };
        let line = position.get("line").and_then(Json::as_i64).unwrap_or(0) as u32;
        let character = position
            .get("character")
            .and_then(Json::as_i64)
            .unwrap_or(0) as u32;
        let offset = position_to_byte(text, line, character);

        // The narrowest span containing the offset is the most specific node.
        let best = analysis
            .types
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

    /// Resolve go-to-definition: the narrowest module-level reference under the
    /// cursor, mapped to its definition's `Location` (same document). `Null` when
    /// the cursor is not on a resolvable reference (or the document does not parse).
    fn definition(&self, msg: &Json) -> Json {
        let params = msg.get("params");
        let uri = params
            .and_then(|p| p.get("textDocument"))
            .and_then(|d| d.get("uri"))
            .and_then(Json::as_str);
        let position = params.and_then(|p| p.get("position"));
        let (Some(uri), Some(position)) = (uri, position) else {
            return Json::Null;
        };
        let (Some(text), Some(analysis)) = (self.text(uri), self.analysis(uri)) else {
            return Json::Null;
        };
        let Some(module) = analysis.module.as_ref() else {
            return Json::Null;
        };
        let line = position.get("line").and_then(Json::as_i64).unwrap_or(0) as u32;
        let character = position
            .get("character")
            .and_then(Json::as_i64)
            .unwrap_or(0) as u32;
        let offset = position_to_byte(text, line, character);

        // The narrowest reference span containing the cursor is the identifier.
        let target = resolve::references(module)
            .into_iter()
            .filter(|r| r.span.start <= offset && offset < r.span.end)
            .min_by_key(|r| r.span.end - r.span.start)
            .map(|r| r.target);
        let span = match target {
            // A local binder (param / block `let` / pattern var) jumps to itself.
            Some(resolve::Target::Local(span)) => Some(span),
            // A module symbol is resolved by name against the definition table.
            Some(resolve::Target::Module(name)) => resolve::definitions(module)
                .into_iter()
                .find(|sym| sym.name == name)
                .map(|sym| sym.span),
            None => None,
        };
        match span {
            Some(span) => obj(vec![("uri", str(uri)), ("range", span_range(text, span))]),
            None => Json::Null, // a prelude/builtin — no source location
        }
    }

    /// Find-references: every occurrence of the symbol under the cursor (a `Local`
    /// binder or a `Module` symbol), returned as an array of `Location`s. Honours
    /// the request's `context.includeDeclaration` (default `true`). The cursor may
    /// sit on a use *or* the definition/binder itself.
    fn references(&self, msg: &Json) -> Json {
        let params = msg.get("params");
        let uri = params
            .and_then(|p| p.get("textDocument"))
            .and_then(|d| d.get("uri"))
            .and_then(Json::as_str);
        let position = params.and_then(|p| p.get("position"));
        let (Some(uri), Some(position)) = (uri, position) else {
            return Json::Array(vec![]);
        };
        let (Some(text), Some(analysis)) = (self.text(uri), self.analysis(uri)) else {
            return Json::Array(vec![]);
        };
        let Some(module) = analysis.module.as_ref() else {
            return Json::Array(vec![]);
        };
        let line = position.get("line").and_then(Json::as_i64).unwrap_or(0) as u32;
        let character = position
            .get("character")
            .and_then(Json::as_i64)
            .unwrap_or(0) as u32;
        let offset = position_to_byte(text, line, character);
        // Default `includeDeclaration` to true when the client omits the context.
        let include_declaration = params
            .and_then(|p| p.get("context"))
            .and_then(|c| c.get("includeDeclaration"))
            .map(|v| v == &Json::Bool(true))
            .unwrap_or(true);

        let Some((_, symbol)) = resolve::symbol_at(module, offset) else {
            return Json::Array(vec![]);
        };
        let locations = resolve::find_references(module, &symbol, include_declaration)
            .into_iter()
            .map(|span| obj(vec![("uri", str(uri)), ("range", span_range(text, span))]))
            .collect();
        Json::Array(locations)
    }

    /// `prepareRename`: validate that the cursor is on a renameable symbol and, if
    /// so, return the range of the identifier to rename (so the editor pre-fills its
    /// rename box). `Null` when the symbol cannot be renamed (a prelude/builtin, or
    /// a constructor/type/extern whose occurrences aren't all precisely tracked).
    fn prepare_rename(&self, msg: &Json) -> Json {
        let Some((text, offset, analysis)) = self.locate(msg) else {
            return Json::Null;
        };
        // Refuse rename unless the document fully parses: a partial module could
        // hide occurrences in the unparsed region, making the rewrite unsound.
        let Some(module) = analysis.module.as_ref().filter(|_| analysis.parse_ok) else {
            return Json::Null;
        };
        match resolve::symbol_at(module, offset) {
            Some((span, target)) if is_renameable(module, &target) => span_range(text, span),
            _ => Json::Null,
        }
    }

    /// `rename`: produce a `WorkspaceEdit` replacing every occurrence of the symbol
    /// under the cursor (declaration included) with `newName`. `Null` when the
    /// symbol is not renameable or `newName` is not a valid value identifier — the
    /// editor's `prepareRename` call normally rules these out first.
    ///
    /// Only **locals** and top-level **`let`** values are renameable: their every
    /// occurrence is a precise span. Constructors / types / `extern`s are refused —
    /// their declaration span is the whole declaration and their type-annotation
    /// uses are not tracked as references, so a rename would be unsound. No
    /// capture-avoidance check is done (renaming to a name already bound nearby can
    /// shadow); editors surface the diff for review.
    fn rename(&self, msg: &Json) -> Json {
        let params = msg.get("params");
        let new_name = params.and_then(|p| p.get("newName")).and_then(Json::as_str);
        let (Some((text, offset, analysis)), Some(new_name)) = (self.locate(msg), new_name) else {
            return Json::Null;
        };
        let uri = params
            .and_then(|p| p.get("textDocument"))
            .and_then(|d| d.get("uri"))
            .and_then(Json::as_str)
            .unwrap_or_default();
        if !is_value_identifier(new_name) {
            return Json::Null;
        }
        // Only rename when the document fully parses (see `prepare_rename`).
        let Some(module) = analysis.module.as_ref().filter(|_| analysis.parse_ok) else {
            return Json::Null;
        };
        let Some((_, target)) = resolve::symbol_at(module, offset) else {
            return Json::Null;
        };
        if !is_renameable(module, &target) {
            return Json::Null;
        }
        let edits: Vec<Json> = resolve::find_references(module, &target, true)
            .into_iter()
            .map(|span| {
                obj(vec![
                    ("range", span_range(text, span)),
                    ("newText", str(new_name)),
                ])
            })
            .collect();
        // A single-document `WorkspaceEdit`: { changes: { <uri>: [TextEdit…] } }.
        obj(vec![("changes", obj(vec![(uri, Json::Array(edits))]))])
    }

    /// Resolve a position-bearing request to its document text, byte offset, and
    /// (cached) analysis — everything a navigation/rename handler needs.
    fn locate(&self, msg: &Json) -> Option<(&str, usize, Rc<Analysis>)> {
        let params = msg.get("params");
        let uri = params
            .and_then(|p| p.get("textDocument"))
            .and_then(|d| d.get("uri"))
            .and_then(Json::as_str)?;
        let position = params.and_then(|p| p.get("position"))?;
        let analysis = self.analysis(uri)?;
        let text = self.text(uri)?;
        let line = position.get("line").and_then(Json::as_i64).unwrap_or(0) as u32;
        let character = position
            .get("character")
            .and_then(Json::as_i64)
            .unwrap_or(0) as u32;
        Some((text, position_to_byte(text, line, character), analysis))
    }

    /// Completion: module-level symbols (when the document parses) plus the always-
    /// available prelude, builtins, and keywords. The static set is the fallback
    /// while the file is mid-edit and does not yet parse.
    fn completion(&self, msg: &Json) -> Json {
        let analysis = msg
            .get("params")
            .and_then(|p| p.get("textDocument"))
            .and_then(|d| d.get("uri"))
            .and_then(Json::as_str)
            .and_then(|uri| self.analysis(uri));

        let mut items = Vec::new();
        let mut seen = HashSet::new();
        let mut push = |items: &mut Vec<Json>, label: &str, kind: i64| {
            if seen.insert(label.to_string()) {
                items.push(obj(vec![("label", str(label)), ("kind", int(kind))]));
            }
        };

        // User symbols first (they shadow prelude names), from whatever parsed —
        // even a partial module contributes the symbols it recovered.
        if let Some(analysis) = &analysis
            && let Some(module) = analysis.module.as_ref()
        {
            for sym in resolve::definitions(module) {
                push(&mut items, &sym.name, completion_kind(sym.kind));
            }
        }

        for (name, _) in crate::types::PRELUDE {
            push(&mut items, name, KIND_FUNCTION);
        }
        // Module members are offered fully qualified (`List.map`, `Set.add`, …).
        for (module, members) in crate::types::MODULE_PRELUDES {
            for (member, _) in *members {
                push(&mut items, &format!("{module}.{member}"), KIND_FUNCTION);
            }
        }
        for name in BUILTIN_CTORS {
            push(&mut items, name, KIND_CONSTRUCTOR);
        }
        for name in BUILTIN_TYPES {
            push(&mut items, name, KIND_CLASS);
        }
        for name in KEYWORDS {
            push(&mut items, name, KIND_KEYWORD);
        }
        Json::Array(items)
    }

    /// Document symbols (the outline): every module-level definition as a flat
    /// `DocumentSymbol[]`, reusing the same `resolve::definitions` that powers
    /// go-to-definition and completion. Works on whatever parsed (a partial module
    /// still outlines its good items).
    fn document_symbols(&self, msg: &Json) -> Json {
        let uri = msg
            .get("params")
            .and_then(|p| p.get("textDocument"))
            .and_then(|d| d.get("uri"))
            .and_then(Json::as_str);
        let (Some(uri), Some(analysis)) = (uri, uri.and_then(|u| self.analysis(u))) else {
            return Json::Array(vec![]);
        };
        let Some(text) = self.text(uri) else {
            return Json::Array(vec![]);
        };
        let Some(module) = analysis.module.as_ref() else {
            return Json::Array(vec![]);
        };
        let symbols = resolve::definitions(module)
            .into_iter()
            .map(|sym| {
                // `range` is the full extent, `selectionRange` the name to reveal;
                // we have one precise span, valid for both (selection ⊆ range).
                let range = span_range(text, sym.span);
                obj(vec![
                    ("name", str(sym.name)),
                    ("kind", int(symbol_kind(sym.kind))),
                    ("range", range.clone()),
                    ("selectionRange", range),
                ])
            })
            .collect();
        Json::Array(symbols)
    }
}

/// `CompletionItemKind` codes (LSP spec) for the kinds we emit.
const KIND_FUNCTION: i64 = 3;
const KIND_CONSTRUCTOR: i64 = 4;
const KIND_VALUE: i64 = 12;
const KIND_CLASS: i64 = 7;
const KIND_MODULE: i64 = 9;
const KIND_UNIT: i64 = 11;
const KIND_KEYWORD: i64 = 14;

/// Whether the symbol under the cursor can be safely renamed: a local binder, or a
/// top-level `let` value. Constructors / types / records / `extern`s / measures are
/// refused — their declaration span isn't a precise name and their type-annotation
/// occurrences aren't tracked, so renaming them would be unsound.
fn is_renameable(module: &crate::syntax::Module, target: &resolve::Target) -> bool {
    match target {
        resolve::Target::Local(_) => true,
        resolve::Target::Module(name) => resolve::definitions(module)
            .iter()
            .any(|sym| &sym.name == name && sym.kind == resolve::SymbolKind::Value),
    }
}

/// Whether `name` is a valid value identifier (lowercase-leading, then word
/// characters) — the shape of every renameable symbol, so a rename can't turn a
/// value into a constructor/keyword or otherwise break the program.
fn is_value_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !is_keyword(name)
}

/// Reserved words that a value identifier must not collide with.
fn is_keyword(name: &str) -> bool {
    KEYWORDS.contains(&name)
}

/// Map a resolved symbol kind to a `CompletionItemKind`.
fn completion_kind(kind: resolve::SymbolKind) -> i64 {
    use resolve::SymbolKind::*;
    match kind {
        Value => KIND_VALUE,
        Constructor => KIND_CONSTRUCTOR,
        Type | Record => KIND_CLASS,
        Extern => KIND_FUNCTION,
        Measure => KIND_UNIT,
        Module => KIND_MODULE,
    }
}

/// `SymbolKind` codes (LSP spec) for the document-outline icons.
const SYM_MODULE: i64 = 2;
const SYM_ENUM: i64 = 10;
const SYM_FUNCTION: i64 = 12;
const SYM_NUMBER: i64 = 16;
const SYM_ENUM_MEMBER: i64 = 22;
const SYM_STRUCT: i64 = 23;

/// Map a resolved symbol kind to an LSP document-symbol `SymbolKind`.
fn symbol_kind(kind: resolve::SymbolKind) -> i64 {
    use resolve::SymbolKind::*;
    match kind {
        Value | Extern => SYM_FUNCTION,
        Constructor => SYM_ENUM_MEMBER,
        Type => SYM_ENUM,
        Record => SYM_STRUCT,
        Measure => SYM_NUMBER,
        Module => SYM_MODULE,
    }
}

/// Reserved data constructors always in scope (`DESIGN.md` §8.1, `result`).
const BUILTIN_CTORS: &[&str] = &["Ok", "Error", "Some", "None"];

/// Built-in and reserved type names.
const BUILTIN_TYPES: &[&str] = &[
    "int", "float", "bool", "string", "unit", "Result", "Async", "Seq", "List", "Set", "Map",
    "Option",
];

/// Pyfun keywords (and contextual builder/CE words) offered as completions.
const KEYWORDS: &[&str] = &[
    "let", "mut", "pure", "type", "measure", "extern", "if", "then", "else", "match", "with",
    "fun", "and", "or", "not", "true", "false", "return", "in", "async", "seq", "result",
];

/// The `initialize` result advertising our capabilities (full document sync +
/// hover).
fn initialize_result() -> Json {
    obj(vec![
        (
            "capabilities",
            obj(vec![
                ("textDocumentSync", int(1)), // 1 = Full
                ("hoverProvider", Json::Bool(true)),
                ("definitionProvider", Json::Bool(true)),
                ("referencesProvider", Json::Bool(true)),
                // Rename, with prepare support so the editor validates first.
                (
                    "renameProvider",
                    obj(vec![("prepareProvider", Json::Bool(true))]),
                ),
                // An (empty) CompletionOptions object enables completion.
                ("completionProvider", obj(vec![])),
                ("documentSymbolProvider", Json::Bool(true)),
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
    fn uri_to_path_handles_file_uris() {
        // Percent-escapes decode; a non-file URI yields None.
        assert_eq!(uri_to_path("untitled:foo"), None);
        let decoded = uri_to_path("file:///tmp/my%20mod.pyfun").unwrap();
        assert!(decoded.to_string_lossy().contains("my mod.pyfun"));
        #[cfg(windows)]
        assert_eq!(
            uri_to_path("file:///C:/a/b.pyfun").unwrap(),
            std::path::PathBuf::from("C:/a/b.pyfun")
        );
        #[cfg(not(windows))]
        assert_eq!(
            uri_to_path("file:///a/b.pyfun").unwrap(),
            std::path::PathBuf::from("/a/b.pyfun")
        );
    }

    #[test]
    fn import_aware_analysis_resolves_a_sibling_module() {
        // Open a multi-module file whose `import` resolves to a real sibling file:
        // the qualified reference must not be flagged. A non-existent sibling (the
        // control) would leave the "not a member" diagnostic in place.
        let dir = std::env::temp_dir().join(format!("pyfun_lsp_imports_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("geometry.pyfun"), "let area w h = w * h").unwrap();
        let main = dir.join("main.pyfun");
        let p = main.to_string_lossy().replace('\\', "/");
        // `file:///C:/…` on Windows, `file:///tmp/…` on Unix (a leading-slash path
        // already supplies the third slash).
        let uri = if p.starts_with('/') {
            format!("file://{p}")
        } else {
            format!("file:///{p}")
        };

        let mut server = Server::default();
        let out = server.handle(
            &json::parse(&open_msg(
                &uri,
                "import Geometry\nlet floor = Geometry.area 4 5",
            ))
            .unwrap(),
        );
        let diags = out[0]
            .get("params")
            .unwrap()
            .get("diagnostics")
            .unwrap()
            .as_array()
            .unwrap();
        assert!(diags.is_empty(), "expected clean analysis, got {diags:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn exit_sets_the_flag() {
        let mut server = Server::default();
        server.handle(&json::parse(r#"{"jsonrpc":"2.0","method":"exit"}"#).unwrap());
        assert!(server.exit);
    }

    #[test]
    fn goto_definition_jumps_to_the_binding() {
        let mut server = Server::default();
        let uri = "file:///d.pyfun";
        // `one` is defined on line 0; referenced on line 1 at character 10.
        server.handle(&json::parse(&open_msg(uri, "let one = 1\nlet two = one")).unwrap());
        let req = pos_msg("textDocument/definition", uri, 1, 11);
        let out = server.handle(&json::parse(&req).unwrap());
        let range = out[0].get("result").unwrap().get("range").unwrap();
        let start = range.get("start").unwrap();
        // Jumps back to the name span of `one` on line 0 (character 4).
        assert_eq!(start.get("line").unwrap().as_i64(), Some(0));
        assert_eq!(start.get("character").unwrap().as_i64(), Some(4));
    }

    #[test]
    fn goto_definition_jumps_to_a_parameter() {
        let mut server = Server::default();
        let uri = "file:///d2.pyfun";
        // `let id x = x` — the reference to `x` (char 11) jumps to the parameter
        // binder `x` (char 7).
        server.handle(&json::parse(&open_msg(uri, "let id x = x")).unwrap());
        let req = pos_msg("textDocument/definition", uri, 0, 11);
        let out = server.handle(&json::parse(&req).unwrap());
        let start = out[0]
            .get("result")
            .unwrap()
            .get("range")
            .unwrap()
            .get("start")
            .unwrap();
        assert_eq!(start.get("line").unwrap().as_i64(), Some(0));
        assert_eq!(start.get("character").unwrap().as_i64(), Some(7));
    }

    #[test]
    fn hover_reports_a_parameter_type() {
        let mut server = Server::default();
        let uri = "file:///h.pyfun";
        // Hover the parameter `n` in `let inc n = n + 1` (char 8) → its type.
        server.handle(&json::parse(&open_msg(uri, "let inc n = n + 1")).unwrap());
        let out = server.handle(&json::parse(&hover_msg(uri, 0, 8)).unwrap());
        let value = out[0]
            .get("result")
            .unwrap()
            .get("contents")
            .unwrap()
            .get("value")
            .unwrap()
            .as_str()
            .unwrap();
        assert!(value.contains("int"), "param hover was {value:?}");
    }

    #[test]
    fn references_lists_all_uses_and_the_declaration() {
        let mut server = Server::default();
        let uri = "file:///r.pyfun";
        server.handle(&json::parse(&open_msg(uri, "let one = 1\nlet two = one + one")).unwrap());
        // Request references from the definition name `one` (line 0, char 4).
        let req = references_msg(uri, 0, 4, true);
        let out = server.handle(&json::parse(&req).unwrap());
        let locs = out[0].get("result").unwrap().as_array().unwrap();
        // Two uses + the declaration.
        assert_eq!(locs.len(), 3);
        // Excluding the declaration leaves the two uses.
        let req = references_msg(uri, 0, 4, false);
        let out = server.handle(&json::parse(&req).unwrap());
        assert_eq!(out[0].get("result").unwrap().as_array().unwrap().len(), 2);
    }

    #[test]
    fn rename_rewrites_every_occurrence() {
        let mut server = Server::default();
        let uri = "file:///rn.pyfun";
        server.handle(&json::parse(&open_msg(uri, "let one = 1\nlet two = one + one")).unwrap());
        // Rename `one` (definition name, line 0 char 4) to `uno`.
        let req = rename_msg(uri, 0, 4, "uno");
        let out = server.handle(&json::parse(&req).unwrap());
        let edits = out[0]
            .get("result")
            .unwrap()
            .get("changes")
            .unwrap()
            .get(uri)
            .unwrap()
            .as_array()
            .unwrap();
        // Declaration + two uses, all rewritten to `uno`.
        assert_eq!(edits.len(), 3);
        assert!(
            edits
                .iter()
                .all(|e| e.get("newText").unwrap().as_str() == Some("uno"))
        );
    }

    #[test]
    fn prepare_rename_returns_a_range_for_a_value() {
        let mut server = Server::default();
        let uri = "file:///pr.pyfun";
        server.handle(&json::parse(&open_msg(uri, "let one = 1")).unwrap());
        let req = pos_msg("textDocument/prepareRename", uri, 0, 4);
        let out = server.handle(&json::parse(&req).unwrap());
        let start = out[0].get("result").unwrap().get("start").unwrap();
        assert_eq!(start.get("character").unwrap().as_i64(), Some(4));
    }

    #[test]
    fn rename_refuses_a_constructor() {
        let mut server = Server::default();
        let uri = "file:///ct.pyfun";
        server
            .handle(&json::parse(&open_msg(uri, "type Color = Red | Green\nlet c = Red")).unwrap());
        // `Red` use on line 1 (char 8) — a constructor, not renameable.
        let req = rename_msg(uri, 1, 9, "Crimson");
        let out = server.handle(&json::parse(&req).unwrap());
        assert_eq!(out[0].get("result").unwrap(), &Json::Null);
    }

    #[test]
    fn rename_rejects_an_invalid_new_name() {
        let mut server = Server::default();
        let uri = "file:///iv.pyfun";
        server.handle(&json::parse(&open_msg(uri, "let one = 1\nlet two = one")).unwrap());
        // `match` is a keyword — not a valid value identifier.
        let req = rename_msg(uri, 0, 4, "match");
        let out = server.handle(&json::parse(&req).unwrap());
        assert_eq!(out[0].get("result").unwrap(), &Json::Null);
    }

    #[test]
    fn completion_lists_user_symbols_and_prelude() {
        let mut server = Server::default();
        let uri = "file:///c.pyfun";
        server.handle(&json::parse(&open_msg(uri, "let foo = 1")).unwrap());
        let req = pos_msg("textDocument/completion", uri, 0, 0);
        let out = server.handle(&json::parse(&req).unwrap());
        let labels: Vec<&str> = out[0]
            .get("result")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|i| i.get("label").and_then(Json::as_str))
            .collect();
        assert!(labels.contains(&"foo"), "user symbol missing: {labels:?}");
        assert!(
            labels.contains(&"List.map"),
            "qualified list member missing"
        );
        assert!(
            labels.contains(&"Map.tryFind"),
            "qualified map member missing"
        );
        assert!(labels.contains(&"print"), "prelude missing");
        assert!(labels.contains(&"match"), "keyword missing");
        assert!(labels.contains(&"List"), "builtin type missing");
    }

    #[test]
    fn half_typed_file_still_diagnoses_and_hovers() {
        // The middle `let bad =` is broken; the surrounding items are fine.
        let mut server = Server::default();
        let uri = "file:///partial.pyfun";
        let src = "let good = 1\nlet bad =\nlet also = 2";
        let out = server.handle(&json::parse(&open_msg(uri, src)).unwrap());
        // A syntax diagnostic is still published (recovery, not a hard failure).
        let diags = out[0]
            .get("params")
            .unwrap()
            .get("diagnostics")
            .unwrap()
            .as_array()
            .unwrap();
        assert_eq!(diags.len(), 1, "diags: {diags:?}");
        // Hover over `good` (line 0, char 4) still resolves a type from the
        // recovered items.
        let out = server.handle(&json::parse(&hover_msg(uri, 0, 4)).unwrap());
        let value = out[0]
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
    fn lex_error_does_not_blank_the_file() {
        // An unterminated string is a *lexing* error; the earlier `good` must still
        // diagnose and hover (lexer recovery, not a hard failure).
        let mut server = Server::default();
        let uri = "file:///lexerr.pyfun";
        let src = "let good = 1\nlet s = \"oops";
        let out = server.handle(&json::parse(&open_msg(uri, src)).unwrap());
        let diags = out[0]
            .get("params")
            .unwrap()
            .get("diagnostics")
            .unwrap()
            .as_array()
            .unwrap();
        assert!(!diags.is_empty(), "expected a lex diagnostic");
        let out = server.handle(&json::parse(&hover_msg(uri, 0, 4)).unwrap());
        let value = out[0]
            .get("result")
            .unwrap()
            .get("contents")
            .unwrap()
            .get("value")
            .unwrap()
            .as_str()
            .unwrap();
        assert!(value.contains("int"), "hover after lex error: {value:?}");
    }

    #[test]
    fn rename_is_refused_when_the_file_does_not_fully_parse() {
        let mut server = Server::default();
        let uri = "file:///partialrn.pyfun";
        // `one` is well-formed, but the file has a syntax error below it; renaming
        // could miss an occurrence in the unparsed region, so it is refused.
        let src = "let one = 1\nlet bad =\nlet two = one";
        server.handle(&json::parse(&open_msg(uri, src)).unwrap());
        let req = rename_msg(uri, 0, 4, "uno");
        let out = server.handle(&json::parse(&req).unwrap());
        assert_eq!(out[0].get("result").unwrap(), &Json::Null);
    }

    #[test]
    fn analysis_is_cached_per_version() {
        let mut server = Server::default();
        let uri = "file:///cache.pyfun";
        server.handle(&json::parse(&open_msg(uri, "let n = 1")).unwrap());
        // Two requests on the unchanged document share one analysis (same Rc).
        let a = server.analysis(uri).unwrap();
        let b = server.analysis(uri).unwrap();
        assert!(Rc::ptr_eq(&a, &b), "unchanged document re-analyzed");
        // An edit bumps the version, so the cache is recomputed.
        server.handle(&json::parse(&change_msg(uri, "let n = 2")).unwrap());
        let c = server.analysis(uri).unwrap();
        assert!(!Rc::ptr_eq(&a, &c), "edited document served stale analysis");
    }

    #[test]
    fn document_symbols_lists_module_definitions() {
        let mut server = Server::default();
        let uri = "file:///sym.pyfun";
        server.handle(
            &json::parse(&open_msg(
                uri,
                "type Color = Red | Green\nlet x = 1\nlet inc n = n + 1",
            ))
            .unwrap(),
        );
        let req = pos_msg("textDocument/documentSymbol", uri, 0, 0);
        let out = server.handle(&json::parse(&req).unwrap());
        let symbols = out[0].get("result").unwrap().as_array().unwrap();
        let names: Vec<&str> = symbols
            .iter()
            .filter_map(|s| s.get("name").and_then(Json::as_str))
            .collect();
        assert!(names.contains(&"Color"), "names: {names:?}");
        assert!(names.contains(&"Red"));
        assert!(names.contains(&"Green"));
        assert!(names.contains(&"x"));
        assert!(names.contains(&"inc"));
        // Every entry carries a range and a contained selectionRange.
        assert!(symbols.iter().all(|s| s.get("range").is_some()
            && s.get("selectionRange").is_some()
            && s.get("kind").and_then(Json::as_i64).is_some()));
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
        pos_msg("textDocument/hover", uri, line, character)
    }

    /// A full-sync `textDocument/didChange` carrying the whole new document text.
    fn change_msg(uri: &str, text: &str) -> String {
        obj(vec![
            ("jsonrpc", str("2.0")),
            ("method", str("textDocument/didChange")),
            (
                "params",
                obj(vec![
                    ("textDocument", obj(vec![("uri", str(uri))])),
                    (
                        "contentChanges",
                        Json::Array(vec![obj(vec![("text", str(text))])]),
                    ),
                ]),
            ),
        ])
        .to_string()
    }

    /// A `textDocument/rename` request with a `newName`.
    fn rename_msg(uri: &str, line: i64, character: i64, new_name: &str) -> String {
        obj(vec![
            ("jsonrpc", str("2.0")),
            ("id", int(1)),
            ("method", str("textDocument/rename")),
            (
                "params",
                obj(vec![
                    ("textDocument", obj(vec![("uri", str(uri))])),
                    (
                        "position",
                        obj(vec![("line", int(line)), ("character", int(character))]),
                    ),
                    ("newName", str(new_name)),
                ]),
            ),
        ])
        .to_string()
    }

    /// A `textDocument/references` request with an `includeDeclaration` context.
    fn references_msg(uri: &str, line: i64, character: i64, include_decl: bool) -> String {
        obj(vec![
            ("jsonrpc", str("2.0")),
            ("id", int(1)),
            ("method", str("textDocument/references")),
            (
                "params",
                obj(vec![
                    ("textDocument", obj(vec![("uri", str(uri))])),
                    (
                        "position",
                        obj(vec![("line", int(line)), ("character", int(character))]),
                    ),
                    (
                        "context",
                        obj(vec![("includeDeclaration", Json::Bool(include_decl))]),
                    ),
                ]),
            ),
        ])
        .to_string()
    }

    /// A position-bearing request (hover/definition/completion) as JSON text.
    fn pos_msg(method: &str, uri: &str, line: i64, character: i64) -> String {
        obj(vec![
            ("jsonrpc", str("2.0")),
            ("id", int(1)),
            ("method", str(method)),
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
