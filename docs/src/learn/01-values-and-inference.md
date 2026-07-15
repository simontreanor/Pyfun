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

Inference is not guessing. The compiler knows enough about each value to reject code that
does not fit. Python allows `+` to mean both numeric addition and string joining, so a
mistake there surfaces only when the line runs. Pyfun keeps the two apart and reports the
mismatch before any Python is produced:

```console
error: `+` is numeric and does not concatenate strings — use `String.concat a b`
 --> 1:13
  |
1 | let label = "age: " + 36
  |             ^^^^^^^
```

The compiler saw that `"age: "` is a `string` and that `+` works on numbers, so it stopped.
`print` and f-strings (`f"{x}"`, the same interpolation Python 3.12 uses) are how you
observe a value once it is bound. Because a `let` binding is immutable, there is no
statement that overwrites it in place. That capability exists, and it arrives in
lesson 10, but the default is a value that stays put.

## Exercise

Two baskets hold fruit. Fill the hole so the program adds them and prints the total. Run
`pyfun check` on the starter: the compiler reports the type the hole expects and lists the
names in scope that fit. Lesson 9 covers holes in full. For now, read the note and put the
right name where `?count` sits.

```pyfun
let apples = 4
let oranges = 3
let fruit = apples + ?count
print (f"total fruit: {fruit}")
```

Expected output:

```console
total fruit: 7
```

[Open in the playground](https://simontreanor.github.io/Pyfun/playground/#code=bGV0IGFwcGxlcyA9IDQKbGV0IG9yYW5nZXMgPSAzCmxldCBmcnVpdCA9IGFwcGxlcyArID9jb3VudApwcmludCAoZiJ0b3RhbCBmcnVpdDoge2ZydWl0fSIpCg)

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
