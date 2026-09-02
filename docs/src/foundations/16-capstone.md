# 16. Capstone

One program, built from what you already know. Nothing here is new.

Write a program about a small shelf of books:

1. A `Genre` type with three cases: `Fiction`, `NonFiction`, `Reference`.
2. A `Book` record with a `title` (string), a `genre` (`Genre`), and a `rating` (float).
3. A `describeGenre` function that turns a `Genre` into text (`"fiction"`, `"non-fiction"`,
   `"reference"`) with an exhaustive `match`.
4. A list of three books, one of each genre, with whatever ratings you like.
5. Print one line per book: its title, its genre as text, and its rating.
6. Declare `extern pure mean: List float -> float = statistics.mean`, and use it to print the
   shelf's average rating.
7. A function that looks up a book by title and reports either what it found or that no book by
   that name exists, using the pattern from lesson 9. Try it once on a title that's on the shelf,
   and once on a title that isn't.

There's no single right shelf of books, so match the shape of the output below rather than the
exact titles.

Expected output:

```console
['Dune (fiction): 4.5', 'Cosmos (non-fiction): 4.8', 'Atlas (reference): 4.2']
average rating: 4.5
found Cosmos, rated 4.8
no book called Nope
```

<details>
<summary>Show solution</summary>

```pyfun
extern pure mean: List float -> float = statistics.mean

type Genre =
  | Fiction
  | NonFiction
  | Reference

type Book = { title: string, genre: Genre, rating: float }

let describeGenre g =
  match g:
    case Fiction: "fiction"
    case NonFiction: "non-fiction"
    case Reference: "reference"

let books =
  [ Book { title = "Dune", genre = Fiction, rating = 4.5 },
    Book { title = "Cosmos", genre = NonFiction, rating = 4.8 },
    Book { title = "Atlas", genre = Reference, rating = 4.2 } ]

let describe b = f"{b.title} ({describeGenre b.genre}): {b.rating}"

print (List.map describe books)

let avgRating = mean (List.map (fun b -> b.rating) books)
print (f"average rating: {avgRating}")

let findBook title = List.find (fun b -> b.title == title) books

let report title =
  match findBook title:
    case Some b: f"found {b.title}, rated {b.rating}"
    case None: f"no book called {title}"

print (report "Cosmos")
print (report "Nope")
```

Every piece of this is a lesson you've already done: `Genre` and `Book` are lessons 6 and 5,
`describeGenre` is the exhaustive `match` from lesson 7, the list and its printing are lesson 8,
`mean` is lesson 15, and `findBook`/`report` are lesson 9. Nothing about writing a bigger program
needed a bigger idea, just more of the same small ones, put together.

You've built a program with your own data, your own rules for it, and a call out to a Python
library, and the compiler stood behind every line of it before any of it ran. From here, the
[Learn Pyfun](../learn/README.md) track picks up the pace and goes further, into computation
expressions, units of measure, and multi-file projects.
</details>
