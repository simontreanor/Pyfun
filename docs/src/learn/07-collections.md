# 7. Lists, sets, and maps

A `List a` is a Python list, written with the same square brackets: `[1, 2, 3]`. The functions that
work over lists live in a `List` module, so you call them as `List.map`, `List.filter`,
`List.fold`. Keeping them behind a module name means a shared word like `len` can belong to every
collection without clashing. `Set` and `Map` follow the same pattern and lower to Python's `set` and
`dict`. You write them the way Python does too: `{1, 2, 3}` is a set, and `{"ada": 10}` is a map.

```pyfun
let ns = [1, 2, 3, 4, 5]

let squares = List.map (fun x -> x * x) ns
let bigs = List.filter (fun x -> x > 2) ns
let total = List.fold (+) 0 ns
let doubled = List.map ((*) 2) ns

let third = Option.with_default 0 (List.get 2 ns)
let missing = Option.with_default 0 (List.get 9 ns)

let uniq = {1, 1, 2, 3}
let scores = {"ada": 10, "alan": 9}

print squares
print bigs
print total
print doubled
print third
print missing
uniq |> Set.len |> print
scores |> Map.find "ada" |> print
```

```console
[1, 4, 9, 16, 25]
[3, 4, 5]
15
[2, 4, 6, 8, 10]
3
0
3
Some(10)
```

`fun x -> x * x` is a lambda, the same anonymous function you met in lesson 2 written inline. Where
the function is just an operator, an operator section is shorter: `(+)` is addition as a two
argument function, and `(*) 2` is multiplication with one argument already supplied, so
`List.map ((*) 2) ns` doubles every element. `List.fold` walks the list carrying an accumulator,
starting from `0` here and adding each element, which is how you reduce a list to a single value.

There is a fourth collection, and it is the lazy one. A `Seq` describes elements that are produced
as they are asked for rather than stored, which is Python's own iterator idea with a type on it.
`Seq.of_list` starts one from a list, the familiar combinators work over it, and `Seq.to_list` runs
it out into a `List` at the end. Nothing between those two lines does any work until the last line
asks:

```pyfun
let ns = [4, 8, 15, 16, 23]

ns
|> Seq.of_list
|> Seq.map (fun n -> n * n)
|> Seq.filter (fun n -> n > 100)
|> Seq.take 2
|> Seq.to_list
|> print

Seq.init_infinite (fun i -> i * 3) |> Seq.take 4 |> Seq.to_list |> print
```

```console
[225, 256]
[0, 3, 6, 9]
```

The second line is the point of laziness: `Seq.init_infinite` describes an endless sequence, and
`Seq.take 4` is what makes it finite. It lowers to a Python generator, so a `Seq` over a large file
holds one element at a time however long the file is. Reach for `List` when you want the elements,
and `Seq` when you want the pipeline.

The four modules go well beyond what these examples use. `List` alone carries sorting (`sort_by`),
splitting (`partition`, `chunk`), positional edits (`update_at`, `insert_at`, `remove_at`) and much
else, `Map` has `keys`/`values`/`find_or`, and `Set` has the set algebra you would expect. Two
conventions hold across all of them: functions are total, so `List.take 99` on a five element list
clamps rather than raising, and any accessor that can come up empty returns an `Option`, which is
why `List.head` hands back `Option a` instead of raising on an empty list. Hover a module name in
your editor, or type `:type` in the REPL, to see what a module offers.

Totality carries over from lesson 3. In Python, `ns[9]` on a five element list raises `IndexError`.
Pyfun has no bracket indexing at all. Instead `List.get 9 ns` returns an `Option`, `None` when the
index is out of range, and you supply a fallback with `Option.with_default`. The lookup cannot crash,
because the empty case is a value you have to handle. `Map.find` works the same way, returning an
`Option` so a missing key is data rather than an exception.

## Exercise

This program has two holes. `List.fold` needs a function to combine the running total with each
element, and `Option.with_default` needs a fallback for when index 10 is out of range. Run
`pyfun check` to see the type each hole expects, then fill them so the program prints the sum and a
safe default.

```pyfun
let ns = [4, 8, 15, 16, 23]

let total = List.fold ? 0 ns
let at_ten = Option.with_default ? (List.get 10 ns)

print total
print at_ten
```

The checker reports:

```console
note: hole `?` has type `int -> int -> int` — try: min, max, const — or: flip ?
 --> 3:23
  |
3 | let total = List.fold ? 0 ns
  |                       ^

note: hole `?` has type `int` — try: total — or: List.sum ?, Seq.sum ?, String.len ?, round ?, and 13 more
 --> 4:32
  |
4 | let at_ten = Option.with_default ? (List.get 10 ns)
  |                                ^
2 unfilled holes
```

Expected output:

```console
66
0
```

[Open in the playground](https://simontreanor.github.io/Pyfun/playground/#code=bGV0IG5zID0gWzQsIDgsIDE1LCAxNiwgMjNdCgpsZXQgdG90YWwgPSBMaXN0LmZvbGQgPyAwIG5zCmxldCBhdF90ZW4gPSBPcHRpb24ud2l0aF9kZWZhdWx0ID8gKExpc3QuZ2V0IDEwIG5zKQoKcHJpbnQgdG90YWwKcHJpbnQgYXRfdGVuCg)

<details>
<summary>Show solution</summary>

```pyfun
let ns = [4, 8, 15, 16, 23]

let total = List.fold (+) 0 ns
let at_ten = Option.with_default 0 (List.get 10 ns)

print total
print at_ten
```

The `(+)` section adds each element into the accumulator, and `0` is the default returned because
index 10 is past the end of the list.
</details>
