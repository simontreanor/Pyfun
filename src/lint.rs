//! The warnings the syntax alone shows (`DESIGN.md` §3): a local `let` nothing
//! reads, an `extern import` no `extern` is rooted at, and a top-level name
//! bound twice.
//!
//! These need no types, so they run on the parsed module. The warnings that do
//! need the checker (an arm that cannot match, an import nothing uses) are
//! found there and joined with these by [`crate::types`]; a caller asks the
//! checker and gets them all.

use std::collections::HashMap;

use crate::diagnostics::{Warning, WarningKind};
use crate::lexer::Span;
use crate::lsp::resolve;
use crate::syntax::{Item, Module};

/// Every syntax-level warning for `module`, in source order. `has_holes` says
/// the module has a typed hole (`?`): it is unfinished on purpose, and a
/// binding nothing reads yet is very likely what the hole is waiting for, so
/// unused bindings are not reported until the holes are filled.
pub fn lint(module: &Module, has_holes: bool) -> Vec<Warning> {
    let mut out = Vec::new();
    if !has_holes {
        unused_lets(module, &mut out);
    }
    unused_extern_imports(module, &mut out);
    rebound_names(module, &mut out);
    out.sort_by_key(|w| (w.span.start, w.span.end));
    out
}

/// A `let` inside a block or a computation expression whose name nothing reads.
/// Parameters, pattern variables and loop targets are not reported: they are
/// often there for the shape of the function or the pattern, not for the name.
fn unused_lets(module: &Module, out: &mut Vec<Warning>) {
    for (name, span) in resolve::unused_lets(module) {
        out.push(Warning {
            message: format!(
                "`{name}` is never used (name it `_{name}` if that is intended, or `_` to \
                 drop the value)"
            ),
            span,
            kind: WarningKind::UnusedBinding,
        });
    }
}

/// An `extern import` that no `extern` in the file is rooted at. The rule for
/// "rooted at" is the one the checker and lowering use: an alias matches a
/// target's first segment, a bare path matches as a proper prefix.
fn unused_extern_imports(module: &Module, out: &mut Vec<Warning>) {
    let targets: Vec<&[String]> = module
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Extern(decl) => Some(decl.target.as_slice()),
            _ => None,
        })
        .collect();
    for item in &module.items {
        let Item::ExternImport { path, alias, span } = item else {
            continue;
        };
        let used = targets.iter().any(|target| match alias {
            Some(a) => target.first() == Some(a),
            None => target.len() > path.len() && target.starts_with(path),
        });
        if !used {
            let spelled = match alias {
                Some(a) => format!("{} as {a}", path.join(".")),
                None => path.join("."),
            };
            out.push(Warning {
                message: format!(
                    "`extern import {spelled}` is never used: no `extern` in this file is \
                     rooted at it"
                ),
                span: span.span(),
                kind: WarningKind::UnusedExternImport,
            });
        }
    }
}

/// A top-level `let` that binds a name an earlier top-level `let` already
/// bound. The later one wins for the code below it, which is rarely what a
/// second definition of the same name was meant to do.
fn rebound_names(module: &Module, out: &mut Vec<Warning>) {
    let mut seen: HashMap<String, Span> = HashMap::new();
    for item in &module.items {
        let Item::Let(binding) = item else { continue };
        for name in binding.bound_names() {
            if name.starts_with('_') {
                continue;
            }
            let span = binding.target_span.span();
            if seen.insert(name.clone(), span).is_some() {
                out.push(Warning {
                    message: format!(
                        "`{name}` is already bound above; this `let` replaces it for the \
                         rest of the file"
                    ),
                    span,
                    kind: WarningKind::Rebound,
                });
            }
        }
    }
}
