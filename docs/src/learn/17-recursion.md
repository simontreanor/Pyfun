# 17. Recursion

Lessons 1 to 16 are the graded course. From here on the lessons go a little deeper into corners of
the language you can reach for once the basics are comfortable.

A function can call itself. In Pyfun a name is in scope inside its own body, the same way a Python
`def` can refer to itself, so recursion just works with no `rec` keyword to turn it on:

```pyfun
let fact n =
  if n == 0 then 1
  else n * fact (n - 1)

print (fact 5)
```

```console
120
```

`fact` refers to `fact` in its own else branch, and each call shrinks `n` toward the `n == 0` base
case that stops the descent. Two functions can lean on each other the same way. Mutual recursion
needs no special form either, and the order you write them in does not matter, because both names
are in scope across the pair:

```pyfun
let isEven n = if n == 0 then true else isOdd (n - 1)
let isOdd n = if n == 0 then false else isEven (n - 1)

print (isEven 10)
print (isOdd 7)
```

```console
True
True
```

How deep a recursion can go depends on its shape. When a function's recursive call is the *last*
thing it does, and it calls itself, the compiler turns it into a loop:

```pyfun
let countDown n acc = if n == 0 then acc else countDown (n - 1) (acc + n)

print (countDown 100000 0)
```

```console
5000050000
```

That is a hundred thousand levels deep and it does not overflow anything, because the emitted Python
is the `while` loop you would have written by hand:

```python
def countDown(n, acc):
    while True:
        if n == 0:
            return acc
        else:
            n, acc = (n - 1, acc + n)
            continue
```

Every other shape compiles to a plain Python function and shares Python's call stack and its
recursion limit. `fact` above is one: its recursive call sits under a `*`, so the multiplication
still has to happen after the call returns and a frame has to wait for it. A recursion thousands of
levels deep in that shape will overflow the stack, the same as it would in hand-written Python. For
walking a long list or summing a large collection, reach for `List.fold` and the other combinators
from lesson 7, which loop underneath and stay flat. Recursion earns its keep on tree-shaped data,
where the depth follows the structure rather than the size of the input.

An arithmetic expression is a natural tree: a number is a leaf, and an addition or a multiplication
holds two smaller expressions. The lesson 5 material models it as an ADT, and `eval` walks it by
recursing into each branch:

```pyfun
type Expr =
  | Num int
  | Add Expr Expr
  | Mul Expr Expr

let eval e =
  match e:
    case Num n: n
    case Add l r: eval l + eval r
    case Mul l r: eval l * eval r

let tree = Add (Num 1) (Mul (Num 2) (Num 3))

print (eval tree)
```

```console
7
```

Each `case` handles one shape, and the `Add` and `Mul` arms call `eval` on their sub-expressions, so
the recursion bottoms out at the `Num` leaves. The depth of the calls matches the depth of the tree,
which for a balanced expression stays small even when the tree holds many nodes.

## Exercise

`Tree` holds an `int` at every leaf and branches in two. `total` should add up every leaf. The
`Leaf` case is done, but the `Branch` case is a hole. Run `pyfun check` to see the type it expects,
then combine the totals of the two sub-trees.

```pyfun
type Tree =
  | Leaf int
  | Branch Tree Tree

let total t =
  match t:
    case Leaf n: n
    case Branch l r: ?

let sample = Branch (Leaf 1) (Branch (Leaf 2) (Leaf 3))

print (total sample)
```

The checker reports:

```console
note: hole `?` has type `int` — or: total ?, List.sum ?, Seq.sum ?, String.len ?
 --> 8:22
  |
8 |     case Branch l r: ?
  |                      ^
1 unfilled hole
```

Expected output:

```console
6
```

[Open in the playground](https://simontreanor.github.io/Pyfun/playground/#code=dHlwZSBUcmVlID0KICB8IExlYWYgaW50CiAgfCBCcmFuY2ggVHJlZSBUcmVlCgpsZXQgdG90YWwgdCA9CiAgbWF0Y2ggdDoKICAgIGNhc2UgTGVhZiBuOiBuCiAgICBjYXNlIEJyYW5jaCBsIHI6ID8KCmxldCBzYW1wbGUgPSBCcmFuY2ggKExlYWYgMSkgKEJyYW5jaCAoTGVhZiAyKSAoTGVhZiAzKSkKCnByaW50ICh0b3RhbCBzYW1wbGUpCg)

<details>
<summary>Show solution</summary>

```pyfun
type Tree =
  | Leaf int
  | Branch Tree Tree

let total t =
  match t:
    case Leaf n: n
    case Branch l r: total l + total r

let sample = Branch (Leaf 1) (Branch (Leaf 2) (Leaf 3))

print (total sample)
```

The `Branch` arm calls `total` on the left and right sub-trees and adds the two results, so the
recursion follows the branches down to the leaves and sums them on the way back up. The hole even
suggests `total ?` under `or:`, pointing at the recursive call itself.
</details>
