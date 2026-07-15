# Session 4: The Pyfun workflow

## Objectives

By the end of this session, students can:

- Use a typed hole to sketch a program and let the compiler report the type and fitting names at
  each gap.
- Fill a pipeline inward from the types the holes report, without guessing function signatures.
- Declare a deliberate local accumulator with `let mut` and reassign it with `<-`.
- Explain why immutability is the default and mutation is opt-in and visible.
- Read an inferred effect, assert purity with `let pure`, and fix a failed purity assertion by
  moving the effect to the call site.

## Prerequisites

Sessions 1 through 3 (values, functions, `match`, records, collections). Students have already seen
holes in passing when the earlier exercises reported a hole type.

## Demo script

The thread is the day-to-day loop of writing Pyfun: sketch with holes, let types guide the fill,
reach for mutation only when it is genuinely clearest, and let the compiler track effects for you.

1. Open the [Lesson 9 exercise](https://simontreanor.github.io/Pyfun/playground/#code=bGV0IHNjb3JlcyA9IE1hcC5hZGQgImFkYSIgMTAgKE1hcC5hZGQgImFsYW4iIDkgTWFwLmVtcHR5KQoKbGV0IHJlcG9ydCBuYW1lID0KICBNYXAudHJ5RmluZCBuYW1lIHNjb3JlcwogIHw-IE9wdGlvbi53aXRoRGVmYXVsdCA_bWlzc2luZwogIHw-ID9yZW5kZXIKCnByaW50IChyZXBvcnQgImFkYSIpCnByaW50IChyZXBvcnQgImN5IikK).
   Run `check`. Two holes report at once. Read the `?render` note aloud: it has type `int -> 'a` and
   suggests `String.fromInt` among the fits. This is type-driven development in one screen.
2. Fill `?missing` with `0` and `?render` with `String.fromInt`, then Run to get `10` and `0`. Make
   the point that you never looked up a signature: each hole reported exactly what belonged there.
3. Emphasize that a hole blocks compilation by design. Nothing runs until every blank is filled, so a
   half-written program cannot accidentally execute. Contrast with a Python `pass` or `TODO`, which
   runs and fails later.
4. Move to mutation. Open the [Lesson 10 exercise](https://simontreanor.github.io/Pyfun/playground/#code=bGV0IGJhbGFuY2Ugc3RhcnQgPQogIGxldCBhY2MgPSBzdGFydAogIGFjYyA8LSBhY2MgKyAxMDAKICBhY2MgPC0gYWNjIC0gMzAKICBhY2MgPC0gYWNjICsgNTAKICBhY2MKCnByaW50IChiYWxhbmNlIDApCg).
   It reassigns `acc` with `<-` but declared it with a plain `let`. Run `check` and read the
   diagnostic: it says `acc` is immutable and points to `let mut`. Add `mut` and Run to get `120`.
5. Show the emitted Python: the block became the plain statement sequence you would write by hand,
   `total = ...` reassigned in place. The `<-` arrow is what makes mutation visible in the source,
   and it is scoped to the block. Note there is no `for` or `while`, so `let mut` is for a genuine
   accumulator, not for iterating a collection.
6. Move to effects. Open the [Lesson 11 exercise](https://simontreanor.github.io/Pyfun/playground/#code=bGV0IHB1cmUgZ3JlZXQgbmFtZSA9CiAgcHJpbnQgKGYiSGVsbG8sIHtuYW1lfSIpCiAgbmFtZQoKbGV0IGdyZWV0aW5nID0gZ3JlZXQgImFkYSIK).
   `greet` is declared `pure` but prints inside its body. Run `check` and read the diagnostic:
   `greet is declared pure but performs io`, pointing at the `print` line.
7. Fix it the right way. Make `greet` return the string (`let pure greet name = f"Hello, {name}"`)
   and move the `print` to the call site (`print (greet "ada")`). Run. Make the point: the fix is to
   separate the calculating part, which stays provably pure, from the part that talks to the world,
   which now says so.
8. To reinforce inference, add a second function that calls `greet` and prints, and show that its
   `io` effect is inferred without any annotation, while a `let pure` on it would be rejected. Effect
   tracking propagates outward on its own.

## Assigned exercises

- In class: the [Lesson 9 exercise](../learn/09-typed-holes.md) and the
  [Lesson 10 exercise](../learn/10-mutation.md), both driven from the demo.
- Homework: the [Lesson 11 exercise](../learn/11-effects.md) (make `greet` pure by returning the
  string and printing at the call site).

## Common misconceptions

- "A hole is like `pass`, so the program still runs with a blank." Correction: a hole blocks
  compilation. Nothing executes until every hole is filled, which is what makes it safe to sketch.
- "I have to know a function's signature before I can use it." Correction: the hole reports the type
  it expects and lists in-scope names that fit. You fill inward from what the compiler tells you.
- "`let mut` means I should reach for it whenever I loop." Correction: there is no `for` or `while`.
  Iterating a collection is `List.fold` and friends. `let mut` is for a local accumulator built from
  a few explicit steps.
- "`acc <- x` is the same as `acc = x`, just different syntax." Correction: `<-` reassigns an
  existing `mut` binding and is visible on purpose. A plain `let` cannot be reassigned at all.
- "Marking something `pure` makes it pure." Correction: `let pure` is an assertion the compiler
  checks. If the body performs an effect, it is a compile error. You cannot declare away an effect.

## Timing

- 10 min: recap Session 3, then frame the write-with-holes workflow.
- 20 min: demo steps 1 to 3 (typed holes and type-driven development).
- 20 min: demo steps 4 to 5 (deliberate mutation).
- 20 min: demo steps 6 to 8 (inferred effects and `let pure`).
- 15 min: students work the Lesson 9 and Lesson 10 exercises in the playground.
- 5 min: recap and assign the Lesson 11 exercise as homework.

Answer keys: [answer-keys.md](answer-keys.md#session-4)
