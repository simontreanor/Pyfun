# 2. Functions

A function is a value you name, the same as any other, except its name stands for a calculation
that still has blanks in it.

```pyfun
let celsiusToFahrenheit c = c * 9.0 / 5.0 + 32.0

let boiling = celsiusToFahrenheit 100.0
let freezing = celsiusToFahrenheit 0.0

print boiling
print freezing
print (celsiusToFahrenheit 37.0)
```

```console
212.0
32.0
98.6
```

`let celsiusToFahrenheit c = ...` names a function. `c` is the blank, the parameter, and
everything after the `=` is the calculation that fills it in once you give it a real number.
Calling the function is just writing its name next to the value: `celsiusToFahrenheit 100.0`.
There's no comma and no parentheses around the argument, you just put the two next to each other.

A function can take more than one blank. List the parameters one after another, and call the
function the same way, one argument after another:

```pyfun
let total price qty = price * qty

print (total 3.0 4)
```

```console
12.0
```

`total` has two parameters, `price` and `qty`, and `price * qty` is its whole body. Calling it
with `total 3.0 4` fills `price` with `3.0` and `qty` with `4`.

Notice that a function's body is not a set of instructions to carry out one after another. It is a
single expression, the answer the function produces. `celsiusToFahrenheit` doesn't compute a
temperature and then hand it back, computing the temperature *is* the whole function. That is why
the parentheses around `celsiusToFahrenheit 100.0` on the third line matter: without them, `print`
would try to take `celsiusToFahrenheit` and `100.0` as two separate things, instead of one call
whose result gets printed.

## Exercise

Read this program and work out what it prints, before you run it.

```pyfun
let addTax price = price + price * 0.2

let coffee = addTax 4.0
let book = addTax 15.0

print coffee
print book
```

[Open in the playground](https://simontreanor.github.io/Pyfun/playground/#code=bGV0IGFkZFRheCBwcmljZSA9IHByaWNlICsgcHJpY2UgKiAwLjIKCmxldCBjb2ZmZWUgPSBhZGRUYXggNC4wCmxldCBib29rID0gYWRkVGF4IDE1LjAKCnByaW50IGNvZmZlZQpwcmludCBib29rCg)
to check.

Expected output:

```console
4.8
18.0
```

<details>
<summary>Show solution</summary>

```pyfun
let addTax price = price + price * 0.2

let coffee = addTax 4.0
let book = addTax 15.0

print coffee
print book
```

`addTax 4.0` is `4.0 + 4.0 * 0.2`, which is `4.0 + 0.8`, which is `4.8`. `addTax 15.0` works the
same way: `15.0 + 3.0` is `18.0`.
</details>
