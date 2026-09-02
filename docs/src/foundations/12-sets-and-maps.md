# 12. Sets and maps

A list keeps things in order and lets the same value appear more than once. Two other
collections cover the cases a list handles awkwardly: a `Map` looks things up by a key, and a
`Set` keeps only one of each value.

```pyfun
let phonebook = Map.ofList [("Ada", "555-0101"), ("Alan", "555-0102")]

let lookup book name =
  match Map.tryFind name book:
    case Some number: f"{name}: {number}"
    case None: f"no number for {name}"

print (lookup phonebook "Ada")
print (lookup phonebook "Grace")

let visitors = Set.ofList ["Ada", "Alan", "Ada", "Grace"]
print (Set.len visitors)
print (Set.contains "Alan" visitors)
```

```console
Ada: 555-0101
no number for Grace
3
True
```

`Map.ofList` builds a lookup table from a list of `(key, value)` pairs, here names paired with
phone numbers. `Map.tryFind name book` looks a name up, and true to a pattern you've seen twice
now, it doesn't just hand back a number, it hands back an `Option`: `Some number` if the name is
in the book, `None` if it isn't. There's no other way to ask, which means there's no way to
forget to handle a name that isn't listed.

`Set.ofList` builds a set from a list, and duplicates disappear: four names went in, three came
out, because `"Ada"` was listed twice. `Set.contains` asks whether a value is in the set at all.

## Exercise

Extend the program above: add Grace's number to the phonebook with `Map.add`, and look her up in
the updated book.

```pyfun
let phonebook = Map.ofList [("Ada", "555-0101"), ("Alan", "555-0102")]

let lookup book name =
  match Map.tryFind name book:
    case Some number: f"{name}: {number}"
    case None: f"no number for {name}"

print (lookup phonebook "Ada")
print (lookup phonebook "Grace")

let visitors = Set.ofList ["Ada", "Alan", "Ada", "Grace"]
print (Set.len visitors)
print (Set.contains "Alan" visitors)
```

[Open in the playground](https://simontreanor.github.io/Pyfun/playground/#code=bGV0IHBob25lYm9vayA9IE1hcC5vZkxpc3QgWygiQWRhIiwgIjU1NS0wMTAxIiksICgiQWxhbiIsICI1NTUtMDEwMiIpXQoKbGV0IGxvb2t1cCBib29rIG5hbWUgPQogIG1hdGNoIE1hcC50cnlGaW5kIG5hbWUgYm9vazoKICAgIGNhc2UgU29tZSBudW1iZXI6IGYie25hbWV9OiB7bnVtYmVyfSIKICAgIGNhc2UgTm9uZTogZiJubyBudW1iZXIgZm9yIHtuYW1lfSIKCnByaW50IChsb29rdXAgcGhvbmVib29rICJBZGEiKQpwcmludCAobG9va3VwIHBob25lYm9vayAiR3JhY2UiKQoKbGV0IHZpc2l0b3JzID0gU2V0Lm9mTGlzdCBbIkFkYSIsICJBbGFuIiwgIkFkYSIsICJHcmFjZSJdCnByaW50IChTZXQubGVuIHZpc2l0b3JzKQpwcmludCAoU2V0LmNvbnRhaW5zICJBbGFuIiB2aXNpdG9ycykK)

Expected output:

```console
Ada: 555-0101
no number for Grace
3
True
Grace: 555-0103
```

<details>
<summary>Show solution</summary>

```pyfun
let phonebook = Map.ofList [("Ada", "555-0101"), ("Alan", "555-0102")]

let lookup book name =
  match Map.tryFind name book:
    case Some number: f"{name}: {number}"
    case None: f"no number for {name}"

print (lookup phonebook "Ada")
print (lookup phonebook "Grace")

let visitors = Set.ofList ["Ada", "Alan", "Ada", "Grace"]
print (Set.len visitors)
print (Set.contains "Alan" visitors)

let updated = Map.add "Grace" "555-0103" phonebook
print (lookup updated "Grace")
```

`Map.add` doesn't change `phonebook`, it builds a new map with Grace's number added, the same way
a record update built a new record back in lesson 5. `lookup` still works on the new map, because
it takes the book as a parameter instead of assuming which one.
</details>
