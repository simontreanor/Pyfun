# Session 2: Making illegal states unrepresentable

## Objectives

By the end of this session, students can:

- Explain why `None` is a source of runtime bugs in Python and how `Option` moves that possibility
  into the type.
- Take an `Option` or `Result` apart with `match` and handle both shapes.
- Turn a Python exception into a `Result` value with `try` and read its `errorKind` and
  `errorMessage`.
- Define their own algebraic data type with several constructors.
- Read a non-exhaustive-match diagnostic, name the case it reports, and add it.

## Prerequisites

Session 1 (values, functions, inference). Python's own `match`/`case` helps but is not required.

## Demo script

The centerpiece is the exhaustiveness error naming the exact case the code forgot. Build up to it
with `Option` and `Result`, then land it on a hand-written ADT.

1. Open the [Lesson 3 exercise](https://simontreanor.github.io/Pyfun/playground/#code=bGV0IGxhYmVsIHMgPQogIG1hdGNoIFN0cmluZy50b0ludCBzOgogICAgY2FzZSBTb21lIG46IGYibnVtYmVyOiB7bn0iCgpwcmludCAobGFiZWwgIjciKQpwcmludCAobGFiZWwgIm5vcGUiKQo).
   Run `check`. The compiler reports `non-exhaustive match: None is not matched`. Ask the class what
   value is missing before revealing it, then add `case None: "no number"` and Run.
2. Make the point that in Python `int("nope")` raises, and a caller has to remember to guard it.
   Here the `None` case is not a convention to remember, it is a case the compiler refuses to let you
   skip.
3. Open the [Lesson 4 exercise](https://simontreanor.github.io/Pyfun/playground/#code=ZXh0ZXJuIHBhcnNlSW50OiBzdHJpbmcgLT4gaW50ID0gaW50CgpsZXQgZGVzY3JpYmUgcyA9CiAgbWF0Y2ggcGFyc2VJbnQgczoKICAgIGNhc2UgT2sgbjogZiJvazoge259IgogICAgY2FzZSBFcnJvciBlOiBmImJhZDoge2UuZXJyb3JLaW5kfSIKCnByaW50IChkZXNjcmliZSAiODgiKQpwcmludCAoZGVzY3JpYmUgImJhZCIpCg).
   It matches on `parseInt s` directly, so `check` reports a type mismatch: it found `int`, not a
   `Result`. Wrap the call as `try (parseInt s)` and Run. Show that the caught `ValueError` reaches
   the `Error` arm as data.
4. Draw the contrast: `Option` models "a value might be absent," `Result` models "an operation might
   fail with a reason." Both force the caller to answer for the unhappy path.
5. Now the centerpiece. Open the [Lesson 5 exercise](https://simontreanor.github.io/Pyfun/playground/#code=dHlwZSBMaWdodCA9CiAgfCBSZWQKICB8IEFtYmVyCiAgfCBHcmVlbgoKbGV0IGFjdGlvbiBsID0KICBtYXRjaCBsOgogICAgY2FzZSBSZWQ6ICJzdG9wIgogICAgY2FzZSBHcmVlbjogImdvIgoKcHJpbnQgKGFjdGlvbiBSZWQpCnByaW50IChhY3Rpb24gQW1iZXIpCnByaW50IChhY3Rpb24gR3JlZW4pCg).
   The `Light` type has three constructors but `action` answers for two. Run `check` and let the
   diagnostic speak: `non-exhaustive match: Amber is not matched`. The compiler names the exact
   forgotten state.
6. Add `case Amber: "wait"` and Run to get the three expected lines. Then delete `case Green:`
   instead and show the diagnostic now names `Green`. The point is that the checker tracks every
   constructor, not just the first gap.
7. Show the emitted Python for the working `Light` program. Point out the trailing
   `case _: raise RuntimeError("non-exhaustive match")` guard and explain that because the compiler
   already proved coverage, that guard can never fire. Exhaustiveness is checked before any Python
   exists.
8. Optional stretch: add a fourth constructor `| Flashing` to the type without touching `action`.
   The same diagnostic reappears naming `Flashing`, showing that widening a type reopens every match
   over it.

## Assigned exercises

- In class: the [Lesson 3 exercise](../learn/03-option.md) and the
  [Lesson 5 exercise](../learn/05-adts-and-match.md), both driven from the demo.
- Homework: the [Lesson 4 exercise](../learn/04-result.md) (wrap the parse in `try`). Ask students to
  read `errorKind` and `errorMessage` off the caught value in their answer.

## Common misconceptions

- "`Option` is just `None` with extra steps." Correction: the difference is enforcement. With
  `Option`, the compiler refuses to compile until the `None` case is handled, so the forgotten check
  cannot happen.
- "`try` here works like Python's `try`/`except` block." Correction: `try e` is an expression that
  produces a `Result` value. There is no jump to a handler. The failure is data you match on in the
  same expression.
- "An ADT is basically a Python `Enum`." Correction: an `Enum` lists bare names, but ADT
  constructors can carry data of different types (`Circle float`, `Rect float float`), and matching
  binds that data.
- "I can add a `case _:` catch-all to silence the exhaustiveness error." Correction: that works but
  throws away the guarantee. When you later add a constructor, a real `match` reopens with a
  diagnostic, while a catch-all silently swallows the new case.
- "The exhaustiveness check happens at runtime, like the `RuntimeError` guard." Correction: the check
  runs during type checking, before any Python is emitted. The runtime guard is a backstop that a
  proven-total match never reaches.

## Timing

- 10 min: recap Session 1, then motivate the `None` problem in Python.
- 20 min: demo steps 1 to 4 (`Option` and `Result`).
- 25 min: demo steps 5 to 8 (the exhaustiveness centerpiece on a hand-written ADT).
- 25 min: students work the Lesson 3 and Lesson 5 exercises in the playground.
- 10 min: recap and assign the Lesson 4 exercise as homework.

Answer keys: [answer-keys.md](answer-keys.md#session-2)
