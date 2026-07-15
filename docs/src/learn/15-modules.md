# 15. Modules and projects

As a program grows you want to group related definitions and give them a namespace, the way Python
uses modules and packages. Pyfun offers this at two scales. The small one is an in-file `module`, a
named block of `let` bindings. Inside the block members call each other by their bare names, and from
outside you reach them qualified as `Module.member`.

```pyfun
module Geom =
  let square x = x * x
  let area w h = w * h
  let diagSq w h = square w + square h

print (Geom.area 4 5)
print (Geom.diagSq 3 4)
```

```console
20
25
```

`diagSq` calls `square` directly because they share the module. From the top level you write
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

let hit = Store.lookup 1 |> Option.withDefault 0
let miss = Store.lookup 9 |> Option.withDefault 0

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
acyclic. A `Some` built in `Store` is the same `Option` type `main` inspects, because the runtime
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

## Exercise

Here is a flat program with two temperature conversions. Group both functions into an in-file module
called `Temp`, then call them qualified as `Temp.cToF` and `Temp.fToC`. The output stays the same.

```pyfun
let cToF c = c * 9 // 5 + 32
let fToC f = (f - 32) * 5 // 9

print (cToF 100)
print (fToC 212)
```

Expected output:

```console
212
100
```

[Open in the playground](https://simontreanor.github.io/Pyfun/playground/#code=bGV0IGNUb0YgYyA9IGMgKiA5IC8vIDUgKyAzMgpsZXQgZlRvQyBmID0gKGYgLSAzMikgKiA1IC8vIDkKCnByaW50IChjVG9GIDEwMCkKcHJpbnQgKGZUb0MgMjEyKQo)

<details>
<summary>Show solution</summary>

```pyfun
module Temp =
  let cToF c = c * 9 // 5 + 32
  let fToC f = (f - 32) * 5 // 9

print (Temp.cToF 100)
print (Temp.fToC 212)
```

The two functions now live under `Temp`, and the call sites qualify them. As a take-home step, move
the module body into its own `temp.pyfun`, `import Temp` from a `main.pyfun`, and run the project with
`pyfun run main.pyfun`.
</details>
