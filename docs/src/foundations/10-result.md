# 10. When something might go wrong: Result

`Option` covers a value that might be missing. Some operations are worse than missing, they can
fail outright, and when they do, you usually want to know why. Splitting a bill among nobody is
exactly that kind of failure.

```pyfun
let splitBill total people =
  match try (total / people):
    case Ok share: f"each person pays {share}"
    case Error e: f"can't split the bill: {e.errorKind}"

print (splitBill 60.0 4.0)
print (splitBill 60.0 0.0)
```

```console
each person pays 15.0
can't split the bill: ZeroDivisionError
```

`try (total / people)` runs the division and catches anything that goes wrong while it runs.
Dividing by zero is exactly that kind of wrong. Instead of crashing the program, `try` turns the
outcome into a `Result`: `Ok share` when the division worked, `Error e` when it didn't, with `e`
carrying a name for what went wrong. `match` takes it apart the same way you've been taking
`Option` apart, one `case` per shape.

`Result` is `Option` with a reason attached. Where `None` just says "nothing," `Error e` says what
happened, here `e.errorKind` gives you `"ZeroDivisionError"`. Like `Option`, there's a shortcut for
when you only want a fallback and not the reason:

```pyfun
let safeShare = Result.withDefault 0.0 (try (60.0 / 0.0))
print safeShare
```

```console
0.0
```

`Result.withDefault 0.0` unwraps an `Ok`, or hands back `0.0` in its place if the division failed.

## Exercise

Extend the program above: write a `shareOrZero` function that returns the share, or `0.0` if the
split fails, using `Result.withDefault` instead of a full `match`.

```pyfun
let splitBill total people =
  match try (total / people):
    case Ok share: f"each person pays {share}"
    case Error e: f"can't split the bill: {e.errorKind}"

print (splitBill 60.0 4.0)
print (splitBill 60.0 0.0)
```

[Open in the playground](https://simontreanor.github.io/Pyfun/playground/#code=bGV0IHNwbGl0QmlsbCB0b3RhbCBwZW9wbGUgPQogIG1hdGNoIHRyeSAodG90YWwgLyBwZW9wbGUpOgogICAgY2FzZSBPayBzaGFyZTogZiJlYWNoIHBlcnNvbiBwYXlzIHtzaGFyZX0iCiAgICBjYXNlIEVycm9yIGU6IGYiY2FuJ3Qgc3BsaXQgdGhlIGJpbGw6IHtlLmVycm9yS2luZH0iCgpwcmludCAoc3BsaXRCaWxsIDYwLjAgNC4wKQpwcmludCAoc3BsaXRCaWxsIDYwLjAgMC4wKQo)

Expected output:

```console
each person pays 15.0
can't split the bill: ZeroDivisionError
15.0
0.0
```

<details>
<summary>Show solution</summary>

```pyfun
let splitBill total people =
  match try (total / people):
    case Ok share: f"each person pays {share}"
    case Error e: f"can't split the bill: {e.errorKind}"

print (splitBill 60.0 4.0)
print (splitBill 60.0 0.0)

let shareOrZero total people = Result.withDefault 0.0 (try (total / people))

print (shareOrZero 60.0 4.0)
print (shareOrZero 60.0 0.0)
```

`shareOrZero` runs the same division, and `Result.withDefault 0.0` supplies the fallback in one
step instead of writing out both `match` arms.
</details>
