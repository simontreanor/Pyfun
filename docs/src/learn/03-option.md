# 3. The None problem: Option

Python uses `None` to mean "no value here." A function that might not find something
returns `None`, and every caller is trusted to remember to check for it. When someone
forgets, the program runs until an `AttributeError` surfaces far from the cause. Pyfun makes
the absence part of the type. A value that might be missing has type `Option`, and there are
exactly two shapes it can take: `Some x` when a value is present, and `None` when it is not.

Because the possibility of `None` is in the type, the compiler makes you handle both cases.
You take an `Option` apart with `match`, which works like Python's `match` statement: you
list the shapes the value can have and give each one a result.

```pyfun
let describe s =
  match String.toInt s:
    case Some n: f"got the number {n}"
    case None: "not a number"

print (describe "41")
print (describe "hello")

let ns = [10, 20, 30]
let third = Option.withDefault 0 (List.get 2 ns)
let missing = Option.withDefault 0 (List.get 9 ns)
print third
print missing
```

Running this prints:

```console
got the number 41
not a number
30
0
```

`String.toInt` is a total parse. In Python `int("hello")` raises, so calling code wraps it
in `try`/`except`. Pyfun's `String.toInt` returns `Some 41` for `"41"` and `None` for
`"hello"`, and the `match` handles each. `List.get` behaves the same way: instead of an
index that might raise `IndexError`, it returns `Some` when the position exists and `None`
when it does not. When you only want a fallback and not a full `match`,
`Option.withDefault` supplies one: `Option.withDefault 0 (List.get 9 ns)` is `0` because
position 9 is past the end.

## Exercise

The function below labels a parsed number, but it only handles the `Some` case. Run
`pyfun check` and the compiler reports what is missing:

```console
error: non-exhaustive match: `None` is not matched
 --> 2:3
  |
2 |   match String.toInt s:
  |   ^^^^^^^^^^^^^^^^^^^^^
```

Add the `None` case so the match is total and the program runs.

```pyfun
let label s =
  match String.toInt s:
    case Some n: f"number: {n}"

print (label "7")
print (label "nope")
```

Expected output:

```console
number: 7
no number
```

[Open in the playground](https://simontreanor.github.io/Pyfun/playground/#code=bGV0IGxhYmVsIHMgPQogIG1hdGNoIFN0cmluZy50b0ludCBzOgogICAgY2FzZSBTb21lIG46IGYibnVtYmVyOiB7bn0iCgpwcmludCAobGFiZWwgIjciKQpwcmludCAobGFiZWwgIm5vcGUiKQo)

<details>
<summary>Show solution</summary>

```pyfun
let label s =
  match String.toInt s:
    case Some n: f"number: {n}"
    case None: "no number"

print (label "7")
print (label "nope")
```

Adding `case None:` covers the missing shape, so the compiler accepts the match and the
program runs.
</details>
