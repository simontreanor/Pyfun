# 14. Talking to the real world: effects

`price * qty` just calculates. `print` reaches out and puts something on the screen. Those are
different kinds of code, and Pyfun keeps track of the difference for you.

```pyfun
let pure itemCost price qty = price * qty

let announceOrder price qty =
  print (f"total: {itemCost price qty}")

announceOrder 4.0 3
```

```console
total: 12.0
```

Writing `pure` in front of `itemCost` is a claim: this function only calculates, it doesn't touch
the outside world. The compiler checks the claim, and it's true here, `price * qty` touches
nothing but its own two numbers. `announceOrder` calls `print`, so it reaches out, and Pyfun knows
that without being told, just from seeing `print` inside it.

Try putting `pure` on a function that prints, and the claim fails:

```pyfun
let pure loudCost price qty =
  print "calculating..."
  price * qty
```

```console
error: `loudCost` is declared `pure` but performs `io`
 --> 2:3
  |
2 |   print "calculating..."
  |   ^^^^^^^^^^^^^^^^^^^^^^
```

The fix isn't to remove `pure` and move on, it's to notice that `loudCost` was doing two jobs at
once, calculating and printing, and to split them: keep the calculation pure, and print at the
place that actually needs to.

`print` shows something. `input` is its other half, it asks for something and reads what comes
back:

```pyfun
let name = input "Your name: "
print (f"Hello, {name}")
```

Run this with `pyfun run` and it waits for you to type a name, then greets you. Both `print` and
`input` reach outside the program, so both count as the same kind of effect, and any function that
calls either one is impure too, all the way up its callers.

## Exercise

Extend the program above: write a `discountedCost` pure function that applies a 10% discount, and
an `announceDiscounted` function that prints it, then call it for the same order.

```pyfun
let pure itemCost price qty = price * qty

let announceOrder price qty =
  print (f"total: {itemCost price qty}")

announceOrder 4.0 3
```

[Open in the playground](https://simontreanor.github.io/Pyfun/playground/#code=bGV0IHB1cmUgaXRlbUNvc3QgcHJpY2UgcXR5ID0gcHJpY2UgKiBxdHkKCmxldCBhbm5vdW5jZU9yZGVyIHByaWNlIHF0eSA9CiAgcHJpbnQgKGYidG90YWw6IHtpdGVtQ29zdCBwcmljZSBxdHl9IikKCmFubm91bmNlT3JkZXIgNC4wIDMK)

Expected output:

```console
total: 12.0
discounted total: 10.8
```

<details>
<summary>Show solution</summary>

```pyfun
let pure itemCost price qty = price * qty

let announceOrder price qty =
  print (f"total: {itemCost price qty}")

announceOrder 4.0 3

let pure discountedCost price qty = price * qty * 0.9

let announceDiscounted price qty =
  print (f"discounted total: {discountedCost price qty}")

announceDiscounted 4.0 3
```

`discountedCost` only calculates, so `pure` holds, and `announceDiscounted` is the one place that
prints, the same split `itemCost` and `announceOrder` already used.
</details>
