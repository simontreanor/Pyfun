# 21. Strings and numbers, in detail

Earlier lessons used strings and numbers as they came up. This one is a reference tour of what the
two built-in types offer, grouped so you can find a feature when you need it.

## The String module

There is no `char` type, so everything is a `string`, and the operations live in a `String` module.
Concatenation, case, splitting, joining and slicing are all pure, and the parses are total:
`String.toInt` and `String.toFloat` return an `Option`, `None` when the text does not parse, so they
can never raise.

```pyfun
let title = String.concat "Hello, " "World"
let loud = String.upper title
let words = String.split ", " title
let csv = String.join "," words
let first5 = String.slice 0 5 title

print title
print loud
print words
print csv
print first5
print (Option.withDefault 0 (String.toInt "41"))
print (Option.withDefault 0 (String.toInt "x"))
print (Option.withDefault 0.0 (String.toFloat "2.5"))
```

```console
Hello, World
HELLO, WORLD
['Hello', 'World']
Hello,World
Hello
41
0
2.5
```

`String.slice 0 5` is total Python slicing, so an out-of-range bound clamps rather than raising, and
`String.toInt "x"` hands back `None`, defaulted to `0` here.

The module covers the rest of the everyday string work too, with the names a Python programmer
expects: `String.startsWith`, `String.endsWith`, `String.contains`, `String.replace`,
`String.splitLines`, `String.trimStart` and `String.trimEnd`, among others.

## Formatting numbers for people

Interpolation puts a value into a sentence, and a `Format` function decides how it should read.
`f"{price}"` gives you Python's own rendering; `Format` gives you the presentation choices you would
otherwise reach for a format specifier to express:

```pyfun
print (Format.fixed 2 3.14159)
print (Format.thousands 2 1234567.891)
print (Format.grouped 1234567)
print (Format.percent 1 0.256)
print (Format.padLeft 8 " " "42")
```

```console
3.14
1,234,567.89
1,234,567
25.6%
      42
```

`Format.fixed` sets the decimal places, `Format.thousands` adds grouping to a float and `grouped`
does the same for an integer, `Format.percent` turns a fraction into a percentage, and
`padLeft`/`padRight` align text in a field of a given width using a fill string. `Format.currency`
takes the symbol first, so `Format.currency "£" 2` partially applies into a reusable formatter.

These are ordinary functions, which is the reason they exist. A format specifier inside a string
(`f"{x:.2f}"` in Python) is a small language the compiler cannot see into, so `.f2` for `.2f` goes
wrong only when it runs. A misapplied `Format.fixed` is a type error, and a house style for money or
percentages is one function everything calls rather than a spelling everyone has to remember.

## String literals

An f-string interpolates any expression in `{...}`, exactly like Python. A `{expr=}` hole is
self-documenting: it prints the source text and then the value, handy for a quick trace. To put a
literal brace in the output, double it as `{{` or `}}`. A raw string `r"..."` turns off escape
processing, so a backslash stays a backslash, which is what you want for a Windows path or a regex.
A triple-quoted string spans lines, with newlines and lone quotes kept as content.

```pyfun
let who = "Ada"
let score = 41

let line = f"{who} scored {score} ({String.upper who})"
let debug = f"{score=}"
let braces = f"{{literal braces}} around {score}"
let winPath = r"C:\Users\pyfun\data.csv"
let banner = """== pyfun ==
a "quoted" word"""
let emoji = "hi \u{1F600}"

print line
print debug
print braces
print winPath
print banner
print (String.len "café")
print (String.len emoji)
```

```console
Ada scored 41 (ADA)
score=41
{literal braces} around 41
C:\Users\pyfun\data.csv
== pyfun ==
a "quoted" word
4
4
```

Regular strings still process escapes: `\n`, `\t`, `\"`, `\\`, and `\u{HEX}` for a Unicode code
point, so `"hi \u{1F600}"` ends with an emoji. Length counts characters, not bytes:
`String.len "café"` is 4, because the string is real UTF-8 and `é` is one character, and the emoji
string is 4 as well, the `h`, the `i`, the space and the one emoji.

## Numbers

Integer and float literals carry the conveniences you would reach for in Python. Underscores group
digits, `0x`, `0o` and `0b` give hex, octal and binary, and a float can use scientific notation like
`3.0e8`. Exponentiation `**` is float-only and right-associative, so `2.0 ** 3.0 ** 2.0` reads as
`2.0 ** (3.0 ** 2.0)`. Floor division `//` and modulo `%` behave as in Python, and a chained
comparison such as `0 <= x < 10` reads as one range test and evaluates `x` once.

```pyfun
let million = 1_000_000
let mask = 0xFF
let bits = 0b1010
let perm = 0o17
let lightSpeed = 3.0e8
let tower = 2.0 ** 3.0 ** 2.0
let q = 17 // 5
let r = 17 % 5
let inRange = 0 <= 7 < 10

print million
print mask
print bits
print perm
print lightSpeed
print tower
print q
print r
print inRange
```

```console
1000000
255
10
15
300000000.0
512.0
3
2
True
```

`0xFF` is 255, `0b1010` is 10 and `0o17` is 15, all the same kind of integer written in different
bases. `2.0 ** 3.0 ** 2.0` is 512 because the right-hand `**` binds first, giving `2.0 ** 9.0`. And
`0 <= 7 < 10` is a single `True`, the way Python chains the two comparisons into one range check.

## Exercise

`String.toInt` gives back an `Option`, and this program formats the parse into a sentence. The
default supplied to `Option.withDefault` is a hole. Run `pyfun check` to confirm its type, then fill
it so a string that does not parse falls back to `0`.

```pyfun
let label raw =
  let n = Option.withDefault ? (String.toInt raw)
  f"{raw} parses to {n}"

print (label "42")
print (label "oops")
```

The checker reports:

```console
note: hole `?` has type `int` — or: List.sum ?, Seq.sum ?, String.len ?, ceil ?
 --> 2:30
  |
2 |   let n = Option.withDefault ? (String.toInt raw)
  |                              ^
1 unfilled hole
```

Expected output:

```console
42 parses to 42
oops parses to 0
```

[Open in the playground](https://simontreanor.github.io/Pyfun/playground/#code=bGV0IGxhYmVsIHJhdyA9CiAgbGV0IG4gPSBPcHRpb24ud2l0aERlZmF1bHQgPyAoU3RyaW5nLnRvSW50IHJhdykKICBmIntyYXd9IHBhcnNlcyB0byB7bn0iCgpwcmludCAobGFiZWwgIjQyIikKcHJpbnQgKGxhYmVsICJvb3BzIikK)

<details>
<summary>Show solution</summary>

```pyfun
let label raw =
  let n = Option.withDefault 0 (String.toInt raw)
  f"{raw} parses to {n}"

print (label "42")
print (label "oops")
```

`String.toInt "42"` is `Some 42`, so the default is ignored and the line reads `42 parses to 42`.
`String.toInt "oops"` is `None`, so `Option.withDefault 0` supplies the `0`.
</details>
