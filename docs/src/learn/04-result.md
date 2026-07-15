# 4. Errors as values: Result

Lesson 3 handled the absence of a value. This lesson handles an operation that can fail with
a reason. In Python a failing operation raises an exception, and control jumps to whatever
`try`/`except` happens to be in scope. Pyfun offers a type that carries the outcome as a
value instead. `Result` has two shapes: `Ok v` when the operation succeeded and produced
`v`, and `Error e` when it failed with an error `e`. Like `Option`, you take it apart with
`match`, and the compiler makes you handle both shapes.

To turn a Python exception into a `Result`, you first give the Python function a Pyfun type
with `extern`, then wrap the call in `try`. An `extern` declares that a Python callable
exists and states its type; lesson 12 covers it in full. The `try` expression runs the call
and catches any exception into an `Error`.

```pyfun
extern parseInt: string -> int = int

let describe s =
  match try (parseInt s):
    case Ok n: f"parsed {n}"
    case Error e: f"failed with {e.errorKind}: {e.errorMessage}"

print (describe "42")
print (describe "oops")

let safe = Result.withDefault 0 (try (parseInt "100"))
let fallback = Result.withDefault 0 (try (parseInt "bad"))
print safe
print fallback
```

Running this prints:

```console
parsed 42
failed with ValueError: invalid literal for int() with base 10: 'oops'
100
0
```

`try (parseInt s)` has type `Result int Exception`. When `int("42")` returns cleanly you get
`Ok 42`; when `int("oops")` raises, the exception is caught and delivered as `Error e`. The
caught value is a record with two fields you can read: `e.errorKind` is the Python exception
class name, here `ValueError`, and `e.errorMessage` is its text. Nothing escapes as a raised
exception, so the failure is data you handle in the same expression. When you only want a
default on failure, `Result.withDefault 0` gives back the `Ok` value or the fallback,
mirroring `Option.withDefault` from lesson 3.

## Exercise

This function tries to read a number and report the result, but it matches on `parseInt s`
directly. That call has type `int`, not `Result`, so the match does not fit. Run
`pyfun check`:

```console
error: type mismatch: expected Result 'a 'b, found int
 --> 4:9
  |
4 |   match parseInt s:
  |         ^^^^^^^^^^
```

Wrap the parse in `try` so it becomes a `Result` the match can take apart.

```pyfun
extern parseInt: string -> int = int

let describe s =
  match parseInt s:
    case Ok n: f"ok: {n}"
    case Error e: f"bad: {e.errorKind}"

print (describe "88")
print (describe "bad")
```

Expected output:

```console
ok: 88
bad: ValueError
```

[Open in the playground](https://simontreanor.github.io/Pyfun/playground/#code=ZXh0ZXJuIHBhcnNlSW50OiBzdHJpbmcgLT4gaW50ID0gaW50CgpsZXQgZGVzY3JpYmUgcyA9CiAgbWF0Y2ggcGFyc2VJbnQgczoKICAgIGNhc2UgT2sgbjogZiJvazoge259IgogICAgY2FzZSBFcnJvciBlOiBmImJhZDoge2UuZXJyb3JLaW5kfSIKCnByaW50IChkZXNjcmliZSAiODgiKQpwcmludCAoZGVzY3JpYmUgImJhZCIpCg)

<details>
<summary>Show solution</summary>

```pyfun
extern parseInt: string -> int = int

let describe s =
  match try (parseInt s):
    case Ok n: f"ok: {n}"
    case Error e: f"bad: {e.errorKind}"

print (describe "88")
print (describe "bad")
```

`try (parseInt s)` produces a `Result int Exception`, so the `Ok`/`Error` arms line up and
the caught `ValueError` reaches the `Error` case.
</details>
