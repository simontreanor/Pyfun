# 4. Making decisions

Every program you've seen so far runs straight through. This one chooses between answers.

```pyfun
let grade n =
  if n >= 90 then "A"
  elif n >= 80 then "B"
  elif n >= 70 then "C"
  else "F"

print (grade 95)
print (grade 82)
print (grade 40)
```

```console
A
B
F
```

`if ... then ... else` is not a command that skips over code, it's an expression that comes out to
one of its branches. `grade 95` checks `95 >= 90`, finds it true, and the whole `if` becomes `"A"`,
the same way `2 + 2` becomes `4`. `elif` is short for "else if," and you can chain as many as you
need, but every branch has to produce the same kind of value. If one branch gave back a number and
another gave back text, there would be no single answer for `grade` to be, and the compiler won't
let you write it.

Order matters. `n >= 90` is checked first, so a 95 never gets the chance to match `n >= 80`, even
though 95 is also at least 80. Each branch only runs if every branch above it failed.

## Deciding with match

`if` chooses between two directions. `match` chooses between several, by comparing a value against
a list of possibilities:

```pyfun
let describeDay d =
  match d:
    case 6: "weekend"
    case 7: "weekend"
    case _: "weekday"

print (describeDay 1)
print (describeDay 6)
print (describeDay 7)
```

```console
weekday
weekend
weekend
```

Each `case` lists one value to compare against, and `_` is a catch-all that matches whatever
nothing above it caught. You'll use `match` far more than this once you start describing your own
kinds of data, starting in a couple of lessons. For now, think of it as an `if` with more than two
directions.

## Exercise

This `grade` function compiles and runs, but it gets a 95 wrong: it should return `"pass with
honors"` for anything 90 or above, and it doesn't. Work out why, then fix it.

```pyfun
let grade n =
  if n >= 60 then "pass"
  elif n >= 90 then "pass with honors"
  else "fail"

print (grade 95)
print (grade 70)
print (grade 40)
```

[Open in the playground](https://simontreanor.github.io/Pyfun/playground/#code=bGV0IGdyYWRlIG4gPQogIGlmIG4gPj0gNjAgdGhlbiAicGFzcyIKICBlbGlmIG4gPj0gOTAgdGhlbiAicGFzcyB3aXRoIGhvbm9ycyIKICBlbHNlICJmYWlsIgoKcHJpbnQgKGdyYWRlIDk1KQpwcmludCAoZ3JhZGUgNzApCnByaW50IChncmFkZSA0MCkK)

Expected output:

```console
pass with honors
pass
fail
```

<details>
<summary>Show solution</summary>

```pyfun
let grade n =
  if n >= 90 then "pass with honors"
  elif n >= 60 then "pass"
  else "fail"

print (grade 95)
print (grade 70)
print (grade 40)
```

The original checked `n >= 60` first, and 95 satisfies that too, so it never reached the
`n >= 90` branch below. Putting the more specific condition first fixes it: now 95 is caught by
`n >= 90` before it ever reaches `n >= 60`.
</details>
