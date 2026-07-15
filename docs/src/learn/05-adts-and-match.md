# 5. Your own types: ADTs and exhaustive match

So far you have used types the language gives you, like `int` and `Option`. Now you define your
own. An algebraic data type lists the shapes a value can take, one per line. In Python you might
reach for an `Enum`, or a set of classes, or just a string like `"circle"` and hope every reader
remembers the spelling. Pyfun writes the choices down as a type, and the compiler holds you to them.

```pyfun
type Shape =
  | Circle float
  | Rect float float

let area s =
  match s:
    case Circle r: 3.14159 * r * r
    case Rect w h: w * h

print (area (Circle 2.0))
print (area (Rect 3.0 4.0))
```

```console
12.56636
12.0
```

`Circle` and `Rect` are the constructors. Each one is a function: `Circle` takes a `float` and
returns a `Shape`, `Rect` takes two. You build a value by applying a constructor, and you take one
apart with `match`, binding the fields with names of your choosing (`r`, then `w` and `h`).

Python 3.10 and later has `match`/`case`, and this lowers to it almost one for one:

```python
def area(s):
    match s:
        case Circle(r):
            return 3.14159 * r * r
        case Rect(w, h):
            return w * h
        case _:
            raise RuntimeError("non-exhaustive match")
```

What Pyfun adds is the proof, before any Python is written, that the `match` covers every
constructor. Delete the `Rect` case and the compiler stops you with the exact value you left out,
so the trailing `case _` guard never fires at runtime. This is what "making illegal states
unrepresentable" means in practice: you model the domain so that a wrong value cannot be built, and
then the compiler checks that your code answers for each shape that can.

## Guards and or-patterns

A `case` arm can carry a condition. Write `if` after the pattern and the arm only fires when the
guard is true:

```pyfun
let sign n =
  match n:
    case 0: "zero"
    case m if m > 0: "positive"
    case _: "negative"

print (sign 5)
print (sign 0)
print (sign (0 - 4))
```

```console
positive
zero
negative
```

This is where exhaustiveness gets careful. A guarded arm might not fire, because the guard could be
false, so the compiler does not count it toward covering the type. Drop the `case _` and keep only
the guarded arm, and the check fails:

```pyfun
let sign n =
  match n:
    case 0: "zero"
    case m if m > 0: "positive"
```

```console
error: non-exhaustive match: add a wildcard `_` arm
 --> 2:3
  |
2 |   match n:
  |   ^^^^^^^^
```

The `case m if m > 0` covers positive numbers at runtime, but the compiler cannot see that, so it
asks for a catch-all to answer for the values a guard might let through.

An or-pattern matches any of several alternatives joined with `|`, exactly like Python's
`case 1 | 2 | 3`. Every alternative has to bind the same variables, so here they bind none:

```pyfun
let size n =
  match n:
    case 1 | 2 | 3: "small"
    case _: "big"

print (size 2)
print (size 9)
```

```console
small
big
```

## Exercise

A traffic light has three states, but `action` only answers for two. Run `pyfun check` and read the
diagnostic: it names the state you forgot. Add the missing `case` so the program is total and prints
the three lines below.

```pyfun
type Light =
  | Red
  | Amber
  | Green

let action l =
  match l:
    case Red: "stop"
    case Green: "go"

print (action Red)
print (action Amber)
print (action Green)
```

The checker reports:

```console
error: non-exhaustive match: `Amber` is not matched
 --> 7:3
  |
7 |   match l:
  |   ^^^^^^^^
```

Expected output:

```console
stop
wait
go
```

[Open in the playground](https://simontreanor.github.io/Pyfun/playground/#code=dHlwZSBMaWdodCA9CiAgfCBSZWQKICB8IEFtYmVyCiAgfCBHcmVlbgoKbGV0IGFjdGlvbiBsID0KICBtYXRjaCBsOgogICAgY2FzZSBSZWQ6ICJzdG9wIgogICAgY2FzZSBHcmVlbjogImdvIgoKcHJpbnQgKGFjdGlvbiBSZWQpCnByaW50IChhY3Rpb24gQW1iZXIpCnByaW50IChhY3Rpb24gR3JlZW4pCg)

<details>
<summary>Show solution</summary>

```pyfun
type Light =
  | Red
  | Amber
  | Green

let action l =
  match l:
    case Red: "stop"
    case Amber: "wait"
    case Green: "go"

print (action Red)
print (action Amber)
print (action Green)
```

The added `case Amber` makes the match cover all three constructors, so the checker is satisfied and
the program runs.
</details>
