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
//!   uses (module-level or local), via the name resolver in [`resolve`]. Values and
//!   constructors cross files (go-to-definition on `Geometry.area` jumps to that
//!   module's `.pyfun`; find-references spans the project — bare uses, every
//!   qualified use, and constructor *patterns*); types navigate in-file (their
//!   declaration and annotation uses).
//! - **Rename**: rewrite every occurrence of a local, a top-level `let` value, a
//!   constructor, or a type. Values and constructors rename project-wide (a value to
//!   a value, a constructor to a constructor; a strict scan refuses rather than
//!   half-rename if any project file fails to parse); a type renames in-file (type
//!   names have no cross-file dimension — there is no qualified-type syntax).
//! - **Completion**: in-scope module symbols plus prelude, builtins, and keywords.
//! - **Document symbols** and **workspace symbols**: the module outline and a
//!   project-wide symbol search across the directory's `.pyfun` files.
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
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
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

/// Replace the final path segment of a `file:` URI with `filename`, yielding a
/// sibling module's URI without re-encoding the path (preserves the client's
/// exact encoding).
fn sibling_uri(uri: &str, filename: &str) -> String {
    match uri.rfind('/') {
        Some(i) => format!("{}/{filename}", &uri[..i]),
        None => filename.to_string(),
    }
}

/// Whether `module` has an `import <name>` (so `name` is an imported *file*
/// module, eligible for cross-file navigation).
fn module_imports(module: &crate::syntax::Module, name: &str) -> bool {
    module
        .items
        .iter()
        .any(|i| matches!(i, crate::syntax::Item::Import { name: n, .. } if n == name))
}

/// The module name a document URI belongs to (`…/geometry.pyfun` → `Geometry`).
fn current_module(uri: &str) -> Option<String> {
    crate::project::module_name_from_path(&uri_to_path(uri)?)
}

/// Within a qualified reference's source span (`Geometry.area`), the sub-span of
/// just the **member** identifier (`area`) — the part a rename rewrites, keeping
/// the `Geometry.` qualifier. Found as the text after the last `.` (skipping any
/// whitespace), so it tolerates `Geometry . area`.
fn member_subspan(
    source: &str,
    expr_span: crate::lexer::Span,
    member_len: usize,
) -> crate::lexer::Span {
    use crate::lexer::Span;
    let slice = &source[expr_span.start..expr_span.end];
    match slice.rfind('.') {
        Some(dot) => {
            let after = slice[dot + 1..]
                .bytes()
                .take_while(|b| b.is_ascii_whitespace())
                .count();
            let start = expr_span.start + dot + 1 + after;
            Span::new(start, (start + member_len).min(expr_span.end))
        }
        None => expr_span,
    }
}

/// The cross-file *value* target at `offset`, as (defining module, member): a
/// qualified reference (`Geometry.area`), or a bare module-level name resolved
/// against the current file's own module. `None` for a local binder or no symbol.
fn value_target(
    module: &crate::syntax::Module,
    uri: &str,
    offset: usize,
) -> Option<(String, String)> {
    if let Some(q) = resolve::qualified_at(module, offset) {
        return Some((q.module, q.member));
    }
    match resolve::symbol_at(module, offset) {
        Some((_, resolve::Target::Module(name))) => Some((current_module(uri)?, name)),
        _ => None,
    }
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

/// A content fingerprint for cache validation. An analysis is a pure function
/// of the entry document's text plus the text of every imported module file it
/// consulted, so equal fingerprints of all of them prove an equal result.
fn fingerprint(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// The module files an analysis (or an exports computation) consulted: each
/// file's URI with the fingerprint of the content found there — `None` when the
/// file was missing/unreadable, recording the *absence* so that creating the
/// file later also invalidates.
type Deps = Vec<(String, Option<u64>)>;

/// A cached per-document analysis: valid while the document is still at
/// `version` **and** every imported module file consulted during the analysis
/// (`deps`) still has the same content fingerprint — so editing an imported
/// file (in an open buffer or on disk) re-analyzes its dependents on their
/// next request.
struct CachedAnalysis {
    version: u64,
    deps: Deps,
    analysis: Rc<Analysis>,
}

/// A project-wide cached module interface, keyed by the module file's URI: its
/// checked exports plus every file its computation consulted (itself first,
/// then its transitive imports). Fresh iff every dep fingerprint still matches,
/// in which case the exports are provably identical (they are a pure function
/// of those sources).
struct CachedExports {
    deps: Deps,
    exports: crate::types::ModuleExports,
}

/// State for one import-resolution pass (one analysis): modules already
/// resolved in this pass — mirroring the per-call memo of
/// `project::resolve_imports` — and the DFS `visiting` set that breaks import
/// cycles. The memoized flag marks a **tainted** result: one computed in a
/// cycle context (an import was skipped because it was being visited), which is
/// context-dependent and must not enter the project-wide exports cache.
#[derive(Default)]
struct ResolvePass {
    memo: HashMap<String, (crate::types::ModuleExports, bool)>,
    visiting: HashSet<String>,
}

/// The server state: open documents by URI, the two-level **analysis cache**
/// (per-document analyses plus project-wide module interfaces, both validated
/// by content fingerprints — the "incremental" half), plus a flag set by
/// `exit`.
#[derive(Default)]
pub struct Server {
    documents: HashMap<String, Doc>,
    /// Cached analysis per URI, tagged with the document version and the
    /// imported files it was computed from; a stale entry (older version, or a
    /// changed/appeared/vanished import) is recomputed on demand. `RefCell`
    /// because the read handlers take `&self`.
    cache: RefCell<HashMap<String, CachedAnalysis>>,
    /// Project-wide cache of imported modules' export interfaces, keyed by the
    /// module file's URI, so two open documents importing `Geometry` share one
    /// parse + check of `geometry.pyfun` across requests (`DESIGN.md` §9).
    exports: RefCell<HashMap<String, CachedExports>>,
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
            Some("workspace/symbol") => {
                let result = self.workspace_symbols(msg);
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

    /// The analysis of `uri`, served from the cache when the document **and
    /// every imported file its analysis consulted** are unchanged since it was
    /// computed, otherwise analyzed fresh and cached. This is what makes
    /// repeated requests (hover, then go-to-def, then references) on one
    /// document version share a single parse + type-check — and what re-analyzes
    /// a dependent when one of its imports changes.
    fn analysis(&self, uri: &str) -> Option<Rc<Analysis>> {
        let doc = self.documents.get(uri)?;
        if let Some(entry) = self.cache.borrow().get(uri)
            && entry.version == doc.version
            && self.deps_fresh(&entry.deps)
        {
            return Some(entry.analysis.clone());
        }
        // Resolve `import`s against sibling modules (open buffer preferred,
        // else disk — `DESIGN.md` §6.1), reusing the project-wide exports
        // cache, and record every file consulted (`deps`) so this entry is
        // invalidated when one of them changes.
        let mut deps = Vec::new();
        let analysis = Rc::new(crate::analyze_with_imports(&doc.text, |module| {
            self.resolve_imports_cached(uri, module, &mut deps)
        }));
        self.cache.borrow_mut().insert(
            uri.to_string(),
            CachedAnalysis {
                version: doc.version,
                deps,
                analysis: analysis.clone(),
            },
        );
        Some(analysis)
    }

    /// The current source of the module file at `uri`: the open buffer when the
    /// file is open in the editor, else the on-disk content (the convention of
    /// every cross-file feature, e.g. [`locate_cross_file`](Self::locate_cross_file)
    /// — so an unsaved edit to an imported file is seen).
    fn module_source(&self, uri: &str) -> Option<String> {
        if let Some(doc) = self.documents.get(uri) {
            return Some(doc.text.clone());
        }
        std::fs::read_to_string(uri_to_path(uri)?).ok()
    }

    /// The fingerprint of the module file at `uri` as it stands right now
    /// (open buffer preferred, else disk); `None` when it is unreadable.
    fn current_fingerprint(&self, uri: &str) -> Option<u64> {
        if let Some(doc) = self.documents.get(uri) {
            return Some(fingerprint(&doc.text));
        }
        let source = std::fs::read_to_string(uri_to_path(uri)?).ok()?;
        Some(fingerprint(&source))
    }

    /// Whether every recorded dependency still has the content it had when the
    /// cache entry was computed (including still-absent for a `None` entry).
    fn deps_fresh(&self, deps: &Deps) -> bool {
        deps.iter()
            .all(|(uri, fp)| self.current_fingerprint(uri) == *fp)
    }

    /// Resolve `module`'s imports to their export interfaces, mirroring the
    /// forgiving `project::resolve_imports` (a missing/broken/cyclic import is
    /// simply omitted) but through the caches: see
    /// [`resolve_exports_cached`](Self::resolve_exports_cached). `deps` collects
    /// every file consulted, for the caller's own cache validation.
    fn resolve_imports_cached(
        &self,
        uri: &str,
        module: &crate::syntax::Module,
        deps: &mut Deps,
    ) -> HashMap<String, crate::types::ModuleExports> {
        let mut pass = ResolvePass::default();
        let mut out = HashMap::new();
        // The entry document's own analysis is keyed by its version + deps (its
        // cycle context is fixed — resolution always starts here), so taint
        // only matters for the shared exports cache, not at this level.
        let mut tainted = false;
        for item in &module.items {
            if let crate::syntax::Item::Import { name, .. } = item
                && let Some(exports) =
                    self.resolve_exports_cached(uri, name, &mut pass, deps, &mut tainted)
            {
                out.insert(name.clone(), exports);
            }
        }
        out
    }

    /// Resolve the export interface of the imported module `name` (a sibling of
    /// `base_uri`), mirroring the compiler-side `project::resolve_exports`
    /// (parse + recursively resolve its own imports + `check_module`; a missing
    /// file, parse error, or cycle yields `None`) with three editor additions:
    /// the source comes from the open buffer when the file is open (else disk);
    /// the **project-wide exports cache** is consulted first — an entry is
    /// reused when every file its computation consulted still fingerprints the
    /// same, which proves the exports identical; and every file consulted here
    /// is appended to `deps` so the caller's cache entry can be validated the
    /// same way later.
    ///
    /// `caller_tainted` is set when the result is context-dependent — a cycle
    /// made us skip an import — in which case it must not enter the project-wide
    /// cache (a different entry document would resolve the cycle from a
    /// different side); the per-pass memo still reuses it within this pass.
    fn resolve_exports_cached(
        &self,
        base_uri: &str,
        name: &str,
        pass: &mut ResolvePass,
        deps: &mut Deps,
        caller_tainted: &mut bool,
    ) -> Option<crate::types::ModuleExports> {
        if pass.visiting.contains(name) {
            *caller_tainted = true; // the caller is checked without this import
            return None;
        }
        if let Some((exports, tainted)) = pass.memo.get(name) {
            *caller_tainted |= *tainted;
            return Some(exports.clone());
        }
        let uri = sibling_uri(base_uri, &crate::project::module_file_name(name));
        let Some(source) = self.module_source(&uri) else {
            deps.push((uri, None)); // record the absence: creating it invalidates
            return None;
        };
        // Project-wide cache hit: this module and everything its computation
        // read are unchanged, so its exports are identical.
        if let Some(entry) = self.exports.borrow().get(&uri)
            && self.deps_fresh(&entry.deps)
        {
            deps.extend(entry.deps.iter().cloned());
            pass.memo
                .insert(name.to_string(), (entry.exports.clone(), false));
            return Some(entry.exports.clone());
        }
        let mut my_deps = vec![(uri.clone(), Some(fingerprint(&source)))];
        let Ok(module) = crate::parse(&source) else {
            deps.extend(my_deps); // fingerprinted, so fixing the file invalidates
            return None;
        };
        pass.visiting.insert(name.to_string());
        let mut imports = HashMap::new();
        let mut tainted = false;
        for item in &module.items {
            if let crate::syntax::Item::Import { name: import, .. } = item
                && let Some(exports) =
                    self.resolve_exports_cached(&uri, import, pass, &mut my_deps, &mut tainted)
            {
                imports.insert(import.clone(), exports);
            }
        }
        pass.visiting.remove(name);
        let (_errors, exports) = crate::types::check_module(&module, &imports);
        deps.extend(my_deps.iter().cloned());
        if !tainted {
            self.exports.borrow_mut().insert(
                uri,
                CachedExports {
                    deps: my_deps,
                    exports: exports.clone(),
                },
            );
        }
        pass.memo
            .insert(name.to_string(), (exports.clone(), tainted));
        *caller_tainted |= tainted;
        Some(exports)
    }

    /// Build a `publishDiagnostics` notification for `uri` from its analysis.
    fn diagnostics_for(&self, uri: &str) -> Json {
        let (Some(text), Some(analysis)) = (self.text(uri), self.analysis(uri)) else {
            return publish_diagnostics(uri, Json::Array(vec![]));
        };
        let mut items: Vec<Json> = analysis
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
        // Typed holes are published at Information severity (3) — an intentional
        // blank reporting its type, not a mistake.
        for h in &analysis.holes {
            items.push(obj(vec![
                ("range", span_range(text, h.span)),
                ("severity", int(3)), // Information
                ("source", str("pyfun")),
                ("message", str(h.message())),
            ]));
        }
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
        // A `##` doc comment attached to the declaration under the cursor — the
        // declaration name itself or any reference resolving to it — is appended
        // below the type (`DESIGN.md` §9).
        let doc = analysis.module.as_ref().and_then(|module| {
            let (span, target) = resolve::symbol_at(module, offset)?;
            match target {
                resolve::Target::Module(name) => item_doc(module, &name).map(|d| (span, d)),
                resolve::Target::Local(_) => None,
            }
        });
        match (best, doc) {
            (None, None) => Json::Null,
            (Some(t), doc) => {
                let mut value = format!("```pyfun\n{}\n```", t.ty);
                // A dedicated effect line when the value performs a concrete effect
                // on full application — a quicker read than spotting `->{io}` inline,
                // and it surfaces the effect buried on a curried function's inner
                // arrow (`DESIGN.md` §4, §9).
                if let Some(eff) = &t.effect {
                    value.push_str(&format!("\n\n**Effect:** `{eff}`"));
                }
                if let Some((_, doc)) = doc {
                    value.push_str("\n\n---\n\n");
                    value.push_str(&doc_to_markdown(&doc));
                }
                obj(vec![
                    (
                        "contents",
                        obj(vec![("kind", str("markdown")), ("value", str(value))]),
                    ),
                    ("range", span_range(text, t.span)),
                ])
            }
            // No inferred type under the cursor (e.g. an `extern` or `type` name,
            // which the collecting pass doesn't record) but the symbol has a doc.
            (None, Some((span, doc))) => obj(vec![
                (
                    "contents",
                    obj(vec![
                        ("kind", str("markdown")),
                        ("value", str(doc_to_markdown(&doc))),
                    ]),
                ),
                ("range", span_range(text, span)),
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

        // A qualified reference to an *imported file module* (`Geometry.area`,
        // `Geometry.Circle`) jumps across files to the definition in that module's
        // `.pyfun` (`DESIGN.md` §6.1). Built-in / in-file module members fall
        // through to the in-file resolver below.
        if let Some(q) = resolve::qualified_at(module, offset)
            && module_imports(module, &q.module)
            && let Some(location) = self.locate_cross_file(uri, &q.module, &q.member)
        {
            return location;
        }

        // A type-name occurrence jumps to its (in-file) declaration.
        if let Some((name, _)) = resolve::type_at(module, offset)
            && let Some(span) = user_type_decl_span(module, &name)
        {
            return obj(vec![("uri", str(uri)), ("range", span_range(text, span))]);
        }

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

    /// Resolve a qualified reference to an imported file module (`Geometry.area`)
    /// to a `Location` in that module's `.pyfun` file: find its sibling URI, read
    /// it (preferring an open buffer over disk), and locate the member's
    /// definition span. `None` if the file or member can't be found.
    fn locate_cross_file(&self, uri: &str, module_name: &str, member: &str) -> Option<Json> {
        let target_uri = sibling_uri(uri, &crate::project::module_file_name(module_name));
        let source = match self.documents.get(&target_uri) {
            Some(doc) => doc.text.clone(),
            None => std::fs::read_to_string(uri_to_path(&target_uri)?).ok()?,
        };
        let module = crate::parse(&source).ok()?;
        let sym = resolve::definitions(&module)
            .into_iter()
            .find(|s| s.name == member)?;
        Some(obj(vec![
            ("uri", str(&target_uri)),
            ("range", span_range(&source, sym.span)),
        ]))
    }

    /// Every occurrence of the top-level **value or constructor** `member` defined
    /// in module `target_module`, across the project directory's `.pyfun` files: its
    /// definition (when `include_decl`), its bare uses in the defining file, and its
    /// qualified uses (`Geometry.member`) elsewhere — as `(file uri, range)` pairs
    /// (`DESIGN.md` §6.1). A constructor's uses include both construction
    /// expressions and patterns (the resolver records pattern constructors in the
    /// same channels). The kind sought is chosen by the name's case
    /// ([`symbol_kind_of`]). `None` if `member` is not such a definition of
    /// `target_module` (a type, or a built-in, is not cross-file renameable here),
    /// the directory can't be read, or — when `strict` — some project file fails to
    /// parse (so a rewrite could miss an occurrence). Reads each file from its open
    /// buffer if present, else disk.
    fn symbol_occurrences(
        &self,
        uri: &str,
        target_module: &str,
        member: &str,
        include_decl: bool,
        strict: bool,
    ) -> Option<Vec<(String, Json)>> {
        let wanted_kind = symbol_kind_of(member);
        let dir = uri_to_path(uri)?.parent()?.to_path_buf();
        let mut files: Vec<String> = std::fs::read_dir(&dir)
            .ok()?
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                (p.extension().and_then(|x| x.to_str()) == Some("pyfun"))
                    .then(|| p.file_name()?.to_str().map(str::to_string))
                    .flatten()
            })
            .collect();
        files.sort(); // deterministic output

        let mut out: Vec<(String, Json)> = Vec::new();
        let mut defining_found = false;
        for fname in files {
            let file_uri = sibling_uri(uri, &fname);
            let source = match self.documents.get(&file_uri) {
                Some(doc) => doc.text.clone(),
                None => match std::fs::read_to_string(dir.join(&fname)) {
                    Ok(s) => s,
                    Err(_) => continue,
                },
            };
            let Ok(m) = crate::parse(&source) else {
                if strict {
                    return None; // an unparseable file might hide an occurrence
                }
                continue;
            };
            let is_defining = crate::project::module_name_from_path(Path::new(&fname)).as_deref()
                == Some(target_module);
            if is_defining {
                // The defining file: the definition (a *value*) plus its bare uses.
                if let Some(sym) = resolve::definitions(&m)
                    .into_iter()
                    .find(|s| s.name == member && s.kind == wanted_kind)
                {
                    defining_found = true;
                    if include_decl {
                        out.push((file_uri.clone(), span_range(&source, sym.span)));
                    }
                }
                for r in resolve::references(&m) {
                    if r.target == resolve::Target::Module(member.to_string()) {
                        out.push((file_uri.clone(), span_range(&source, r.span)));
                    }
                }
            } else {
                // An importer: qualified uses `target_module.member` (rewrite the
                // member identifier only, keeping the `Module.` qualifier).
                for q in resolve::qualified_references(&m) {
                    if q.module == target_module && q.member == member {
                        let span = member_subspan(&source, q.span, member.len());
                        out.push((file_uri.clone(), span_range(&source, span)));
                    }
                }
            }
        }
        defining_found.then_some(out)
    }

    /// Find-references: every occurrence of the symbol under the cursor (a `Local`
    /// binder or a `Module` symbol), returned as an array of `Location`s. Honours
    /// the request's `context.includeDeclaration` (default `true`). The cursor may
    /// sit on a use *or* the definition/binder itself. A top-level value is searched
    /// across the whole project; a local stays within the file.
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

        // A type name: its in-file annotation uses, plus its declaration when asked.
        if let Some((name, _)) = resolve::type_at(module, offset) {
            let mut spans = resolve::type_use_references(module, &name);
            if include_declaration && let Some(decl) = user_type_decl_span(module, &name) {
                spans.push(decl);
            }
            let locations = spans
                .into_iter()
                .map(|span| obj(vec![("uri", str(uri)), ("range", span_range(text, span))]))
                .collect();
            return Json::Array(locations);
        }

        // A top-level value or constructor (bare or qualified) is searched across
        // all project files; anything else (a local binder) stays within this file.
        if let Some((tmod, member)) = value_target(module, uri, offset)
            && let Some(occ) =
                self.symbol_occurrences(uri, &tmod, &member, include_declaration, false)
        {
            let locations = occ
                .into_iter()
                .map(|(u, range)| obj(vec![("uri", str(u)), ("range", range)]))
                .collect();
            return Json::Array(locations);
        }

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
        let uri = msg
            .get("params")
            .and_then(|p| p.get("textDocument"))
            .and_then(|d| d.get("uri"))
            .and_then(Json::as_str)
            .unwrap_or_default();
        let Some((text, offset, analysis)) = self.locate(msg) else {
            return Json::Null;
        };
        // Refuse rename unless the document fully parses: a partial module could
        // hide occurrences in the unparsed region, making the rewrite unsound.
        let Some(module) = analysis.module.as_ref().filter(|_| analysis.parse_ok) else {
            return Json::Null;
        };
        // A user type: the editable range is the type-name occurrence.
        if let Some((name, span)) = resolve::type_at(module, offset) {
            return if user_type_decl_span(module, &name).is_some() {
                span_range(text, span)
            } else {
                Json::Null // a builtin type is not renameable
            };
        }
        // A cross-file value or constructor: the editable range is the member
        // identifier (the part a qualified `Geometry.area` rewrites, or a bare
        // name's own span).
        if let Some((tmod, member)) = value_target(module, uri, offset)
            && self
                .symbol_occurrences(uri, &tmod, &member, true, true)
                .is_some()
        {
            return if let Some(q) = resolve::qualified_at(module, offset) {
                span_range(text, member_subspan(text, q.span, q.member.len()))
            } else if let Some((span, _)) = resolve::symbol_at(module, offset) {
                span_range(text, span)
            } else {
                Json::Null
            };
        }
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
        // Only rename when the document fully parses (see `prepare_rename`).
        let Some(module) = analysis.module.as_ref().filter(|_| analysis.parse_ok) else {
            return Json::Null;
        };
        // A user type renames in-file (type names have no cross-file dimension):
        // its declaration plus its annotation uses. A type renames to a type
        // (uppercase identifier).
        if let Some((name, _)) = resolve::type_at(module, offset)
            && let Some(decl_span) = user_type_decl_span(module, &name)
        {
            if !is_ctor_identifier(new_name) {
                return Json::Null; // a type renames to an uppercase type name
            }
            let mut spans = resolve::type_use_references(module, &name);
            spans.push(decl_span);
            let edits: Vec<Json> = spans
                .into_iter()
                .map(|span| {
                    obj(vec![
                        ("range", span_range(text, span)),
                        ("newText", str(new_name)),
                    ])
                })
                .collect();
            return obj(vec![("changes", obj(vec![(uri, Json::Array(edits))]))]);
        }

        // A top-level value or constructor renames across the whole project (its
        // definition, its bare uses in the defining file, and every qualified use —
        // construction *and* pattern — elsewhere). The non-strict scan decides
        // whether the target *is* such a symbol; if so, a strict scan must succeed
        // (every project file parses) or we refuse — never silently fall back to an
        // incomplete in-file-only rewrite.
        if let Some((tmod, member)) = value_target(module, uri, offset)
            && self
                .symbol_occurrences(uri, &tmod, &member, true, false)
                .is_some()
        {
            // A value renames to a value, a constructor to a constructor.
            if !valid_rename(&member, new_name) {
                return Json::Null;
            }
            let Some(occ) = self.symbol_occurrences(uri, &tmod, &member, true, true) else {
                return Json::Null;
            };
            let mut by_uri: Vec<(String, Vec<Json>)> = Vec::new();
            for (u, range) in occ {
                let edit = obj(vec![("range", range), ("newText", str(new_name))]);
                match by_uri.iter_mut().find(|(k, _)| *k == u) {
                    Some((_, edits)) => edits.push(edit),
                    None => by_uri.push((u, vec![edit])),
                }
            }
            let changes = by_uri
                .into_iter()
                .map(|(u, edits)| (u, Json::Array(edits)))
                .collect();
            return Json::Object(vec![("changes".to_string(), Json::Object(changes))]);
        }
        let Some((_, target)) = resolve::symbol_at(module, offset) else {
            return Json::Null;
        };
        if !is_renameable(module, &target) || !is_value_identifier(new_name) {
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

    /// Workspace symbols: every module-level definition across the project's
    /// `.pyfun` files (the flat single-directory namespace), filtered by the query
    /// substring (case-insensitive), as `SymbolInformation[]` with cross-file
    /// `Location`s. Scans the directories of all open documents — reading each
    /// sibling file from its open buffer if present, else from disk (`DESIGN.md`
    /// §6.1). Cross-file LSP navigation; rich x-file find-references / rename
    /// remain deferred (the latter needs constructor-pattern spans).
    fn workspace_symbols(&self, msg: &Json) -> Json {
        let query = msg
            .get("params")
            .and_then(|p| p.get("query"))
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_lowercase();
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for doc_uri in self.documents.keys() {
            let Some(dir) = uri_to_path(doc_uri).and_then(|p| p.parent().map(Path::to_path_buf))
            else {
                continue;
            };
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            let mut files: Vec<String> = entries
                .flatten()
                .filter_map(|e| {
                    let p = e.path();
                    (p.extension().and_then(|x| x.to_str()) == Some("pyfun"))
                        .then(|| p.file_name()?.to_str().map(str::to_string))
                        .flatten()
                })
                .collect();
            files.sort(); // deterministic output
            for fname in files {
                let file_uri = sibling_uri(doc_uri, &fname);
                if !seen.insert(file_uri.clone()) {
                    continue;
                }
                let source = match self.documents.get(&file_uri) {
                    Some(doc) => doc.text.clone(),
                    None => match std::fs::read_to_string(dir.join(&fname)) {
                        Ok(s) => s,
                        Err(_) => continue,
                    },
                };
                let Ok(module) = crate::parse(&source) else {
                    continue;
                };
                for sym in resolve::definitions(&module) {
                    if !query.is_empty() && !sym.name.to_lowercase().contains(&query) {
                        continue;
                    }
                    out.push(obj(vec![
                        ("name", str(&sym.name)),
                        ("kind", int(symbol_kind(sym.kind))),
                        (
                            "location",
                            obj(vec![
                                ("uri", str(&file_uri)),
                                ("range", span_range(&source, sym.span)),
                            ]),
                        ),
                    ]));
                }
            }
        }
        Json::Array(out)
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

/// Whether `name` is a valid constructor identifier (uppercase-leading word
/// characters, not a keyword) — a constructor renames only to another constructor.
fn is_ctor_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_uppercase())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !is_keyword(name)
}

/// The declaration span of a *user* type `name` (a `type`/record decl in this
/// module), if any — so only user types, not builtins, are go-to-def / rename
/// targets.
/// Render doc-comment text for a Markdown hover. `collect_docs` joins successive
/// `##` lines with a single `\n`, but Markdown collapses a lone newline into a
/// space — so promote each line break to a hard break (two trailing spaces),
/// preserving the author's line structure in the hover popup.
fn doc_to_markdown(doc: &str) -> String {
    doc.replace('\n', "  \n")
}

/// The doc comment attached to the module-level declaration named `name`
/// (a top-level `let`, `type`, or `extern`), if any.
fn item_doc(module: &crate::syntax::Module, name: &str) -> Option<String> {
    use crate::syntax::Item;
    module.items.iter().find_map(|item| match item {
        Item::Let(binding) if binding.name == name => binding.doc.clone(),
        Item::Type(decl) if decl.name == name => decl.doc.clone(),
        Item::Extern(decl) if decl.name == name => decl.doc.clone(),
        _ => None,
    })
}

fn user_type_decl_span(module: &crate::syntax::Module, name: &str) -> Option<crate::lexer::Span> {
    resolve::definitions(module).into_iter().find_map(|s| {
        (s.name == name
            && matches!(
                s.kind,
                resolve::SymbolKind::Type | resolve::SymbolKind::Record
            ))
        .then_some(s.span)
    })
}

/// Whether `new_name` is a valid rename for a symbol named `member`: a
/// constructor (uppercase-leading) renames to a constructor, a value to a value —
/// so a rename can't turn a value into a constructor or vice-versa.
fn valid_rename(member: &str, new_name: &str) -> bool {
    if member
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_uppercase())
    {
        is_ctor_identifier(new_name)
    } else {
        is_value_identifier(new_name)
    }
}

/// The definition kind a cross-file symbol search matches for `member`: a
/// constructor when the name is uppercase-leading, otherwise a value.
fn symbol_kind_of(member: &str) -> resolve::SymbolKind {
    if member
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_uppercase())
    {
        resolve::SymbolKind::Constructor
    } else {
        resolve::SymbolKind::Value
    }
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
    "Option", "Decoder",
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
                ("workspaceSymbolProvider", Json::Bool(true)),
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

    /// Extract the hover markdown `value` from a handled hover response.
    fn hover_value(server: &mut Server, uri: &str, line: i64, character: i64) -> String {
        let out = server.handle(&json::parse(&hover_msg(uri, line, character)).unwrap());
        out[0]
            .get("result")
            .unwrap()
            .get("contents")
            .unwrap()
            .get("value")
            .unwrap()
            .as_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn hover_appends_doc_comment_on_declaration_and_reference() {
        let mut server = Server::default();
        let uri = "file:///doc.pyfun";
        let src = "## Doubles a number.\nlet double x = x * 2\nlet r = double 4";
        server.handle(&json::parse(&open_msg(uri, src)).unwrap());
        // Hover the declaration name `double` (line 1, char 5): type + doc.
        let value = hover_value(&mut server, uri, 1, 5);
        assert!(value.contains("int"), "hover value was {value:?}");
        assert!(
            value.contains("Doubles a number."),
            "hover value was {value:?}"
        );
        // Hover the *reference* `double` (line 2, char 9): same doc.
        let value = hover_value(&mut server, uri, 2, 9);
        assert!(
            value.contains("Doubles a number."),
            "hover value was {value:?}"
        );
        // An undocumented symbol shows no doc separator.
        let value = hover_value(&mut server, uri, 2, 4); // `r`
        assert!(!value.contains("---"), "hover value was {value:?}");
    }

    #[test]
    fn hover_shows_a_dedicated_effect_line_for_an_impure_function() {
        let mut server = Server::default();
        let uri = "file:///eff.pyfun";
        // `go` prints, so it performs `io`; hovering its name adds the effect line.
        let src = "let go u = print u";
        server.handle(&json::parse(&open_msg(uri, src)).unwrap());
        let value = hover_value(&mut server, uri, 0, 4); // `go`
        assert!(
            value.contains("**Effect:** `io`"),
            "hover value was {value:?}"
        );
    }

    #[test]
    fn hover_omits_the_effect_line_for_a_pure_function() {
        let mut server = Server::default();
        let uri = "file:///pure.pyfun";
        let src = "let add a b = a + b";
        server.handle(&json::parse(&open_msg(uri, src)).unwrap());
        let value = hover_value(&mut server, uri, 0, 4); // `add`
        assert!(
            !value.contains("**Effect:**"),
            "hover value was {value:?}"
        );
    }

    #[test]
    fn hover_effect_line_reports_async_from_an_async_block() {
        let mut server = Server::default();
        let uri = "file:///asy.pyfun";
        let src = "let go x = async { return x }";
        server.handle(&json::parse(&open_msg(uri, src)).unwrap());
        let value = hover_value(&mut server, uri, 0, 4); // `go`
        assert!(
            value.contains("**Effect:** `async`"),
            "hover value was {value:?}"
        );
    }

    #[test]
    fn hover_shows_doc_alone_on_a_type_declaration_name() {
        // A `type` name has no entry in the inferred-type table, so the hover is
        // doc-only.
        let mut server = Server::default();
        let uri = "file:///doct.pyfun";
        let src = "## A 2D point.\ntype Point = { x: int, y: int }";
        server.handle(&json::parse(&open_msg(uri, src)).unwrap());
        let value = hover_value(&mut server, uri, 1, 6);
        assert!(value.contains("A 2D point."), "hover value was {value:?}");
    }

    #[test]
    fn hover_preserves_line_breaks_in_a_multi_line_doc_comment() {
        // Successive `##` lines join with `\n`; the hover promotes each to a
        // Markdown hard break (two trailing spaces) so the lines don't collapse.
        let mut server = Server::default();
        let uri = "file:///docm.pyfun";
        let src = "## First line.\n## Second line.\ntype Signal = Walk | Wait";
        server.handle(&json::parse(&open_msg(uri, src)).unwrap());
        let value = hover_value(&mut server, uri, 2, 6);
        assert!(
            value.contains("First line.  \nSecond line."),
            "hover value was {value:?}"
        );
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
    fn a_typed_hole_is_published_at_information_severity_and_hovers_its_type() {
        let mut server = Server::default();
        let uri = "file:///hole.pyfun";
        let src = "let f = ?body + 1";
        let out = server.handle(&json::parse(&open_msg(uri, src)).unwrap());
        let diags = out[0]
            .get("params")
            .unwrap()
            .get("diagnostics")
            .unwrap()
            .as_array()
            .unwrap();
        assert_eq!(diags.len(), 1);
        // Severity 3 = Information (an intentional blank, not a red error).
        assert_eq!(diags[0].get("severity").unwrap().as_i64(), Some(3));
        assert!(
            diags[0]
                .get("message")
                .unwrap()
                .as_str()
                .unwrap()
                .contains("hole `?body`"),
            "diagnostic was {:?}",
            diags[0]
        );
        // Hover on the hole shows its inferred type.
        let value = hover_value(&mut server, uri, 0, 9); // the `?body`
        assert!(value.contains("int"), "hover value was {value:?}");
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

    /// Build a `file:` URI for `path` (Windows `file:///C:/…`, Unix `file:///…`).
    fn file_uri(path: &std::path::Path) -> String {
        let p = path.to_string_lossy().replace('\\', "/");
        if p.starts_with('/') {
            format!("file://{p}")
        } else {
            format!("file:///{p}")
        }
    }

    #[test]
    fn goto_definition_jumps_across_files() {
        let dir = std::env::temp_dir().join(format!("pyfun_lsp_gotodef_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("geometry.pyfun"), "let area w h = w * h").unwrap();
        let main_uri = file_uri(&dir.join("main.pyfun"));
        let geom_uri = file_uri(&dir.join("geometry.pyfun"));

        let mut server = Server::default();
        server.handle(
            &json::parse(&open_msg(
                &main_uri,
                "import Geometry\nlet floor = Geometry.area 4 5",
            ))
            .unwrap(),
        );
        // Cursor on `Geometry.area` (line 1, inside the qualified reference).
        let req = pos_msg("textDocument/definition", &main_uri, 1, 16);
        let out = server.handle(&json::parse(&req).unwrap());
        let result = out[0].get("result").unwrap();
        assert_eq!(result.get("uri").unwrap().as_str(), Some(geom_uri.as_str()));
        // `area` is defined on line 0 of geometry.pyfun.
        let line = result
            .get("range")
            .unwrap()
            .get("start")
            .unwrap()
            .get("line")
            .unwrap()
            .as_i64();
        assert_eq!(line, Some(0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn goto_definition_on_a_qualified_record_tag_jumps_across_files() {
        // A qualified record literal `Geometry.Point { … }` records a cross-file
        // module ref on its tag, so go-to-def jumps to the `type Point = { … }` decl.
        let dir = std::env::temp_dir().join(format!("pyfun_lsp_rectag_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("geometry.pyfun"),
            "type Point = { x: int, y: int }",
        )
        .unwrap();
        let main_uri = file_uri(&dir.join("main.pyfun"));
        let geom_uri = file_uri(&dir.join("geometry.pyfun"));

        let mut server = Server::default();
        server.handle(
            &json::parse(&open_msg(
                &main_uri,
                "import Geometry\nlet p = Geometry.Point { x = 1, y = 2 }",
            ))
            .unwrap(),
        );
        // Cursor on `Geometry.Point` (line 1, within the tag, before the `{`).
        let req = pos_msg("textDocument/definition", &main_uri, 1, 18);
        let out = server.handle(&json::parse(&req).unwrap());
        let result = out[0].get("result").unwrap();
        assert_eq!(result.get("uri").unwrap().as_str(), Some(geom_uri.as_str()));
        let line = result
            .get("range")
            .unwrap()
            .get("start")
            .unwrap()
            .get("line")
            .unwrap()
            .as_i64();
        assert_eq!(line, Some(0), "jumps to the `type Point` decl on line 0");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn workspace_symbols_span_the_project_files() {
        let dir = std::env::temp_dir().join(format!("pyfun_lsp_wsym_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("geometry.pyfun"), "let area w h = w * h").unwrap();
        let main_uri = file_uri(&dir.join("main.pyfun"));
        let geom_uri = file_uri(&dir.join("geometry.pyfun"));

        let mut server = Server::default();
        server
            .handle(&json::parse(&open_msg(&main_uri, "import Geometry\nlet floor = 1")).unwrap());
        let req = obj(vec![
            ("jsonrpc", str("2.0")),
            ("id", int(1)),
            ("method", str("workspace/symbol")),
            ("params", obj(vec![("query", str("area"))])),
        ])
        .to_string();
        let out = server.handle(&json::parse(&req).unwrap());
        let syms = out[0].get("result").unwrap().as_array().unwrap();
        // `area` lives in geometry.pyfun, found from the open `main` document's dir.
        let area = syms
            .iter()
            .find(|s| s.get("name").and_then(Json::as_str) == Some("area"))
            .expect("workspace symbols should include `area`");
        assert_eq!(
            area.get("location").unwrap().get("uri").unwrap().as_str(),
            Some(geom_uri.as_str())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_references_spans_files() {
        let dir = std::env::temp_dir().join(format!("pyfun_lsp_xrefs_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("geometry.pyfun"), "let area w h = w * h").unwrap();
        let main_src = "import Geometry\nlet floor = Geometry.area 4 5";
        std::fs::write(dir.join("main.pyfun"), main_src).unwrap();
        let main_uri = file_uri(&dir.join("main.pyfun"));
        let geom_uri = file_uri(&dir.join("geometry.pyfun"));

        let mut server = Server::default();
        server.handle(&json::parse(&open_msg(&main_uri, main_src)).unwrap());
        // References from the qualified use `Geometry.area` (line 1, on `area`).
        let req = references_msg(&main_uri, 1, 22, true);
        let out = server.handle(&json::parse(&req).unwrap());
        let locs = out[0].get("result").unwrap().as_array().unwrap();
        let uris: std::collections::HashSet<&str> = locs
            .iter()
            .filter_map(|l| l.get("uri").and_then(Json::as_str))
            .collect();
        assert!(
            uris.contains(geom_uri.as_str()),
            "missing def file: {uris:?}"
        );
        assert!(
            uris.contains(main_uri.as_str()),
            "missing use file: {uris:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rename_value_rewrites_across_files() {
        let dir = std::env::temp_dir().join(format!("pyfun_lsp_xrename_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("geometry.pyfun"), "let area w h = w * h").unwrap();
        let main_src = "import Geometry\nlet floor = Geometry.area 4 5";
        std::fs::write(dir.join("main.pyfun"), main_src).unwrap();
        let main_uri = file_uri(&dir.join("main.pyfun"));
        let geom_uri = file_uri(&dir.join("geometry.pyfun"));

        let mut server = Server::default();
        server.handle(&json::parse(&open_msg(&main_uri, main_src)).unwrap());
        let req = obj(vec![
            ("jsonrpc", str("2.0")),
            ("id", int(1)),
            ("method", str("textDocument/rename")),
            (
                "params",
                obj(vec![
                    ("textDocument", obj(vec![("uri", str(&main_uri))])),
                    (
                        "position",
                        obj(vec![("line", int(1)), ("character", int(22))]),
                    ),
                    ("newName", str("surface")),
                ]),
            ),
        ])
        .to_string();
        let out = server.handle(&json::parse(&req).unwrap());
        let changes = out[0].get("result").unwrap().get("changes").unwrap();
        // Both files are edited; the qualified use rewrites only the member.
        let geom_edits = changes.get(&geom_uri).unwrap().as_array().unwrap();
        let main_edits = changes.get(&main_uri).unwrap().as_array().unwrap();
        assert_eq!(geom_edits.len(), 1);
        assert_eq!(main_edits.len(), 1);
        assert_eq!(
            geom_edits[0].get("newText").and_then(Json::as_str),
            Some("surface")
        );
        // The def edit is on line 0 of geometry.pyfun (`area`).
        let line = geom_edits[0]
            .get("range")
            .unwrap()
            .get("start")
            .unwrap()
            .get("line")
            .unwrap()
            .as_i64();
        assert_eq!(line, Some(0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_references_for_a_constructor_spans_construction_and_patterns() {
        let dir = std::env::temp_dir().join(format!("pyfun_lsp_ctorref_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("shape.pyfun"),
            "type Shape = Circle float | Rect float float",
        )
        .unwrap();
        let main_src = "import Shape\n\
             let c = Shape.Circle 2.0\n\
             let describe x =\n  match x:\n    case Shape.Circle r: r\n    case Shape.Rect w h: w";
        std::fs::write(dir.join("main.pyfun"), main_src).unwrap();
        let main_uri = file_uri(&dir.join("main.pyfun"));
        let shape_uri = file_uri(&dir.join("shape.pyfun"));

        let mut server = Server::default();
        server.handle(&json::parse(&open_msg(&main_uri, main_src)).unwrap());
        // From the construction `Shape.Circle` (line 1, on `Circle`).
        let req = references_msg(&main_uri, 1, 16, true);
        let out = server.handle(&json::parse(&req).unwrap());
        let locs = out[0].get("result").unwrap().as_array().unwrap();
        let count = |u: &str| {
            locs.iter()
                .filter(|l| l.get("uri").and_then(Json::as_str) == Some(u))
                .count()
        };
        // The declaration in shape.pyfun, plus construction + pattern in main.
        assert_eq!(count(&shape_uri), 1, "decl: {locs:?}");
        assert_eq!(count(&main_uri), 2, "construction + pattern: {locs:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rename_constructor_rewrites_across_files() {
        let dir = std::env::temp_dir().join(format!("pyfun_lsp_ctorren_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("shape.pyfun"),
            "type Shape = Circle float | Rect float float",
        )
        .unwrap();
        let main_src = "import Shape\n\
             let c = Shape.Circle 2.0\n\
             let describe x =\n  match x:\n    case Shape.Circle r: r\n    case Shape.Rect w h: w";
        std::fs::write(dir.join("main.pyfun"), main_src).unwrap();
        let main_uri = file_uri(&dir.join("main.pyfun"));
        let shape_uri = file_uri(&dir.join("shape.pyfun"));

        let mut server = Server::default();
        server.handle(&json::parse(&open_msg(&main_uri, main_src)).unwrap());
        let req = obj(vec![
            ("jsonrpc", str("2.0")),
            ("id", int(1)),
            ("method", str("textDocument/rename")),
            (
                "params",
                obj(vec![
                    ("textDocument", obj(vec![("uri", str(&main_uri))])),
                    (
                        "position",
                        obj(vec![("line", int(1)), ("character", int(16))]),
                    ),
                    ("newName", str("Disk")),
                ]),
            ),
        ])
        .to_string();
        let out = server.handle(&json::parse(&req).unwrap());
        let changes = out[0].get("result").unwrap().get("changes").unwrap();
        assert_eq!(
            changes.get(&shape_uri).unwrap().as_array().unwrap().len(),
            1
        );
        assert_eq!(changes.get(&main_uri).unwrap().as_array().unwrap().len(), 2);
        assert_eq!(
            changes.get(&shape_uri).unwrap().as_array().unwrap()[0]
                .get("newText")
                .and_then(Json::as_str),
            Some("Disk")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rename_constructor_rejects_a_lowercase_new_name() {
        // A constructor renames only to a constructor (uppercase).
        let dir = std::env::temp_dir().join(format!("pyfun_lsp_ctorbad_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("color.pyfun"), "type Color = Red | Green").unwrap();
        let main_src = "import Color\nlet c = Color.Red";
        std::fs::write(dir.join("main.pyfun"), main_src).unwrap();
        let main_uri = file_uri(&dir.join("main.pyfun"));

        let mut server = Server::default();
        server.handle(&json::parse(&open_msg(&main_uri, main_src)).unwrap());
        let req = obj(vec![
            ("jsonrpc", str("2.0")),
            ("id", int(1)),
            ("method", str("textDocument/rename")),
            (
                "params",
                obj(vec![
                    ("textDocument", obj(vec![("uri", str(&main_uri))])),
                    (
                        "position",
                        obj(vec![("line", int(1)), ("character", int(14))]),
                    ),
                    ("newName", str("scarlet")),
                ]),
            ),
        ])
        .to_string();
        let out = server.handle(&json::parse(&req).unwrap());
        assert_eq!(out[0].get("result"), Some(&Json::Null));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn type_find_references_and_go_to_definition() {
        let mut server = Server::default();
        let uri = "file:///ty.pyfun";
        let src = "type Shape = Mk int\ntype Box = { it: Shape }";
        server.handle(&json::parse(&open_msg(uri, src)).unwrap());

        // find-references from the decl name `Shape` (line 0): decl + the field use.
        let req = references_msg(uri, 0, 6, true);
        let out = server.handle(&json::parse(&req).unwrap());
        let locs = out[0].get("result").unwrap().as_array().unwrap();
        assert_eq!(locs.len(), 2, "decl + one use: {locs:?}");

        // go-to-definition from the use `Shape` (line 1) jumps to the decl (line 0).
        let req = pos_msg("textDocument/definition", uri, 1, 18);
        let out = server.handle(&json::parse(&req).unwrap());
        let line = out[0]
            .get("result")
            .unwrap()
            .get("range")
            .unwrap()
            .get("start")
            .unwrap()
            .get("line")
            .unwrap()
            .as_i64();
        assert_eq!(line, Some(0));
    }

    #[test]
    fn type_rename_rewrites_declaration_and_uses() {
        let mut server = Server::default();
        let uri = "file:///tyr.pyfun";
        let src = "type Shape = Mk int\ntype Box = { it: Shape }";
        server.handle(&json::parse(&open_msg(uri, src)).unwrap());
        let req = obj(vec![
            ("jsonrpc", str("2.0")),
            ("id", int(1)),
            ("method", str("textDocument/rename")),
            (
                "params",
                obj(vec![
                    ("textDocument", obj(vec![("uri", str(uri))])),
                    (
                        "position",
                        obj(vec![("line", int(1)), ("character", int(18))]),
                    ),
                    ("newName", str("Figure")),
                ]),
            ),
        ])
        .to_string();
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
        assert_eq!(edits.len(), 2, "decl + one use rewritten");
        assert!(
            edits
                .iter()
                .all(|e| e.get("newText").and_then(Json::as_str) == Some("Figure"))
        );
    }

    #[test]
    fn type_rename_rejects_a_lowercase_name_and_builtins() {
        let mut server = Server::default();
        let uri = "file:///tyb.pyfun";
        let src = "type Shape = Mk int\nlet xs = [1, 2, 3]";
        server.handle(&json::parse(&open_msg(uri, src)).unwrap());
        // A type renames only to an uppercase type name.
        let lower = obj(vec![
            ("jsonrpc", str("2.0")),
            ("id", int(1)),
            ("method", str("textDocument/rename")),
            (
                "params",
                obj(vec![
                    ("textDocument", obj(vec![("uri", str(uri))])),
                    (
                        "position",
                        obj(vec![("line", int(0)), ("character", int(6))]),
                    ),
                    ("newName", str("shape")),
                ]),
            ),
        ])
        .to_string();
        let out = server.handle(&json::parse(&lower).unwrap());
        assert_eq!(out[0].get("result"), Some(&Json::Null));
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

    /// Diagnostics currently published for `uri`, recomputed via the cache path.
    fn diags_of(server: &Server, uri: &str) -> Vec<Json> {
        server
            .diagnostics_for(uri)
            .get("params")
            .unwrap()
            .get("diagnostics")
            .unwrap()
            .as_array()
            .unwrap()
            .to_vec()
    }

    #[test]
    fn editing_an_imported_file_reanalyzes_dependents() {
        // Both files are open buffers under a common (virtual) directory — the
        // resolver prefers open buffers, so no on-disk files are needed.
        let mut server = Server::default();
        let geom_uri = "file:///proj/geometry.pyfun";
        let main_uri = "file:///proj/main.pyfun";
        server.handle(&json::parse(&open_msg(geom_uri, "let area w h = w * h")).unwrap());
        let out = server.handle(
            &json::parse(&open_msg(
                main_uri,
                "import Geometry\nlet r = Geometry.area 4 5",
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

        // Editing the *imported* buffer must invalidate the dependent's cached
        // analysis: `area` disappears, so main's next analysis flags the member.
        server.handle(&json::parse(&change_msg(geom_uri, "let volume x = x")).unwrap());
        let diags = diags_of(&server, main_uri);
        assert_eq!(diags.len(), 1, "stale import served: {diags:?}");
        // And back: restoring `area` re-analyzes clean again.
        server.handle(&json::parse(&change_msg(geom_uri, "let area w h = w * h")).unwrap());
        let diags = diags_of(&server, main_uri);
        assert!(diags.is_empty(), "restored import still stale: {diags:?}");
    }

    #[test]
    fn analysis_cache_validates_import_fingerprints() {
        let mut server = Server::default();
        let geom_uri = "file:///proj2/geometry.pyfun";
        let main_uri = "file:///proj2/main.pyfun";
        server.handle(&json::parse(&open_msg(geom_uri, "let area w h = w * h")).unwrap());
        server.handle(
            &json::parse(&open_msg(
                main_uri,
                "import Geometry\nlet r = Geometry.area 4 5",
            ))
            .unwrap(),
        );
        // Unchanged everywhere → the same Rc is served.
        let a = server.analysis(main_uri).unwrap();
        assert!(Rc::ptr_eq(&a, &server.analysis(main_uri).unwrap()));
        // A content-identical edit to the import bumps its version but not its
        // fingerprint: the dependent's analysis is still reused.
        server.handle(&json::parse(&change_msg(geom_uri, "let area w h = w * h")).unwrap());
        assert!(
            Rc::ptr_eq(&a, &server.analysis(main_uri).unwrap()),
            "content-identical import edit re-analyzed the dependent"
        );
        // A real edit to the import busts the dependent's cache entry.
        server.handle(&json::parse(&change_msg(geom_uri, "let area w h = w + h")).unwrap());
        let b = server.analysis(main_uri).unwrap();
        assert!(
            !Rc::ptr_eq(&a, &b),
            "changed import served a stale analysis"
        );
        assert!(Rc::ptr_eq(&b, &server.analysis(main_uri).unwrap()));
    }

    #[test]
    fn imported_file_changed_on_disk_invalidates_the_cache() {
        let dir = std::env::temp_dir().join(format!("pyfun_lsp_diskinv_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("geometry.pyfun"), "let area w h = w * h").unwrap();
        let main_uri = file_uri(&dir.join("main.pyfun"));

        let mut server = Server::default();
        let out = server.handle(
            &json::parse(&open_msg(
                &main_uri,
                "import Geometry\nlet r = Geometry.area 4 5",
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
        let a = server.analysis(&main_uri).unwrap();

        // Rewrite the import on disk (it is not open in the editor): the content
        // fingerprint changes, so the dependent re-analyzes on its next request.
        std::fs::write(dir.join("geometry.pyfun"), "let volume x = x").unwrap();
        let diags = diags_of(&server, &main_uri);
        assert_eq!(diags.len(), 1, "disk change not seen: {diags:?}");
        assert!(!Rc::ptr_eq(&a, &server.analysis(&main_uri).unwrap()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn exports_cache_is_shared_project_wide() {
        // Two dependents of `Geometry` share one cached interface entry rather
        // than each re-parsing + re-checking geometry.pyfun.
        let mut server = Server::default();
        let geom_uri = "file:///proj3/geometry.pyfun";
        server.handle(&json::parse(&open_msg(geom_uri, "let area w h = w * h")).unwrap());
        for (uri, name) in [
            ("file:///proj3/alpha.pyfun", "a"),
            ("file:///proj3/beta.pyfun", "b"),
        ] {
            let out = server.handle(
                &json::parse(&open_msg(
                    uri,
                    &format!("import Geometry\nlet {name} = Geometry.area 1 2"),
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
            assert!(diags.is_empty(), "{uri}: {diags:?}");
        }
        assert_eq!(
            server.exports.borrow().len(),
            1,
            "one shared interface entry serves both dependents"
        );
    }

    #[test]
    fn cyclic_imports_stay_out_of_the_project_cache() {
        // `alpha` and `beta` import each other. Resolution bails on the cycle
        // (as the forgiving `project::resolve_imports` does), and the resulting
        // context-dependent interfaces must not be cached project-wide — a
        // different entry document resolves the cycle from a different side.
        let mut server = Server::default();
        let alpha = "file:///cyc/alpha.pyfun";
        let beta = "file:///cyc/beta.pyfun";
        server.handle(&json::parse(&open_msg(alpha, "import Beta\nlet a = 1")).unwrap());
        server.handle(&json::parse(&open_msg(beta, "import Alpha\nlet b = Alpha.a")).unwrap());
        assert!(server.analysis(alpha).is_some());
        assert!(server.analysis(beta).is_some());
        assert!(
            server.exports.borrow().is_empty(),
            "cycle-context interfaces must not be cached"
        );
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
