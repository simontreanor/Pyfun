# 5. Describing a thing: records

So far every value has been on its own: one name, one number or one string. A record lets you
bundle a few values together under one name, each piece with its own label.

```pyfun
type Card = { rank: string, suit: string }

let queen = Card { rank = "Queen", suit = "Hearts" }
let reSuited = { queen with suit = "Spades" }

print queen.rank
print queen.suit
print reSuited.suit
print queen
```

```console
Queen
Hearts
Spades
Card(rank='Queen', suit='Hearts')
```

`type Card = { rank: string, suit: string }` describes the shape once: a `Card` always has a
`rank` and a `suit`, and both are text. `Card { rank = "Queen", suit = "Hearts" }` builds one, and
a dot reads a field back out, `queen.rank`, `queen.suit`.

The second line is worth slowing down on. `{ queen with suit = "Spades" }` does not change
`queen`. It builds a brand new card, equal to `queen` in every field except `suit`. That's why
`queen.suit` is still `"Hearts"` if you print it again after this line: `queen` never changed, you
just built something new out of it. Every value you've met so far has worked this way, and records
keep it up: once something is built, it stays exactly as built.

## Exercise

This won't compile. `pages` is declared as a whole number, `int`, but the starter gives it text
instead. Run `pyfun check` and read what it tells you, then fix the field so it's a real number.

```pyfun
type Book = { title: string, pages: int }

let dune = Book { title = "Dune", pages = "412" }

print dune.title
print dune.pages
```

The checker reports:

```console
error: type mismatch: expected int, found string
 --> 3:43
  |
3 | let dune = Book { title = "Dune", pages = "412" }
  |                                           ^^^^^
```

[Open in the playground](https://simontreanor.github.io/Pyfun/playground/#code=dHlwZSBCb29rID0geyB0aXRsZTogc3RyaW5nLCBwYWdlczogaW50IH0KCmxldCBkdW5lID0gQm9vayB7IHRpdGxlID0gIkR1bmUiLCBwYWdlcyA9ICI0MTIiIH0KCnByaW50IGR1bmUudGl0bGUKcHJpbnQgZHVuZS5wYWdlcwo)

Expected output:

```console
Dune
412
```

<details>
<summary>Show solution</summary>

```pyfun
type Book = { title: string, pages: int }

let dune = Book { title = "Dune", pages = 412 }

print dune.title
print dune.pages
```

Dropping the quotes around `412` makes it a number instead of text, which is what the `Book` type
promised for `pages`.
</details>
