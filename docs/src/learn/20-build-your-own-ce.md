# 20. Build your own computation expression

Lesson 13 used `result { }` to write a chain of `Result` steps top to bottom, with the
short-circuit on the first `Error` handled for you. The braces notation is not magic. Underneath, a
computation expression is just method calls, and any in-file `module` (lesson 15) that provides
`bind` and `return_` becomes a builder you can use with the same `{ let! ... return ... }` syntax.

Here is a builder for the `Option` type, so `Maybe { }` chains steps that might be `None` and stops
at the first one:

```pyfun
module Maybe =
  let bind m f =
    match m:
      case Some x: f x
      case None: None
  let return_ x = Some x

let addOpt a b =
  Maybe {
    let! x = a
    let! y = b
    return x + y
  }

print (addOpt (Some 3) (Some 4))
print (addOpt (Some 3) None)
```

```console
Some(7)
None_
```

`bind` says how to run one step and feed its unwrapped value into the next, and `return_` wraps a
final value back up. That is all a builder needs. The `{ let! ... return ... }` block desugars to
plain calls on the module, and ordinary type inference handles the rest, with no builder-specific
rule in the compiler. The emitted Python shows the desugaring exactly:

```python
def Maybe_bind(m, f):
    match m:
        case Some(x):
            return f(x)
        case None_():
            return None_()
        case _:
            raise RuntimeError("non-exhaustive match")
def Maybe_return_(x):
    return Some(x)
def addOpt(a, b):
    return Maybe_bind(a, lambda x: Maybe_bind(b, lambda y: Maybe_return_(x + y)))
```

Each `let!` becomes a `Maybe_bind` call whose continuation is a lambda binding the next name, and the
`return` becomes `Maybe_return_`. The short-circuit falls out of `bind`: when a step is `None`, that
branch never calls `f`, so `addOpt (Some 3) None` is `None` without running the `+`.

Builders can do more than `bind` and `return_`. A sequence-style builder adds `yield_` for a
`yield`, and `combine`/`delay` to glue and defer multiple yields, the same members the built-in
`seq { }` relies on. The three built-in builders, `async { }`, `seq { }`, and `result { }`, keep
bespoke lowerings so their output reads as idiomatic Python (`async`/`await`, generators, and a
`Result` short-circuit) rather than a chain of method calls. The full desugaring rules live in the
internals chapter on [desugaring](../internals/03-desugaring.md).

## Exercise

Complete the `Maybe` builder by filling both holes, then `addThree` chains three optional values and
returns their sum, stopping at the first `None`. The first hole (in the `Some x` arm of `bind`) has
type `Option 'a`, and the compiler suggests `f` among the fits, so run the continuation `f` on the
unwrapped `x`. The second hole is the body of `return_`, which must wrap its argument back up as an
`Option`.

```pyfun
module Maybe =
  let bind m f =
    match m:
      case Some x: ?
      case None: None
  let return_ x = ?

let addThree a b c =
  Maybe {
    let! x = a
    let! y = b
    let! z = c
    return x + y + z
  }

print (addThree (Some 1) (Some 2) (Some 3))
print (addThree (Some 1) None (Some 3))
```

Expected output:

```console
Some(6)
None_
```

[Open in the playground](https://simontreanor.github.io/Pyfun/playground/#code=bW9kdWxlIE1heWJlID0KICBsZXQgYmluZCBtIGYgPQogICAgbWF0Y2ggbToKICAgICAgY2FzZSBTb21lIHg6ID8KICAgICAgY2FzZSBOb25lOiBOb25lCiAgbGV0IHJldHVybl8geCA9ID8KCmxldCBhZGRUaHJlZSBhIGIgYyA9CiAgTWF5YmUgewogICAgbGV0ISB4ID0gYQogICAgbGV0ISB5ID0gYgogICAgbGV0ISB6ID0gYwogICAgcmV0dXJuIHggKyB5ICsgegogIH0KCnByaW50IChhZGRUaHJlZSAoU29tZSAxKSAoU29tZSAyKSAoU29tZSAzKSkKcHJpbnQgKGFkZFRocmVlIChTb21lIDEpIE5vbmUgKFNvbWUgMykpCg)

<details>
<summary>Show solution</summary>

```pyfun
module Maybe =
  let bind m f =
    match m:
      case Some x: f x
      case None: None
  let return_ x = Some x

let addThree a b c =
  Maybe {
    let! x = a
    let! y = b
    let! z = c
    return x + y + z
  }

print (addThree (Some 1) (Some 2) (Some 3))
print (addThree (Some 1) None (Some 3))
```

`f x` runs the rest of the block on the unwrapped value, and `Some x` wraps the final sum. The
middle `None` in the second call makes `bind` take its `None` arm, so the block short-circuits
before the `+`.
</details>
