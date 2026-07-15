# 19. Active patterns

A `match` (lesson 5) matches on the shape of data: constructors, records, tuples. Sometimes the
distinction you care about is not a shape but a test, like whether a number is even or a string is
blank. In Python that becomes an `if`/`elif` chain, and nothing checks that you covered every case.
An active pattern gives those tests names and lets you match on them like constructors, so the
exhaustiveness checking from lesson 5 applies to your own recognizers.

## Total active patterns

A total active pattern lists a closed set of cases between `(| ... |)`. Its body returns one of those
cases for every input, so a `match` over it needs no catch-all: it is exhaustive by construction.

```pyfun
let (|Even|Odd|) n = if n % 2 == 0 then Even else Odd

let parity n =
  match n:
    case Even: "even"
    case Odd: "odd"

print (parity 4)
print (parity 7)
```

```console
even
odd
```

The recognizer `(|Even|Odd|)` is an ordinary function that returns `Even` or `Odd`, and `parity`
matches on the result as if `Even` and `Odd` were constructors. The emitted Python makes the
mechanism plain: the recognizer becomes a function, and each `case` becomes an `isinstance` check
against a small tag class.

```python
def _ap_Even_Odd(n):
    if n % 2 == 0:
        return _Even()
    else:
        return _Odd()
def parity(n):
    _pf_t0 = _ap_Even_Odd(n)
    if isinstance(_pf_t0, _Even):
        return "even"
    elif isinstance(_pf_t0, _Odd):
        return "odd"
    else:
        raise RuntimeError("non-exhaustive match")
```

## Partial active patterns

Not every recognizer covers every input. A partial active pattern ends in `|_|` and either binds a
payload or tests a predicate, so its `match` needs a fallthrough `case _`.

The payload form returns `Some value` when it matches and `None` when it does not. The bound value
flows into the case body:

```pyfun
let (|Positive|_|) n = if n > 0 then Some n else None

let clamped n =
  match n:
    case Positive p: p
    case _: 0

print (clamped 9)
print (clamped (0 - 5))
```

```console
9
0
```

The predicate form returns a plain `bool`, so it binds nothing. `case Blank` matches when the
recognizer is true:

```pyfun
let (|Blank|_|) s = String.strip s == ""

let lineKind s =
  match s:
    case Blank: "empty"
    case _: "text"

print (lineKind "   ")
print (lineKind "hi")
```

```console
empty
text
```

The framing for Python folks is simple. An active pattern is a name for one branch of an `if`/`elif`
chain, with the test tucked inside the recognizer. Call sites stay declarative, reading like a match
over data, and for a total pattern the compiler still checks that you covered every case.

## Exercise

Fill the hole so the partial pattern `(|Hot|_|)` recognizes a temperature of 25 or more. The hole
has type `bool` (the condition that decides whether the pattern matches), so reach for a comparison
on `c`. When it matches, `Hot t` binds the temperature; otherwise the `case _` arm handles it.

```pyfun
let (|Hot|_|) c = if ? then Some c else None

let describe c =
  match c:
    case Hot t: f"hot at {t}"
    case _: "not hot"

print (describe 30)
print (describe 5)
```

Expected output:

```console
hot at 30
not hot
```

[Open in the playground](https://simontreanor.github.io/Pyfun/playground/#code=bGV0ICh8SG90fF98KSBjID0gaWYgPyB0aGVuIFNvbWUgYyBlbHNlIE5vbmUKCmxldCBkZXNjcmliZSBjID0KICBtYXRjaCBjOgogICAgY2FzZSBIb3QgdDogZiJob3QgYXQge3R9IgogICAgY2FzZSBfOiAibm90IGhvdCIKCnByaW50IChkZXNjcmliZSAzMCkKcHJpbnQgKGRlc2NyaWJlIDUpCg)

<details>
<summary>Show solution</summary>

```pyfun
let (|Hot|_|) c = if c >= 25 then Some c else None

let describe c =
  match c:
    case Hot t: f"hot at {t}"
    case _: "not hot"

print (describe 30)
print (describe 5)
```

`c >= 25` is the condition. When it holds, the recognizer returns `Some c`, so `Hot t` binds the
temperature and the first arm runs. Otherwise it returns `None` and the `case _` arm handles it.
</details>
