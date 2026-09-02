# 13. Changing your mind, on purpose

Every `let` you've written so far has named a value once and left it alone. That's been true
since lesson 1, and it stays true almost everywhere in Pyfun: a name means one thing, and nothing
quietly changes it behind your back. Once in a while, though, a running total really is the
clearest way to build up an answer, and Pyfun lets you ask for that, explicitly.

```pyfun
let total order =
  let mut sum = 0
  sum <- sum + order
  sum <- sum + 5
  sum

print (total 20)
```

```console
25
```

`let mut sum = 0` declares `sum` as mutable, meaning it's now allowed to change. `sum <- sum +
order` reassigns it, and the arrow, `<-`, is what makes that step visible right there in the
source, not something you have to go looking for. The last line, just `sum` on its own, is the
block's answer: whatever `sum` holds by the time the block finishes.

Leave off `mut` and the first `<-` refuses to compile:

```console
error: cannot assign to `sum`: it is immutable (declare it with `let mut`)
 --> 3:3
  |
3 |   sum <- sum + order
  |   ^^^^^^^^^^^^^^^^^^
```

That's the whole point of the default from lesson 1. A plain `let` can never be reassigned, so if
you see one, you know its value is fixed for good. `mut` is the one word that lifts that
guarantee, and only for the name it's written on.

## Exercise

Extend the program above: write a `totalWithDiscount` function that works the same way but also
subtracts a $3 discount, and use it for two different orders.

```pyfun
let total order =
  let mut sum = 0
  sum <- sum + order
  sum <- sum + 5
  sum

print (total 20)
```

[Open in the playground](https://simontreanor.github.io/Pyfun/playground/#code=bGV0IHRvdGFsIG9yZGVyID0KICBsZXQgbXV0IHN1bSA9IDAKICBzdW0gPC0gc3VtICsgb3JkZXIKICBzdW0gPC0gc3VtICsgNQogIHN1bQoKcHJpbnQgKHRvdGFsIDIwKQo)

Expected output:

```console
25
22
52
```

<details>
<summary>Show solution</summary>

```pyfun
let total order =
  let mut sum = 0
  sum <- sum + order
  sum <- sum + 5
  sum

print (total 20)

let totalWithDiscount order =
  let mut sum = 0
  sum <- sum + order
  sum <- sum + 5
  sum <- sum - 3
  sum

print (totalWithDiscount 20)
print (totalWithDiscount 50)
```

`totalWithDiscount` reuses the same three steps as `total`, with one more `sum <- sum - 3` added
before the block ends.
</details>
