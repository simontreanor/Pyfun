# Learn Pyfun

This course teaches functional programming through Pyfun, in short lessons written for people who
already know some Python. Each lesson introduces one idea, shows it working, and ends with an
exercise. You do not need to install anything: every exercise links to the
[playground](../playground/index.html), where the real compiler checks your code as you type and a
Run button executes it.

## How the exercises work

The compiler is the marker. Each exercise gives you starter code that is almost right, plus the
output the finished program should print. Three kinds of task repeat through the course:

- **Fill the hole.** The starter contains `?` or `?name` where an expression is missing. The
  compiler tells you the type it expects there, and suggests in-scope values that fit. Replace the
  hole with something of that type until the program compiles.
- **Make it total.** The starter forgets a case. The compiler names the exact value it cannot
  handle. Add the missing case.
- **Make it compile.** The starter contains one deliberate mistake (a type, effect, or unit
  error). Read the diagnostic and fix it.

When the program compiles, press Run and compare the output with the expected output shown in the
exercise. If they match, you are done. Every exercise also has a collapsible solution, but the
diagnostics usually get you there without it.

## Running lessons your own way

The playground is enough for the whole course. If you prefer your own machine:

```console
pip install pyfun-lang
pyfun run lesson.pyfun
```

Or work in a notebook with the Jupyter kernel (`pip install "pyfun-lang[jupyter]"`, then
`python -m pyfun_kernel.install`). Lesson 15 uses files and folders, so it needs the installed
compiler; everything else runs anywhere.

## The course at a glance

Lessons 1 to 8 cover the core: values, functions, and data types, and the habit the whole course
builds toward, which is letting the compiler prove your program handles every case. Lessons 9 to 11
cover the workflow: type-driven development with holes, deliberate mutation, and inferred effects.
Lessons 12 to 16 reach outward: calling Python libraries, computation expressions, units of
measure, multi-file projects, and a capstone that puts everything together.
