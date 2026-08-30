# 18. Async

Lesson 13 introduced computation expressions with `result { }`, `option { }` and `seq { }`. The
fourth and last built-in builder is `async { }`, and it maps onto Python's own `async`/`await` the
way `seq` maps onto generators. An `async { }` block builds an `Async a` value. Inside it, `let!` awaits another async
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
of those effect labels, and an `async { }` block performs it. So a `let pure` body that builds an
async block is a compile error, the same way a `let pure` that prints is:

```pyfun
let pure grab x = async { return x + 1 }
```

```console
error: `grab` is declared `pure` but performs `async`
 --> 1:19
  |
1 | let pure grab x = async { return x + 1 }
  |                   ^^^^^^^^^^^^^^^^^^^^^^

1 error
```

The effect is impossible to lie about. If a function awaits real async work, its type says so, and
purity cannot be claimed over it.

## Binding an async Python function

An async Python function returns a coroutine when you call it, and nothing happens until that
coroutine is awaited. The Pyfun type says exactly that: the result is `Async a`, and `let!` (or
`do!` when there is no value) inside an `async { }` block is where the await happens.

```pyfun
extern runAsync: Async a -> a = asyncio.run
extern sleep: float -> Async unit = asyncio.sleep

let nap =
  async {
    do! sleep 0.01
    return "rested"
  }

print (runAsync nap)
```

```console
rested
```

`do! sleep 0.01` is the await, and `return "rested"` sets the value, so `nap` has type `Async
string`. Emitted, the `do!` is `await asyncio.sleep(0.01)` inside the `async def`.

The tempting spelling is `float ->{async} unit`, marking the arrow with the `async` label instead of
giving the result the `Async` type. The checker rejects it and names the working spelling, because a
coroutine that nothing awaits would sleep for zero seconds and print a warning:

```console
error: `sleep` is declared `->{async}` but returns `unit`, which cannot be awaited: an async Python function returns a coroutine, so declare the result as `Async unit` (`… -> Async unit`) and bind it with `let!` inside an `async { }` block
```

## The `Async` module

`Async` is also a module, with the combinators that are awkward to write as externs yourself.
`Async.parallel` awaits a whole list at once and keeps the results in order; `Async.timeout` gives an
await a deadline and hands back an `Option`; `Async.catch` turns an exception raised *at the await*
into a `Result` (a `try` around the call would only wrap the coroutine, because the call itself does
not fail); `Async.race` takes the first to finish and cancels the rest; and `Async.toThread` runs a
blocking function, `input` say, on a worker thread so the event loop stays free.

```pyfun
extern runAsync: Async a -> a = asyncio.run

let slow n =
  async {
    do! Async.sleep 0.01
    return n
  }

let main =
  async {
    let! xs = Async.parallel [slow 1, slow 2, slow 3]
    print xs
    let! quick = Async.timeout 0.001 (Async.sleep 1.0)
    print quick
    let! r = Async.catch (slow 4)
    match r:
      case Ok n: print f"finished with {n}"
      case Error e: print f"failed: {e.errorKind}"
    return ()
  }

runAsync main
```

```console
[1, 2, 3]
None
finished with 4
```

`Async.sleep` is `asyncio.sleep` itself; the others compile to small `async def` helpers over
`asyncio.gather`, `asyncio.wait_for`, `asyncio.wait` and `asyncio.to_thread`, so the emitted Python is
still the code you would have written.

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
