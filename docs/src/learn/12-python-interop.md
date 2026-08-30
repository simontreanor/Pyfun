# 12. Talking to Python: extern

Pyfun compiles to Python, so the whole Python ecosystem is within reach. The way in is `extern`: you name a real Python callable and give it a Pyfun type. `extern name: Type = dotted.target` imports the target and lets the rest of your program call it with full type checking. The boundary is effectful by default, because most of the world is, so a plain `extern` is `io`. When a call is genuinely deterministic and side-effect free, `extern pure` asserts that, and then the purity checking from lesson 11 can prove whole pipelines pure across the boundary.

```pyfun
extern pure mean: List float -> float = statistics.mean

let readings = [2.0, 4.0, 9.0]

readings |> mean |> print
```

This prints `5.0`. The emitted Python is the direct call you would expect, with the import added for you:

```python
import statistics
readings = [2.0, 4.0, 9.0]
print(statistics.mean(readings))
```

## Naming the module when the target cannot be read

`= statistics.mean` gave the compiler an easy job: `statistics` is the module and `mean` is the function, so `import statistics` is the only sensible import. Deeper targets are not always so clear. In `sys.stdout.flush`, the middle segment `stdout` could be a submodule the way `os.path` is, or an object the way `sys.stdout` actually is, and which one it is depends on the running Python rather than on the text. So the compiler declines to guess:

```pyfun
extern flush: unit -> unit = sys.stdout.flush
```

```console
error: cannot tell which part of `sys.stdout.flush` names the module: `stdout` is lowercase, so it could be a submodule (like `os.path`) or an object (like `sys.stdout`), and only the running environment knows which; declare it with `extern import sys` — or `extern import sys.stdout` if `stdout` really is a module
```

The fix is the line the error names. `extern import` is Python's own import statement, and it settles the question for every target in the file:

```pyfun
extern import sys

extern flush: unit -> unit = sys.stdout.flush

print "written"
flush ()
```

This emits `import sys` followed by `sys.stdout.flush()`, which is what you would have written by hand. `extern import` takes an alias too, so `extern import numpy as np` lets your targets say `np.zeros` and emits `import numpy as np`. Reach for it whenever a target has a lowercase segment in the middle, and whenever you want a specific import spelling regardless.

## Calling a method on a value

Plenty of Python libraries hand you an object and expect you to call methods on it. A target that begins with a dot is a member of the *first argument* rather than a name in a module, which is how a Pyfun function signature wraps a method:

```pyfun
extern type Path
extern pure toPath: string -> Path = pathlib.Path
extern pure suffix: Path -> string = .suffix
extern pure withName: Path -> string -> Path = .with_name
extern pure asText: Path -> string = builtins.str

let renamed = withName (toPath "report.csv") "summary.csv"

"report.csv" |> toPath |> suffix |> print
renamed |> asText |> print
```

This prints `.csv` then `summary.csv`. `extern type Path` declares an opaque handle: Pyfun knows the type exists and keeps it distinct, and it never looks inside. The dotted targets work on attributes (`.suffix`) and on methods with their own arguments (`.with_name`), and because the receiver is just the first parameter, these compose in a pipe like any other Pyfun function. Reading the signature tells you exactly what crosses the boundary, which is the whole point of writing it down.

The framing worth keeping is boundary versus engine. Pyfun shines at the boundary where the world is untyped and can fail, which is parsing, files, and the network. It adds little wrapped around an engine like numpy, whose speed lives in native code Pyfun cannot touch. Call the boundary safely and stay out of the engine's way.

The clearest boundary is untrusted JSON. When an `extern` can raise, `try` from lesson 4 turns the exception into a `Result` you must handle. Building on that, the built-in `Decode` module turns raw JSON straight into your own record type or a structured error, so the rest of your program never sees an untyped shape. `Decode.field` pulls one field and runs a decoder on it, `Decode.string` and `Decode.int` decode strictly, `Decode.map2` combines two field decoders into one that builds a record, and `Decode.decodeString` runs the whole thing over a JSON string to yield `Result a Exception`.

```pyfun
type Book = { title: string, pages: int }

let bookDecoder =
  Decode.map2 (fun title pages -> Book { title = title, pages = pages })
    (Decode.field "title" Decode.string)
    (Decode.field "pages" Decode.int)

let describe r =
  match r:
    case Ok b: f"{b.title}, {b.pages} pages"
    case Error e: f"failed ({e.errorKind})"

let wellFormed = """{"title": "Dune", "pages": 412}"""
let missingField = """{"title": "Dune"}"""

wellFormed |> Decode.decodeString bookDecoder |> describe |> print
missingField |> Decode.decodeString bookDecoder |> describe |> print
```

The well-formed object decodes to a typed `Book`. The object missing `pages` short-circuits to an `Error` carrying the Python exception, which `match` forces you to handle. The output is `Dune, 412 pages` then `failed (KeyError)`.

## Derived codecs

Hand-written decoders are the right tool at a boundary you do not control. When both ends of the
wire are your own Pyfun types, the compiler already knows every field and every case, so it can
derive the codec. `Encode.auto` turns any value into JSON text, and `Decode.auto` is a decoder
derived from the type it is used at:

```pyfun
type Player = Ann | Bob
type Msg = Hello Player | Move string | Resign
type Turn = { player: Player, msg: Msg, score: Option int }

let turn = Turn { player = Bob, msg = Move "K11 a QUIZ", score = Some 42 }
let wire = Encode.auto turn
print wire

let describe t = f"{t.player} played {t.msg}"

match Decode.decodeString Decode.auto wire:
  case Ok back: print (describe back)
  case Error e: print f"failed: {e.errorMessage}"
```

```console
{"player": {"type": "Bob"}, "msg": {"type": "Move", "fields": ["K11 a QUIZ"]}, "score": 42}
Bob played Move('K11 a QUIZ')
```

A record is an object keyed by its field names, a case is `{"type": …, "fields": […]}`, an `Option`
is `null` or the value, and a `Map` with string keys is an object. The decoder is strict like the
primitives, so `{"type": "Nope"}` is an `Error` naming the unknown case rather than a crash later.
`Decode.auto` reads the type from where it is used: here `describe back` fixes `back` to a `Turn`. If
nothing fixes it, the compiler says so at the site instead of guessing.

## Handing Python a function

A callback crosses the boundary the other way, and two rules keep it honest. Write a callback of
several parameters *curried*, never over a tuple: a curried Pyfun function is a plain
multi-parameter `def`, which is what Python calls as `cb(reader, writer)`, while a function over a
pair is one parameter that Python would have to know to bundle. And a thunk, a parameter typed
`unit -> a`, is called by Python with no arguments at all, so Pyfun wraps it for you at the call:

```pyfun
extern runAsync: Async a -> a = asyncio.run
extern toThread: (unit -> a) -> Async a = asyncio.to_thread

let answer =
  async {
    let! v = toThread (fun _ -> 6 * 7)
    return v
  }

print (runAsync answer)
```

```console
42
```

`asyncio.to_thread` calls the function it is given with no arguments, on a worker thread; the
Pyfun thunk takes the unit value, so the call site hands Python `lambda: f(None)`. The callback's
effects go on the extern's parameter arrow, because declared effects are exact: `(unit -> a)`
accepts only a pure thunk, `(unit ->{io} a)` one that prints, and an effect variable
`(unit ->{e} a)` either, with `e` flowing to the result arrow (`->{io, e}`) so the caller inherits
whatever the callback performs.

## Exercise

Complete the decoder by filling both holes with the strict field decoders. `pyfun check` reports each hole's type and suggests the fit. The first report is:

```console
note: hole `?titleDec` has type `Decoder string` — try: Decode.string — or: Decode.fail ?, Decode.oneOf ?, Decode.succeed ?, Decode.field ? ?
```

```pyfun
type Book = { title: string, pages: int }

let bookDecoder =
  Decode.map2 (fun title pages -> Book { title = title, pages = pages })
    (Decode.field "title" ?titleDec)
    (Decode.field "pages" ?pagesDec)

let describe r =
  match r:
    case Ok b: f"{b.title}, {b.pages} pages"
    case Error e: f"failed ({e.errorKind}): {e.errorMessage}"

let wellFormed = """{"title": "Dune", "pages": 412}"""
let missingField = """{"title": "Dune"}"""

wellFormed |> Decode.decodeString bookDecoder |> describe |> print
missingField |> Decode.decodeString bookDecoder |> describe |> print
```

Expected output:

```console
Dune, 412 pages
failed (KeyError): 'pages'
```

[Open in the playground](https://simontreanor.github.io/Pyfun/playground/#code=dHlwZSBCb29rID0geyB0aXRsZTogc3RyaW5nLCBwYWdlczogaW50IH0KCmxldCBib29rRGVjb2RlciA9CiAgRGVjb2RlLm1hcDIgKGZ1biB0aXRsZSBwYWdlcyAtPiBCb29rIHsgdGl0bGUgPSB0aXRsZSwgcGFnZXMgPSBwYWdlcyB9KQogICAgKERlY29kZS5maWVsZCAidGl0bGUiID90aXRsZURlYykKICAgIChEZWNvZGUuZmllbGQgInBhZ2VzIiA_cGFnZXNEZWMpCgpsZXQgZGVzY3JpYmUgciA9CiAgbWF0Y2ggcjoKICAgIGNhc2UgT2sgYjogZiJ7Yi50aXRsZX0sIHtiLnBhZ2VzfSBwYWdlcyIKICAgIGNhc2UgRXJyb3IgZTogZiJmYWlsZWQgKHtlLmVycm9yS2luZH0pOiB7ZS5lcnJvck1lc3NhZ2V9IgoKbGV0IHdlbGxGb3JtZWQgPSAiIiJ7InRpdGxlIjogIkR1bmUiLCAicGFnZXMiOiA0MTJ9IiIiCmxldCBtaXNzaW5nRmllbGQgPSAiIiJ7InRpdGxlIjogIkR1bmUifSIiIgoKd2VsbEZvcm1lZCB8PiBEZWNvZGUuZGVjb2RlU3RyaW5nIGJvb2tEZWNvZGVyIHw-IGRlc2NyaWJlIHw-IHByaW50Cm1pc3NpbmdGaWVsZCB8PiBEZWNvZGUuZGVjb2RlU3RyaW5nIGJvb2tEZWNvZGVyIHw-IGRlc2NyaWJlIHw-IHByaW50Cg)

<details>
<summary>Show solution</summary>

```pyfun
type Book = { title: string, pages: int }

let bookDecoder =
  Decode.map2 (fun title pages -> Book { title = title, pages = pages })
    (Decode.field "title" Decode.string)
    (Decode.field "pages" Decode.int)

let describe r =
  match r:
    case Ok b: f"{b.title}, {b.pages} pages"
    case Error e: f"failed ({e.errorKind}): {e.errorMessage}"

let wellFormed = """{"title": "Dune", "pages": 412}"""
let missingField = """{"title": "Dune"}"""

wellFormed |> Decode.decodeString bookDecoder |> describe |> print
missingField |> Decode.decodeString bookDecoder |> describe |> print
```

`Decode.string` decodes the `title` field and `Decode.int` decodes `pages`. The valid object builds a `Book`, and the incomplete one short-circuits to a `KeyError` that `describe` reports through the `Error` arm.
</details>
