# Session 5: Reaching the Python ecosystem

## Objectives

By the end of this session, students can:

- Import a real Python callable with `extern` and give it a Pyfun type, using `extern pure` where a
  call is genuinely side-effect free.
- Decode untrusted JSON into a typed record or a structured error with the `Decode` module.
- Rewrite a staircase of `Result` matches as a flat `result { }` computation expression using `let!`
  and `return`.
- Tag a number with a unit of measure and let the compiler check dimensions, then explain that units
  erase at lowering.
- Group definitions into a module and qualify calls as `Module.member`.
- Assemble a small end-to-end pipeline that decodes, folds, and computes a unit-checked result.

## Prerequisites

Sessions 1 through 4. The capstone leans on records (Session 3), `Result` and `match` (Session 2),
and folds (Session 3), so this session is best after the whole unit.

## Six-session variant

If you are running the unit as six sessions, split here. Session 5 becomes lessons 12 and 13 (extern
and decoding, then computation expressions), and Session 6 becomes lessons 14 through 16 (units,
modules, and the capstone), with the capstone as the finale. The demo script below is ordered so the
split falls cleanly after step 4.

## Demo script

The thread is the boundary versus the engine: Pyfun earns its keep where the outside world is untyped
and can fail, and it stays out of the way of fast native code.

1. Open the [Lesson 12 exercise](https://simontreanor.github.io/Pyfun/playground/#code=dHlwZSBCb29rID0geyB0aXRsZTogc3RyaW5nLCBwYWdlczogaW50IH0KCmxldCBib29rRGVjb2RlciA9CiAgRGVjb2RlLm1hcDIgKGZ1biB0aXRsZSBwYWdlcyAtPiBCb29rIHsgdGl0bGUgPSB0aXRsZSwgcGFnZXMgPSBwYWdlcyB9KQogICAgKERlY29kZS5maWVsZCAidGl0bGUiID90aXRsZURlYykKICAgIChEZWNvZGUuZmllbGQgInBhZ2VzIiA_cGFnZXNEZWMpCgpsZXQgZGVzY3JpYmUgciA9CiAgbWF0Y2ggcjoKICAgIGNhc2UgT2sgYjogZiJ7Yi50aXRsZX0sIHtiLnBhZ2VzfSBwYWdlcyIKICAgIGNhc2UgRXJyb3IgZTogZiJmYWlsZWQgKHtlLmVycm9yS2luZH0pOiB7ZS5lcnJvck1lc3NhZ2V9IgoKbGV0IHdlbGxGb3JtZWQgPSAiIiJ7InRpdGxlIjogIkR1bmUiLCAicGFnZXMiOiA0MTJ9IiIiCmxldCBtaXNzaW5nRmllbGQgPSAiIiJ7InRpdGxlIjogIkR1bmUifSIiIgoKd2VsbEZvcm1lZCB8PiBEZWNvZGUuZGVjb2RlU3RyaW5nIGJvb2tEZWNvZGVyIHw-IGRlc2NyaWJlIHw-IHByaW50Cm1pc3NpbmdGaWVsZCB8PiBEZWNvZGUuZGVjb2RlU3RyaW5nIGJvb2tEZWNvZGVyIHw-IGRlc2NyaWJlIHw-IHByaW50Cg).
   Run `check` and read the `?titleDec` note: it has type `Decoder string` and suggests
   `Decode.string`. Fill both holes (`Decode.string`, `Decode.int`) and Run. The well-formed object
   becomes a typed `Book`, and the object missing `pages` becomes `failed (KeyError): 'pages'`.
2. Make the boundary-versus-engine point: the JSON is the untyped, failable edge, and decoding turns
   it into a `Book` or a structured error before the rest of the program sees it. Contrast with
   wrapping something like numpy, where Pyfun adds little because the speed lives in native code.
3. Open the [Lesson 13 exercise](https://simontreanor.github.io/Pyfun/playground/#code=bGV0IG11bHRpcGx5IGEgYiA9CiAgcmVzdWx0IHsKICAgIGxldCEgeCA9ID8KICAgIGxldCEgeSA9ID8KICAgIHJldHVybiB4ICogeQogIH0KCnByaW50IChtdWx0aXBseSAiNiIgIjciKQpwcmludCAobXVsdGlwbHkgIjYiICJub3BlIikK).
   Before filling it, show the staircase form from the top of lesson 13 (two nested `Result` matches)
   and point out that every `Error` branch does the same thing. Then fill the two holes with
   `Option.toResult "not a number" (String.toInt a)` and its `b` twin and Run.
4. Make the point that `let!` unwraps an `Ok`, `return` wraps the final value, and the short-circuit
   on the first `Error` is automatic. The `result { }` block is the staircase written once. (If you
   are splitting into six sessions, this is the break point.)
5. Open the [Lesson 14 exercise](https://simontreanor.github.io/Pyfun/playground/#code=bWVhc3VyZSBtCm1lYXN1cmUgcwoKbGV0IGRpc3RhbmNlID0gMjQwLjA8bT4KbGV0IGVsYXBzZWQgPSAzMC4wPHM-CgojIFNwZWVkIGlzIGRpc3RhbmNlIHBlciB1bml0IG9mIHRpbWUuIFRoaXMgbGluZSBkb2VzIG5vdCB0eXBlLWNoZWNrIHlldC4KbGV0IHNwZWVkID0gZGlzdGFuY2UgKyBlbGFwc2VkCgpwcmludCBzcGVlZAo).
   It adds a distance to a time. Run `check` and read the mismatch: `expected float<m>, found
   float<s>`. Change the `+` to `/` and Run to get `8.0`. Show the Python panel: the units are gone,
   the output is plain `distance / elapsed`. Units are checked, then erased.
6. Open the [Lesson 15 exercise](https://simontreanor.github.io/Pyfun/playground/#code=bGV0IGNUb0YgYyA9IGMgKiA5IC8vIDUgKyAzMgpsZXQgZlRvQyBmID0gKGYgLSAzMikgKiA1IC8vIDkKCnByaW50IChjVG9GIDEwMCkKcHJpbnQgKGZUb0MgMjEyKQo).
   Wrap the two functions in `module Temp =` and qualify the calls as `Temp.cToF` and `Temp.fToC`.
   Run. Note that this in-file module runs in the playground, while one-module-per-file projects need
   the installed compiler.
7. Finish with the capstone. Open the [Lesson 16 exercise](https://simontreanor.github.io/Pyfun/playground/#code=bWVhc3VyZSBtCm1lYXN1cmUgcwoKdHlwZSBSdW4gPSB7IG5hbWU6IHN0cmluZywgZGlzdGFuY2U6IGZsb2F0LCB0aW1lOiBmbG9hdCB9CgpsZXQgcnVuRGVjb2RlciA9CiAgRGVjb2RlLm1hcDMgKGZ1biBuIGQgdCAtPiBSdW4geyBuYW1lID0gbiwgZGlzdGFuY2UgPSBkLCB0aW1lID0gdCB9KQogICAgKERlY29kZS5maWVsZCAibmFtZSIgRGVjb2RlLnN0cmluZykKICAgIChEZWNvZGUuZmllbGQgImRpc3RhbmNlIiA_KQogICAgKERlY29kZS5maWVsZCAidGltZSIgRGVjb2RlLmZsb2F0KQoKbGV0IGlucHV0ID0gIiIiWwogIHsibmFtZSI6ICJhZGEiLCAiZGlzdGFuY2UiOiAxMDAuMCwgInRpbWUiOiAyMC4wfSwKICB7Im5hbWUiOiAiYm8iLCAiZGlzdGFuY2UiOiAxNDAuMCwgInRpbWUiOiAzMC4wfQpdIiIiCgpsZXQgcmVwb3J0ID0KICBtYXRjaCBEZWNvZGUuZGVjb2RlU3RyaW5nIChEZWNvZGUubGlzdCBydW5EZWNvZGVyKSBpbnB1dDoKICAgIGNhc2UgT2sgcnVuczoKICAgICAgbGV0IG5hbWVzID0gcnVucyB8PiBMaXN0Lm1hcCAoZnVuIHIgLT4gci5uYW1lKSB8PiBTdHJpbmcuam9pbiAiLCAiCiAgICAgIGxldCBkaXN0ID0gcnVucyB8PiBMaXN0LmZvbGQgKGZ1biBhIHIgLT4gYSArIHIuZGlzdGFuY2UgKiA_KSAwLjA8bT4KICAgICAgbGV0IHRpbWUgPSBydW5zIHw-IExpc3QuZm9sZCAoZnVuIGEgciAtPiBhICsgci50aW1lICogMS4wPHM-KSAwLjA8cz4KICAgICAgbGV0IGF2ZyA9IGRpc3QgLyA_CiAgICAgIGYie25hbWVzfTogYXZlcmFnZSBzcGVlZCB7YXZnfSBtL3Mgb3ZlciB7TGlzdC5sZW4gcnVuc30gcnVucyIKICAgIGNhc2UgRXJyb3IgZTogZiJjb3VsZCBub3QgcmVhZCBpbnB1dCAoe2UuZXJyb3JLaW5kfSkiCgpwcmludCByZXBvcnQK).
   Run `check` and let the three hole notes name each type: `Decoder float`, `float<m>`, `float<'a>`.
   Fill them (`Decode.float`, `1.0<m>`, `time`) and Run to get the one-line report. Point at the join
   where JSON meets units: a decoded number is a plain `float`, and multiplying by `1.0<m>` lifts it
   into metres so the fold carries a `float<m>`.
8. Close the unit by naming what the capstone touched: types, matching, records, collections,
   decoding, computation expressions, units, and modules. Every piece the language showcases, in one
   small program that fails safely on bad input.

## Assigned exercises

- In class: the [Lesson 12 exercise](../learn/12-python-interop.md) and the
  [Lesson 13 exercise](../learn/13-computation-expressions.md), driven from the demo.
- Homework: the [Lesson 14 exercise](../learn/14-units-of-measure.md) and the
  [Lesson 15 exercise](../learn/15-modules.md).
- Capstone: the [Lesson 16 exercise](../learn/16-capstone.md), fill all three holes. Suitable as an
  in-class finale or a take-home assessment. As a stretch, ask students to feed the decoder a
  malformed JSON string and confirm the `Error` branch reports rather than crashes.

## Common misconceptions

- "`extern` means the Python call is safe now." Correction: `extern` gives the call a type, but the
  boundary is effectful by default and can still raise. Wrap a call that can fail in `try`, or decode
  the data, to turn failure into a value.
- "Decoding JSON is just `json.loads`." Correction: `json.loads` hands back an untyped shape.
  `Decode.decodeString` produces your record or a structured error, so the rest of the program never
  meets an untyped value.
- "`result { }` is a special block that runs differently at runtime." Correction: it desugars to the
  same `Result` short-circuit you would write by hand with nested matches. `let!` and `return` are
  sugar for bind and wrap.
- "Units of measure add runtime overhead to every number." Correction: units exist only during type
  checking and erase at lowering. The emitted Python is plain numbers.
- "A module changes how the code runs or performs." Correction: an in-file module is purely
  organizational namespacing. Members call each other by bare name inside, and you qualify them as
  `Module.member` from outside.

## Timing

- 10 min: recap the unit so far, then frame boundary versus engine.
- 20 min: demo steps 1 to 2 (extern and JSON decoding).
- 15 min: demo steps 3 to 4 (computation expressions).
- 15 min: demo steps 5 to 6 (units and modules).
- 20 min: demo steps 7 to 8 (the capstone) or students start it themselves.
- 10 min: unit wrap-up and pointers to the full course and playground.

Answer keys: [answer-keys.md](answer-keys.md#session-5)
