# 15. Modules and projects

As a program grows you want to group related definitions and give them a namespace, the way Python
uses modules and packages. Pyfun offers this at two scales. The small one is an in-file `module`, a
named block of `let` bindings. Inside the block members call each other by their bare names, and from
outside you reach them qualified as `Module.member`.

```pyfun
module Geom =
  let square x = x * x
  let area w h = w * h
  let diag_sq w h = square w + square h

print (Geom.area 4 5)
print (Geom.diag_sq 3 4)
```

```console
20
25
```

`diag_sq` calls `square` directly because they share the module. From the top level you write
`Geom.area`, which reads like `math.sqrt` does in Python. This grouping is purely organizational, so
an in-file module runs anywhere Pyfun runs, including the playground.

The larger scale is one module per file. A sibling source file becomes a module under its capitalized
name, and you bring it into scope with `import`. The project in `examples/modules` splits work across
`geometry.pyfun`, `store.pyfun`, and a `main.pyfun` that imports both:

```pyfun
import Geometry
import Store

let floor = Geometry.area 4 5
let nine = Geometry.square 3
let widen = Geometry.area 10
let strip = widen 2

let hit = Store.lookup 1 |> Option.with_default 0
let miss = Store.lookup 9 |> Option.with_default 0

print floor
print nine
print strip
print hit
print miss
```

Running the whole project with `pyfun run examples/modules/main.pyfun` prints:

```console
20
9
20
100
0
```

`import Geometry` refers to `geometry.pyfun` by its capitalized name, and the import graph must be
acyclic. A file with two words in its name is spelled the way Python spells a module, so
`import TrainTimes` reads `train_times.pyfun`. A `Some` built in `Store` is the same `Option` type `main` inspects, because the runtime
classes are shared. Compiling with `pyfun compile examples/modules/main.pyfun -o out` writes a
readable Python file tree, one `.py` per module plus a shared `_pyfun_rt.py`:

```console
_pyfun_rt.py
geometry.py
main.py
store.py
```

A cross-module call lowers to plain Python attribute access like `geometry.area(4, 5)`. Multi-file
projects need the installed compiler on disk, since the playground runs a single source at a time.
For practice there, an in-file `module` gives you the same qualified-use experience.

## Asking for names

Writing `Geometry.` in front of every use is clear, and it gets long when one file leans on a few
functions all the time. You can ask the module for those names and then write them bare:

```pyfun
from Geometry import area, square

let floor = area 4 5
let nine = square 3

print floor
print nine
```

This is Python's statement and it does what Python's does. It brings in the names you list and
nothing more, so `Geometry.pi` would still need its own `import Geometry` line. You can have both
lines in one file.

A name of your own wins inside the place where you bind it. In `let double area = area * 2` the
`area` is the parameter. At the top of the file a name means one thing, so defining your own `area`
beside the import is an error, and the message tells you which two met.

The Python that comes out is the same either way: `import geometry`, then `geometry.area(4, 5)`.

## Types cross too

Functions are the obvious thing to share, but the types themselves cross a module boundary as well,
which is what lets you split a program along its data rather than around it. Given a `shapes.pyfun`
that declares an ADT and a record:

```pyfun
type Shape =
  | Circle float
  | Rect float float

type Placed = { shape: Shape, x: float, y: float }
```

another module can build those values, match them, and name the types in its own declarations:

```pyfun
import Shapes

type Scene = { name: str, item: Shapes.Placed }

let scene =
  Scene { name = "demo", item = Shapes.Placed { shape = Shapes.Circle 2.0, x = 0.0, y = 0.0 } }

let describe p =
  match p.shape:
    case Shapes.Circle r: f"circle of radius {r}"
    case Shapes.Rect w h: f"{w} by {h} rectangle"

print scene.name
print (describe scene.item)
```

```console
demo
circle of radius 2.0
```

Constructors qualify like values do, in expressions and in `case` patterns alike, so
`Shapes.Circle 2.0` builds one and `case Shapes.Circle r:` takes it apart. In *type* position the
bare name works as well, so the record above could equally say `item: Placed`. Because a type can be
named from a file other than the one declaring it, a record that holds another module's data no
longer forces the two into a single file.

## Exercise

Here is a flat program with two temperature conversions. Group both functions into an in-file module
called `Temp`, then call them qualified as `Temp.c_to_f` and `Temp.f_to_c`. The output stays the same.

```pyfun
let c_to_f c = c * 9 // 5 + 32
let f_to_c f = (f - 32) * 5 // 9

print (c_to_f 100)
print (f_to_c 212)
```

Expected output:

```console
212
100
```

[Open in the playground](https://simontreanor.github.io/Pyfun/playground/#code=bGV0IGNfdG9fZiBjID0gYyAqIDkgLy8gNSArIDMyCmxldCBmX3RvX2MgZiA9IChmIC0gMzIpICogNSAvLyA5CgpwcmludCAoY190b19mIDEwMCkKcHJpbnQgKGZfdG9fYyAyMTIpCg)

<details>
<summary>Show solution</summary>

```pyfun
module Temp =
  let c_to_f c = c * 9 // 5 + 32
  let f_to_c f = (f - 32) * 5 // 9

print (Temp.c_to_f 100)
print (Temp.f_to_c 212)
```

The two functions now live under `Temp`, and the call sites qualify them. As a take-home step, move
the module body into its own `temp.pyfun`, `import Temp` from a `main.pyfun`, and run the project with
`pyfun run main.pyfun`.
</details>
