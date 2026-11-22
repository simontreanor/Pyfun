# 16. Capstone: a typed pipeline

This last lesson puts the course together in one small program. It takes a JSON string of race
results, decodes each into a record you define (lesson 12), handles the failure branch as a value
(lessons 4 and 5), folds over the list to combine them (lesson 7), and computes an average speed that
the compiler checks dimensionally (lesson 14). The whole thing prints a one-line report with an
f-string.

The one join worth watching is where JSON meets units. A decoded number is a plain `float` with no
unit attached, because JSON has no notion of metres or seconds. You attach the unit yourself by
multiplying by a unit constant like `1.0<m>`, which turns a `float` into a `float<m>`. From there the
units carry through the arithmetic on their own, so dividing a folded `float<m>` by a folded
`float<s>` gives the average speed in `float<m/s>` without any annotation.

The exercise is the program itself. Three holes sit at the interesting joints: the decoder for the
distance field, the unit constant that lifts a decoded distance into metres, and the divisor that
turns total distance and total time into a speed. Run `pyfun check` and the compiler tells you the
type each hole expects, along with names in scope that fit. Fill all three and the report prints.

```pyfun
measure m
measure s

type Run = { name: str, distance: float, time: float }

let run_decoder =
  Decode.map3 (fun n d t -> Run { name = n, distance = d, time = t })
    (Decode.field "name" Decode.str)
    (Decode.field "distance" ?)
    (Decode.field "time" Decode.float)

let input = """[
  {"name": "ada", "distance": 100.0, "time": 20.0},
  {"name": "bo", "distance": 140.0, "time": 30.0}
]"""

let report =
  match Decode.decode_string (Decode.list run_decoder) input:
    case Ok runs:
      let names = runs |> List.map (fun r -> r.name) |> String.join ", "
      let dist = runs |> List.fold (fun a r -> a + r.distance * ?) 0.0<m>
      let time = runs |> List.fold (fun a r -> a + r.time * 1.0<s>) 0.0<s>
      let avg = dist / ?
      f"{names}: average speed {avg} m/s over {List.len runs} runs"
    case Error e: f"could not read input ({e.error_kind})"

print report
```

The checker names each expected type:

```console
note: hole `?` has type `Decoder float` — try: Decode.float, Decode.auto — or: Decode.succeed ?, Decode.fail ?, Decode.one_of ?, Decode.field ? ?
note: hole `?` has type `float<m>` — try: a — or: sqrt ?, cbrt ?, abs ?, min ? ?
note: hole `?` has type `float<'a>` — try: dist, time — or: sqrt ?, cbrt ?, abs ?, min ? ?
```

Expected output:

```console
ada, bo: average speed 4.8 m/s over 2 runs
```

[Open in the playground](https://simontreanor.github.io/Pyfun/playground/#code=bWVhc3VyZSBtCm1lYXN1cmUgcwoKdHlwZSBSdW4gPSB7IG5hbWU6IHN0ciwgZGlzdGFuY2U6IGZsb2F0LCB0aW1lOiBmbG9hdCB9CgpsZXQgcnVuX2RlY29kZXIgPQogIERlY29kZS5tYXAzIChmdW4gbiBkIHQgLT4gUnVuIHsgbmFtZSA9IG4sIGRpc3RhbmNlID0gZCwgdGltZSA9IHQgfSkKICAgIChEZWNvZGUuZmllbGQgIm5hbWUiIERlY29kZS5zdHIpCiAgICAoRGVjb2RlLmZpZWxkICJkaXN0YW5jZSIgPykKICAgIChEZWNvZGUuZmllbGQgInRpbWUiIERlY29kZS5mbG9hdCkKCmxldCBpbnB1dCA9ICIiIlsKICB7Im5hbWUiOiAiYWRhIiwgImRpc3RhbmNlIjogMTAwLjAsICJ0aW1lIjogMjAuMH0sCiAgeyJuYW1lIjogImJvIiwgImRpc3RhbmNlIjogMTQwLjAsICJ0aW1lIjogMzAuMH0KXSIiIgoKbGV0IHJlcG9ydCA9CiAgbWF0Y2ggRGVjb2RlLmRlY29kZV9zdHJpbmcgKERlY29kZS5saXN0IHJ1bl9kZWNvZGVyKSBpbnB1dDoKICAgIGNhc2UgT2sgcnVuczoKICAgICAgbGV0IG5hbWVzID0gcnVucyB8PiBMaXN0Lm1hcCAoZnVuIHIgLT4gci5uYW1lKSB8PiBTdHJpbmcuam9pbiAiLCAiCiAgICAgIGxldCBkaXN0ID0gcnVucyB8PiBMaXN0LmZvbGQgKGZ1biBhIHIgLT4gYSArIHIuZGlzdGFuY2UgKiA_KSAwLjA8bT4KICAgICAgbGV0IHRpbWUgPSBydW5zIHw-IExpc3QuZm9sZCAoZnVuIGEgciAtPiBhICsgci50aW1lICogMS4wPHM-KSAwLjA8cz4KICAgICAgbGV0IGF2ZyA9IGRpc3QgLyA_CiAgICAgIGYie25hbWVzfTogYXZlcmFnZSBzcGVlZCB7YXZnfSBtL3Mgb3ZlciB7TGlzdC5sZW4gcnVuc30gcnVucyIKICAgIGNhc2UgRXJyb3IgZTogZiJjb3VsZCBub3QgcmVhZCBpbnB1dCAoe2UuZXJyb3Jfa2luZH0pIgoKcHJpbnQgcmVwb3J0Cg)

<details>
<summary>Show solution</summary>

```pyfun
measure m
measure s

type Run = { name: str, distance: float, time: float }

let run_decoder =
  Decode.map3 (fun n d t -> Run { name = n, distance = d, time = t })
    (Decode.field "name" Decode.str)
    (Decode.field "distance" Decode.float)
    (Decode.field "time" Decode.float)

let input = """[
  {"name": "ada", "distance": 100.0, "time": 20.0},
  {"name": "bo", "distance": 140.0, "time": 30.0}
]"""

let report =
  match Decode.decode_string (Decode.list run_decoder) input:
    case Ok runs:
      let names = runs |> List.map (fun r -> r.name) |> String.join ", "
      let dist = runs |> List.fold (fun a r -> a + r.distance * 1.0<m>) 0.0<m>
      let time = runs |> List.fold (fun a r -> a + r.time * 1.0<s>) 0.0<s>
      let avg = dist / time
      f"{names}: average speed {avg} m/s over {List.len runs} runs"
    case Error e: f"could not read input ({e.error_kind})"

print report
```

`Decode.float` decodes the distance field, `1.0<m>` lifts each decoded distance into metres so the
fold accumulates a `float<m>`, and dividing that by the folded `float<s>` gives `float<m/s>`. A
malformed input would take the `Error` branch instead of crashing, which is the whole point of
decoding at the edge. From here you have every piece the language showcases: types, matching,
records, collections, effects, decoding, computation expressions, units, and modules.
</details>
