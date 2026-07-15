# Educator pack

This pack turns the 16-lesson [Learn Pyfun](../learn/README.md) track into a ready-made
functional-programming unit you can slot into an existing Python course. It is organized as five
class sessions with demo scripts, exercise assignments, and an instructor answer key. You can lift
it wholesale or adapt any part of it.

## Who this is for

Instructors of an intro or intermediate Python course who want to add a functional-programming unit
without leaving the Python ecosystem. The sessions assume you are comfortable teaching Python and
want to introduce ideas like immutability, algebraic data types, exhaustive matching, and effects
using a language that compiles to readable Python your students already recognize.

## What students need beforehand

Students should already know Python variables, functions, lists, and dictionaries. No functional
programming background is assumed. The unit introduces every functional idea from scratch and keeps
returning to the Python each construct compiles to, so students always have a familiar anchor.

## Time budget

The unit runs as five sessions of about 90 minutes of class time each, plus homework between
sessions. Each session doc includes a rough minute budget for the demo, discussion, and in-class
exercises.

If your schedule favors shorter sessions, split the unit into six. The natural split point is inside
Session 5: run lessons 12 and 13 as one session and lessons 14 through 16 as a sixth, with the
capstone (lesson 16) as the finale. That split is noted again in the Session 5 doc.

| Session | Title | Lessons |
| --- | --- | --- |
| 1 | Functional basics in Python's clothing | 1 to 2 |
| 2 | Making illegal states unrepresentable | 3 to 5 |
| 3 | Data modeling | 6 to 8 |
| 4 | The Pyfun workflow | 9 to 11 |
| 5 | Reaching the Python ecosystem | 12 to 16 |

Session docs:

- [Session 1: Functional basics in Python's clothing](session-1.md)
- [Session 2: Making illegal states unrepresentable](session-2.md)
- [Session 3: Data modeling](session-3.md)
- [Session 4: The Pyfun workflow](session-4.md)
- [Session 5: Reaching the Python ecosystem](session-5.md)
- [Answer keys](answer-keys.md)

## Running the material with zero setup

The whole unit runs in a browser. Every lesson and every demo step links to the
[playground](../playground/index.html), where the real compiler checks code as you type and a Run
button executes it. Nothing needs to be installed for students to follow along or to do the
exercises.

If you have lab machines and want students working locally, two options add a native workflow:

```console
pip install pyfun-lang
pyfun run lesson.pyfun
```

Or a notebook workflow with the Jupyter kernel:

```console
pip install "pyfun-lang[jupyter]"
python -m pyfun_kernel.install
```

Only lesson 15 (modules across files) needs the installed compiler, because it uses a folder of
source files. Every other lesson runs in the playground as it is.

## Licensing

This teaching material is licensed under
[Creative Commons Attribution 4.0 International (CC BY 4.0)](https://creativecommons.org/licenses/by/4.0/).
You are free to adapt, remix, and redistribute it, including for classroom and commercial use, as
long as you give attribution. Institutions should feel free to lift it wholesale into their own
courses.

The Pyfun code samples themselves remain under the Apache 2.0 license that covers the rest of the
repository.
