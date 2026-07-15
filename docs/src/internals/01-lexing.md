# 01 - Lexing

The lexer turns a source string into a flat stream of tokens. In Pyfun it also does one job that
a whitespace-insensitive language would skip: it runs an **offside rule** that turns indentation
into explicit block structure. Everything in this chapter lives in
[`src/lexer/`](https://github.com/simontreanor/Pyfun/blob/main/src/lexer/mod.rs), with the token
kinds in [`src/lexer/token.rs`](https://github.com/simontreanor/Pyfun/blob/main/src/lexer/token.rs).

## Tokens

The token set is a single Rust enum, `Tok`, covering literals, identifiers, the keywords, and
the operators and punctuation. Keyword recognition is a lookup, not a special case in the main
loop: the lexer reads an identifier, then asks whether that spelling is a keyword.

```rust
// src/lexer/token.rs
pub fn keyword(ident: &str) -> Option<Tok> {
    Some(match ident {
        "let" => Tok::Let,
        "match" => Tok::Match,
        "case" => Tok::Case,
        // ...
        _ => return None,
    })
}
```

> If you are new to Rust, this is the language's central idiom in miniature: an enum of variants
> plus an exhaustive `match`. The compiler checks that the `match` handles every possibility, and
> the `_ => return None` arm makes "anything else is an ordinary identifier" explicit.

## The offside rule

Pyfun uses indentation for statement sequencing and blocks, and the lexer is where that becomes
concrete. Its module documentation states the contract:

```rust
// src/lexer/mod.rs
//! Mostly whitespace-insensitive, with an **offside rule** that turns indentation
//! into block structure (outside any `()`/`{}` brackets, where line breaks are
//! always continuations). A layout stack of block columns drives three synthetic
//! tokens:
//! - [`Tok::Indent`] — a `let … =` whose body begins on a *deeper* line opens a
//!   block (the only block opener; `=` at bracket depth 0 primes it).
//! - [`Tok::Dedent`] — a line dedents below the current block's column, closing it.
//! - [`Tok::Sep`] — a line lands on the current block's column and the next token
//!   can begin a statement, separating two statements.
```

The mechanism is a stack of block columns and a `pending_block` flag. After an `=` (or another
tail-position opener) at bracket depth 0, the lexer primes `pending_block`; when the next line is
indented past the current column it pushes that column and emits `Indent`, and when a later line
dedents it pops columns and emits `Dedent`. A line that lands back on the block's own column, and
whose leading token can start a statement, gets a `Sep`. Lines that lead with a continuation token
(an infix operator, `|`, `then`/`else`/`with`) do not, which is what keeps a multi-line `match` or
`if` glued into one statement. Inside brackets the whole rule is suspended, because a line break in
`()` or `{}` is always a continuation.

In the running example, this is what lets the two `case` arms of `area` and the two top-level
`let` bindings be laid out on their own lines without any punctuation. The
[parse chapter](02-parsing.md) consumes these `Indent`/`Dedent`/`Sep` tokens as ordinary grammar.

## A unit annotation is not a token

The example writes `sqrt 16.0<m^2>`. It is tempting to think the lexer recognizes `<m^2>` as a
single unit token, but it does not. The lexer emits ordinary tokens, and a `<` is always the same
token whether it opens a unit or means less-than. The lexer test spells this out:

```rust
// src/lexer/mod.rs (tests)
assert_eq!(
    kinds("5<m>"),
    vec![Tok::Int(5), Tok::Lt, Tok::Ident("m".to_string()), Tok::Gt, Tok::Eof]
);
```

So `5<m>` and `5 < m` produce the *same* token kinds. The distinction is drawn later, in the
parser, by looking at spans: a `<` counts as a unit annotation only when it sits immediately after
the literal with no intervening whitespace. The parser's `maybe_unit` makes the call with
`self.cur_start() == self.prev_end()`, the F# rule that keeps units and comparison apart. Keeping
this out of the lexer is deliberate: the lexer stays a context-free tokenizer, and the one place
that needs positional context (adjacency) reads it off the spans the lexer already recorded.

## Holes are tokens

A typed hole, `?` or `?name`, is a first-class token, lexed like the `f"`/`r"` string prefixes
with any name read adjacently:

```rust
// src/lexer/token.rs
/// A typed hole in expression position: `?` (anonymous) or `?name` ...
Hole(Option<String>),
```

Doc comments get the same treatment: a `##` line at column zero lexes as `Tok::Doc`, while an
ordinary `#` line is discarded as trivia. Making both holes and doc comments real tokens means
they survive into the AST, so the checker can report a hole's inferred type (see the
[inference chapter](04-inference.md)) and hover can show a declaration's docs, rather than either
being lost as whitespace.

## Where you would add a new token

A new keyword is two edits: add the variant to `Tok` in
[`src/lexer/token.rs`](https://github.com/simontreanor/Pyfun/blob/main/src/lexer/token.rs) and a
line to `Tok::keyword`. A new operator or punctuation adds a variant and a branch in the
character-dispatch of `lex_one` in
[`src/lexer/mod.rs`](https://github.com/simontreanor/Pyfun/blob/main/src/lexer/mod.rs), taking
care to lex multi-character operators (like `<<`) before their single-character prefixes. Nothing
about the offside rule needs touching unless the token is itself a new block opener.
