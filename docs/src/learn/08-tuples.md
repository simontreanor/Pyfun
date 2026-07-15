# 8. Tuples and destructuring

A tuple groups a fixed number of values that can have different types, written `(a, b)`, exactly the
Python tuple. A record gives its fields names, while a tuple identifies them by position, which
suits a short pairing like a name and a score. Because the parentheses are also grouping, Pyfun
reads them by how many elements are inside: `()` is unit, the empty value from lesson 1, `(x)` is
just `x` in parentheses, and a real tuple has two or more elements. There is no one element tuple.

You take a tuple apart the same way you take an ADT apart, by matching. A single variable pattern
covers a tuple completely, so one `case` is exhaustive:

```pyfun
let swap p =
  match p:
    case (a, b): (b, a)

let names = ["ada", "alan"]
let scores = [10, 9]

let line pair =
  match pair:
    case (name, score): f"{name}: {score}"

let lines = List.zip names scores |> List.map line

let byName = List.zip names scores |> Map.ofList
let adaScore = Option.withDefault 0 (Map.tryFind "ada" byName)

print (swap (1, 2))
print lines
byName |> Map.toList |> print
print adaScore
```

```console
(2, 1)
['ada: 10', 'alan: 9']
[('ada', 10), ('alan', 9)]
10
```

`List.zip` pairs two lists element by element into a list of tuples, so `List.zip names scores` is
`[("ada", 10), ("alan", 9)]`. Mapping `line` over that list destructures each pair in the `case`
and binds `name` and `score`, the way Python's `for name, score in pairs` unpacks as it goes.

Tuples also bridge lists and maps. `Map.ofList` builds a `Map` from a list of pairs, and
`Map.toList` turns a map back into its pairs, so a zip followed by `Map.ofList` is a compact way to
build a lookup table from two parallel lists.

## Exercise

`List.zip` has paired each name with its score, and the `case (name, score)` arm has already
destructured a pair for you. Fill the hole so each pair becomes a line like `ada: 10`, then the
program joins the lines with a comma. Use the bound `name` and `score` in an interpolated string.

```pyfun
let names = ["ada", "alan"]
let scores = [10, 9]

let line pair =
  match pair:
    case (name, score): ?

let lines = List.zip names scores |> List.map line

lines |> String.join ", " |> print
```

The checker reports:

```console
note: hole `?` has type `'a`
 --> 6:25
  |
6 |     case (name, score): ?
  |                         ^
1 unfilled hole
```

Expected output:

```console
ada: 10, alan: 9
```

[Open in the playground](https://simontreanor.github.io/Pyfun/playground/#code=bGV0IG5hbWVzID0gWyJhZGEiLCAiYWxhbiJdCmxldCBzY29yZXMgPSBbMTAsIDldCgpsZXQgbGluZSBwYWlyID0KICBtYXRjaCBwYWlyOgogICAgY2FzZSAobmFtZSwgc2NvcmUpOiA_CgpsZXQgbGluZXMgPSBMaXN0LnppcCBuYW1lcyBzY29yZXMgfD4gTGlzdC5tYXAgbGluZQoKbGluZXMgfD4gU3RyaW5nLmpvaW4gIiwgIiB8PiBwcmludAo)

<details>
<summary>Show solution</summary>

```pyfun
let names = ["ada", "alan"]
let scores = [10, 9]

let line pair =
  match pair:
    case (name, score): f"{name}: {score}"

let lines = List.zip names scores |> List.map line

lines |> String.join ", " |> print
```

The `case (name, score)` arm binds both parts of the pair, and the f-string builds one line per
pair before `String.join` stitches them together.
</details>
