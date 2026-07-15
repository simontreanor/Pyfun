# 2. Functions, currying, and pipes

A function in Pyfun is a `let` binding with parameters before the `=`. In Python you would
write `def add(a, b): return a + b`. Pyfun writes the parameters inline and the body is the
expression after `=`. Calls leave off the parentheses and commas: you write `add 1 2`, not
`add(1, 2)`.

```pyfun
let add a b = a + b

let inc = add 1

let double x = x * 2

let result = 5 |> inc |> double

print (add 2 3)
print (inc 10)
print result
```

Running this prints:

```console
5
11
12
```

Two things there are new. First, `add 1` is a call with one argument to a two-argument
function. Instead of an error, it produces a new function that is waiting for the second
argument. That is currying: every function takes its arguments one at a time, so a partial
call like `add 1` hands you back a function. Here `inc` adds one to whatever you give it.

Second, `5 |> inc |> double` is a pipeline. The pipe `|>` takes the value on its left and
feeds it to the function on its right, so you read it left to right as "start with 5, then
`inc`, then `double`." In Python you would nest the calls as `double(inc(5))`, which reads
inside out. The pipe keeps the order of operations the same as the order you read.

None of this adds a runtime layer. A fully applied call compiles straight to a normal
Python call, and the pipeline unwinds to plain nesting:

```python
import functools
def add(a, b):
    return a + b
inc = functools.partial(add, 1)
def double(x):
    return x * 2
result = double(inc(5))
```

Currying shows up only where you actually leave an argument off: `inc` becomes a
`functools.partial`. Everything else is ordinary Python. When you want to name a pipeline
without a starting value, `>>` composes two functions into one, left to right, so
`inc >> double` is the function that runs `inc` and then `double`.

## Exercise

Finish the pipeline. The value 3 flows through `double`, then through one more stage. Run
`pyfun check` and the compiler tells you the hole wants a function from `int`. Put the stage
that makes the result 18.

```pyfun
let double x = x + x
let triple x = x + x + x

let result = 3 |> double |> ?stage
print result
```

Expected output:

```console
18
```

[Open in the playground](https://simontreanor.github.io/Pyfun/playground/#code=bGV0IGRvdWJsZSB4ID0geCArIHgKbGV0IHRyaXBsZSB4ID0geCArIHggKyB4CgpsZXQgcmVzdWx0ID0gMyB8PiBkb3VibGUgfD4gP3N0YWdlCnByaW50IHJlc3VsdAo)

<details>
<summary>Show solution</summary>

```pyfun
let double x = x + x
let triple x = x + x + x

let result = 3 |> double |> triple
print result
```

`double 3` is 6, and `triple 6` is 18. The stage is just the next function in the chain.
</details>
