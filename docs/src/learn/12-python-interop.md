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
