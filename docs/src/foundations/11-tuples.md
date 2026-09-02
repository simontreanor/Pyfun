# 11. Pairing values: tuples

Sometimes a function has two things to give back, and neither one is the whole answer without the
other. Sharing cookies among kids gives you a whole number each and a number left over. Both
matter.

```pyfun
let share cookies kids =
  (cookies // kids, cookies % kids)

let (whole, leftover) = share 17 5

print (f"{whole} each, {leftover} left over")
```

```console
3 each, 2 left over
```

`(cookies // kids, cookies % kids)` is a tuple, a fixed pair of values written between
parentheses and separated by a comma. `//` is division that drops the remainder (seventeen
cookies split five ways is three whole cookies each), and `%` is the remainder itself, two
cookies left over. `share` hands back both at once, as one pair.

`let (whole, leftover) = share 17 5` takes the pair apart on the way in. It doesn't give you a
tuple you then have to dig into, it gives you `whole` and `leftover` directly, already named, in
one line.

A function parameter can destructure a tuple the same way:

```pyfun
let describe (name, age) = f"{name} is {age}"

print (describe ("Ada", 12))
```

```console
Ada is 12
```

`(name, age)` in the parameter list isn't two parameters, it's one parameter that happens to be a
pair, taken apart as it comes in. Anywhere Pyfun expects a name for a value, a pattern like this
works instead, as long as it can always match, which a pair always can.

## Exercise

Extend the program above: write a `report` function whose parameter is a `(whole, leftover)` pair
and that returns the same sentence `share` printed, then use it to describe sharing 20 cookies
among 7 kids.

```pyfun
let share cookies kids =
  (cookies // kids, cookies % kids)

let (whole, leftover) = share 17 5

print (f"{whole} each, {leftover} left over")
```

[Open in the playground](https://simontreanor.github.io/Pyfun/playground/#code=bGV0IHNoYXJlIGNvb2tpZXMga2lkcyA9CiAgKGNvb2tpZXMgLy8ga2lkcywgY29va2llcyAlIGtpZHMpCgpsZXQgKHdob2xlLCBsZWZ0b3ZlcikgPSBzaGFyZSAxNyA1CgpwcmludCAoZiJ7d2hvbGV9IGVhY2gsIHtsZWZ0b3Zlcn0gbGVmdCBvdmVyIikK)

Expected output:

```console
3 each, 2 left over
2 each, 6 left over
```

<details>
<summary>Show solution</summary>

```pyfun
let share cookies kids =
  (cookies // kids, cookies % kids)

let (whole, leftover) = share 17 5

print (f"{whole} each, {leftover} left over")

let report (whole, leftover) = f"{whole} each, {leftover} left over"

print (report (share 20 7))
```

`report` takes the pair `share` returns directly as its parameter, already split into `whole` and
`leftover`, so it can build the same sentence without a separate `let` step.
</details>
