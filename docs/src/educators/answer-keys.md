# Answer keys

This page is for instructors. It collects the solution and expected output for every exercise in the
unit, one section per session. Each solution is the verified answer from the lesson's own collapsed
solution block, so it compiles and runs as shown. The linked lesson page holds the full explanation
of each answer.

## Session 1

### Lesson 1: Values and inference

[Lesson page](../learn/01-values-and-inference.md)

```pyfun
let apples = 4
let oranges = 3
let fruit = apples + oranges
print (f"total fruit: {fruit}")
```

Expected output:

```console
total fruit: 7
```

### Lesson 2: Functions, currying, and pipes

[Lesson page](../learn/02-functions-currying-pipes.md)

```pyfun
let double x = x + x
let triple x = x + x + x

let result = 3 |> double |> triple
print result
```

Expected output:

```console
18
```

## Session 2

### Lesson 3: The None problem: Option

[Lesson page](../learn/03-option.md)

```pyfun
let label s =
  match String.toInt s:
    case Some n: f"number: {n}"
    case None: "no number"

print (label "7")
print (label "nope")
```

Expected output:

```console
number: 7
no number
```

### Lesson 4: Errors as values: Result

[Lesson page](../learn/04-result.md)

```pyfun
extern parseInt: string -> int = int

let describe s =
  match try (parseInt s):
    case Ok n: f"ok: {n}"
    case Error e: f"bad: {e.errorKind}"

print (describe "88")
print (describe "bad")
```

Expected output:

```console
ok: 88
bad: ValueError
```

### Lesson 5: Your own types: ADTs and exhaustive match

[Lesson page](../learn/05-adts-and-match.md)

```pyfun
type Light =
  | Red
  | Amber
  | Green

let action l =
  match l:
    case Red: "stop"
    case Amber: "wait"
    case Green: "go"

print (action Red)
print (action Amber)
print (action Green)
```

Expected output:

```console
stop
wait
go
```

## Session 3

### Lesson 6: Records

[Lesson page](../learn/06-records.md)

```pyfun
type Account = { name: string, balance: int }

let opened = Account { name = "Ada", balance = 0 }
let funded = { opened with balance = 100 }

print funded.name
print funded.balance
print opened.balance
```

Expected output:

```console
Ada
100
0
```

### Lesson 7: Lists, sets, and maps

[Lesson page](../learn/07-collections.md)

```pyfun
let ns = [4, 8, 15, 16, 23]

let total = List.fold (+) 0 ns
let atTen = Option.withDefault 0 (List.get 10 ns)

print total
print atTen
```

Expected output:

```console
66
0
```

### Lesson 8: Tuples and destructuring

[Lesson page](../learn/08-tuples.md)

```pyfun
let names = ["ada", "alan"]
let scores = [10, 9]

let line pair =
  match pair:
    case (name, score): f"{name}: {score}"

let lines = List.zip names scores |> List.map line

lines |> String.join ", " |> print
```

Expected output:

```console
ada: 10, alan: 9
```

## Session 4

### Lesson 9: Typed holes and type-driven development

[Lesson page](../learn/09-typed-holes.md)

```pyfun
let scores = Map.add "ada" 10 (Map.add "alan" 9 Map.empty)

let report name =
  Map.tryFind name scores
  |> Option.withDefault 0
  |> String.fromInt

print (report "ada")
print (report "cy")
```

Expected output:

```console
10
0
```

### Lesson 10: Mutation, on purpose

[Lesson page](../learn/10-mutation.md)

```pyfun
let balance start =
  let mut acc = start
  acc <- acc + 100
  acc <- acc - 30
  acc <- acc + 50
  acc

print (balance 0)
```

Expected output:

```console
120
```

### Lesson 11: Effects, inferred

[Lesson page](../learn/11-effects.md)

```pyfun
let pure greet name = f"Hello, {name}"

print (greet "ada")
```

Expected output:

```console
Hello, ada
```

## Session 5

### Lesson 12: Talking to Python: extern

[Lesson page](../learn/12-python-interop.md)

```pyfun
type Book = { title: string, pages: int }

let bookDecoder =
  Decode.map2 (fun title pages -> Book { title = title, pages = pages })
    (Decode.field "title" Decode.string)
    (Decode.field "pages" Decode.int)

let describe r =
  match r:
    case Ok b: f"{b.title}, {b.pages} pages"
    case Error e: f"failed ({e.errorKind}): {e.errorMessage}"

let wellFormed = """{"title": "Dune", "pages": 412}"""
let missingField = """{"title": "Dune"}"""

wellFormed |> Decode.decodeString bookDecoder |> describe |> print
missingField |> Decode.decodeString bookDecoder |> describe |> print
```

Expected output:

```console
Dune, 412 pages
failed (KeyError): 'pages'
```

### Lesson 13: Computation expressions

[Lesson page](../learn/13-computation-expressions.md)

```pyfun
let multiply a b =
  result {
    let! x = Option.toResult "not a number" (String.toInt a)
    let! y = Option.toResult "not a number" (String.toInt b)
    return x * y
  }

print (multiply "6" "7")
print (multiply "6" "nope")
```

Expected output:

```console
Ok(42)
Error('not a number')
```

### Lesson 14: Units of measure

[Lesson page](../learn/14-units-of-measure.md)

```pyfun
measure m
measure s

let distance = 240.0<m>
let elapsed = 30.0<s>

# Speed is distance per unit of time.
let speed = distance / elapsed

print speed
```

Expected output:

```console
8.0
```

### Lesson 15: Modules and projects

[Lesson page](../learn/15-modules.md)

```pyfun
module Temp =
  let cToF c = c * 9 // 5 + 32
  let fToC f = (f - 32) * 5 // 9

print (Temp.cToF 100)
print (Temp.fToC 212)
```

Expected output:

```console
212
100
```

### Lesson 16: Capstone: a typed pipeline

[Lesson page](../learn/16-capstone.md)

```pyfun
measure m
measure s

type Run = { name: string, distance: float, time: float }

let runDecoder =
  Decode.map3 (fun n d t -> Run { name = n, distance = d, time = t })
    (Decode.field "name" Decode.string)
    (Decode.field "distance" Decode.float)
    (Decode.field "time" Decode.float)

let input = """[
  {"name": "ada", "distance": 100.0, "time": 20.0},
  {"name": "bo", "distance": 140.0, "time": 30.0}
]"""

let report =
  match Decode.decodeString (Decode.list runDecoder) input:
    case Ok runs:
      let names = runs |> List.map (fun r -> r.name) |> String.join ", "
      let dist = runs |> List.fold (fun a r -> a + r.distance * 1.0<m>) 0.0<m>
      let time = runs |> List.fold (fun a r -> a + r.time * 1.0<s>) 0.0<s>
      let avg = dist / time
      f"{names}: average speed {avg} m/s over {List.len runs} runs"
    case Error e: f"could not read input ({e.errorKind})"

print report
```

Expected output:

```console
ada, bo: average speed 4.8 m/s over 2 runs
```
