# 14. Units of measure

A plain `float` cannot tell you whether it holds metres, seconds, or kilograms, and mixing them up is
a classic source of real bugs. Pyfun lets you tag a number with a unit and then checks the units the
way it checks types. You declare a base unit with `measure`, and a literal carries that unit when the
annotation touches the digits, like `100.0<m>`.

```pyfun
measure m
measure s
measure kg

measure N = kg m / s^2                # newton, a derived alias

let distance = 100.0<m>
let elapsed = 10.0<s>
let speed = distance / elapsed        # float<m/s>
let side = sqrt 16.0<m^2>             # float<m>
let force = 6.0<N>                    # a value in newtons

print speed
print side
print force
```

```console
10.0
4.0
6.0
```

Arithmetic combines units the way dimensional analysis does. Dividing `float<m>` by `float<s>`
derives `float<m/s>`, so `speed` carries the right unit without you writing it down. `sqrt` halves
the exponents, taking a `float<m^2>` to a `float<m>`, which is the length of a square's side from its
area. A `measure N = kg m / s^2` names a compound of base units, so `6.0<N>` is a newton and the
compiler knows it means the same thing as `6.0<kg m / s^2>`.

Adding quantities with different units is where the checker steps in. Writing `distance + elapsed`
is rejected before any Python is produced:

```console
error: type mismatch: expected float<m>, found float<s>
```

The units exist only during type checking. They erase at lowering, so the emitted Python is plain
numbers with no unit machinery to slow it down:

```python
import math
distance = 100.0
elapsed = 10.0
speed = distance / elapsed
side = math.sqrt(16.0)
```

## Exercise

The program below wants the runner's speed, but it adds a distance to a time, which does not
type-check. Run `pyfun check` to see the mismatch, then change the one operator so the units line up
and the value becomes a speed in `float<m/s>`. Speed is distance divided by time.

```pyfun
measure m
measure s

let distance = 240.0<m>
let elapsed = 30.0<s>

# Speed is distance per unit of time. This line does not type-check yet.
let speed = distance + elapsed

print speed
```

The checker reports:

```console
error: type mismatch: expected float<m>, found float<s>
 --> 8:13
  |
8 | let speed = distance + elapsed
  |             ^^^^^^^^^^^^^^^^^^
```

Expected output:

```console
8.0
```

[Open in the playground](https://simontreanor.github.io/Pyfun/playground/#code=bWVhc3VyZSBtCm1lYXN1cmUgcwoKbGV0IGRpc3RhbmNlID0gMjQwLjA8bT4KbGV0IGVsYXBzZWQgPSAzMC4wPHM-CgojIFNwZWVkIGlzIGRpc3RhbmNlIHBlciB1bml0IG9mIHRpbWUuIFRoaXMgbGluZSBkb2VzIG5vdCB0eXBlLWNoZWNrIHlldC4KbGV0IHNwZWVkID0gZGlzdGFuY2UgKyBlbGFwc2VkCgpwcmludCBzcGVlZAo)

<details>
<summary>Show solution</summary>

```pyfun
measure m
measure s

let distance = 240.0<m>
let elapsed = 30.0<s>

# Speed is distance per unit of time.
let speed = distance / elapsed

print speed
```

Dividing `float<m>` by `float<s>` derives `float<m/s>`, so the units agree and `240.0 / 30.0` prints
`8.0`.
</details>
