# 9. When a value might be missing: Option

Searching a list can come up empty. `List.find` looks for the first element that matches, and it
has to answer honestly for the case where nothing does.

```pyfun
let names = ["Ada", "Alan", "Grace"]

let describe query =
  match List.find (fun n -> n == query) names:
    case Some n: f"found {n}"
    case None: f"no one named {query}"

print (describe "Alan")
print (describe "Nope")
```

```console
found Alan
no one named Nope
```

`List.find` doesn't hand back a name directly. It hands back an `Option`, a value that is either
`Some n`, carrying the name it found, or plain `None`, meaning it found nothing. You take one apart
with `match`, the same as any type with more than one shape.

This matters because there's no other way to ask for a name that might not be there. You can't
just get `n` back and hope it's never empty, the type won't let you treat `Some n` and `None` as
the same thing. If you want the name out, you have to say what happens in both cases, and the
compiler holds you to writing both.

`Option.withDefault` is a shortcut for when you don't need a full `match`, just a fallback:

```pyfun
let names = ["Ada", "Alan", "Grace"]

let firstName = Option.withDefault "nobody" (List.find (fun n -> n == "Alan") names)
print firstName
```

```console
Alan
```

`Option.withDefault "nobody"` unwraps a `Some`, or hands back `"nobody"` in its place if the value
was `None`. Reach for a full `match` when the two cases genuinely differ, like the `describe`
function above, and reach for `Option.withDefault` when a missing value just needs a sensible
stand-in.

## Exercise

Extend the program above: write a `greet` function that looks up a name and prints it if found, or
`"Guest"` if not, using `Option.withDefault` instead of a full `match`.

```pyfun
let names = ["Ada", "Alan", "Grace"]

let describe query =
  match List.find (fun n -> n == query) names:
    case Some n: f"found {n}"
    case None: f"no one named {query}"

print (describe "Alan")
print (describe "Nope")
```

[Open in the playground](https://simontreanor.github.io/Pyfun/playground/#code=bGV0IG5hbWVzID0gWyJBZGEiLCAiQWxhbiIsICJHcmFjZSJdCgpsZXQgZGVzY3JpYmUgcXVlcnkgPQogIG1hdGNoIExpc3QuZmluZCAoZnVuIG4gLT4gbiA9PSBxdWVyeSkgbmFtZXM6CiAgICBjYXNlIFNvbWUgbjogZiJmb3VuZCB7bn0iCiAgICBjYXNlIE5vbmU6IGYibm8gb25lIG5hbWVkIHtxdWVyeX0iCgpwcmludCAoZGVzY3JpYmUgIkFsYW4iKQpwcmludCAoZGVzY3JpYmUgIk5vcGUiKQo)

Expected output:

```console
found Alan
no one named Nope
Grace
Guest
```

<details>
<summary>Show solution</summary>

```pyfun
let names = ["Ada", "Alan", "Grace"]

let describe query =
  match List.find (fun n -> n == query) names:
    case Some n: f"found {n}"
    case None: f"no one named {query}"

print (describe "Alan")
print (describe "Nope")

let greet query = Option.withDefault "Guest" (List.find (fun n -> n == query) names)

print (greet "Grace")
print (greet "Nope")
```

`greet` searches the same way `describe` does, and `Option.withDefault "Guest"` supplies the
fallback in one step instead of writing out both `match` arms.
</details>
