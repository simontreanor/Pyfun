# Interop cookbook

Short Pyfun programs that call **well-known Python libraries** and show what Pyfun's
typed, effect-tracked boundary adds to the calling code. All run offline (stdlib only):

```bash
pyfun run examples/interop/<name>.pyfun
```

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
| [`json_to_adt.pyfun`](./json_to_adt.pyfun) | `json` (stdlib) | ✅ | **the headline** — decode a heterogeneous object into your own record, totally, via `result {}` railway composition (KeyError/ValueError → `Error`) |
| [`sqlite_query.pyfun`](./sqlite_query.pyfun) | `sqlite3` (stdlib) | ✅ | opaque handle types + unbound-method externs; rows as tuples; `List`/tuple decoding |
| [`read_files.pyfun`](./read_files.pyfun) | `pathlib` (stdlib) | ✅ | inferred `io` effect + propagation; `let pure` rejection; `try` → `Result` on a missing file |
| [`http_fetch.pyfun`](./http_fetch.pyfun) | `urllib` (stdlib) | ✅ | inferred `io` / `->{async}` effects; the effect *guarantee* (`let pure` over `io` is a compile error); instance-method body read |

## Reusable patterns (all verified against the current compiler)

- **Typed dotted extern.** `extern loads: string -> List float = json.loads` — the Pyfun
  type is a promise the compiler trusts at the boundary; `= module.fn` auto-imports.
- **Totality via `try`.** `try (loads s) : Result (List float) Exception` turns a raised
  exception into a value; `match` forces you to handle `Error`.
- **Homogeneous decode is free.** A JSON array → `List a`, a flat object → `Map string a`.
  Both lower 1:1 to the Python list/dict they already are.
- **Stateful/OOP libraries.** Model each opaque object as a nullary phantom ADT
  (`type Conn = ConnH`) and reach its members with an **instance-access extern** — a target
  starting with a dot, treating the first argument as the receiver. `= .execute()` (trailing
  `()` = call) is a method, so `execute conn sql` lowers to `conn.execute(sql)`; `= .scheme`
  (no `()`) is a property read, so `scheme url` lowers to `url.scheme`. No class is named or
  imported, inherited/delegated members work, and `execute conn` is the bound method
  `conn.execute` (currying for free).
- **Heterogeneous decode into your ADTs.** Don't cast the whole object — pull each field
  (`operator.getitem`), coerce it (`int`/`str`), wrap each step in `try`, and compose on
  the `result {}` railway so the first bad field short-circuits to `Error`. See
  `json_to_adt.pyfun`; this is the shape a decoder-combinator library would generalize.
- **Effects for free.** A plain `extern` is `io`; the checker infers and propagates it, so
  any function touching the boundary is `io` with no annotation, and `let pure` over it is
  a compile error. Override the boundary default with `->{async}` for async libraries.

## Honest limits (the frontier these examples expose)

- **No raw cast into records.** `extern parseUser: string -> User = json.loads`
  type-checks but crashes (`'dict' object has no attribute 'name'`) — `json` returns a
  Python `dict` (subscript), a Pyfun record lowers to a class (attribute). This is a
  footgun to avoid, not a wall: the field-by-field decoder in `json_to_adt.pyfun` is the
  right way, and it composes cleanly. The gap is only the *ergonomics* of a reusable
  decoder-combinator library (which a package manager, deferred, would let us ship).
- **Handle boilerplate.** The opaque phantom handle types (`type Conn = ConnH`) are repetitive;
  a future "typed façade" module could hide them (kept out of scope here — a shipped wrapper
  library would pull in the deferred package manager).
- **Anonymous record types** aren't accepted in an extern signature, so an ad-hoc request
  or response body needs a named `type`. (Tracked separately.)

The extern-FFI *reach* rough edges these examples first surfaced have all been closed — submodule
imports (`urllib.parse.quote` → `import urllib.parse`), instance access (`= .method()` calls and
`= .attr` property reads, reaching inherited/delegated members and legacy lowercase classes like
`urllib.response.addinfourl.read`, which is why the HTTP entry runs on stdlib `urllib` offline),
nullary calls (`unit -> a` applied to `()` → `time.time()`), and dotted targets on builtin types
(`str.upper`, `int.from_bytes` — no spurious `import`). See `DESIGN.md` §6.
