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

## When every step is an `Option`

The example above bridges each `Option` to a `Result` so that `result { }` can bind it. When you
have no error message to add, and the steps are already `Option`, `option { }` binds them directly
and stops at the first `None`:

```pyfun
let addStrings a b =
  option {
    let! x = String.toInt a
    let! y = String.toInt b
    return x + y
  }

print (addStrings "3" "4")
print (addStrings "3" "oops")
```

```console
Some(7)
None_
```

`None` prints as `None_` because that is the name the emitted Python class carries, `None` being a
Python keyword. You see it only when you print a bare `Option`; matching on it uses `None` as
written.

It reads exactly like `result { }` because it is the same shape: `let!` unwraps a `Some`, `return`
wraps the answer back up, and the first `None` ends the block. The difference is what a failure
carries. A `Result` carries a value explaining what went wrong, so `result { }` passes that value
along, and `option { }` has nothing to pass along because `None` holds nothing.

That is also how you choose between them. Reach for `Result` when the caller needs to know *why* a
step failed, and `Option` when the absence is the whole answer, as it is for a lookup that finds
nothing.

A block earns its keep at two or more `let!` steps. For a single one, `Option.map` and
`Option.bind` say the same thing on one line:

```pyfun
let double s = String.toInt s |> Option.map (fun n -> n * 2)

print (double "21")
```

```console
Some(42)
```

Another built-in builder is `seq { }`, which describes a sequence one `yield` at a time. It stays
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

A block can also loop. `for target in source:` runs its body once per element of a list or a
sequence, with the target bound, exactly the way Python's own `for … in …: yield` reads, and it
lowers to that statement. A body on the same line is one item; a longer body goes on indented lines:

```pyfun
let squares =
  seq {
    for x in [1, 2, 3]:
      let y = x * x
      yield y
    yield 100
  }

squares |> Seq.toList |> print
```

```console
[1, 4, 9, 100]
```

The same `for` works inside `async { }` (awaiting once per element), `result { }` and `option { }`
(a failed step inside the loop short-circuits the whole block), and in a builder of your own
(lesson 20). The one thing a loop body cannot do is `return`, because the block's value comes after
the loop.

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
