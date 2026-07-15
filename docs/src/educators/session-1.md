# Session 1: Functional basics in Python's clothing

## Objectives

By the end of this session, students can:

- Bind values with `let` and explain why a binding is immutable by default.
- Read the type the compiler infers for a value without writing any annotations.
- Define a function with inline parameters and call it without parentheses or commas.
- Explain currying by showing that a partial call returns another function.
- Read a pipeline `a |> f |> g` and predict the equivalent nested Python call.

## Prerequisites

Python variables and functions. No functional programming background. This is the first session of
the unit.

## Demo script

The thread for this session is that Pyfun is just Python underneath. Keep the playground's Python
output panel visible the whole time so the class sees the emitted Python change as you type.

1. Open the Session 1 worked example in the playground:
   [functions and currying](https://simontreanor.github.io/Pyfun/playground/#code=bGV0IGFkZCBhIGIgPSBhICsgYgoKbGV0IGluYyA9IGFkZCAxCgpsZXQgZG91YmxlIHggPSB4ICogMgoKbGV0IHJlc3VsdCA9IDUgfD4gaW5jIHw-IGRvdWJsZQoKcHJpbnQgKGFkZCAyIDMpCnByaW50IChpbmMgMTApCnByaW50IHJlc3VsdAo).
   Press Run first so the class sees `5`, `11`, `12`. Ask them to predict what each line does.
2. Point at the Python panel. Show that `let add a b = a + b` became a plain `def add(a, b)`, and
   that `print (add 2 3)` became `print(add(2, 3))`. Make the point that there is no runtime library:
   the output is ordinary Python.
3. Highlight `let inc = add 1`. This is the currying moment. Show that it compiled to
   `inc = functools.partial(add, 1)`. Explain that a call missing an argument hands back a function
   rather than raising, which is what lets `inc` exist.
4. Highlight `let result = 5 |> inc |> double`. Show that it compiled to `double(inc(5))`. Read the
   pipe left to right ("start with 5, then inc, then double") and contrast with the inside-out
   Python nesting. This is the reading-order payoff of the pipe.
5. Break it live. Change `let inc = add 1` to `let inc = add 1 2 3`. The compiler reports too many
   arguments. Undo, then change `double x = x * 2` to `double x = x * "2"` and show the type error
   arriving before any Python is produced. Restore the working program.
6. Switch to the Lesson 1 idea of inference. Open the
   [Lesson 1 exercise](https://simontreanor.github.io/Pyfun/playground/#code=bGV0IGFwcGxlcyA9IDQKbGV0IG9yYW5nZXMgPSAzCmxldCBmcnVpdCA9IGFwcGxlcyArID9jb3VudApwcmludCAoZiJ0b3RhbCBmcnVpdDoge2ZydWl0fSIpCg).
   Run `check`, show that the hole `?count` reports the type `int` and lists names in scope, and fill
   it with `oranges`. This previews the hole workflow the class will lean on all unit.
7. To close the "just Python" thread, add a line `let label = "age: " + 36` to the Lesson 1 program
   and show the diagnostic that `+` is numeric and does not concatenate strings. Contrast with
   Python, where the same mistake would surface only at runtime.

## Assigned exercises

- In class: the [Lesson 1 exercise](../learn/01-values-and-inference.md) (fill the hole), done
  together in step 6 above.
- Homework: the [Lesson 2 exercise](../learn/02-functions-currying-pipes.md), finishing the pipeline
  so the result is 18. Students should read the hole's reported type before filling it.

## Common misconceptions

- "`let` is just assignment, so I can reassign it later." Correction: a `let` binds a value once and
  reassigning it is a compile error. Mutation is a separate, explicit feature that arrives in
  lesson 10.
- "`add 1` is a bug, because `add` takes two arguments." Correction: a partial call is intentional.
  It returns a function waiting for the remaining argument. That is currying.
- "The pipe must add overhead, like a stream or an iterator." Correction: `|>` is compile-time sugar.
  `5 |> inc |> double` compiles to `double(inc(5))` with no runtime layer.
- "No type annotations means the language is dynamically typed like Python." Correction: types are
  inferred and fully checked before any Python is emitted. The absence of annotations is a design
  choice, not the absence of static typing.
- "Calling `add 2 3` without commas looks like a syntax error." Correction: function application in
  Pyfun is by juxtaposition, so `add 2 3` is the call. Parentheses are only for grouping.

## Timing

- 10 min: framing the unit and what Pyfun is (compiles to readable Python).
- 30 min: demo script steps 1 to 5 (functions, currying, pipes, breaking it live).
- 20 min: demo script steps 6 to 7 (inference and the hole workflow).
- 20 min: students start the Lesson 2 exercise in the playground while you circulate.
- 10 min: recap and assign the Lesson 2 exercise as homework.

Answer keys: [answer-keys.md](answer-keys.md#session-1)
