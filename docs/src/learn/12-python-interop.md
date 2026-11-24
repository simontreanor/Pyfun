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
extern pure to_path: str -> Path = pathlib.Path
extern pure suffix: Path -> str = .suffix
extern pure with_name: Path -> str -> Path = .with_name
extern pure as_text: Path -> str = builtins.str

let renamed = with_name (to_path "report.csv") "summary.csv"

"report.csv" |> to_path |> suffix |> print
renamed |> as_text |> print
```

This prints `.csv` then `summary.csv`. `extern type Path` declares an opaque handle: Pyfun knows the type exists and keeps it distinct, and it never looks inside. The dotted targets work on attributes (`.suffix`) and on methods with their own arguments (`.with_name`), and because the receiver is just the first parameter, these compose in a pipe like any other Pyfun function. Reading the signature tells you exactly what crosses the boundary, which is the whole point of writing it down.

The framing worth keeping is boundary versus engine. Pyfun shines at the boundary where the world is untyped and can fail, which is parsing, files, and the network. It adds little wrapped around an engine like numpy, whose speed lives in native code Pyfun cannot touch. Call the boundary safely and stay out of the engine's way.

The clearest boundary is untrusted JSON. When an `extern` can raise, `try` from lesson 4 turns the exception into a `Result` you must handle. Building on that, the built-in `Decode` module turns raw JSON straight into your own record type or a structured error, so the rest of your program never sees an untyped shape. `Decode.field` pulls one field and runs a decoder on it, `Decode.str` and `Decode.int` decode strictly, `Decode.map2` combines two field decoders into one that builds a record, and `Decode.decode_string` runs the whole thing over a JSON string to yield `Result a Exception`.

```pyfun
type Book = { title: str, pages: int }

let book_decoder =
  Decode.map2 (fun title pages -> Book { title = title, pages = pages })
    (Decode.field "title" Decode.str)
    (Decode.field "pages" Decode.int)

let describe r =
  match r:
    case Ok b: f"{b.title}, {b.pages} pages"
    case Error e: f"failed ({e.error_kind})"

let well_formed = """{"title": "Dune", "pages": 412}"""
let missing_field = """{"title": "Dune"}"""

well_formed |> Decode.decode_string book_decoder |> describe |> print
missing_field |> Decode.decode_string book_decoder |> describe |> print
```

The well-formed object decodes to a typed `Book`. The object missing `pages` short-circuits to an `Error` carrying the Python exception, which `match` forces you to handle. The output is `Dune, 412 pages` then `failed (KeyError)`.

## Derived codecs

Hand-written decoders are the right tool at a boundary you do not control. When both ends of the
wire are your own Pyfun types, the compiler already knows every field and every case, so it can
derive the codec. `Encode.auto` turns any value into JSON text, and `Decode.auto` is a decoder
derived from the type it is used at:

```pyfun
type Player = Ann | Bob
type Msg = Hello Player | Move str | Resign
type Turn = { player: Player, msg: Msg, score: Option int }

let turn = Turn { player = Bob, msg = Move "K11 a QUIZ", score = Some 42 }
let wire = Encode.auto turn
print wire

let describe t = f"{t.player} played {t.msg}"

match Decode.decode_string Decode.auto wire:
  case Ok back: print (describe back)
  case Error e: print f"failed: {e.error_message}"
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

## JSON that does not fit a record

Some files have no fields to name. This one keeps its sprites in an object keyed by id, and stores
each sprite as a small array: position 0 is the name and position 1 is where it stands.

```pyfun
type Sprite = { name: str, x: int }

let sprite =
  Decode.map2 (fun name x -> Sprite { name = name, x = x })
    (Decode.index 0 Decode.str)
    (Decode.index 1 Decode.int)

let saved = """{"k9": ["cat", 10], "a2": ["ball", 0]}"""

match Decode.decode_string (Decode.dict sprite) saved:
  case Ok found: found |> Map.to_list |> print
  case Error e: print e.error_message
```

```console
[('k9', Sprite(name='cat', x=10)), ('a2', Sprite(name='ball', x=0))]
```

`Decode.dict` reads an object whose keys are data and gives you a `Map`, with the keys in the order
the file had them. `Decode.index` reads one position of an array, counting from 0, so each position
can have its own decoder. An array that is too short is an `Error` that says which position was
missing.

Sometimes you want no shape at all. A program that edits a settings file should change the one
value it knows about and hand everything else back as it found it. `Decode.value` reads any JSON
into the built-in `Json` type, and `Encode.value` writes it out again:

```pyfun
let bump j =
  match j:
    case JObject m:
      match Map.find "version" m:
        case Some (JInt n): m |> Map.add "version" (JInt (n + 1)) |> JObject
        case _: j
    case _: j

let saved = """{"version": 3, "theme": {"dark": true, "scale": 1.0}}"""

match Decode.decode_string Decode.value saved:
  case Ok j: j |> bump |> Encode.value |> print
  case Error e: print e.error_message
```

```console
{"version": 4, "theme": {"dark": true, "scale": 1.0}}
```

`Json` is a sum type like the ones you wrote in lesson 5, with seven cases: `JNull`, `JBool`,
`JInt`, `JFloat`, `JStr`, `JList` and `JObject`. A `JList` holds a `List Json` and a `JObject` holds
a `Map str Json`, so everything you know about `match`, `List` and `Map` works on it. Look at
`scale`: it went in as `1.0` and came out as `1.0`, because an int and a float are different cases,
and the keys kept their order. You build a value with the same cases when you have JSON to write
and no type for it: `JObject {"version": JInt 1}`.

Reach for a decoder first. A record tells the rest of your program what it holds, and a `Json`
tells it only that the text was JSON.

## Handing Python a function

A callback crosses the boundary the other way, and two rules keep it honest. Write a callback of
several parameters *curried*, never over a tuple: a curried Pyfun function is a plain
multi-parameter `def`, which is what Python calls as `cb(reader, writer)`, while a function over a
pair is one parameter that Python would have to know to bundle. And a thunk, a parameter typed
`unit -> a`, is called by Python with no arguments at all, so Pyfun wraps it for you at the call:

```pyfun
extern run_async: Async a -> a = asyncio.run
extern to_thread: (unit -> a) -> Async a = asyncio.to_thread

let answer =
  async {
    let! v = to_thread (fun _ -> 6 * 7)
    return v
  }

print (run_async answer)
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
note: hole `?title_dec` has type `Decoder str` — try: Decode.str, Decode.auto — or: Decode.succeed ?, Decode.fail ?, Decode.one_of ?, Decode.field ? ?, and 3 more
```

```pyfun
type Book = { title: str, pages: int }

let book_decoder =
  Decode.map2 (fun title pages -> Book { title = title, pages = pages })
    (Decode.field "title" ?title_dec)
    (Decode.field "pages" ?pages_dec)

let describe r =
  match r:
    case Ok b: f"{b.title}, {b.pages} pages"
    case Error e: f"failed ({e.error_kind}): {e.error_message}"

let well_formed = """{"title": "Dune", "pages": 412}"""
let missing_field = """{"title": "Dune"}"""

well_formed |> Decode.decode_string book_decoder |> describe |> print
missing_field |> Decode.decode_string book_decoder |> describe |> print
```

Expected output:

```console
Dune, 412 pages
failed (KeyError): 'pages'
```

[Open in the playground](https://simontreanor.github.io/Pyfun/playground/#code=dHlwZSBCb29rID0geyB0aXRsZTogc3RyLCBwYWdlczogaW50IH0KCmxldCBib29rX2RlY29kZXIgPQogIERlY29kZS5tYXAyIChmdW4gdGl0bGUgcGFnZXMgLT4gQm9vayB7IHRpdGxlID0gdGl0bGUsIHBhZ2VzID0gcGFnZXMgfSkKICAgIChEZWNvZGUuZmllbGQgInRpdGxlIiA_dGl0bGVfZGVjKQogICAgKERlY29kZS5maWVsZCAicGFnZXMiID9wYWdlc19kZWMpCgpsZXQgZGVzY3JpYmUgciA9CiAgbWF0Y2ggcjoKICAgIGNhc2UgT2sgYjogZiJ7Yi50aXRsZX0sIHtiLnBhZ2VzfSBwYWdlcyIKICAgIGNhc2UgRXJyb3IgZTogZiJmYWlsZWQgKHtlLmVycm9yX2tpbmR9KToge2UuZXJyb3JfbWVzc2FnZX0iCgpsZXQgd2VsbF9mb3JtZWQgPSAiIiJ7InRpdGxlIjogIkR1bmUiLCAicGFnZXMiOiA0MTJ9IiIiCmxldCBtaXNzaW5nX2ZpZWxkID0gIiIieyJ0aXRsZSI6ICJEdW5lIn0iIiIKCndlbGxfZm9ybWVkIHw-IERlY29kZS5kZWNvZGVfc3RyaW5nIGJvb2tfZGVjb2RlciB8PiBkZXNjcmliZSB8PiBwcmludAptaXNzaW5nX2ZpZWxkIHw-IERlY29kZS5kZWNvZGVfc3RyaW5nIGJvb2tfZGVjb2RlciB8PiBkZXNjcmliZSB8PiBwcmludAo)

<details>
<summary>Show solution</summary>

```pyfun
type Book = { title: str, pages: int }

let book_decoder =
  Decode.map2 (fun title pages -> Book { title = title, pages = pages })
    (Decode.field "title" Decode.str)
    (Decode.field "pages" Decode.int)

let describe r =
  match r:
    case Ok b: f"{b.title}, {b.pages} pages"
    case Error e: f"failed ({e.error_kind}): {e.error_message}"

let well_formed = """{"title": "Dune", "pages": 412}"""
let missing_field = """{"title": "Dune"}"""

well_formed |> Decode.decode_string book_decoder |> describe |> print
missing_field |> Decode.decode_string book_decoder |> describe |> print
```

`Decode.str` decodes the `title` field and `Decode.int` decodes `pages`. The valid object builds a `Book`, and the incomplete one short-circuits to a `KeyError` that `describe` reports through the `Error` arm.
</details>
