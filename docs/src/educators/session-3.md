# Session 3: Data modeling

## Objectives

By the end of this session, students can:

- Declare a record type with named fields and construct a value with the tagged constructor syntax.
- Copy and update a record and explain why the original is left unchanged.
- Match on a record, binding a subset of fields with the `{ x }` shorthand.
- Work over lists with `List.map`, `List.filter`, and `List.fold`, using lambdas and operator
  sections.
- Group values by position with tuples and destructure them in a `match` arm.

## Prerequisites

Sessions 1 and 2 (immutability, functions, `match`, ADTs). Python dictionaries and dataclasses give
useful points of comparison.

## Demo script

The thread is choosing the right shape for data: a record for one thing with named parts, a
collection for many things, a tuple for a short positional pairing. Keep the Python panel visible so
students see records become frozen dataclasses.

1. Open the [Lesson 6 exercise](https://simontreanor.github.io/Pyfun/playground/#code=dHlwZSBBY2NvdW50ID0geyBuYW1lOiBzdHJpbmcsIGJhbGFuY2U6IGludCB9CgpsZXQgb3BlbmVkID0gQWNjb3VudCB7IG5hbWUgPSAiQWRhIiwgYmFsYW5jZSA9IDAgfQpsZXQgZnVuZGVkID0geyBvcGVuZWQgd2l0aCBiYWxhbmNlID0gPyB9CgpwcmludCBmdW5kZWQubmFtZQpwcmludCBmdW5kZWQuYmFsYW5jZQpwcmludCBvcGVuZWQuYmFsYW5jZQo).
   Run `check` and read the hole note: the hole has type `int`. Fill it with `100` and Run. Draw
   attention to the last line: `opened.balance` still prints `0`.
2. Explain the copy-and-update line `{ opened with balance = 100 }`. It builds a fresh `Account` and
   leaves `opened` alone, which is why the original balance is unchanged. Connect this back to the
   immutability from Session 1.
3. Show the Python panel: `Account` compiled to a `@dataclass(frozen=True)`, and the update compiled
   to a new `Account(...)` call. The immutability is literal in the output.
4. Break it live: change `Account { name = "Ada", balance = 0 }` to drop the `balance` field. The
   compiler reports the missing field. Restore it. This shows records require all their fields.
5. Move to collections. Open the [Lesson 7 exercise](https://simontreanor.github.io/Pyfun/playground/#code=bGV0IG5zID0gWzQsIDgsIDE1LCAxNiwgMjNdCgpsZXQgdG90YWwgPSBMaXN0LmZvbGQgPyAwIG5zCmxldCBhdFRlbiA9IE9wdGlvbi53aXRoRGVmYXVsdCA_IChMaXN0LmdldCAxMCBucykKCnByaW50IHRvdGFsCnByaW50IGF0VGVuCg).
   Run `check` and show the two hole notes. The first hole wants `int -> int -> int`. Fill it with
   the operator section `(+)`, fill the second with `0`, and Run.
6. Make the totality point: there is no `ns[10]` bracket indexing in Pyfun. `List.get 10 ns` returns
   an `Option`, and `Option.withDefault` supplies the fallback, so an out-of-range lookup cannot
   crash. This is the same lesson from Session 2 applied to collections.
7. Move to tuples. Open the [Lesson 8 exercise](https://simontreanor.github.io/Pyfun/playground/#code=bGV0IG5hbWVzID0gWyJhZGEiLCAiYWxhbiJdCmxldCBzY29yZXMgPSBbMTAsIDldCgpsZXQgbGluZSBwYWlyID0KICBtYXRjaCBwYWlyOgogICAgY2FzZSAobmFtZSwgc2NvcmUpOiA_CgpsZXQgbGluZXMgPSBMaXN0Lm1hcCBsaW5lIChMaXN0LnppcCBuYW1lcyBzY29yZXMpCgpwcmludCAoU3RyaW5nLmpvaW4gIiwgIiBsaW5lcykK).
   Show that `List.zip names scores` produced a list of pairs and that the single `case (name, score)`
   arm destructures each pair. Fill the hole with `f"{name}: {score}"` and Run.
8. Close by contrasting the three shapes with one line each: a record when parts have names, a list
   when there are many of the same thing, a tuple for a short fixed pairing like a name and a score.

## Assigned exercises

- In class: the [Lesson 6 exercise](../learn/06-records.md) and the
  [Lesson 7 exercise](../learn/07-collections.md), both driven from the demo.
- Homework: the [Lesson 8 exercise](../learn/08-tuples.md) (build one line per pair, then join). Ask
  students to note how the tuple pattern binds both parts at once.

## Common misconceptions

- "A record is a dictionary, so I can add keys later." Correction: a record's fields are fixed by its
  type. It compiles to a frozen dataclass, and every field must be present when you build it.
- "`{ opened with balance = 100 }` mutates `opened`." Correction: copy-and-update returns a new
  record. The original is untouched, which is why `opened.balance` still reads `0`.
- "`List.get 10 ns` will raise `IndexError` like Python indexing." Correction: there is no bracket
  indexing. `List.get` returns an `Option`, so an out-of-range index is `None`, a value you handle.
- "`(+)` and `(*) 2` are odd syntax I have to memorize." Correction: an operator in parentheses is
  just that operator as a function, and supplying one argument (`(*) 2`) is the same partial
  application from Session 1.
- "A one-element tuple `(x)` groups one value into a tuple." Correction: `(x)` is just `x` in
  parentheses. There is no one-element tuple. A real tuple has two or more elements, and `()` is
  unit.

## Timing

- 10 min: recap Session 2, then motivate choosing a data shape.
- 20 min: demo steps 1 to 4 (records and copy-and-update).
- 20 min: demo steps 5 to 6 (collections and totality).
- 15 min: demo steps 7 to 8 (tuples and the summary).
- 20 min: students work the Lesson 6 and Lesson 7 exercises in the playground.
- 5 min: recap and assign the Lesson 8 exercise as homework.

Answer keys: [answer-keys.md](answer-keys.md#session-3)
