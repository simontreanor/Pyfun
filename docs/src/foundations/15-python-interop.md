# 15. Reaching into Python's ecosystem

Everything you've written so far compiles to Python underneath. You haven't needed to know that,
but it means something enormous: the moment you can call a Python function from Pyfun, every
library anyone has ever written for Python is within reach. This lesson is how.

```pyfun
extern pure squareRoot: float -> float = math.sqrt

let hypotenuse a b = squareRoot (a * a + b * b)

print (hypotenuse 3.0 4.0)
```

```console
5.0
```

`extern pure squareRoot: float -> float = math.sqrt` says three things at once: there's a real
Python function at `math.sqrt`, it takes a `float` and returns a `float`, and it's `pure`, safe
to treat as an ordinary calculation. Once you've written that line, `squareRoot` behaves exactly
like a function you wrote yourself. `hypotenuse` calls it the same way it would call any other
function, and the compiler checks the types crossing the boundary the same way it checks
everything else. The Python underneath is exactly the call you'd expect:

```python
import math
def hypotenuse(a, b):
    return math.sqrt(a * a + b * b)
print(hypotenuse(3.0, 4.0))
```

Most of the world isn't pure, though. A function that rolls a die, reads a file, or asks a
server for something reaches outside itself, and an `extern` you declare without `pure` is
treated that way by default:

```pyfun
extern rollDie: int -> int -> int = random.randint
```

Leaving off `pure` here isn't a formality. `rollDie` gives a different answer every time you call
it, so it genuinely isn't pure, and the effect tracking from the last lesson applies to it exactly
as it applies to `print`.

## Exercise

Write a program from scratch. Declare an `extern pure` for `statistics.mean`, a Python function
that takes a list of numbers and returns their average: give it the Pyfun type
`List float -> float`. Then build the list `[4.0, 8.0, 15.0, 16.0, 23.0]`, name it, and print its
average.

Expected output:

```console
13.2
```

<details>
<summary>Show solution</summary>

```pyfun
extern pure mean: List float -> float = statistics.mean

let readings = [4.0, 8.0, 15.0, 16.0, 23.0]

print (mean readings)
```

`extern pure mean: List float -> float = statistics.mean` names the function, its type, and
where it lives in Python, all in one line. Calling it is no different from calling `squareRoot`
above, even though `statistics.mean` is code neither of us wrote.
</details>
