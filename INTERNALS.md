# Pyfun — Internals

How this Rust compiler is built: crate layout, the lowering passes, and the language server. It is
the map from a semantic rule to the code that implements it. `DESIGN.md` is the source of truth for
*what the language does and why*; this file is the source of truth for *how this compiler does it*.
Read them together for any compiler change: DESIGN for the rule, this file for where it lives. The
exact current code is always the final authority; when this file and the code disagree, fix one of
them.

## Crate layout

Keep modules small and single-purpose — exhaustiveness, type+effect inference, and codegen each
grow large and must not bleed together.

```
src/
  lexer/           tokenizer, token types, lex errors
  parser/          recursive-descent + precedence climbing; ast.rs = Expr/Pat/Ty/Stmt
  ast/             traversal + visitor utilities, pretty-printer
  desugar/         computation-expression desugaring (DESIGN §8.1): builder{} → bind/return/…
  types/           HM inference + effect inference/checking, exhaustiveness
    units/         units-of-measure inference: abelian-group unit unification (DESIGN §8.2)
  lowering/        Pyfun AST → Python-AST IR; scope/name-binding analysis; unit erasure
  python_emitter/  Python-AST IR → readable source
  diagnostics/     rustc-style errors: codes (E001…), levels, spans, notes
  cli/             clap-based; subcommands compile/check/fmt/lsp
  lsp/             front-end-first language server (stdio JSON-RPC)
    json.rs        hand-rolled, dependency-free JSON value + parser + serializer
prelude/           Pyfun/Python runtime support (Result/Option ADTs, etc.)
editors/vscode/    minimal VS Code client that launches `pyfun lsp`
tests/             parser tests, compile tests, .pyfun fixtures (favor snapshot/golden tests)
```

**Build order:** `lexer` + `parser` + `ast` → `desugar` → `types` (incl. `units`) →
`lowering` + `python_emitter` → `diagnostics` + `cli` → `lsp`.

## Lowering passes

The lowering strategy and its observable contracts are in `DESIGN.md` §5. The performance-directed
passes below are pure implementation: each is default-on, gated by an env-var kill switch for
differential testing, and proven value-identical to the unoptimized lowering by a differential gate
that byte-compares both variants' output on the `network-rail` example.

### In-place linear accumulation (`Seq.fold`/`List.fold`) — implements DESIGN §5.1

The immutable-style collections rebuild a fresh container on every step, so a fold that builds a
collection by repeated `Map.add`/`List.concat`/`Set.add` is quadratic (each step copies the whole
accumulator). The pass (`src/lowering/fold_loop.rs`, hooked in `lower_application`, defaulting on;
`PYFUN_NO_FOLD_OPT` is the kill switch) recognizes the common case and rewrites
`functools.reduce(f, xs, acc)` into a `for`-loop over a **mutable** accumulator, turning the
copy-returning ops into in-place mutations (`Map.add`→`m[k]=v`, `List.concat`→`.append`/`.extend`,
`Set.add`→`.add`), collapsing the build to linear. This adds two Python-IR nodes: `PyStmt::For` and
`PyStmt::SubscriptAssign`.

**Soundness is what the pass must protect.** The rewrite is observable only through a *retained reference* to a
mutated container, so the pass is a set of conservative **syntactic** proof obligations on the AST,
checked with no side effects on the lowerer (a rejected fold falls through to the byte-identical
`_pf_fold` lowering). A fold qualifies only when: it is a fully-applied `Seq.fold`/`List.fold`
(exactly 3 args); the folder is a 2-ary lambda literal or a **top-level** named `let` (an inlinable
body, not a `mut`/extern/imported member/parameter); the accumulator is a fresh literal collection or
a flat tuple of them (a `Var` init is rejected — it may be read after the fold); every slot is
threaded **position-preservingly** (no swap, no duplication, no cross-slot storage, no closure
capture, no escape to a user function — retention is unknowable, so reject); reads of a slot use a
whitelist of ops that return scalars or fresh copies; and inlining is capture-safe (the folder's free
variables and introduced binders are disjoint from every enclosing Python frame; rejected inside an
in-file `module`). Read-before-mutate and effect order are preserved by lowering every op argument
(hoisting non-atomic ones to temps) **before** emitting the mutations. When in doubt, reject. The
folder is always pure (an effectful folder does not typecheck against `fold`'s scheme), so the only
ordering hazard is a value dependency between slots, which the hoisting handles. Full preconditions
(P1–P11) live in the design memo the pass was built from.

**Tier B** extends the qualifying shapes, each behind the same differential gate:
**block-local named folders** (`dedupLegs`'s inner `step` — a scope-coherent registry
(`local_fn_defs`) records 2-ary block `let`s, evicts on rebinding, and is *shadow-guarded* at every
binder introduction (function parameters, match-arm patterns), so a hit is always the innermost
binding; a local folder's frees resolve in the call site's own frame, so the top-level-folder
free-variable check is skipped, like a lambda's); **chained updates in one slot** (`Map.add k2 v2
(Map.add k1 v1 m)` → in-place ops innermost-first — sound because the reduce form never mutates, so
hoisting every read before every mutation preserves what each read sees); **fresh-reset slots**
(`([], List.concat done [cur])` — the slot rebinds to a fresh init; the *store-then-reset* exception
licenses storing a reset slot's whole object into another slot, force-hoisting the old reference
before the rebind); **`Map.remove`/`Set.remove`** (→ `m.pop(k, None)` / `s.discard(x)`, mirroring
the copy helpers exactly); and **`Var` inits** (a mutated slot binds a defensive shallow copy —
`dict(seed)`, constructor inferred from the slot's op family — so the original reads unchanged after
the fold; an unmutated slot binds as an alias, preserving reduce's object identity). For soundness, a
*parameterized* local `let` in a folder body is a deferred closure and is held to the closure rule (no
sensitive mention), not the immediate-read rule.

### Inlining fully-applied pure stdlib predicates — implements DESIGN §5.2

Most stdlib members lower to an emitted `_pf_*` helper (a single callable so partial application
still works). But a handful are pure, total, **one-liner** wrappers over a Python idiom, and when such
a member is **fully applied** there is no reason to route through the helper at all — the same
fully-applied-collapse instinct as the currying rule. `try_inline_stdlib` (in `lower_application`,
gated on the head being a module-qualified `Field` and the arg count matching the member's exact arity)
emits the idiom directly: `String.contains n s`→`n in s`, `String.startsWith p s`→`s.startswith(p)`,
`String.endsWith`→`s.endswith(…)`, `List`/`Set`/`Map.contains`→`x in xs` (a `PyExpr::Compare` with the
`In` op, so precedence/parenthesization is free), and `List.isEmpty xs`→`not xs`. The argument order is
taken verbatim from each helper body — an inverted operand would be a silent miscompile — so the inline
form is value-identical to the helper. Anything short of full arity (a partial application like
`String.contains "x"`, or a bare value reference) is **not** matched and falls through to the helper
unchanged, so `List.map (String.contains "x") xs` still works. The win is mostly readability:
`"CHIPNHM" in line` reads better than `_pf_str_contains("CHIPNHM", line)`, and the call-elimination is
small on wall-clock (~3% on the `network-rail` example; a cProfile line that *looked* like ~6s was
mostly the profiler's own per-call overhead on a ~1.87M-call trivial function, not real runtime — see
`ROADMAP.md`). (`List`/`Set`/`Map.len` are not in this set: they already lower to a bare `len`, so a
fully-applied call is already `len(xs)` with no helper in between.)

### Specializing statically-known `Decode` decoders — implements DESIGN §5.3

`Decode.decodeString dec s` normally builds a runtime decoder *value* — a tree of raising closures —
and interprets it over `json.loads` output. When `dec` is a **syntactically-known composition** of the
simple combinators (`string`/`int`/`float`/`bool`/`field`/`list`/`nullable`/`map`–`map4`/
`succeed`/`fail`/`oneOf`, including through *top-level* `let` decoder names), the pass
(`src/lowering/decode_spec.rs`, hooked in `lower_application`, default-on with `PYFUN_NO_DECODE_OPT`
as the kill switch) deforests the interpreter into **direct dict/list access with inline error
handling**: one `try` whose body reads like a hand-written validating parser (`t = v["name"]`,
`if not isinstance(t, str): raise ValueError(...)`), with the handler folding any raise into
`Error(_Exception(kind, msg))` exactly like `_pf_dec_decode_string`. The `Result` is
**byte-identical** to the interpreter's on every input — same `Ok` payload, same error kind and
message (a missing field is `KeyError` with the key's quoted repr). Faithfulness mechanics:
configuration expressions (fan-in functions, `field` names, `succeed` values) hoist *outside* the
`try` in construction order, matching the interpreter's build-time evaluation; every node decodes from
a bound temp so a subscript evaluates once; `oneOf` nests try/except per alternative, exhausting to the
interpreter's exact message. Anything dynamic — `andThen`, a decoder passed as a value, a shadowed or
cyclic named decoder, a non-literal `oneOf` list, an in-file `module` — rejects and falls back to the
interpreter unchanged. Measured on a decode-dominated workload (400k-object JSON array into records,
in-process best-of-5): **2.8x end-to-end** including the shared `json.loads`, ~4x on the decode portion
itself.

## The language server (`pyfun lsp`) — implements DESIGN §9 tooling

`pyfun lsp` runs a small language server over stdio. It speaks LSP/JSON-RPC with `Content-Length`
framing; to keep the crate **dependency-free** (no `serde`/`lsp-types`), the JSON value type, parser,
and serializer are hand-rolled in `src/lsp/json.rs` — the same choice as the hand-rolled lexer/parser.
The message-handling core (`Server::handle`) is pure (JSON in → JSON out) so it is unit-tested without
spawning a process; a separate integration test (`tests/lsp.rs`) drives the real binary over piped
stdio. All features reuse the existing front end:

- **Diagnostics** — the existing type/effect/unit/exhaustiveness errors, streamed as
  `textDocument/publishDiagnostics` on open/change (full document sync).
- **Hover-for-type** — the inferred type of the narrowest expression, binding name, **parameter, or
  pattern variable** under the cursor, **with latent effects** shown on arrows (e.g.
  `string ->{io} unit`) plus a **dedicated `Effect:` line** summarizing the concrete effect performed
  on full application (the union of the type's *result-spine* arrows — `io`/`async`; argument arrows
  are a callback's effect, not the value's, and pure values omit the line — `types::effect_summary`).
  It works because the checker, in a `record`-enabled pass (`types::check_collecting`, surfaced via
  `analyze`), accumulates a `(span, ty)` table for every expression node, binding name, function
  parameter, and pattern variable, then resolves each entry against the final substitution and renders
  it. Bindings carry a `name_span`, and parameters / pattern variables carry their own spans, so a
  function name hovers to its full inferred signature and a parameter hovers to its element type. A
  `##` doc attached to a top-level `let`/`type`/`extern` (DESIGN §7) is appended below the type when
  hovering the declaration name *or any reference resolving to it* (`resolve::symbol_at` → the item's
  `doc`); a documented symbol with no recorded type hovers to the doc text alone.
- **Go-to-definition** — jump from a reference to its definition, **module-level or local**. Backed by
  a dependency-free name resolver (`src/lsp/resolve.rs`) that walks the parsed AST (independent of the
  type checker, so it works on any program that *parses*): `definitions` collects module-level symbols
  (top-level `let`s with their precise name span; constructors / type / record decls / `extern`s at
  their declaration), and `references` resolves every identifier occurrence to a `Target` — either a
  `Local` binder (function parameter, block-local `let`, pattern variable, or computation-expression
  `let`/`let!`, resolved to the binder's own span) or a `Module` symbol (resolved by name against
  `definitions`). The walk tracks lexical scopes so an inner binding correctly shadows an outer one.
- **Find-references** — every occurrence of the symbol under the cursor (the inverse of
  go-to-definition, reusing the same resolver). The cursor may sit on a *use* or the *definition/binder*
  itself: `symbol_at` maps the offset to its occurrence span and a `Target` (the narrowest enclosing
  reference / local-binder / definition span wins), then `find_references` returns all references with
  that target plus, when `context.includeDeclaration` is set, the declaration(s).
- **Rename** — rewrite every occurrence (declaration included) of the symbol under the cursor to a new
  name, returned as a `WorkspaceEdit`. Built on `symbol_at` + `find_references`; `prepareRename`
  validates first and returns the identifier's range. Only **locals** and top-level **`let` values**
  are renameable — their every occurrence is a precise span; constructors / types / `extern`s are
  refused, because their declaration span covers the whole declaration and their type-annotation uses
  aren't tracked as references, so a rename would be unsound. The new name must be a valid lowercase
  value identifier. No capture-avoidance check is done (the editor shows the diff for review).
- **Completion** — in-scope module symbols (from whatever the recovering parser produced, so even a
  partially-typed file contributes its symbols) plus the always-available prelude (`PRELUDE` +
  `LIST_PRELUDE`), builtins (`Ok`/`Error`, the builtin/reserved type names), and keywords, each tagged
  with a `CompletionItemKind`. The static set is the fallback when nothing parses.
- **Document symbols** — the editor outline: every module-level definition as a flat
  `DocumentSymbol[]`, reusing `resolve::definitions` (each with a precise `range`/`selectionRange` and
  an LSP `SymbolKind` icon). Works on whatever parsed.

**Cross-file navigation** (with file-based modules, DESIGN §6.1): go-to-definition on a qualified
reference to an imported file module (`Geometry.area`, `Geometry.Circle`) jumps to the definition in
that module's `.pyfun` (`resolve::qualified_at` records expression-position qualified refs with spans;
the server resolves the sibling URI and locates the member via `resolve::definitions`);
`workspace/symbol` searches every definition across the project directory's `.pyfun` files; and
find-references + rename for top-level `let` values *and* constructors span the whole project
(`Server::symbol_occurrences` scans the directory's files and collects the definition, its bare uses in
the defining file, and every qualified use elsewhere, rewriting only the member identifier via
`member_subspan` so the `Geometry.` qualifier is preserved). A constructor's uses include both
construction expressions and patterns (`Pattern::Ctor` and the `type` variant declaration each carry a
name span). Rename fires only for a top-level value or constructor and a *strict* scan **refuses**
rather than do a partial rewrite if any project file fails to parse. **Type names** navigate and rename
**in-file only** — there is no qualified-type syntax, so a type name appears only in its own file's
annotations; `TypeExpr::Con` and the `type` declaration each carry a name span, the resolver walks type
annotations (`resolve::walk_type`), and `resolve::type_at` / `type_use_references` drive the three
operations (a type renames to an uppercase type name; builtins are refused).

**Resilient & incremental analysis.** A half-typed file still yields results. The parser has an
error-recovering entry point (`parser::parse_recover → (Module, Vec<ParseError>)`) used by the editor
(the compiler keeps the strict `parse`, as it must reject any broken program): on a failed item it
records the error, guarantees forward progress, then `synchronize`s to the next item boundary (a
statement separator at block depth 0, tracking `Indent`/`Dedent` so a separator *inside* a broken
block isn't mistaken for it). So one broken `let` no longer hides the rest of the file — the items that
parse still drive hover and navigation, and only the *syntax* errors are reported until the file is
clean, at which point the type errors take over. `analyze` returns an
`Analysis { module, diagnostics, types, parse_ok }` bundle; **lexing errors remain fatal** (no AST) and
**rename requires `parse_ok`**. The "incremental" half is a per-document analysis cache keyed on a
monotonic version stamp: repeated requests on an unchanged document reuse one parse + type-check.

**Project-wide cache + import invalidation.** Import-aware analysis is cached at two levels, both
validated by **content fingerprints** (a `DefaultHasher` of the source text; an analysis is a pure
function of the entry text plus every imported source it consulted, so equal fingerprints prove an
equal result). (1) Each per-document cache entry (`CachedAnalysis`) records the imported module files
its analysis consulted — `deps: (uri, Option<fingerprint>)`, with `None` recording the file's *absence*
so that creating it later also invalidates. A cache hit requires the document version *and* every dep
fingerprint to match, so editing an imported file — in an open buffer **or on disk** — re-analyzes its
dependents on their next request. (2) A **project-wide exports cache** (`Server.exports`,
`CachedExports` keyed by module-file URI) memoizes each imported module's checked interface
(`ModuleExports`) together with its own dep list, so two open documents importing `Geometry` share one
parse + check of `geometry.pyfun`. Imported sources are read from the **open buffer when the file is
open** (else disk). The resolver (`Server::resolve_exports_cached`, driven through
`lib::analyze_with_imports`) mirrors the forgiving `project::resolve_imports` semantics — missing/
broken/cyclic imports are omitted — with one care point: an interface computed in an import-*cycle*
context is context-dependent, so such "tainted" results live only in a per-pass memo and never enter
the project-wide cache. Diagnostics for a dependent are still *published* only when that document is
next analyzed; proactively re-publishing dependents' diagnostics on an import edit stays deferred.

The AST changes that enable local navigation: function/binding parameters are `Param { name, span }`
(was `Vec<String>`), `Pattern::Var { name, span }` (was `Var(String)`), and the
`CeItem::Let`/`LetBang` variants carry a `name_span`. The spans are `NodeSpan` (which compares equal
unconditionally), so roundtrip/structural equality is unaffected; lowering erases them (`param_names`).

Deferred: *truly* incremental reparsing — an edit still re-analyzes the whole document — and
deliberately so: a whole-file lex + parse + check is milliseconds at realistic Pyfun file sizes, the
two caches above already eliminate all *redundant* whole-file work, and region-based reparse would
complicate the offside-rule lexer and the recovering parser for no perceptible latency win at this
scale. The `editors/vscode/` client is intentionally thin — all language smarts live in the Rust
server.

## Typed holes — implements DESIGN §7 (typed-hole semantics)

The surface semantics (a `?`/`?name` hole is a typed blank the checker reports and that blocks
`compile`/`run`) are in DESIGN. Implementation: `?` is lexed as `Tok::Hole(Option<String>)` (a name is
lexed adjacently, like `f"`/`r"`); the parser produces `ExprKind::Hole { name }`, which round-trips. A
hole infers as a **fresh type variable that unifies freely**, so it never causes a spurious error and
takes whatever type the context demands (`?body + 1` ⇒ `int`, `List.map ? xs` ⇒ a function type); it is
recorded (`Infer::holes`) and, once the substitution is final, resolved and rendered
(`types::Hole { name, ty, span }`). It is reported **informatively**: `pyfun check` prints each as a
**note** (`` hole `?body` has type `int` ``) and the LSP publishes it at **Information severity** (3)
with hover showing the type. A hole **blocks `compile`/`run`** with a "cannot compile: unfilled hole"
error, and `check` exits non-zero.

**Valid hole fits.** Each note lists in-scope bindings that could fill the hole — the compiler searches
the environment snapshotted at the hole and reports every binding whose type unifies with the hole's.
The test is a real **trial unification** rolled back afterward (`Infer::hole_fits` snapshots the
substitution maps, instantiates each candidate scheme, unifies against the resolved hole type, and
restores). Fits are ranked most-specific (fewest generalized variables) first, unqualified names before
qualified module members, capped at 6; a fully-unconstrained hole (`'a`) lists none. **Refinement fits**
go further: a function binding whose *result* — after applying one or two arguments — unifies with the
hole's type is reported *applied to that many further holes* (`String.upper ?`, `String.concat ? ?`).
`Infer::hole_refinements` peels leading arrows off each candidate (up to `MAX_REFINE_DEPTH` = 2) and
trial-unifies the tail, skipping a peeled result that is a bare variable — a **structural filter** that
keeps out trivially-general combinators (`id`, `const`). Fewest-holes-first, capped at 4, never
duplicating a direct fit.

## Syntax highlighting (TextMate grammar)

Separate from the LSP's semantic smarts, `editors/vscode/pyfun.tmLanguage.json` gives static,
parse-free highlighting (keywords, declarations, types/constructors, numbers + adjacent unit
annotations, operators, strings/comments). One deliberate design choice: the **escape-hatch tokens are
flagged in a caution colour** to signal the opt-outs from Pyfun's immutable-by-default / effect-checked
defaults — `mut` (the mutability opt-out), `<-` (the act of mutation), and `extern` (the untyped,
effectful-by-default Python FFI boundary). `pure` deliberately stays a neutral `storage.modifier` (it
is an *encouraged* assertion, the opposite of an escape hatch), and `->` is scoped apart from `<-` so
only the reassignment arrow is flagged. The colour is applied via TextMate scopes plus a pinned
foreground, not by borrowing a "warning" scope: `mut` → `storage.modifier.mutable.pyfun`, `<-` →
`keyword.operator.mutation.pyfun`, `extern` → `keyword.other.extern.pyfun` (each names what the token
*is*), and the extension pins all three to an amber `#CC5E00` (no italic) via
`contributes.configurationDefaults.editor.tokenColorCustomizations`. Pinning the colour rather than
relying on a theme's rendering of `invalid` keeps the hue consistent across themes and light/dark
auto-switching, and avoids the semantic lie that these valid keywords are errors. Users can override the
colour in their own `editor.tokenColorCustomizations`.

## File-based modules — implements DESIGN §6.1

The module system's semantics (one module per file, qualified use, an acyclic import graph, cross-module ADTs/records/externs/measures, all-public) are in `DESIGN.md` §6.1. The driver, cross-module checking, and project emit are built as follows.

**Resolution & ordering** (`src/project`). *Implementation:* `project::build(entry, load)` walks the graph depth-first with an
injected `load: Fn(&str) -> Option<String>` loader, so the graph/cycle/topo logic is **filesystem-free
and unit-testable**; a back-edge to a module on the DFS path is a `ProjectError::Cycle` (reported as the
path `A -> B -> A`), a `None` from the loader is a `ProjectError::Missing` (naming the importer), a
lex/parse failure is a `ProjectError::Compile` (naming the module), and the DFS post-order is the
returned topological order (dependencies first, entry last). `project::build_from_path(entry)` is the thin
`.pyfun`-file wrapper (module name = stem with first letter uppercased; `import Geometry` → `geometry.pyfun`
in the entry's directory). Cross-module *checking* and *emit* consume this `Project`.

**Cross-module checking** (`types::check_module` + `project::check`). *Implementation:* the single-file `run` was generalized to take the imports map and return the module's
exported value schemes (which now include `extern` names), its exported sum types, **its exported records**,
**and its measures + measure-aliases**; `check_module(module,
imports)` seeds imported values under qualified keys and imported sum types into the decls under **qualified
constructor keys** (`merge_imported_types`: `decls.ctors["Geometry.Circle"]`, `type_ctors["Shape"] =
["Geometry.Circle", …]`), plus imported records under their **bare identity name** with a qualified surface
alias (`decls.record_aliases["Geometry.Point"] = "Point"`) and their fields appended to the field multimap,
plus imported measures merged unqualified into `decls.measures`/`measure_aliases` (with the alias-conflict
check), so construction (the `Record`/`Field` arms), qualified ctor/record patterns (`bind_pattern`),
exhaustiveness (`ctor_signature`), and `<…>` unit resolution all resolve with no special cases. Transplanting a scheme across modules is sound
because a top-level binding (and a constructor) generalizes against an env of closed schemes, so its own
scheme is closed and `instantiate` refreshes the quantified vars in the dependent module's id space.
`project::check` threads the `ModuleExports` map through the topological order, seeding each module from
only the modules it actually imports (so an unimported module's members/constructors stay invisible), and
returns errors grouped by module. *Lowering* routes a qualified constructor — in expression or pattern
position — to the imported module's class (`geometry.Circle`, dotted class pattern `case geometry.Circle(r):`,
with `import geometry` hoisted); a nullary imported constructor used as a value is called
(`palette.Red()`), and imported constructor arities are threaded so a partial application still curries.

**Output & the shared runtime** (`lowering::lower_in_project` + `project::compile`). *Implementation:* the
`Lowerer` gained an `imported_modules` set (drives the `geometry.area` routing) and a `use_runtime` flag
(emit `from _pyfun_rt import …` vs inline); `lower_in_project(module, ctx)` sets them and threads
`ctx.member_arities` (the imported functions' arities) into the arity table so a **cross-module partial
application still lowers to `functools.partial`**. `project::compile` builds each module's `ImportContext`
from its imports, emits `<name>.py` per module, and appends `_pyfun_rt.py` (via `runtime_module()`) iff
any module used the nominal classes.
