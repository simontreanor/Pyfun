# 6. Describing a choice: your own types

A record bundles values that are always there together. Sometimes a value isn't one fixed shape,
it's one of a few possible shapes, and which one depends on the situation. A shape on a page might
be a circle, or it might be a rectangle. Pyfun lets you write that choice down as a type.

```pyfun
type Shape =
  | Circle float
  | Rectangle float float

let describe s =
  match s:
    case Circle r: f"a circle with radius {r}"
    case Rectangle w h: f"a rectangle {w} by {h}"

print (describe (Circle 2.0))
print (describe (Rectangle 3.0 4.0))
```

```console
a circle with radius 2.0
a rectangle 3.0 by 4.0
```

`type Shape = | Circle float | Rectangle float float` lists every shape a `Shape` can take.
`Circle` carries one number, a radius. `Rectangle` carries two, a width and a height. `Circle` and
`Rectangle` are called constructors, and building a value is just calling one: `Circle 2.0` is a
`Shape`, and so is `Rectangle 3.0 4.0`, even though they carry different numbers of values.

`match` is how you ask which shape you actually have. Each `case` names a constructor and gives
names to whatever it carries, `r` for the circle's radius, `w` and `h` for the rectangle's two
sides. Inside that `case`, `r` (or `w` and `h`) is bound to the real number that was passed in when
the value was built.

## Exercise

`Circle` needs a `float`, but this starter hands it text instead. Run `pyfun check` and read what
it tells you, then fix the value.

```pyfun
type Shape =
  | Circle float
  | Rectangle float float

let describe s =
  match s:
    case Circle r: f"a circle with radius {r}"
    case Rectangle w h: f"a rectangle {w} by {h}"

print (describe (Circle "big"))
```

The checker reports:

```console
error: type mismatch: expected float, found string
  --> 10:18
   |
10 | print (describe (Circle "big"))
   |                  ^^^^^^^^^^^^
```

[Open in the playground](https://simontreanor.github.io/Pyfun/playground/#code=dHlwZSBTaGFwZSA9CiAgfCBDaXJjbGUgZmxvYXQKICB8IFJlY3RhbmdsZSBmbG9hdCBmbG9hdAoKbGV0IGRlc2NyaWJlIHMgPQogIG1hdGNoIHM6CiAgICBjYXNlIENpcmNsZSByOiBmImEgY2lyY2xlIHdpdGggcmFkaXVzIHtyfSIKICAgIGNhc2UgUmVjdGFuZ2xlIHcgaDogZiJhIHJlY3RhbmdsZSB7d30gYnkge2h9IgoKcHJpbnQgKGRlc2NyaWJlIChDaXJjbGUgImJpZyIpKQo)

Expected output:

```console
a circle with radius 5.0
```

<details>
<summary>Show solution</summary>

```pyfun
type Shape =
  | Circle float
  | Rectangle float float

let describe s =
  match s:
    case Circle r: f"a circle with radius {r}"
    case Rectangle w h: f"a rectangle {w} by {h}"

print (describe (Circle 5.0))
```

`Circle` carries a `float`, so `5.0` fits where `"big"` didn't.
</details>
