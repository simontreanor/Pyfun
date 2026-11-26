# pyfun-sqlite: typed SQLite for Pyfun

A Pyfun façade over Python's `sqlite3`. Values travel as bound parameters, never as SQL text, and
rows are read into your own records by decoders that report a bad row as a value naming the
column. It needs nothing but the standard library.

```bash
cd my-project
pyfun add pyfun-sqlite                      # once it is on PyPI; until then:
pyfun add path/to/Pyfun/examples/facades/pyfun-sqlite
```

`pyfun add` installs the package, copies `sqlite.pyfun` into `.pyfun/facades/pyfun-sqlite/`, and
records the dependency in `pyfun.toml`. The module is then `import Sqlite`:

```pyfun
import Sqlite
from Sqlite import Integer, Text

type Score = { player: str, points: int }

let score =
  Sqlite.map2 (fun player points -> Score { player = player, points = points })
    (Sqlite.field "player" Sqlite.str)
    (Sqlite.field "points" Sqlite.int)

let db = Sqlite.open "scores.db"
Sqlite.script db "create table if not exists scores (player text, points integer)"
Sqlite.execute db "insert into scores values (?, ?)" [Text "ada", Integer 420] |> ignore

match Sqlite.query db "select player, points from scores where points > ?" [Integer 100] score:
  case Ok scores: scores |> List.iter print
  case Error e: print (Sqlite.explain e)
```

[`demo/`](https://github.com/simontreanor/Pyfun/tree/main/examples/facades/pyfun-sqlite/demo) is a small project that uses it the way a user would (a high-score table with
transactions), with tests: `cd demo && pyfun run main.pyfun`, and `pyfun test`. Its vendored copy
in `.pyfun/facades/` is committed, so it runs as checked out; after changing `sqlite.pyfun`, run
`pyfun add ..` in the demo to vendor the new one.

## The API

| | |
|---|---|
| **Values** | `type Value = Integer int \| Real float \| Text str \| Blob Bytes \| Null` — SQLite's five storage classes. `Sqlite.show_value`; `Sqlite.bytes_of_list` / `bytes_to_list` / `bytes_len` for a `Bytes`. |
| **Connections** | `open : str ->{io, exn} Conn`, `close : Conn ->{io, exn} unit`, `with_open : str -> (Conn -> a) ->{io} Result a Exception` (closes whatever the body did). |
| **Statements** | `execute conn sql params` (rows changed), `insert conn sql params` (the new rowid), `script conn sql` (several statements, no parameters: for schema). All `->{io, exn}`. |
| **Queries** | `query conn sql params row : Result (List a) RowError`, `query_one … : Result (Option a) RowError`. Both `->{io, exn}`. |
| **Cells** | `Sqlite.int`, `float` (accepts an INTEGER), `str`, `bytes`, `bool` (INTEGER 0 or 1 only), `value` (anything, as a `Value`), `nullable cell` (NULL is `None`), `check f cell` (your own test or conversion, `f : a -> Result b str`). |
| **Rows** | `field name cell`, `at index cell`, `succeed`, `map`, `map2`, `map3`, `map4`, `and_then`; `decode row columns` reads a row with no database. |
| **Errors** | `RowError { row, column, problem }` with `Problem = NoSuchColumn (List str) \| WrongType str Value \| Invalid str`; `Sqlite.explain` writes one as a line. |
| **Transactions** | `transaction conn body`: commits when `body` answers `Ok`, rolls back when it answers `Error` or raises. `Result a (Rollback e)`, `Rollback e = Abandoned e \| Raised Exception`. They nest. |

## Design

**The safe path is the only path.** A query is SQL text plus a `List Value`, one for each `?`, and
`sqlite3` binds them. There is no function that takes a value and puts it into SQL, so there is
nothing to forget to escape: `[Text "x'); drop table scores; --"]` stores an odd name. `Value` is a
closed sum of SQLite's own storage classes, so a parameter cannot be a Python object `sqlite3`
would refuse or quietly turn into text.

**Rows are decoded the way `Decode` decodes JSON.** A `Sqlite.Cell a` reads one value and a
`Sqlite.Row a` reads a row, built with `field` and `map2`/`map3`/… exactly as a JSON decoder is built
with `Decode.field` and `Decode.map2`. The difference is what a failure says. Every failure happens
at a column, so every failure names one: the row (from 0), the column, and the problem —
`row 2, column "points": expected int, found TEXT "many"`, or `no such column; the result has
"player", "score"`. The cells are strict where SQLite is loose: a column declared `integer` can hold
TEXT (SQLite's type affinity is a preference, not a rule), and `Sqlite.int` says so rather than
converting. `Sqlite.float` accepts an INTEGER, as `Decode.float` accepts a JSON int.

**Decoding is pure, and the type shows it.** The function inside a `Cell` or `Row` is declared with
a pure arrow, so a decoder cannot do I/O; `Sqlite.decode` runs one over a list of `(name, Value)`
pairs with no database at all, and a `let pure` over it type-checks. That is how the demo's tests
check a decoder.

**Effects are honest.** Everything that talks to `sqlite3` is `->{io, exn}`: `io` because it reads
or changes a file, and `exn` because `sqlite3` raises — on a mistake in the SQL, a broken
constraint, a locked database. The `exn` is not caught for you, because only the caller knows
where the line between "this is a bug" and "this is an outcome" falls; `try` turns it into a
`Result a Exception` wherever you put it (`try (Sqlite.execute db sql params)`). Two functions
catch it themselves, because they must clean up first: `with_open` closes the connection and
`transaction` rolls back, and both return the exception as their `Error`, so their own type no
longer carries the `exn` of the body.

**A query's two kinds of failure stay apart.** `query` is `->{io, exn} Result (List a) RowError`:
the `exn` is SQLite refusing the statement, and the `Error` is data that is not the shape the
program expects. They call for different handling, so they are not merged into one error type.

**Transactions are savepoints.** A connection is opened in autocommit mode (3.12's
`autocommit=True`), so each statement commits as it runs, and nothing is ever left half-open by
Python's legacy implicit `BEGIN`. `transaction` issues `SAVEPOINT`, then `RELEASE` (commit) or
`ROLLBACK TO` + `RELEASE` (undo). A savepoint outside a transaction starts one, and inside one it
nests, so a function that uses a transaction can be called from inside another. The body answers a
`Result`: `Error reason` abandons the transaction on purpose (`Abandoned reason`), and a raise
inside it abandons it too (`Raised exception`). A `RELEASE` that fails (a deferred constraint)
rolls back and is `Raised`.

**No Python helper code.** The package is one `.pyfun` file. The boundary is a few `extern`s onto
`sqlite3`'s own methods and onto builtins (`isinstance`, `int`, `bytes`, `list`), and everything
else — the `Value` conversion, the decoders, the transaction logic — is ordinary Pyfun. Vendoring
the file is therefore all a project needs at compile time, and the pip package is all it needs at
run time.

## What is checked, and what is not

SQL is a language in a string, the same stringly-typed sublanguage Pyfun turned down for f-string
format specs, and this façade does not pretend otherwise. The compiler checks:

- that every parameter is one of the five SQLite values, and that values are never spliced into SQL;
- that a decoder is pure, and that what it builds has the types your record says;
- that the database calls carry `io` and `exn`, so `let pure` keeps them out and `try` handles them.

It does **not** check:

- the SQL itself: a misspelt keyword, table or column is an `exn` when the statement runs;
- that the number of `?`s matches the number of parameters (an `exn` at run time);
- that a decoder's column names and types match the query: that is found when a row is read, and
  is the `RowError`, naming the column. A query with no rows reads no columns, so a mismatch shows
  only once there is data;
- duplicate column names (from a join): `field` reads the first; alias them in the SQL.

Checking SQL against a schema at compile time would need the compiler to know SQL, and is out of
scope for a façade.

## Effects, and why there is no `db` label

A `db` label would let a type say "touches the database" apart from other I/O. Pyfun's labels are
a fixed set (`io`, `async`, `spawn`, `exn`), and an `extern` cannot declare a new one: a lowercase
name in an `extern` arrow that is not a known label is an *effect variable*, so `Conn ->{db} int`
would not mean what it says. The façade uses `io`; the proposal for user-declared labels is in
`ROADMAP.md`.

## Versions

`0.1.0`, the first release. It needs Python 3.12 or later (for `sqlite3.connect(autocommit=True)`),
which is Pyfun's own floor. Not yet published to PyPI.
