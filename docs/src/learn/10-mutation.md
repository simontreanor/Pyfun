# 10. Mutation, on purpose

Everything so far has been immutable. A `let` binds a name to a value once, and that name never changes. This is the default because it removes a whole class of bugs: nothing can quietly reassign a value out from under you. Reassigning a plain `let` is a compile error, not a silent overwrite.

Sometimes a local accumulator really is the clearest way to express a calculation. Pyfun lets you ask for one explicitly. Inside an indented block body, `let mut` declares a mutable binding, and `acc <- new` reassigns it. The arrow is visible in the source, so mutation is something you opt into and a reader can see, not the default everywhere. The block's last expression is its value.

```pyfun
let checkout price =
  let mut total = price
  total <- total + 5
  total <- total * 2
  total

print (checkout 10)
```

This prints `30`: the price plus a five unit fee, then doubled. If you had written `let total` instead of `let mut total`, the first `total <- ...` would fail:

```console
error: cannot assign to `acc`: it is immutable (declare it with `let mut`)
```

The mutation is scoped to the block. Outside `checkout`, nothing mutable escapes. And the emitted Python is exactly the plain statement sequence you would write by hand:

```python
def checkout(price):
    total = price
    total = total + 5
    total = total * 2
    return total
print(checkout(10))
```

Pyfun has no `for` or `while`, so this is not how you iterate over a collection. For that, reach for `List.fold` and friends from lesson 7. `let mut` is for a genuine local accumulator built from a few explicit steps.

## Exercise

`balance` starts from an opening amount and applies three transactions in sequence. The starter forgets to mark the accumulator mutable, so it does not compile. Read the diagnostic and fix the declaration.

```pyfun
let balance start =
  let acc = start
  acc <- acc + 100
  acc <- acc - 30
  acc <- acc + 50
  acc

print (balance 0)
```

Expected output:

```console
120
```

[Open in the playground](https://simontreanor.github.io/Pyfun/playground/#code=bGV0IGJhbGFuY2Ugc3RhcnQgPQogIGxldCBhY2MgPSBzdGFydAogIGFjYyA8LSBhY2MgKyAxMDAKICBhY2MgPC0gYWNjIC0gMzAKICBhY2MgPC0gYWNjICsgNTAKICBhY2MKCnByaW50IChiYWxhbmNlIDApCg)

<details>
<summary>Show solution</summary>

```pyfun
let balance start =
  let mut acc = start
  acc <- acc + 100
  acc <- acc - 30
  acc <- acc + 50
  acc

print (balance 0)
```

Adding `mut` to the declaration allows the three `<-` reassignments. The running total ends at `0 + 100 - 30 + 50`, which is `120`.
</details>
