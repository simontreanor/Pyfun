# 9. Typed holes and type-driven development

When you write a program, you rarely know every piece at once. You know the shape of what you want, and some parts are still blank. In Python you might leave a `TODO` comment or a `pass` and hope you remember what belonged there. Pyfun gives that blank a name and a type. Write `?` or `?name` anywhere an expression is missing, and the compiler tells you what belongs there: the type it expects, the in-scope values that fit directly, and the functions whose result would fit if you gave them more holes.

Here is a pipeline with one part left blank. It takes a list of names typed in every which way, puts each into a common form, and joins the results.

```pyfun
let names = ["Ada", "GRACE", "alan"]

let headline names =
  names
  |> List.map ?normalize
  |> String.join ", "

names |> headline |> print
```

Running `pyfun check` reports the gap rather than a failure:

```console
note: hole `?normalize` has type `'a -> string` — try: input, String.fromFloat, String.fromInt, String.lower, String.ofList, String.rev — or: String.concat ?, String.join ?, String.repeat ?, Format.fixed ?
 --> 5:15
  |
5 |   |> List.map ?normalize
  |               ^^^^^^^^^^
```

The hole needs a function producing a string, and `String.lower` is right there under `try:`. The `or:` list names functions that would also fit if you filled their own holes. Replace `?normalize` with `String.lower` and the program compiles and prints `ada, grace, alan`.

The `try:` list is capped at six, so it is a shortlist rather than every candidate: when the answer you want is a common shape like `string -> string`, it may sit just off the end. Hovering the module in your editor, or `:type` in the REPL, gives you the full surface.

This is the workflow: sketch the whole pipeline with holes, let the compiler name each gap's type, and fill inward from the types and the suggested fits. You never have to guess a function's signature, because the hole reports it for you, and a hole blocks compilation by design, so nothing runs until every blank is filled.

## Exercise

Fill both holes. `report` looks a score up in a map, supplies a fallback when the name is absent, and turns the number into a string. `pyfun check` reports each hole with a `try:` value that fits. The first report is:

```console
note: hole `?render` has type `int -> 'a` — try: Format.padLeft, Format.padRight, List.range, Seq.range, String.fromFloat, String.fromInt — or: String.slice ?, Format.currency ?, Format.fixed ?, Format.percent ?
```

```pyfun
let scores = Map.add "ada" 10 (Map.add "alan" 9 Map.empty)

let report name =
  Map.tryFind name scores
  |> Option.withDefault ?missing
  |> ?render

print (report "ada")
print (report "cy")
```

Expected output:

```console
10
0
```

[Open in the playground](https://simontreanor.github.io/Pyfun/playground/#code=bGV0IHNjb3JlcyA9IE1hcC5hZGQgImFkYSIgMTAgKE1hcC5hZGQgImFsYW4iIDkgTWFwLmVtcHR5KQoKbGV0IHJlcG9ydCBuYW1lID0KICBNYXAudHJ5RmluZCBuYW1lIHNjb3JlcwogIHw-IE9wdGlvbi53aXRoRGVmYXVsdCA_bWlzc2luZwogIHw-ID9yZW5kZXIKCnByaW50IChyZXBvcnQgImFkYSIpCnByaW50IChyZXBvcnQgImN5IikK)

<details>
<summary>Show solution</summary>

```pyfun
let scores = Map.add "ada" 10 (Map.add "alan" 9 Map.empty)

let report name =
  Map.tryFind name scores
  |> Option.withDefault 0
  |> String.fromInt

print (report "ada")
print (report "cy")
```

`?missing` has type `int`, so `0` fits. `?render` has type `int -> 'a`, and `String.fromInt` is one of its suggested fits. Filling the second hole pins `'a` to `string`.
</details>
