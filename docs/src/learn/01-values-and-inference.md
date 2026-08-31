# 1. Values and inference

A Pyfun program is built from values you name with `let`. In Python you would write
`name = "Ada"`. Pyfun uses a keyword, `let`, and the idea is the same: bind a name to a
value. The difference is what happens next. A Pyfun `let` names a value that does not
change, so the program reads as a set of definitions rather than a sequence of updates.

```pyfun
let name = "Ada"
let age = 36
let pi = 3.14
let isAdmin = true

print (f"{name} is {age}")
print (f"pi is about {pi}")
print isAdmin
```

Running this prints:

```console
Ada is 36
pi is about 3.14
True
```

You never wrote a type. The compiler infers one for every binding: `name` is a `string`,
`age` is an `int`, `pi` is a `float`, and `isAdmin` is a `bool`. There are no type
annotations on `let`, and that is a deliberate design choice, not a missing feature. You
get the safety of static types without writing them out.

Inference is not guessing: the compiler knows enough about each value to reject code that
does not fit. Python allows `+` to mean both numeric addition and string joining, so a
mistake there surfaces only when the line runs. Suppose you write `let label = "age: " +
36` in Pyfun. The compiler sees that `"age: "` is a `string` and that `+` works on numbers,
and it refuses to produce any Python at all:

```console
error: `+` is numeric and does not concatenate strings — use `String.concat a b`
 --> 1:13
  |
1 | let label = "age: " + 36
  |             ^^^^^^^
```

Every Pyfun diagnostic has the same shape: `-->` points at the line and column, the `|`
margin quotes the source, and `^^^^` underlines exactly the span at fault. Reading one is
most of the workflow for the rest of this course: `pyfun check` names what is wrong and
where, and you fix the code from that. `print` and f-strings (`f"{x}"`, the same
interpolation Python 3.12 uses) are how you observe a value once it is bound. Because a
`let` binding is immutable, there is no statement that overwrites it in place. That
capability exists, and it arrives in lesson 10, but the default is a value that stays put.

## Exercise

Two baskets hold fruit. Run `pyfun check` on the starter below: the compiler reports the
type the hole `?` expects and lists the names in scope that fit.

```pyfun
let apples = 4
let oranges = 3
let fruit = apples + ?
print (f"total fruit: {fruit}")
```

The checker reports:

```console
note: hole `?` has type `int` — try: apples, oranges — or: List.sum ?, Seq.sum ?, String.len ?, cbrt ?
 --> 3:22
  |
3 | let fruit = apples + ?
  |                      ^
1 unfilled hole
```

Replace `?` with the name that makes the total come out right.

Expected output:

```console
total fruit: 7
```

[Open in the playground](https://simontreanor.github.io/Pyfun/playground/#code=bGV0IGFwcGxlcyA9IDQKbGV0IG9yYW5nZXMgPSAzCmxldCBmcnVpdCA9IGFwcGxlcyArID8KcHJpbnQgKGYidG90YWwgZnJ1aXQ6IHtmcnVpdH0iKQo)

<details>
<summary>Show solution</summary>

```pyfun
let apples = 4
let oranges = 3
let fruit = apples + oranges
print (f"total fruit: {fruit}")
```

The hole had type `int`, and `oranges` was the binding in scope that made the sum work.
</details>
