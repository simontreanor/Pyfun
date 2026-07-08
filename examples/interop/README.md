# Interop cookbook

Short Pyfun programs that call **well-known Python libraries** and show what Pyfun's
typed, effect-tracked boundary adds to the calling code. Most run offline with:

```bash
pyfun run examples/interop/<name>.pyfun
```

(`http_fetch.pyfun` is the exception — it `pyfun check`s offline but needs `requests`
+ network to `run`; see the table.)

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
| [`http_fetch.pyfun`](./http_fetch.pyfun) | `requests`/`httpx` | check-only | inferred `io` / `->{async}` effects; the effect *guarantee* (`let pure` over `io` is a compile error) |

## Reusable patterns (all verified against the current compiler)

- **Typed dotted extern.** `extern loads: string -> List float = json.loads` — the Pyfun
  type is a promise the compiler trusts at the boundary; `= module.fn` auto-imports.
- **Totality via `try`.** `try (loads s) : Result (List float) Exception` turns a raised
  exception into a value; `match` forces you to handle `Error`.
- **Homogeneous decode is free.** A JSON array → `List a`, a flat object → `Map string a`.
  Both lower 1:1 to the Python list/dict they already are.
- **Stateful/OOP libraries.** Model each opaque object as a nullary phantom ADT
  (`type Conn = ConnH`) and call methods through their **unbound** form
  (`sqlite3.Connection.execute` invoked as `execute conn sql`).
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
- **Handle boilerplate.** Phantom types and unbound `Type.method` targets are repetitive;
  a future "typed façade" module could hide them (kept out of scope here — a shipped
  wrapper library would pull in the deferred package manager).
- **Anonymous record types** aren't accepted in an extern signature, so an ad-hoc request
  or response body needs a named `type`. (Tracked separately.)
- **Extern FFI rough edges surfaced while building these** (tracked in `ROADMAP.md`): the
  importer emits only the *first* dotted segment (`import urllib`), so a target in a
  **submodule** (`urllib.request.urlopen`, `http.client`) fails at runtime — which is why
  the HTTP entry uses `requests`, not stdlib; a **nullary** Python function can't be called
  (`gettempdir ()` passes unit as an argument); a dotted target on a **builtin type**
  (`bytes.decode`) tries to `import bytes`; and object **properties** (`response.text`,
  `.status_code`) aren't reachable by the unbound-method trick (only real methods are).
