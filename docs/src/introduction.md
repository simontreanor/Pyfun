# Pyfun

Pyfun is an F#-inspired, functional-first language that compiles to readable Python. It brings
algebraic data types, exhaustive matching, currying, inferred effects, and units of measure to the
Python ecosystem. Its Rust compiler checks all of them before any Python is emitted, then hands you
plain Python with no runtime library attached.

```pyfun
type Shape = Circle float | Rect float float

let area s =
  match s:
    case Circle r: 3.14159 * r * r
    case Rect w h: w * h

let shapes = [Circle 2.0, Rect 3.0 4.0]
print (List.fold (fun acc s -> acc + area s) 0.0 shapes)
```

Delete the `Rect` case and the compiler refuses to build the program, naming the case you forgot.
That check, and everything else Pyfun promises, happens before the Python exists.

This site is the learning home for the language. Pick the track that fits you:

- **[Learn Pyfun](learn/index.html)**: a short, graded course for people who already know some
  Python. Each lesson introduces one idea and ends with an exercise you can solve in the browser.
  The compiler checks your work as you type.
- **For educators** *(coming soon)*: a ready-made functional programming module for an existing
  intro-Python course, with session plans, exercises, and answer keys. Free to adapt under
  CC BY 4.0.
- **Inside the compiler** *(coming soon)*: a guided tour of the Rust compiler, one pipeline stage
  per chapter. Useful if you want to learn how a language is built, read a real dependency-free
  Rust codebase, or contribute.

## Try it right now

The **[playground](playground/index.html)** runs the real compiler in your browser as WebAssembly.
Write Pyfun on the left, watch the Python it compiles to appear on the right, and press Run to
execute it. Nothing to install.

When you want it on your own machine (Python 3.12 or newer):

```console
pip install pyfun-lang
pyfun repl
```

Jupyter users can get Pyfun as a notebook kernel:

```console
pip install "pyfun-lang[jupyter]"
python -m pyfun_kernel.install
```

## More

- The [README](https://github.com/simontreanor/Pyfun#readme) has the full pitch, a feature tour,
  and comparisons with related projects.
- [DESIGN.md](https://github.com/simontreanor/Pyfun/blob/main/DESIGN.md) is the language
  specification and the source of truth for its semantics.
- The [interop cookbook](https://github.com/simontreanor/Pyfun/tree/main/examples/interop) shows
  Pyfun calling real Python libraries with types at the boundary.

The teaching material on this site is licensed under
[CC BY 4.0](https://creativecommons.org/licenses/by/4.0/); the compiler and all code samples are
[Apache 2.0](https://github.com/simontreanor/Pyfun/blob/main/LICENSE).
