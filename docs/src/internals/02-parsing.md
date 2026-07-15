# 02 - Parsing

The parser turns the token stream into an abstract syntax tree. Pyfun's parser is a hand-written
recursive-descent parser with precedence climbing for operators, living in
[`src/parser/mod.rs`](https://github.com/simontreanor/Pyfun/blob/main/src/parser/mod.rs); the AST
it builds is defined in
[`src/parser/ast.rs`](https://github.com/simontreanor/Pyfun/blob/main/src/parser/ast.rs).

## Recursive descent and precedence climbing

Each grammar production is a method. Statements and declarations are parsed by `parse_module`, and
an expression flows down a ladder of methods, one per precedence level, each calling the next
tighter level and looping to fold in operators at its own level. The entry point dispatches the
prefix-keyword forms, then hands the rest to the operator ladder:

```rust
// src/parser/mod.rs
fn parse_expr_head(&mut self) -> Result<Expr, ParseError> {
    match self.peek() {
        Tok::Fun => self.parse_fun(),
        Tok::If => self.parse_if(),
        Tok::Match => self.parse_match(),
        _ => self.parse_pipe(),
    }
}
```

From `parse_pipe` the ladder descends through composition, `or`, `and`, `not`, comparison,
additive, multiplicative, unary minus, application, and finally atoms:
`parse_pipe -> parse_compose -> parse_or -> parse_and -> parse_not -> parse_comparison ->
parse_additive -> parse_multiplicative -> parse_unary -> parse_application -> parse_atom`.
The ordering *is* the precedence table, written as call structure rather than a data table, and
associativity falls out of whether a level loops (left-associative) or recurses (right-associative).
Two details worth noting as you read: application binds tighter than every operator, so `area s` in
`acc + area s` groups as `acc + (area s)`, and comparison is chained Python-style, so
`parse_comparison` collects a run of links into one node rather than nesting them left-associatively.

## The span-carrying AST

Every node carries a source span so the checker and the editor can point at exact code. But spans
would wreck the parser's roundtrip tests, which compare a parsed tree against an
expected tree structurally. The AST solves this with a small, clever type: `NodeSpan` compares
equal to any other `NodeSpan`.

```rust
// src/parser/ast.rs
impl PartialEq for NodeSpan {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}
```

> This is a Rust idiom worth pausing on. Most of the AST derives `PartialEq` automatically with
> `#[derive(PartialEq)]`, which compares fields structurally. By hand-writing `PartialEq` for the
> one field type that should be ignored, the derived equality on every enclosing node skips spans
> for free. The tree keeps real source locations for diagnostics, and structural equality still
> means "same shape."

The expression AST itself is one enum, `ExprKind`, with variants for literals, `Var`, curried
`App`, `Fn`, `If`, `Match`, `Try`, records, tuples, holes, interpolated strings, and the rest.
Application is deliberately single-argument (`App { func, arg }`): a call like `List.fold f z xs`
is a left-nested spine of `App` nodes, which is exactly the curried view the type checker and
lowerer both want.

## Error recovery keeps the editor alive

The compiler uses the strict entry point, which fails on the first parse error, because it must
reject any broken program. The editor cannot afford that: one mistyped line should not blank out
navigation for the whole file. So the parser has a second, error-recovering entry point:

```rust
// src/parser/mod.rs
pub fn parse_recover(tokens: Vec<Token>) -> (Module, Vec<ParseError>) {
    Parser { tokens, pos: 0 }.parse_module_recover()
}
```

On a failed item it records the error, then `synchronize` skips forward to the next item boundary,
tracking `Indent`/`Dedent` so that a statement separator *inside* a broken block is not mistaken
for the top-level boundary:

```rust
// src/parser/mod.rs
fn synchronize(&mut self) {
    let mut depth = 0i32;
    loop {
        match self.peek() {
            Tok::Eof => return,
            Tok::Sep if depth <= 0 => return,
            Tok::Indent => depth += 1,
            Tok::Dedent => depth -= 1,
            _ => {}
```

So one broken `let` no longer hides the rest of the file; the items that parse still drive hover
and go-to-definition. The language server chapter (10) returns to how the front end reuses this.

## Parsing the running example

The parser has a canonical pretty-printer (`src/ast/`), and `pyfun parse` shows the AST by
printing it back. This is the real output for the running example:

```
type Shape = Circle float | Rect float float
let area s =
    match s:
        case (Circle r): ((3.14159 * r) * r)
        case (Rect w h): (w * h)
measure m
let shapes = [(Circle 2.0), ((Rect 3.0) 4.0)]
let total = (shapes |> ((List.fold (fun acc s -> (acc + (area s)))) 0.0))
let side = (sqrt 16.0<m^2>)
(print f"total {total}, side {side}")
```

The fully-parenthesized rendering makes the tree's shape visible. `3.14159 * r * r` prints as
`((3.14159 * r) * r)`, showing `*` is left-associative; the `total` line shows two things at once:
`|>` parses as an ordinary binary operator node (the [lowering chapter](07-lowering.md) is where it
vanishes), and its right operand `((List.fold (fun …)) 0.0)` is a two-deep `App` spine, showing
curried application; and `sqrt 16.0<m^2>` keeps the unit annotation attached to the literal,
showing the adjacency decision from the [lexing chapter](01-lexing.md) has already been made. Because the printer is canonical, feeding this output back to the parser produces
the same tree, which is what the roundtrip tests check.

## Where you would add a new expression form

A new operator adds a `BinOp` variant in
[`src/parser/ast.rs`](https://github.com/simontreanor/Pyfun/blob/main/src/parser/ast.rs) and a
branch at the right rung of the precedence ladder in
[`src/parser/mod.rs`](https://github.com/simontreanor/Pyfun/blob/main/src/parser/mod.rs); a new
prefix form (a keyword-led expression) adds an `ExprKind` variant and a case in `parse_expr_head`
or `parse_atom`. Whatever you add, give its node a span via the parser's `mk` helper and teach the
pretty-printer in `src/ast/` to print it, so the roundtrip test still holds.
