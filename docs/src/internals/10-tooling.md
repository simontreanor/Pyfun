# 10 - The compiler as a library

Everything the earlier chapters described, lexing through emission, is exposed as a plain Rust
library with a few pure entry points: `pyfun::analyze` (resilient diagnostics and inferred types)
and `pyfun::compile` (source to readable Python). The CLI is one caller. The language server, the
REPL, the Jupyter kernel, and the browser playground are the others, and they all reuse the same
front end rather than reimplementing any of it. This chapter is the tour of those callers.

## The language server

[src/lsp/mod.rs](https://github.com/simontreanor/Pyfun/blob/main/src/lsp/mod.rs) is a
dependency-free LSP server over stdio. To keep the core crate free of `serde` and `lsp-types`,
the JSON value type, parser, and serializer are hand-rolled in
[src/lsp/json.rs](https://github.com/simontreanor/Pyfun/blob/main/src/lsp/json.rs), the same
choice made for the lexer and parser. The message-handling core, `Server::handle`, is pure (JSON
in, JSON out), so it is unit-tested without spawning a process.

Every feature is a view over one `analyze` result. Diagnostics are the same type, effect, unit,
and exhaustiveness errors from earlier chapters, streamed as `publishDiagnostics`. Hover shows an
inferred type, which is the only way to see one since Pyfun never writes them; it comes from a
`(span, type)` table the checker fills in its collecting pass. Go-to-definition, find-references,
and rename run over a dependency-free name resolver
([src/lsp/resolve.rs](https://github.com/simontreanor/Pyfun/blob/main/src/lsp/resolve.rs)) that
walks the parsed AST, so navigation works on any program that parses even if it does not yet
type-check. That resilience is deliberate: the parser has an error-recovering entry point, so a
half-typed file still yields hover and navigation for the parts that parse. Details of the caches
and cross-file navigation are in
[INTERNALS.md](https://github.com/simontreanor/Pyfun/blob/main/INTERNALS.md).

## The module graph

Multi-file projects are resolved by
[src/project/mod.rs](https://github.com/simontreanor/Pyfun/blob/main/src/project/mod.rs). From an
entry file it follows `import` edges, builds a dependency graph, rejects cycles and missing files,
and returns the modules in topological order. The graph logic is decoupled from the filesystem:
`build` takes a loader closure mapping a module name to its source, so it is unit-testable with an
in-memory map, and `build_from_path` is the thin wrapper that resolves names to `.pyfun` files.
Cross-module checking and the parallel `.py` emit build on that ordered `Project`.

## The REPL

[src/repl.rs](https://github.com/simontreanor/Pyfun/blob/main/src/repl.rs) pairs the Rust checker
with one long-lived Python worker process holding a single namespace for the session. Each entry
is type-checked against the accumulated definition source, and what reaches Python is a diff:
because emission is deterministic (see [emission](08-emission.md)), the program is split into
top-level chunks and only chunks the worker has not run yet are sent, length-framed over the
worker's stdin/stdout. The worker itself is a tiny driver that `exec`s each blob in a persistent
dict and returns captured output:

```python
# src/repl.rs (the DRIVER fed to python -c)
ns = {}
# ... read a length-framed blob, then:
    exec(code, ns)
```

So a definition's effects run once at entry, an expression runs once per entry, and state
including top-level `let mut` persists. A dead worker is respawned and the namespace rebuilt.

## The Jupyter kernel

[src/kernel.rs](https://github.com/simontreanor/Pyfun/blob/main/src/kernel.rs) (`pyfun
kernel-engine`) is the REPL turned inside out. It does the same accumulate, analyze, compile,
chunk-diff bookkeeping, but instead of driving a worker it returns the new-chunk blob for the
caller (the `pyfun_kernel` Python package inside Jupyter) to `exec` in its own namespace. The
kernel process is itself Python, so there is no separate worker and Jupyter's own stdout capture
routes cell output. It reuses the REPL's chunking helpers directly (`blob_of_new`,
`chunk_python`), which is why the two stay in step.

## The playground

[playground/src/lib.rs](https://github.com/simontreanor/Pyfun/blob/main/playground/src/lib.rs) is
a WebAssembly shim. It is a separate crate so the core stays dependency-free (only this binding
pulls in `wasm-bindgen`), and it calls the very same `pyfun::analyze` and `pyfun::compile`:

```rust
// playground/src/lib.rs
let analysis = pyfun::analyze(source);
// ... then, when clean:
match pyfun::compile(source) { Ok(py) => Some(py), Err(e) => /* one more diagnostic */ }
```

Neither path touches the filesystem or spawns a process, which is exactly what makes them safe in
WebAssembly. The result is that the browser shows byte-for-byte what the CLI emits. To close the
loop: the playground on this very site is that shim, running the real compiler in your browser.

## Where you would add a new LSP capability

A new editor feature is a new request handler in `Server::handle` in
[src/lsp/mod.rs](https://github.com/simontreanor/Pyfun/blob/main/src/lsp/mod.rs), reading the
same `Analysis` bundle the other features use and, if it needs to walk names, extending
[src/lsp/resolve.rs](https://github.com/simontreanor/Pyfun/blob/main/src/lsp/resolve.rs). Because
`handle` is pure, you test it with JSON in and JSON out before ever touching stdio, and the thin
[VS Code client](https://github.com/simontreanor/Pyfun/blob/main/editors/vscode) needs no change.
