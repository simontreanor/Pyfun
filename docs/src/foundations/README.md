# Foundations

This is where you start if you have never written a program before. Not "never written Pyfun,"
never written anything. You do not need to know Python. You do not need to know what a compiler
is. All you need is a web browser.

Pyfun happens to compile to Python, and that matters a lot once you can write real programs,
because it means everything you build here can use the huge world of Python libraries. But that
comes later. Right now, forget Python exists. You're learning to program, and Pyfun is the
language you're learning it in.

Each lesson teaches one idea, shows it working, and gives you one thing to do with it. Nothing
to install: every lesson links to the [playground](../playground/index.html), where the real
compiler checks your code as you type, and a Run button runs it for you.

## How the exercises work

The compiler marks your work. Most lessons give you a small program and ask you to change it, and
tell you exactly what output the finished program should print. As you go, the exercises ask
different things of you:

- Sometimes you just read a program and guess what it will print, then run it to see if you were
  right.
- Sometimes a piece is missing and you fill it in.
- Sometimes the program runs but prints the wrong thing, and you have to work out why.
- Sometimes the compiler refuses to build the program at all. Read what it says. It usually tells
  you exactly what is wrong and where.
- Later on, you'll be given a sentence describing what a program should do, and an empty editor.

That last kind feels like the biggest jump, but by the time you reach it you will have written
enough Pyfun that a blank page won't feel blank. Every exercise has a solution you can open if
you get stuck, but try the compiler's own messages first. They are usually enough.

## Running lessons your own way

The playground is enough for the whole course. If later you want Pyfun on your own computer:

```console
pip install pyfun-lang
pyfun run lesson.pyfun
```

That step is optional and can wait until you finish the course.

## The course at a glance

Lessons 1 to 7 teach the core building blocks: naming values, writing functions, making
decisions, and describing your own kinds of data. Lesson 7 is the one to pay attention to: you'll
write a program with a piece missing on purpose, and watch the compiler refuse to build it until
you've covered every case. That's the heart of what makes Pyfun different, and it shows up in
lesson 7, not lesson 17.

Lessons 8 to 14 build outward from there: lists, values that might be missing, values that might
be wrong, pairing things up, looking things up, and changing your mind about a value on purpose.

Lesson 15 is where Python shows up for the first time. By then you'll know enough Pyfun to call a
real, well-known Python library and understand exactly what you're doing when you do it. Lesson 16
is a capstone that puts everything together in one program.

When you finish, the [Learn Pyfun](../learn/README.md) track is waiting for you. It moves faster
and goes further, into computation expressions, units of measure, and multi-file projects.
