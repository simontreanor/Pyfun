# Interop cookbook

Short Pyfun programs that call **well-known Python libraries** and show what Pyfun's
typed, effect-tracked boundary adds to the calling code. All run offline (stdlib only):

```bash
pyfun run examples/interop/<name>.pyfun
```

**Start with [`json_to_adt.pyfun`](./json_to_adt.pyfun)** — decoding untrusted JSON into
your own typed records is the clearest, most relatable win. The rest cover databases,
files, and HTTP. Each file is self-contained and heavily annotated; the framing,
reusable patterns, and honest limits below give the bigger picture.

## The idea: boundary libraries, not engine libraries

Popular Python libraries split in two, and only one kind showcases Pyfun:

- **Boundary libraries** sit where the world is untyped and effectful — parsing, I/O,
  persistence, the network: `json`, `sqlite3`, `requests`, `pathlib`, `subprocess`.
  Their results are exactly what Pyfun tames: a throwing call becomes
  `try … : Result a Exception`, a side effect is tracked as `io`/`async`, and a decoded
  value is ordinary typed Pyfun data consumed with exhaustive `match`. **These are the
  showcase.**
- **Engine libraries** *are* the computation, behind a rich native-backed API — `numpy`,
  `pandas`, `torch`. Pyfun erases to plain numerics (it can't touch their throughput) and
  can only wrap their fluent API opaquely, so it adds little. Call them and stay safe
  *around* them; don't rewrite them.

The honest headline is therefore **not** "rewrite the popular libraries in Pyfun" — it's
"Pyfun shines at the boundary every one of those libraries makes you cross unsafely."

## Entries

| File | Library | Runs offline | Shows |
|------|---------|:---:|-------|
| [`json_decode.pyfun`](./json_decode.pyfun) | `json` (stdlib) | ✅ | `try` → `Result` totality; homogeneous JSON → `List`/`Map`; total `Map.tryFind` lookup |
| [`json_to_adt.pyfun`](./json_to_adt.pyfun) | `Decode` (built-in) | ✅ | **the headline** — decode a heterogeneous object into your own record, totally, with the Elm-style built-in `Decode` module (`Decode.map3` + `Decode.field`); the whole pipeline is *pure* |
| [`sqlite_query.pyfun`](./sqlite_query.pyfun) | `sqlite3` (stdlib) | ✅ | opaque handle types + unbound-method externs; rows as tuples; `List`/tuple decoding |
| [`read_files.pyfun`](./read_files.pyfun) | `pathlib` (stdlib) | ✅ | inferred `io` effect + propagation; `let pure` rejection; `try` → `Result` on a missing file |
| [`http_fetch.pyfun`](./http_fetch.pyfun) | `urllib` (stdlib) | ✅ | inferred `io` / `->{async}` effects; the effect *guarantee* (`let pure` over `io` is a compile error); instance-method body read |
| [`datetime.pyfun`](./datetime.pyfun) | `datetime` (stdlib) | ✅ | a **pure** FFI: `extern pure` + `let pure` prove a date pipeline effect-free; class target as constructor; `extern import` for classmethods (`now`, `fromisoformat`); `operator.add`/`sub` as extern targets; `try` on an impossible date |

## Reusable patterns (all verified against the current compiler)

- **Typed dotted extern.** `extern loads: string -> List float = json.loads` — the Pyfun
  type is a promise the compiler trusts at the boundary; `= module.fn` auto-imports.
- **Totality via `try`.** `try (loads s) : Result (List float) Exception` turns a raised
  exception into a value; `match` forces you to handle `Error`.
- **Homogeneous decode is free.** A JSON array → `List a`, a flat object → `Map string a`.
  Both lower 1:1 to the Python list/dict they already are.
- **Stateful/OOP libraries.** Declare each opaque object as an **`extern type`**
  (`extern type Conn`) — a typed handle with no constructor — and reach its members with an
  **instance-access extern** — a target starting with a dot, treating the first argument as
  the receiver. `= .execute()` (trailing
  `()` = call) is a method, so `execute conn sql` lowers to `conn.execute(sql)`; `= .scheme`
  (no `()`) is a property read, so `scheme url` lowers to `url.scheme`. No class is named or
  imported, inherited/delegated members work, and `execute conn` is the bound method
  `conn.execute` (currying for free).
- **Heterogeneous decode into your ADTs.** Don't cast the whole object — build a
  **decoder** with the built-in `Decode` module and run it with `Decode.decodeString :
  Decoder a -> string -> Result a Exception`. `Decode.field "age" Decode.int` pulls and
  strictly decodes a field; `Decode.map3`/`map2`/`map4` fan several field decoders into
  one that builds your record; `Decode.list`/`nullable`/`oneOf`/`andThen` cover arrays,
  optional/`null`, unions, and value-dependent shapes. The first bad field
  short-circuits to `Error`, and the whole pipeline is **pure** (a `let pure` over it
  type-checks). See `json_to_adt.pyfun` — this is Elm's `elm/json`, on the one library
  every Python programmer has imported.
- **Effects for free.** A plain `extern` is `io`; the checker infers and propagates it, so
  any function touching the boundary is `io` with no annotation, and `let pure` over it is
  a compile error. Override the boundary default with `->{async}` for async libraries. For a
  call that is genuinely deterministic and effect-free, `extern pure` asserts it — and then
  `let pure` can *prove* whole pipelines over the boundary effect-free (`datetime.pyfun`).
  A callback-taking extern uses an effect *variable* so both pure and effectful callbacks
  flow: `extern each : (a ->{e} unit) -> List a ->{io, e} unit` (`DESIGN.md` §4).
- **Classes as constructors, operators as functions.** A class target is callable —
  `extern pure date : int -> int -> int -> Datetime = datetime.datetime` — so no factory
  wrapper is needed. And where a library defines its API on `+`/`-` (as `datetime` does),
  Python's `operator` module exposes every operator as a plain function: `= operator.add`
  is a ready-made extern target (`datetime.pyfun`).
- **`extern import` when the heuristic can't see the module.** A dotted target's module is
  guessed by its lowercase prefix, which mis-reads a lowercase *class* (or value attribute)
  as a submodule. Declare it explicitly — Python's own import statement, `as` and all:
  `extern import datetime` roots every `datetime.datetime.*` target in the file
  (`datetime.pyfun`); `extern import numpy as np` lets targets say `np.zeros` and emits
  `import numpy as np` (`DESIGN.md` §6).

## Honest limits (the frontier these examples expose)

- **No raw cast into records.** `extern parseUser: string -> User = json.loads`
  type-checks but crashes (`'dict' object has no attribute 'name'`) — `json` returns a
  Python `dict` (subscript), a Pyfun record lowers to a class (attribute). This is a
  footgun to avoid, not a wall: the built-in `Decode` module (`json_to_adt.pyfun`) is the
  right way, and it composes cleanly and totally. The residual gap is only the *generalized*
  decoder story — user-registered combinators and a `Value` type for already-parsed data —
  which is deferred; the shipped set already covers records, lists, options, and unions.
- **Handle boilerplate.** Each opaque object still needs its own one-line `extern type Conn`
  declaration. That is now a single honest line (was the phantom-ADT `type Conn = ConnH`); a
  future "typed façade" module could bundle a library's handles + methods together, but that
  would pull in the deferred package manager, so it is kept out of scope here.
- **Anonymous record types** aren't accepted in an extern signature, so an ad-hoc request
  or response body needs a named `type`. (Tracked separately.)

The extern-FFI *reach* rough edges these examples first surfaced have all been closed — submodule
imports (`urllib.parse.quote` → `import urllib.parse`), instance access (`= .method()` calls and
`= .attr` property reads, reaching inherited/delegated members and legacy lowercase classes like
`urllib.response.addinfourl.read`, which is why the HTTP entry runs on stdlib `urllib` offline),
nullary calls (`unit -> a` applied to `()` → `time.time()`), and dotted targets on builtin types
(`str.upper`, `int.from_bytes` — no spurious `import`). See `DESIGN.md` §6.
