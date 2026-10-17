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
It describes work to do later, much as calling an `async def` in Python hands you something to await
rather than a result. Nothing happens until something awaits it. Here `let! x = async { return 20 }`
awaits an inner block and binds `20` to `x`, then `return x + 1` produces the final `21`, but only
once the whole block is run.

The emitted Python shows the mapping directly. Each `async { }` becomes a nested `async def`, each
`let!` becomes an `await`, and the value itself is a small `_pf_Async` that remembers which function
to run. (Two short helpers, `_pf_Async` and `_pf_run`, sit at the top of the file and are left out
here.)

```python
async def _pf_fn1():
    async def _pf_fn0():
        return 20
    x = await _pf_fn0()
    return x + 1
fetchScore = _pf_Async(_pf_fn1)
print(asyncio.run(_pf_run(fetchScore)))
```

## Awaiting a value twice

Here is one place where Pyfun goes further than Python. Try awaiting the same value twice:

```pyfun
extern runAsync: Async a -> a = asyncio.run

let roll =
  async {
    print "rolling"
    return 6
  }

let twice =
  async {
    let! a = roll
    let! b = roll
    return a + b
  }

print (runAsync twice)
```

```console
rolling
rolling
12
```

The block runs twice, once for each `let!`. A Python coroutine can only be awaited once, and a second
`await` of it raises an error. A Pyfun `Async` value can be awaited as often as you like, and each
await runs it again from the start. F# calls this a *cold* async. It means you can keep an `Async` in
a variable, pass it to a function that retries it, or put the same one in a list twice.

It also means nothing starts early. Two `let!` lines in a row run one after the other, because the
second value has not started by the time the first finishes. When you want several things to run at
the same time, such as two web requests, hand them to `Async.parallel` together, which you will meet
in a moment.

## Running one at the top level

An `Async` value has to be awaited somewhere, and at the top level there is no enclosing `async`
function to await it in. Python solves this with `asyncio.run(main())`, and Pyfun makes the same
move through `extern` (lesson 12). The line

```pyfun
extern runAsync: Async a -> a = asyncio.run
```

names Python's `asyncio.run` and gives it the Pyfun type `Async a -> a`: hand it an `Async a` and it
runs it to completion and returns the `a`. `asyncio.run` wants a Python coroutine, so Pyfun hands it
a fresh one made from the value, which is the `_pf_run(fetchScore)` you saw in the output. So `fetchScore |> runAsync` turns the
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
`do!` when there is no value) inside an `async { }` block is where the await happens. Pyfun saves
the call itself for the await, so awaiting the result twice calls the Python function twice, just as
it runs an `async { }` block twice.

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

## Structured concurrency

Starting a task is where async programs leak: a task nobody joins keeps running after the code
that started it has moved on. Pyfun's `Task` module types the discipline. `Task.scope` runs an
async body, and `Task.start` starts a task that the enclosing scope owns. When the body ends, the
scope waits for every task it started (or cancels them all if one fails). There is no `cancel` to
forget.

```pyfun
extern runAsync: Async a -> a = asyncio.run

let worker name =
  async {
    do! Async.sleep 0.01
    print f"{name} done"
  }

let session =
  Task.scope (async {
    Task.start (worker "a")
    Task.start (worker "b")
    return "both joined"
  })

print (runAsync session)
```

```console
a done
b done
both joined
```

The body returns before the workers finish, and the scope holds the result until they have.

What keeps a task inside a scope is an effect. `Task.start` performs `spawn`, the same way `print`
performs `io`, and a `Task.scope` body is the one place `spawn` stops. A function that starts
tasks is fine to write (its type says `->{spawn}`), and calling it inside a scope's body is fine
too. Starting a task anywhere else is a compile error:

```pyfun
let stray = Task.start (Async.sleep 1.0)
```

```console
error: this starts a task outside any `Task.scope`, so nothing would own it: it performs `spawn`,
which only a `Task.scope` body discharges (start the task inside `Task.scope (async { ... })`)
```

The interop cookbook's `structured_concurrency` entry builds a mailbox agent over `asyncio.Queue`
on the same two functions.

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
`asyncio.run` is already in place, and `runAsync combined` runs it to a value.

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
b` produces `30` once `runAsync` runs the block.
</details>
