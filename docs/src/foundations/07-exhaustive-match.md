# 7. The compiler checks your work

Add a third shape to the type from the last lesson.

```pyfun
type Shape =
  | Circle float
  | Rectangle float float
  | Triangle float float

let area s =
  match s:
    case Circle r: 3.14159 * r * r
    case Rectangle w h: w * h
    case Triangle b h: 0.5 * b * h

print (area (Circle 2.0))
print (area (Rectangle 3.0 4.0))
print (area (Triangle 6.0 4.0))
```

```console
12.56636
12.0
12.0
```

Nothing new in the syntax there, just a third constructor and a third `case`. Now watch what
happens if you forget the third `case`.

```pyfun
type Shape =
  | Circle float
  | Rectangle float float
  | Triangle float float

let area s =
  match s:
    case Circle r: 3.14159 * r * r
    case Rectangle w h: w * h

print (area (Circle 2.0))
print (area (Rectangle 3.0 4.0))
print (area (Triangle 6.0 4.0))
```

```console
error: non-exhaustive match: `Triangle _ _` is not matched
 --> 7:3
  |
7 |   match s:
  |   ^^^^^^^^
```

The program doesn't run with the wrong area for a triangle, and it doesn't crash three lines down
when a `Triangle` finally reaches `area`. It doesn't run at all. The compiler read `Shape`, saw it
has three constructors, checked the `match` against all three, and refused to build the program
until you'd written something for the one you missed. It even names the exact value you forgot,
`Triangle _ _`.

This is what people mean when they say a language like this one catches bugs before they happen.
It isn't a test you had to remember to write, and it isn't a warning you can shrug off. Forgetting
a case is a common, easy mistake. Here, it simply cannot make it into a running program.

## Exercise

Add a `Square` to `Shape`, but don't touch `area` yet. Run `pyfun check` and read what it tells
you is missing, then add the case yourself.

```pyfun
type Shape =
  | Circle float
  | Rectangle float float
  | Triangle float float

let area s =
  match s:
    case Circle r: 3.14159 * r * r
    case Rectangle w h: w * h

print (area (Circle 2.0))
print (area (Rectangle 3.0 4.0))
print (area (Triangle 6.0 4.0))
```

The checker reports:

```console
error: non-exhaustive match: `Triangle _ _` is not matched
 --> 7:3
  |
7 |   match s:
  |   ^^^^^^^^
```

[Open in the playground](https://simontreanor.github.io/Pyfun/playground/#code=dHlwZSBTaGFwZSA9CiAgfCBDaXJjbGUgZmxvYXQKICB8IFJlY3RhbmdsZSBmbG9hdCBmbG9hdAogIHwgVHJpYW5nbGUgZmxvYXQgZmxvYXQKCmxldCBhcmVhIHMgPQogIG1hdGNoIHM6CiAgICBjYXNlIENpcmNsZSByOiAzLjE0MTU5ICogciAqIHIKICAgIGNhc2UgUmVjdGFuZ2xlIHcgaDogdyAqIGgKCnByaW50IChhcmVhIChDaXJjbGUgMi4wKSkKcHJpbnQgKGFyZWEgKFJlY3RhbmdsZSAzLjAgNC4wKSkKcHJpbnQgKGFyZWEgKFRyaWFuZ2xlIDYuMCA0LjApKQo)

Expected output:

```console
12.56636
12.0
12.0
```

<details>
<summary>Show solution</summary>

```pyfun
type Shape =
  | Circle float
  | Rectangle float float
  | Triangle float float

let area s =
  match s:
    case Circle r: 3.14159 * r * r
    case Rectangle w h: w * h
    case Triangle b h: 0.5 * b * h

print (area (Circle 2.0))
print (area (Rectangle 3.0 4.0))
print (area (Triangle 6.0 4.0))
```

Adding `case Triangle b h: 0.5 * b * h` covers the constructor the checker named, and the match is
whole again.
</details>
