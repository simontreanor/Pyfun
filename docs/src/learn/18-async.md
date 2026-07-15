# 18. Async

Lesson 13 introduced computation expressions with `result { }` and `seq { }`. The third built-in
builder is `async { }`, and it maps onto Python's own `async`/`await` the way `seq` maps onto
generators. An `async { }` block builds an `Async a` value. Inside it, `let!` awaits another async
step, and `return` hands back the final value.

```pyfun
extern runAsync: Async a -> a = asyncio.run

let fetchScore =
  async {
    let! x = async { return 20 }
    return x + 1
  }

print (runAsync fetchScore)
```

```console
21
```

The key idea carries straight over from Python. An `async { }` block does not run when you write it.
It builds a coroutine, exactly like calling an `async def` in Python hands you an awaitable rather
than a result. Nothing happens until something drives that coroutine. Here `let! x = async { return
20 }` awaits an inner block and binds `20` to `x`, then `return x + 1` produces the final `21`, but
only once the coroutine is run.

The emitted Python shows the mapping directly. Each `async { }` becomes a nested `async def`, each
`let!` becomes an `await`, and the whole value is a coroutine object:

```python
import asyncio
async def _pf_fn1():
    async def _pf_fn0():
        return 20
    x = await _pf_fn0()
    return x + 1
fetchScore = _pf_fn1()
print(asyncio.run(fetchScore))
```

## Running one at the top level

A coroutine has to be awaited somewhere, and at the top level there is no enclosing `async`
function to await it in. Python solves this with `asyncio.run(main())`, and Pyfun makes the same
move through `extern` (lesson 12). The line

```pyfun
extern runAsync: Async a -> a = asyncio.run
```

names Python's `asyncio.run` and gives it the Pyfun type `Async a -> a`: hand it an `Async a` and it
drives the coroutine to completion and returns the `a`. So `fetchScore |> runAsync` turns the
`Async int` into a plain `int` you can `print`.

## Async is an inferred effect

Lesson 11 showed the compiler inferring effects and checking `let pure` assertions. `async` is one
of those effect labels. Effects are still inferred everywhere, and an `extern` arrow may state the
`async` label explicitly with `->{async}`, overriding the default `io`:

```pyfun
extern fetchAsync: string ->{async} string = httpx.get
```

A caller of `fetchAsync` then performs `async`, and that propagates outward. So a `let pure` body
that performs async is a compile error, the same way a `let pure` that prints is:

```pyfun
extern fetchAsync: string ->{async} string = httpx.get

let pure grab url = fetchAsync url

print (grab "http://example.com")
```

```console
error: `grab` is declared `pure` but performs `async`
 --> 3:21
  |
3 | let pure grab url = fetchAsync url
  |                     ^^^^^^^^^^^^^^

1 error
```

The effect is impossible to lie about. If a function awaits real async work, its type says so, and
purity cannot be claimed over it.

## Where async pays off, and where it does not

Async earns its keep when real I/O overlaps, so waiting on one network request or file read lets
another proceed. The examples in this lesson are compute-shaped on purpose, because the browser
playground has no network access, and Pyodide runs `asyncio.run` in its worker, so a self-contained
block like `fetchScore` runs there and prints `21`.

For real overlapping I/O, install the compiler with `pip install pyfun-lang` and read the
`http_fetch` entry in the
[interop cookbook](https://github.com/simontreanor/Pyfun/tree/main/examples/interop), which fetches
URLs with inferred `io` and `async` effects over `urllib` and `httpx`. If you know Python's asyncio,
you can reach as far as you like through `extern`: any async client wraps the same way, and the
effect system tracks it for you.

## Exercise

Fill the hole so `combined` awaits two async blocks and returns their sum. The hole has type `int`
(the value the second block returns), so any integer literal works. The runner bridge over
`asyncio.run` is already in place, and `runAsync combined` drives the coroutine to a value.

```pyfun
extern runAsync: Async a -> a = asyncio.run

let combined =
  async {
    let! a = async { return 10 }
    let! b = async { return ? }
    return a + b
  }

print (runAsync combined)
```

Expected output:

```console
30
```

[Open in the playground](https://simontreanor.github.io/Pyfun/playground/#code=ZXh0ZXJuIHJ1bkFzeW5jOiBBc3luYyBhIC0-IGEgPSBhc3luY2lvLnJ1bgoKbGV0IGNvbWJpbmVkID0KICBhc3luYyB7CiAgICBsZXQhIGEgPSBhc3luYyB7IHJldHVybiAxMCB9CiAgICBsZXQhIGIgPSBhc3luYyB7IHJldHVybiA_IH0KICAgIHJldHVybiBhICsgYgogIH0KCnByaW50IChydW5Bc3luYyBjb21iaW5lZCkK)

<details>
<summary>Show solution</summary>

```pyfun
extern runAsync: Async a -> a = asyncio.run

let combined =
  async {
    let! a = async { return 10 }
    let! b = async { return 20 }
    return a + b
  }

print (runAsync combined)
```

`let!` awaits each inner block and binds its result, so `a` is `10` and `b` is `20`, and `return a +
b` produces `30` once `runAsync` drives the coroutine.
</details>
