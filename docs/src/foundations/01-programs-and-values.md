# 1. Programs and values

Here is a complete Pyfun program.

```pyfun
let name = "Ada"
let age = 12
let favoriteNumber = 7

print name
print (f"{name} is {age}")
print (age + favoriteNumber)
```

Running it prints:

```console
Ada
Ada is 12
19
```

Each `let` line names a value. `let name = "Ada"` doesn't just store `"Ada"` somewhere, it gives
that text the name `name`, so anywhere below you can write `name` instead of typing `"Ada"` again.
The same happens for `age` and `favoriteNumber`. Once a value has a name, the rest of the program
is built by combining names: `age + favoriteNumber` adds the two numbers, and `f"{name} is {age}"`
builds a new string with `name` and `age` dropped into it.

`print` is how you see a value. Without it, a program still computes everything, but nothing shows
up. Try covering the third line with your hand and working out what `age + favoriteNumber` is
before you look at the output above. That's the whole game of reading a program: work out what
each name stands for, then work out what the expressions built from those names come to.

One more thing worth noticing: a name only ever means one thing. `name` is `"Ada"` on the first
line, and it is still `"Ada"` on the last line. Nothing in this program changes a name once it has
been given. You'll meet an exception to that much later, but it's rare, and you'll always be able
to see it happening.

## Exercise

Read this program and work out what it prints, line by line, before you run it.

```pyfun
let name = "Ada"
let books = 3
let pages = 200

print name
print books
print (books * pages)
```

[Open in the playground](https://simontreanor.github.io/Pyfun/playground/#code=bGV0IG5hbWUgPSAiQWRhIgpsZXQgYm9va3MgPSAzCmxldCBwYWdlcyA9IDIwMAoKcHJpbnQgbmFtZQpwcmludCBib29rcwpwcmludCAoYm9va3MgKiBwYWdlcykK)
to check your answer.

Expected output:

```console
Ada
3
600
```

<details>
<summary>Show solution</summary>

```pyfun
let name = "Ada"
let books = 3
let pages = 200

print name
print books
print (books * pages)
```

`name` and `books` print exactly what they were given. The last line multiplies `books` by
`pages`, three times two hundred, which is six hundred.
</details>
