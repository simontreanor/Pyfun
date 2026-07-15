# 13. Computation expressions

Chaining steps that each return a `Result` gets awkward fast. Every step needs a `match`, and the
success branch of one becomes the place you write the next. In lesson 4 you saw one such check. Two
in a row already leans right, like a staircase:

```pyfun
let addStrings a b =
  match Option.toResult "bad a" (String.toInt a):
    case Error e: Error e
    case Ok x:
      match Option.toResult "bad b" (String.toInt b):
        case Error e2: Error e2
        case Ok y: Ok (x + y)

print (addStrings "3" "4")
print (addStrings "3" "oops")
```

```console
Ok(7)
Error('bad b')
```

Every `Error` branch does the same thing: stop and pass the error along. A computation expression
writes that repetition once. A `result { }` block lets you bind the success value of a step with
`let!`, and if any step is an `Error` the whole block stops there and returns it. The same logic,
read top to bottom:

```pyfun
let addStrings a b =
  result {
    let! x = Option.toResult "bad a" (String.toInt a)
    let! y = Option.toResult "bad b" (String.toInt b)
    return x + y
  }

print (addStrings "3" "4")
print (addStrings "3" "oops")
```

```console
Ok(7)
Error('bad b')
```

`let!` unwraps an `Ok`, `return` wraps the final value back up, and the short-circuit on the first
`Error` is automatic. This is the same idea as F#'s computation expressions. `String.toInt` gives
back an `Option` (lesson 3), so `Option.toResult` bridges it to a `Result` with a message for the
`None` case before `let!` binds it.

A second built-in builder is `seq { }`, which describes a sequence one `yield` at a time. It stays
lazy, and it lowers to a Python generator function, which you may already know from Python's own
`yield`:

```pyfun
let counts =
  seq {
    yield 1
    yield 2
    yield 3
  }

counts |> Seq.toList |> print
```

```python
def _pf_fn0():
    yield 1
    yield 2
    yield 3
counts = _pf_fn0()
print(list(counts))
```

## Exercise

Fill the two holes so `multiply` parses both strings and returns their product as a `Result`. Each
hole has type `Result int 'a`, so reach for the same `Option.toResult ... (String.toInt ...)` bridge
the worked example used. When both parse you get `Ok`, and a bad string short-circuits to `Error`.

```pyfun
let multiply a b =
  result {
    let! x = ?
    let! y = ?
    return x * y
  }

print (multiply "6" "7")
print (multiply "6" "nope")
```

Expected output:

```console
Ok(42)
Error('not a number')
```

[Open in the playground](https://simontreanor.github.io/Pyfun/playground/#code=bGV0IG11bHRpcGx5IGEgYiA9CiAgcmVzdWx0IHsKICAgIGxldCEgeCA9ID8KICAgIGxldCEgeSA9ID8KICAgIHJldHVybiB4ICogeQogIH0KCnByaW50IChtdWx0aXBseSAiNiIgIjciKQpwcmludCAobXVsdGlwbHkgIjYiICJub3BlIikK)

<details>
<summary>Show solution</summary>

```pyfun
let multiply a b =
  result {
    let! x = Option.toResult "not a number" (String.toInt a)
    let! y = Option.toResult "not a number" (String.toInt b)
    return x * y
  }

print (multiply "6" "7")
print (multiply "6" "nope")
```

Each `let!` unwraps a successful parse, and the first `None` turned into an `Error` stops the block
before the `return`.
</details>
