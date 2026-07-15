# 6. Records

An ADT models a choice between shapes. A record models one shape with named fields, the way a
Python `dataclass` or a dictionary with fixed keys does. You declare the field names and their
types once, then build values that carry all of them.

```pyfun
type Point = { x: int, y: int }

let here = Point { x = 3, y = 4 }
let moved = { here with x = 10 }

print here.x
print moved.x
print here.x
print here
```

```console
3
10
3
Point(x=3, y=4)
```

You construct a record by writing the type's constructor name, then the fields in braces:
`Point { x = 3, y = 4 }`. The name in front is what tags the value as a `Point`, which is why the
printout reads `Point(x=3, y=4)`. You read a field with a dot, `here.x`, exactly as in Python.

The line `{ here with x = 10 }` is copy and update. It does not change `here`. It returns a new
record equal to `here` except for `x`, which is why the third `print here.x` still shows `3`. The
immutability from lesson 1 carries straight over: a record is like a frozen dataclass, so every
update hands you a fresh value and the original stays put. The emitted Python makes this literal:

```python
from dataclasses import dataclass
@dataclass(frozen=True)
class Point:
    x: int
    y: int
here = Point(3, 4)
moved = Point(10, here.y)
```

Records also match. A `case` may name a subset of the fields, and `{ x }` shorthand binds the field
to a variable of the same name:

```pyfun
type Point = { x: int, y: int }

let name p =
  match p:
    case Point { x = 0, y = 0 }: "origin"
    case Point { x, y }: "elsewhere"

print (name (Point { x = 0, y = 0 }))
print (name (Point { x = 3, y = 4 }))
```

```console
origin
elsewhere
```

When a field itself holds a record, the update sugar nests: `{ seg with start.x = 99 }` reaches
through `start` and rebuilds the path for you, copying the siblings along the way.

## Exercise

The hole marked `?` is the new balance for a funded account. Run `pyfun check`: it reports the type
the hole expects. Replace `?` with `100` so the program prints the three lines below. Notice that
`opened.balance` still shows `0` afterward, because the update produced a new record.

```pyfun
type Account = { name: string, balance: int }

let opened = Account { name = "Ada", balance = 0 }
let funded = { opened with balance = ? }

print funded.name
print funded.balance
print opened.balance
```

The checker reports:

```console
note: hole `?` has type `int` — or: List.sum ?, String.len ?, ceil ?, floor ?
 --> 4:38
  |
4 | let funded = { opened with balance = ? }
  |                                      ^
1 unfilled hole
```

Expected output:

```console
Ada
100
0
```

[Open in the playground](https://simontreanor.github.io/Pyfun/playground/#code=dHlwZSBBY2NvdW50ID0geyBuYW1lOiBzdHJpbmcsIGJhbGFuY2U6IGludCB9CgpsZXQgb3BlbmVkID0gQWNjb3VudCB7IG5hbWUgPSAiQWRhIiwgYmFsYW5jZSA9IDAgfQpsZXQgZnVuZGVkID0geyBvcGVuZWQgd2l0aCBiYWxhbmNlID0gPyB9CgpwcmludCBmdW5kZWQubmFtZQpwcmludCBmdW5kZWQuYmFsYW5jZQpwcmludCBvcGVuZWQuYmFsYW5jZQo)

<details>
<summary>Show solution</summary>

```pyfun
type Account = { name: string, balance: int }

let opened = Account { name = "Ada", balance = 0 }
let funded = { opened with balance = 100 }

print funded.name
print funded.balance
print opened.balance
```

The update builds a fresh `Account` with the new balance and leaves `opened` untouched.
</details>
