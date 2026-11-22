# 23. Keyword arguments at the boundary

Python functions lean on keyword arguments with defaults. `int` takes a `base`, `open` takes an
`encoding`, and `requests.get` takes a `timeout`. A Pyfun arrow is positional, so an `extern` needs a
way to say which keyword a value belongs to. A trailing `(kw = value, …)` on the target says it, and
the value comes in two kinds.

The first kind is a literal, fixed once at the declaration:

```pyfun
extern parse_hex: str -> int = int(base = 16)

let raw = "ff"

raw |> parse_hex |> print
```

```console
255
```

`parse_hex` reads hexadecimal and nothing else, because the `16` belongs to the declaration rather
than the call. The emitted Python puts the keyword back where Python wants it:

```python
raw = "ff"
print(int(raw, base=16))
```

That suits a value which never changes. When it does change, `...` in place of the literal takes the
value from the caller:

```pyfun
extern parse_in: str -> int -> int = int(base = ...)

let from_hex s = parse_in s 16
let from_binary s = parse_in s 2

let a = "ff"
let b = "1011"

a |> from_hex |> print
b |> from_binary |> print
```

```console
255
11
```

One extern now covers every base. The type gained an argument for the slot to claim, and
`parse_in : str -> int -> int` is an ordinary curried arrow, so inference, effects, and the checker
treat it like any other function. What changes is where the argument lands in the emitted call:

```python
def from_hex(s):
    return int(s, base=16)
def from_binary(s):
    return int(s, base=2)
```

The binding rule reads the way a Python call reads. The target takes the leading arguments
positionally, and the slots take the trailing ones in the order the keywords are written. A pinned
literal claims no argument, so the two mix freely: `builtins.open(mode = "rt", encoding = ...)` takes
a path first and an encoding second. Slots also work on the instance-access targets from lesson 12,
where the receiver claims the first argument, as in `= .write_text(encoding = ...)`.

Currying holds across the boundary. A slot extern applied to some of its arguments is a function
awaiting the rest, like any other Pyfun function:

```pyfun
extern parse_in: str -> int -> int = int(base = ...)

let ff = parse_in "ff"

ff 16 |> print
```

```console
255
```

The spelling comes from Python's own stub files, where `def get(url, timeout=...)` marks a value the
signature declines to spell out. Reach for a pinned literal when every call wants the same value, and
for a slot when the caller decides.

## Exercise

The program below declares two externs that differ only in the base each one pins. Replace them with
a single extern whose base comes from the caller, and keep the output the same.

```pyfun
# Two externs that differ only in the base each one pins.
extern parse_hex: str -> int = int(base = 16)
extern parse_binary: str -> int = int(base = 2)

let hex = "ff"
let binary = "1011"

hex |> parse_hex |> print
binary |> parse_binary |> print
```

Expected output:

```console
255
11
```

[Open in the playground](https://simontreanor.github.io/Pyfun/playground/#code=IyBUd28gZXh0ZXJucyB0aGF0IGRpZmZlciBvbmx5IGluIHRoZSBiYXNlIGVhY2ggb25lIHBpbnMuCmV4dGVybiBwYXJzZV9oZXg6IHN0ciAtPiBpbnQgPSBpbnQoYmFzZSA9IDE2KQpleHRlcm4gcGFyc2VfYmluYXJ5OiBzdHIgLT4gaW50ID0gaW50KGJhc2UgPSAyKQoKbGV0IGhleCA9ICJmZiIKbGV0IGJpbmFyeSA9ICIxMDExIgoKaGV4IHw-IHBhcnNlX2hleCB8PiBwcmludApiaW5hcnkgfD4gcGFyc2VfYmluYXJ5IHw-IHByaW50Cg)

<details>
<summary>Show solution</summary>

```pyfun
extern parse_in: str -> int -> int = int(base = ...)

let from_hex s = parse_in s 16
let from_binary s = parse_in s 2

let hex = "ff"
let binary = "1011"

hex |> from_hex |> print
binary |> from_binary |> print
```

One declaration replaces both, and the base moves from the declaration to the call. The two named
helpers keep the call sites reading as pipelines, and either one can be passed around on its own,
because `parse_in s` is a function awaiting a base.
</details>
