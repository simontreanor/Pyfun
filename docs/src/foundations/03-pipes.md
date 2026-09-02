# 3. Chaining work: pipes

Once you have more than one function, you'll want to run a value through several of them, one
after another. Pyfun has an arrow, `|>`, for exactly that: it takes the value on its left and
feeds it into the function on its right.

```pyfun
let addBonus x = x + 10
let applyMultiplier x = x * 2

let score = 5 |> addBonus |> applyMultiplier

print score
```

```console
30
```

Read `5 |> addBonus |> applyMultiplier` left to right: start with `5`, run it through `addBonus`,
then run that result through `applyMultiplier`. `addBonus 5` is `15`, and `applyMultiplier 15` is
`30`. Without the pipe you would write the same thing as `applyMultiplier (addBonus 5)`, which
says the same calculation but puts the last step first and the first step in the middle. `|>`
lets you write it in the order the work actually happens.

## Leaving an argument for later

A function doesn't have to be given all its arguments at once. Give it fewer than it needs, and
instead of an error, you get back a function waiting for the rest.

```pyfun
let addPoints bonus x = x + bonus

let addTen = addPoints 10

print (addTen 5)
print (addPoints 3 5)
```

```console
15
8
```

`addPoints 10` doesn't compute anything yet, because `addPoints` needs two numbers and only got
one. It hands back a new function, and `addTen` names that function. `addTen 5` then supplies the
missing number and finishes the calculation. `addPoints 3 5` shows the ordinary way of calling
`addPoints` with both numbers at once, for comparison. Both ways reach the same function, they
just differ in how many blanks get filled in one go.

## Exercise

Finish the pipeline. `3` flows through `addBonus`, then through one more stage that should make the
final result `16`. Run `pyfun check` on the starter and read what type the hole wants, then decide
which of the two functions above fits.

```pyfun
let addBonus x = x + 5
let double x = x * 2

let result = 3 |> addBonus |> ?stage
print result
```

[Open in the playground](https://simontreanor.github.io/Pyfun/playground/#code=bGV0IGFkZEJvbnVzIHggPSB4ICsgNQpsZXQgZG91YmxlIHggPSB4ICogMgoKbGV0IHJlc3VsdCA9IDMgfD4gYWRkQm9udXMgfD4gP3N0YWdlCnByaW50IHJlc3VsdAo)

Expected output:

```console
16
```

<details>
<summary>Show solution</summary>

```pyfun
let addBonus x = x + 5
let double x = x * 2

let result = 3 |> addBonus |> double
print result
```

`addBonus 3` is `8`, and `double 8` is `16`. The hole was standing in for the last stage of the
pipeline, and `double` is the function that gets you there.
</details>
