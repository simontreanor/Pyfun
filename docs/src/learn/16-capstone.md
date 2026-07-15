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

type Run = { name: string, distance: float, time: float }

let runDecoder =
  Decode.map3 (fun n d t -> Run { name = n, distance = d, time = t })
    (Decode.field "name" Decode.string)
    (Decode.field "distance" ?)
    (Decode.field "time" Decode.float)

let input = """[
  {"name": "ada", "distance": 100.0, "time": 20.0},
  {"name": "bo", "distance": 140.0, "time": 30.0}
]"""

let report =
  match Decode.decodeString (Decode.list runDecoder) input:
    case Ok runs:
      let names = String.join ", " (List.map (fun r -> r.name) runs)
      let dist = List.fold (fun a r -> a + r.distance * ?) 0.0<m> runs
      let time = List.fold (fun a r -> a + r.time * 1.0<s>) 0.0<s> runs
      let avg = dist / ?
      f"{names}: average speed {avg} m/s over {List.len runs} runs"
    case Error e: f"could not read input ({e.errorKind})"

print report
```

The checker names each expected type:

```console
note: hole `?` has type `Decoder float` — try: Decode.float
note: hole `?` has type `float<m>` — try: a
note: hole `?` has type `float<'a>` — try: dist, time
```

Expected output:

```console
ada, bo: average speed 4.8 m/s over 2 runs
```

[Open in the playground](https://simontreanor.github.io/Pyfun/playground/#code=bWVhc3VyZSBtCm1lYXN1cmUgcwoKdHlwZSBSdW4gPSB7IG5hbWU6IHN0cmluZywgZGlzdGFuY2U6IGZsb2F0LCB0aW1lOiBmbG9hdCB9CgpsZXQgcnVuRGVjb2RlciA9CiAgRGVjb2RlLm1hcDMgKGZ1biBuIGQgdCAtPiBSdW4geyBuYW1lID0gbiwgZGlzdGFuY2UgPSBkLCB0aW1lID0gdCB9KQogICAgKERlY29kZS5maWVsZCAibmFtZSIgRGVjb2RlLnN0cmluZykKICAgIChEZWNvZGUuZmllbGQgImRpc3RhbmNlIiA_KQogICAgKERlY29kZS5maWVsZCAidGltZSIgRGVjb2RlLmZsb2F0KQoKbGV0IGlucHV0ID0gIiIiWwogIHsibmFtZSI6ICJhZGEiLCAiZGlzdGFuY2UiOiAxMDAuMCwgInRpbWUiOiAyMC4wfSwKICB7Im5hbWUiOiAiYm8iLCAiZGlzdGFuY2UiOiAxNDAuMCwgInRpbWUiOiAzMC4wfQpdIiIiCgpsZXQgcmVwb3J0ID0KICBtYXRjaCBEZWNvZGUuZGVjb2RlU3RyaW5nIChEZWNvZGUubGlzdCBydW5EZWNvZGVyKSBpbnB1dDoKICAgIGNhc2UgT2sgcnVuczoKICAgICAgbGV0IG5hbWVzID0gU3RyaW5nLmpvaW4gIiwgIiAoTGlzdC5tYXAgKGZ1biByIC0-IHIubmFtZSkgcnVucykKICAgICAgbGV0IGRpc3QgPSBMaXN0LmZvbGQgKGZ1biBhIHIgLT4gYSArIHIuZGlzdGFuY2UgKiA_KSAwLjA8bT4gcnVucwogICAgICBsZXQgdGltZSA9IExpc3QuZm9sZCAoZnVuIGEgciAtPiBhICsgci50aW1lICogMS4wPHM-KSAwLjA8cz4gcnVucwogICAgICBsZXQgYXZnID0gZGlzdCAvID8KICAgICAgZiJ7bmFtZXN9OiBhdmVyYWdlIHNwZWVkIHthdmd9IG0vcyBvdmVyIHtMaXN0LmxlbiBydW5zfSBydW5zIgogICAgY2FzZSBFcnJvciBlOiBmImNvdWxkIG5vdCByZWFkIGlucHV0ICh7ZS5lcnJvcktpbmR9KSIKCnByaW50IHJlcG9ydAo)

<details>
<summary>Show solution</summary>

```pyfun
measure m
measure s

type Run = { name: string, distance: float, time: float }

let runDecoder =
  Decode.map3 (fun n d t -> Run { name = n, distance = d, time = t })
    (Decode.field "name" Decode.string)
    (Decode.field "distance" Decode.float)
    (Decode.field "time" Decode.float)

let input = """[
  {"name": "ada", "distance": 100.0, "time": 20.0},
  {"name": "bo", "distance": 140.0, "time": 30.0}
]"""

let report =
  match Decode.decodeString (Decode.list runDecoder) input:
    case Ok runs:
      let names = String.join ", " (List.map (fun r -> r.name) runs)
      let dist = List.fold (fun a r -> a + r.distance * 1.0<m>) 0.0<m> runs
      let time = List.fold (fun a r -> a + r.time * 1.0<s>) 0.0<s> runs
      let avg = dist / time
      f"{names}: average speed {avg} m/s over {List.len runs} runs"
    case Error e: f"could not read input ({e.errorKind})"

print report
```

`Decode.float` decodes the distance field, `1.0<m>` lifts each decoded distance into metres so the
fold accumulates a `float<m>`, and dividing that by the folded `float<s>` gives `float<m/s>`. A
malformed input would take the `Error` branch instead of crashing, which is the whole point of
decoding at the edge. From here you have every piece the language showcases: types, matching,
records, collections, effects, decoding, computation expressions, units, and modules.
</details>
