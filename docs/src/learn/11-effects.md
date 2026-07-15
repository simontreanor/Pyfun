# 11. Effects, inferred

You already sense the difference between a function that computes and one that talks to the world. `price * qty` just calculates. `print` reaches out and shows something. In Python that distinction lives only in your head. Pyfun tracks it for you. It infers which functions perform effects, starting from the ones that obviously do, like `print` performing an `io` effect, and it propagates that outward: any function that calls an impure function is itself impure.

You never annotate effects. The inference is silent until you ask the compiler to confirm something is pure. Writing `let pure` in front of a definition is an assertion the compiler checks. If the body performs an effect, it is a compile error.

```pyfun
let pure total price qty = price * qty

let announce price qty =
  print (f"total: {total price qty}")

announce 12 3
```

This compiles and prints `total: 36`. `total` is pure, and the compiler agrees, because multiplication touches nothing outside itself. `announce` calls `print`, so it performs `io`. Its effect is inferred, not declared, and it needs no annotation. If you tried to mark `announce` as `pure`, the compiler would reject it, because purity propagates and a function that prints cannot be pure.

Here is what the check looks like when an assertion is wrong. A `let pure` whose body prints reports exactly where the effect happens:

```console
error: `greet` is declared `pure` but performs `io`
 --> 2:3
  |
2 |   print name
  |   ^^^^^^^^^^
```

The fix is not to weaken the assertion but to separate the two jobs. Keep the pure part pure by having it return a value, and perform the effect at the call site, where the `io` belongs. That separation is the point: your calculating code stays provably free of side effects, and the parts that talk to the world are the parts that say so.

## Exercise

`greet` is declared `pure`, but it prints inside its body, so it does not compile. Fix it by making `greet` return the greeting string and moving the `print` to the call site.

```pyfun
let pure greet name =
  print (f"Hello, {name}")
  name

let greeting = greet "ada"
```

Expected output:

```console
Hello, ada
```

[Open in the playground](https://simontreanor.github.io/Pyfun/playground/#code=bGV0IHB1cmUgZ3JlZXQgbmFtZSA9CiAgcHJpbnQgKGYiSGVsbG8sIHtuYW1lfSIpCiAgbmFtZQoKbGV0IGdyZWV0aW5nID0gZ3JlZXQgImFkYSIK)

<details>
<summary>Show solution</summary>

```pyfun
let pure greet name = f"Hello, {name}"

print (greet "ada")
```

Now `greet` only builds a string, so the `pure` assertion holds. The `io` effect lives at the call site, where `print` is.
</details>
