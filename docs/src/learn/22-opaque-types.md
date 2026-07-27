# 22. Opaque types

A user id and an email address can both live in a `string`, and a plain `string` will let you pass
one where the other belongs. An opaque type gives a value of an existing type its own name, and the
checker then keeps the two apart everywhere. You declare one with `opaque type`, wrap with the
constructor of the same name, and unwrap with a single-case `match`.

```pyfun
opaque type UserId = string
opaque type Email = string

let uid = UserId "u-1001"
let contact = Email "ana@example.org"

let describe u =
  match u:
    case UserId s: s

uid |> describe |> print
```

```console
u-1001
```

`UserId` and `Email` are distinct types built on the same underlying `string`. The constructor
`UserId : string -> UserId` wraps a value, and the pattern `case UserId s:` binds the underlying
string back out. That one case makes the match exhaustive, because the type has exactly one shape.

Mixing the two up is where the checker steps in. Passing an `Email` to a function that takes a
`UserId` is rejected before any Python is produced:

```pyfun
opaque type UserId = string
opaque type Email = string

let describe u =
  match u:
    case UserId s: s

let bad = describe (Email "ana@example.org")
```

```console
error: type mismatch: expected UserId, found Email
```

The distinction exists only during type checking. Like units of measure, an opaque type erases at
lowering, so the emitted Python is the plain underlying value with no wrapper class and no
allocation:

```python
uid = "u-1001"
contact = "ana@example.org"
def describe(u):
    match u:
        case s:
            return s
print(describe(uid))
```

The wrap compiled to nothing, and the pattern became a plain capture. This zero-cost story pays off
at the Python boundary: because the running value is the underlying string, an `extern` can carry
the opaque type in its signature, and the Python side receives exactly the `str` it expects.

```pyfun
opaque type UserId = string

extern pure shout: UserId -> string = str.upper

let uid = UserId "u-1001"
let loud = uid |> shout
print loud
```

```console
U-1001
```

The signature enforces the domain distinction on the Pyfun side, and `str.upper` runs on a plain
string at runtime. An opaque type can also take parameters (`opaque type Tag a = List a`) and wrap
any type, including lists and tuples. For numeric quantities with arithmetic, units of measure
(lesson 14) remain the sharper tool, since they combine algebraically; opaque types cover ids,
tokens, sanitized text, and every other value whose meaning outgrows its representation.

## Exercise

The program below reads an order id as a plain string, then hands it straight to a function that
takes an `OrderId`. Run `pyfun check` to see the mismatch, then wrap the string at the call site so
the program type-checks.

```pyfun
opaque type OrderId = string

let orderLabel o =
  match o:
    case OrderId s: String.concat "order " s

# The id arrives as a plain string from the outside world.
let raw = "o-9"

# This line does not type-check yet: orderLabel wants an OrderId.
let label = orderLabel raw

print label
```

The checker reports:

```console
error: type mismatch: expected OrderId, found string
  --> 11:13
   |
11 | let label = orderLabel raw
   |             ^^^^^^^^^^^^^^
```

Expected output:

```console
order o-9
```

[Open in the playground](https://simontreanor.github.io/Pyfun/playground/#code=b3BhcXVlIHR5cGUgT3JkZXJJZCA9IHN0cmluZwoKbGV0IG9yZGVyTGFiZWwgbyA9CiAgbWF0Y2ggbzoKICAgIGNhc2UgT3JkZXJJZCBzOiBTdHJpbmcuY29uY2F0ICJvcmRlciAiIHMKCiMgVGhlIGlkIGFycml2ZXMgYXMgYSBwbGFpbiBzdHJpbmcgZnJvbSB0aGUgb3V0c2lkZSB3b3JsZC4KbGV0IHJhdyA9ICJvLTkiCgojIFRoaXMgbGluZSBkb2VzIG5vdCB0eXBlLWNoZWNrIHlldDogb3JkZXJMYWJlbCB3YW50cyBhbiBPcmRlcklkLgpsZXQgbGFiZWwgPSBvcmRlckxhYmVsIHJhdwoKcHJpbnQgbGFiZWwK)

<details>
<summary>Show solution</summary>

```pyfun
opaque type OrderId = string

let orderLabel o =
  match o:
    case OrderId s: String.concat "order " s

# The id arrives as a plain string from the outside world.
let raw = "o-9"

let label = orderLabel (OrderId raw)

print label
```

Wrapping the string with `OrderId` at the boundary is the whole fix, and the whole idiom: raw data
enters, gets named once, and every function past that point can trust what it holds. The wrap costs
nothing at runtime.
</details>
