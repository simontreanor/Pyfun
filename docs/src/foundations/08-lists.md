# 8. Lists

A list holds many values of the same kind, in order, written with square brackets.

```pyfun
let prices = [4, 8, 15, 16, 23]

let discounted = List.map (fun p -> p - 2) prices
let cheap = List.filter (fun p -> p < 10) prices
let total = List.fold (+) 0 prices

print discounted
print cheap
print total
```

```console
[2, 6, 13, 14, 21]
[4, 8]
66
```

There's no loop anywhere in that program, and there won't be one anywhere in this course. Instead
there are three functions that each do one job over an entire list at once:

- `List.map` builds a new list by applying a function to every element. `fun p -> p - 2` is that
  function, written inline: it takes one value, `p`, and gives back `p - 2`. `List.map` runs it
  over every price and collects the results.
- `List.filter` builds a new list of only the elements a function says yes to. `fun p -> p < 10`
  answers true or false for each price, and only the ones it says yes to survive.
- `List.fold` combines every element into one answer, starting from a seed. `List.fold (+) 0
  prices` starts at `0` and adds each price in turn, ending at the sum of all of them. `(+)` here
  is just addition written as a function, the same way you'd write `add` yourself.

Each one describes *what* you want done to the list, not the mechanics of stepping through it one
at a time. That turns out to matter later, once lists get long or the work gets more involved: you
read `List.filter (fun p -> p < 10)` as "the prices under ten," not as a loop you have to trace.

## Exercise

Extend the program above: also print how many prices are strictly more than 10.

```pyfun
let prices = [4, 8, 15, 16, 23]

let discounted = List.map (fun p -> p - 2) prices
let cheap = List.filter (fun p -> p < 10) prices
let total = List.fold (+) 0 prices

print discounted
print cheap
print total
```

[Open in the playground](https://simontreanor.github.io/Pyfun/playground/#code=bGV0IHByaWNlcyA9IFs0LCA4LCAxNSwgMTYsIDIzXQoKbGV0IGRpc2NvdW50ZWQgPSBMaXN0Lm1hcCAoZnVuIHAgLT4gcCAtIDIpIHByaWNlcwpsZXQgY2hlYXAgPSBMaXN0LmZpbHRlciAoZnVuIHAgLT4gcCA8IDEwKSBwcmljZXMKbGV0IHRvdGFsID0gTGlzdC5mb2xkICgrKSAwIHByaWNlcwoKcHJpbnQgZGlzY291bnRlZApwcmludCBjaGVhcApwcmludCB0b3RhbAo)

You'll need `List.filter` again, and `List.len` to count what's left in a list.

Expected output:

```console
[2, 6, 13, 14, 21]
[4, 8]
66
3
```

<details>
<summary>Show solution</summary>

```pyfun
let prices = [4, 8, 15, 16, 23]

let discounted = List.map (fun p -> p - 2) prices
let cheap = List.filter (fun p -> p < 10) prices
let total = List.fold (+) 0 prices

print discounted
print cheap
print total

let expensive = List.filter (fun p -> p > 10) prices
print (List.len expensive)
```

`List.filter (fun p -> p > 10) prices` keeps `15`, `16`, and `23`, three prices, and `List.len`
counts them.
</details>
