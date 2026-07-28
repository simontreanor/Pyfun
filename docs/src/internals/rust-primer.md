# Learning Rust Through the Pyfun Compiler

A practical guide to Rust fundamentals using real examples from a production compiler written in Rust.

This primer is for readers new to Rust who want to understand the Pyfun compiler's source code. We'll walk through 18 core Rust concepts using excerpts from the actual compiler. If you're already familiar with Rust, skip ahead to the numbered chapters.

## 1. Ownership and Borrowing

Rust's superpower is memory safety without garbage collection. It achieves this through a system of **ownership** rules enforced at compile-time.

### The Three Rules
1. **Each value has exactly one owner** — the variable responsible for cleaning it up
2. **You can borrow (reference) a value** — temporarily access it without taking ownership
3. **Mutable borrows are exclusive** — only one `&mut` at a time; immutable `&` borrows can be many

### Real Example: The Lexer

From `src/lexer/mod.rs`:

```rust
struct Lexer<'a> {
    src: &'a [u8],        // Borrowed byte slice with lifetime 'a
    pos: usize,
    out: Vec<Token>,      // Owned vector
    errors: Vec<LexError>, // Owned vector
}

impl<'a> Lexer<'a> {
    fn new(source: &'a str) -> Self {
        Lexer {
            src: source.as_bytes(),  // Borrow the source
            pos: 0,
            out: Vec::new(),         // Create new owned vector
            errors: Vec::new(),
        }
    }
}
```

**What's happening:**
- `Lexer` borrows the input `source` for its entire lifetime (`'a`)
- The `'a` annotation means: "this reference is valid as long as `'a` is valid"
- `out` and `errors` are **owned** by the struct — when `Lexer` is dropped, these vectors are automatically freed
- `source` is **not** freed when `Lexer` is dropped; the original owner still owns it

**Why this matters:**
This pattern lets the compiler prevent use-after-free bugs. The type system guarantees that `src` won't be freed while `Lexer` exists.

### Mutable Borrows: The Lexer's Main Loop

```rust
fn run(mut self) -> (Vec<Token>, Vec<LexError>) {
    loop {
        let crossed_newline = self.skip_trivia();
        // ...
        if let Err(error) = self.lex_one() {
            self.errors.push(error);  // Mutate self.errors
        }
        // ...
    }
    (self.out, self.errors)  // Move ownership of out/errors back to caller
}
```

**Key points:**
- `self` is `mut`, so we can call methods that mutate `self`
- `self.errors.push(error)` mutates the vector — this is only allowed because `self` is uniquely owned
- At the end, we return ownership of `out` and `errors` to the caller

---

## 2. The `impl` Keyword: Adding Methods to Types

`impl` stands for **implement**. It's how you add methods (functions attached to a type) to that type. Think of it as "we're implementing behavior for this type."

### Basic impl Block

```rust
struct Point {
    x: i32,
    y: i32,
}

impl Point {
    fn new(x: i32, y: i32) -> Point {
        Point { x, y }
    }

    fn distance_from_origin(&self) -> f64 {
        (((self.x.pow(2) + self.y.pow(2)) as f64).sqrt())
    }
}

// Usage
let p = Point::new(3, 4);
println!("{}", p.distance_from_origin());  // Prints 5.0
```

**Reading this:**
- `impl Point` says: "We're adding methods to the `Point` type"
- `Point::new(...)` is an **associated function** (called on the type itself, not an instance)
- `p.distance_from_origin()` is a **method** (called on an instance)
- `&self` means the method borrows the point (doesn't modify it or take ownership)

### Multiple impl Blocks

You can split methods across multiple `impl` blocks:

```rust
impl Point {
    fn new(x: i32, y: i32) -> Point { Point { x, y } }
}

impl Point {
    fn translate(&mut self, dx: i32, dy: i32) {
        self.x += dx;
        self.y += dy;
    }
}
```

Both blocks add to the same type. This is useful for organizing code.

### Implementing a Trait

From `src/lib.rs`:

```rust
impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompileError::Lex(e) => write!(f, "lex error: {e}"),
            CompileError::Parse(e) => write!(f, "parse error: {e}"),
            CompileError::Type(e) => write!(f, "type error: {e}"),
            CompileError::Lower(e) => write!(f, "lowering error: {e}"),
        }
    }
}
```

`impl Trait for Type` means: "Implement this trait for this type."

After this, you can do:
```rust
let err = CompileError::Lex(...);
println!("{}", err);  // Calls the Display::fmt method
```

### Generic impl Blocks

```rust
impl<T> Vec<T> {
    fn len(&self) -> usize {
        // ...
    }
}
```

This adds a method `len()` to `Vec<T>` for *any* type `T`.

### Real Example: Unit Operations

From `src/types/mod.rs`:

```rust
impl Unit {
    fn dimensionless() -> Unit {
        Unit::default()
    }

    fn base(name: &str) -> Unit {
        let mut u = Unit::default();
        u.insert(Atom::Base(name.to_string()), 1);
        u
    }

    fn mul(&self, other: &Unit) -> Unit {
        let mut r = self.clone();
        for (a, e) in &other.factors {
            r.insert(a.clone(), *e);
        }
        r
    }

    fn is_dimensionless(&self) -> bool {
        self.factors.is_empty()
    }
}
```

**Methods:**
- `Unit::dimensionless()` — associated function (creates a default unit)
- `Unit::base("m")` — associated function (creates a unit from a base measure)
- `unit1.mul(&unit2)` — method (multiplies two units)
- `unit.is_dimensionless()` — method (checks if dimensionless)

---

## 3. Pattern Matching and Enums

Rust enums are **tagged unions** (like discriminated unions in TypeScript). Pattern matching on them is exhaustive — the compiler won't let you miss a case.

### Representing Errors with Enums

From `src/lib.rs`:

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum CompileError {
    Lex(lexer::LexError),
    Parse(parser::ParseError),
    Type(types::TypeError),
    Lower(lowering::LowerError),
}
```

This says: "A `CompileError` is one of four things: a lex error, a parse error, a type error, or a lowering error. Each variant can carry associated data."

### Pattern Matching: Exhaustive Handling

```rust
impl CompileError {
    pub fn span(&self) -> lexer::Span {
        match self {
            CompileError::Lex(e) => e.span,
            CompileError::Parse(e) => e.span,
            CompileError::Type(e) => e.span,
            CompileError::Lower(_) => lexer::Span::new(0, 0),
        }
    }
}
```

**The compiler enforces:**
1. **All cases are handled** — if you forget one variant, it won't compile
2. **The return type is consistent** — all arms return the same type (`lexer::Span`)
3. **No null pointers** — you can't have a `CompileError` that's somehow uninitialized

Compare to null-checking in other languages:
```javascript
// JavaScript — you can forget to check
if (error.type === 'Lex') { ... }
// What if error is null? What if type is undefined?
```

```rust
// Rust — you must handle all cases
match error {
    CompileError::Lex(e) => { ... }
    CompileError::Parse(e) => { ... }
    CompileError::Type(e) => { ... }
    CompileError::Lower(e) => { ... }
    // Compiler error if you forget one!
}
```

### Pattern Matching with Destructuring

From `src/main.rs`:

```rust
fn has_imports(module: &Module) -> bool {
    module
        .items
        .iter()
        .any(|i| matches!(i, Item::Import { .. }))
}
```

The `matches!` macro checks if an item matches a pattern without extracting the data. The `..` means "ignore the contents of this variant."

More explicit version:

```rust
for item in &module.items {
    if let Item::Import { name, span } = item {
        println!("Found import: {}", name);
    }
}
```

This extracts `name` and `span` only if `item` is an `Import`. If it's any other variant, the body is skipped.

---

## 4. The Result Type: Representing Failures

Instead of exceptions, Rust uses `Result<T, E>` — a type that says "this can either succeed with a value of type `T` or fail with an error of type `E`."

### Defining Results

From `src/lib.rs`:

```rust
pub fn parse(source: &str) -> Result<syntax::Module, CompileError> {
    let tokens = lexer::lex(source).map_err(CompileError::Lex)?;
    parser::parse(tokens).map_err(CompileError::Parse)
}
```

**Reading this:**
- `lexer::lex(source)` returns `Result<Vec<Token>, LexError>`
- `.map_err(CompileError::Lex)` converts `Err(LexError)` to `Err(CompileError::Lex(...))`
- `?` is the "propagate error" operator: if `lex` fails, return immediately with the error
- If `lex` succeeds, unwrap the `Vec<Token>` and assign to `tokens`

This is equivalent to exception handling:
```rust
// Rust (explicit)
match lexer::lex(source) {
    Ok(tokens) => { /* continue */ }
    Err(e) => return Err(CompileError::Lex(e)),
}
```

But the `?` operator makes it concise like exception handling while remaining explicit about error paths.

### Handling Results at Call Sites

From `src/main.rs`:

```rust
fn check(path: &str) -> ExitCode {
    let Some(source) = read(path) else {
        return ExitCode::FAILURE;
    };
    
    let module = match pyfun::parse(&source) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("{}", diagnostics::render(&source, Level::Error, &e.message(), e.span()));
            return ExitCode::FAILURE;
        }
    };
    // Continue with module...
}
```

**Pattern: `let Some(...) else`**
- If `read(path)` returns `Some(source)`, bind it and continue
- Otherwise, execute the `else` block (early return with failure)

This is Rust's way of handling nullable values without `null` — either a value is `Some(x)` or it's `None`.

---

## 5. Type Traits: Shared Behavior

A **trait** is like an interface: it defines a set of methods that types can implement.

### Simple Trait: Display

```rust
impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompileError::Lex(e) => write!(f, "lex error: {e}"),
            CompileError::Parse(e) => write!(f, "parse error: {e}"),
            CompileError::Type(e) => write!(f, "type error: {e}"),
            CompileError::Lower(e) => write!(f, "lowering error: {e}"),
        }
    }
}

impl std::error::Error for CompileError {}
```

This says:
- `CompileError` can be formatted as a string (supports `format!("{}", error)` and `print!("{}", error)`)
- `CompileError` implements the standard `Error` trait (so it can be used anywhere an error is expected)

### Generic Traits: Handling Any Error Type

From `src/main.rs`, here's where the `Display` trait proves valuable:

```rust
fn main() -> ExitCode {
    match pyfun::compile(&source) {
        Ok(python) => { /* ... */ }
        Err(e) => {
            eprintln!("{}", diagnostics::render(&source, Level::Error, &e.message(), e.span()));
            ExitCode::FAILURE
        }
    }
}
```

Because `CompileError` implements the `Display` trait, we can call `e.message()` uniformly on any error, whether it came from the lexer, parser, type-checker, or lowerer.

---

## 6. Generics and Type Parameters

Generics let you write code that works for many types while staying type-safe.

### Generic Data Structures

From `src/parser/ast.rs`:

```rust
pub enum TypeExpr {
    Con(String, NodeSpan, Vec<TypeExpr>),           // Vec of TypeExpr
    Fun(Box<TypeExpr>, Box<TypeExpr>, Vec<String>), // Nested TypeExpr
    Tuple(Vec<TypeExpr>),
}
```

This is recursive: `TypeExpr` contains `Vec<TypeExpr>`. The compiler knows the size only because `Vec` is a heap-allocated pointer, so a `TypeExpr` is always a fixed size.

### Lifetimes: Tying References Together

Lifetimes are **generic parameters for references**. They connect the lifetime of a borrow to the lifetime of the data being borrowed.

From `src/lexer/mod.rs`:

```rust
struct Lexer<'a> {
    src: &'a [u8],
}

impl<'a> Lexer<'a> {
    fn new(source: &'a str) -> Self { ... }
}
```

**Reading this:**
- `'a` is a lifetime parameter (any valid lifetime, written as `'variable_name`)
- `&'a [u8]` means "a reference to a byte slice valid for lifetime `'a`"
- The `impl<'a>` says: "implement these methods for any lifetime `'a`"

This lets the compiler check: "Does the `Lexer` live longer than the source? If so, compilation fails."

Without lifetimes:
```rust
struct BadLexer {
    src: &[u8],  // Compiler error! How long should the reference live?
}
```

Rust won't let you write this because it can't guarantee the reference won't outlive the source.

---

## 7. Memory Layout: Stack vs Heap

Rust gives you fine-grained control over where values live.

### Stack-Allocated Structs

```rust
#[derive(Debug, Clone, Copy)]
pub struct NodeSpan(pub Span);
```

- `#[derive(Debug)]` auto-implements a debug printer
- `#[derive(Clone)]` auto-implements a copy operation
- `Copy` means the value is automatically copied when moved (for tiny values like pointers)
- `NodeSpan` lives on the stack if it's a local variable — cheap to create/destroy

### Heap-Allocated Collections

```rust
pub struct Module {
    pub items: Vec<Item>,
}
```

- `Vec` is a heap-allocated vector (like `ArrayList` in Java or a Python list)
- When `Module` is dropped, the `Vec` is automatically freed
- This is zero-cost abstraction: no garbage collector, just deterministic cleanup

### Heap Allocation with Box

```rust
pub enum TypeExpr {
    Fun(Box<TypeExpr>, Box<TypeExpr>, Vec<String>),
}
```

- `Box<TypeExpr>` is a heap-allocated `TypeExpr`
- We use `Box` here because `TypeExpr` is recursive — if we used `TypeExpr` directly, the size would be infinite
- `Box` gives us a pointer (fixed size) to the actual `TypeExpr` on the heap

---

## 8. Closures and Higher-Order Functions

Closures are functions that capture variables from their environment.

### Simple Closures

From `src/types/mod.rs`:

```rust
pub fn float_literal_spans(types: &[types::TypeSpan]) -> std::collections::HashSet<lexer::Span> {
    types
        .iter()
        .filter(|t| t.ty == "float" || t.ty.starts_with("float<"))
        .map(|t| t.span)
        .collect()
}
```

- `|t| t.ty == "float"` is a closure taking one parameter `t` and returning a bool
- `|t| t.span` is a closure that returns `t.span`
- These closures don't capture any external variables (they only use their parameter)

### Closures That Capture Environment

```rust
let parse_errors: Vec<_> = parse_errors
    .iter()
    .map(|e| to_type_error(&CompileError::Parse(e.clone())))
    .collect();
```

The closure `|e| to_type_error(&CompileError::Parse(...))` captures nothing from the environment but creates a new value that includes `CompileError::Parse`.

### Mutable Closures

```rust
let mut result = Vec::new();
items.iter().for_each(|item| {
    result.push(process(item));  // Captures and mutates result
});
```

The closure captures `result` mutably, so it can push to it. This requires `result` to be declared `mut`.

---

## 9. Error Handling Patterns

### The Question Mark Operator

```rust
pub fn compile(source: &str) -> Result<String, CompileError> {
    let module = parse(source)?;  // If parse fails, return the error immediately
    let (mut errors, types, holes, ordered) = types::check_collecting(&module);
    if !errors.is_empty() {
        return Err(CompileError::Type(errors.remove(0)));  // Explicit early return
    }
    // ... continue
}
```

The `?` operator is syntactic sugar for:
```rust
let module = match parse(source) {
    Ok(m) => m,
    Err(e) => return Err(e),
};
```

### Checked/Unchecked Indexing

From `src/main.rs`:

```rust
while i < args.len() {
    match args[i].as_str() {
        "-o" | "--output" => {
            i += 1;
            out = Some(args.get(i).ok_or("`-o` needs a path")?.clone());
        }
        // ...
    }
}
```

**Safe indexing:**
- `args[i]` — panics if out of bounds (use when you're sure it's safe)
- `args.get(i)` — returns `Option<T>`: `Some(value)` if in bounds, `None` if not
- `.ok_or(...)` converts `None` to an `Err`, then `?` propagates it

---

## 10. Modules and Visibility

The module system organizes code into namespaces.

### File Structure

From the Pyfun `src/` directory structure:
```
src/
├── lib.rs          (defines what's public from the whole crate)
├── main.rs         (CLI binary)
├── lexer/
│   ├── mod.rs      (defines the lexer module)
│   └── token.rs    (sub-module of lexer)
├── parser/
│   ├── mod.rs
│   └── ast.rs
└── types/
    └── mod.rs
```

### Visibility Control

From `src/lib.rs`:

```rust
pub mod ast;           // Public module, accessible to users of the crate
pub mod desugar;
pub mod diagnostics;
pub mod lexer;
pub mod lsp;
pub mod parser;
pub mod project;
pub mod python_emitter;
pub mod types;

pub use parser::ast as syntax;  // Re-export as `syntax` for convenience
```

- `pub mod name` — the module is public
- `pub use` — re-export something under a new name
- Without `pub`, a module/function is private to the crate

### Functions and their Visibility

```rust
pub fn parse(source: &str) -> Result<syntax::Module, CompileError> {
    // ...
}

fn to_type_error(error: &CompileError) -> types::TypeError {
    // Private function, used only within this module
}
```

---

## 11. Deriving Traits

The `#[derive(...)]` attribute auto-implements common traits.

From `src/parser/ast.rs`:

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum CompileError {
    Lex(lexer::LexError),
    Parse(parser::ParseError),
    Type(types::TypeError),
    Lower(lowering::LowerError),
}
```

- `#[derive(Debug)]` — auto-generates a debug printer (for `{:?}` in format strings)
- `#[derive(Clone)]` — auto-generates a clone method (deep copy)
- `#[derive(PartialEq)]` — auto-generates equality comparison

These traits are derived only for types whose fields also implement them.

---

## 12. Smart Pointers and Reference Counting

### Box: Unique Ownership

```rust
pub enum TypeExpr {
    Fun(Box<TypeExpr>, Box<TypeExpr>, Vec<String>),
}
```

`Box<T>` means: "I own a single heap-allocated `T`. When I'm dropped, the `T` is freed."

### Rc: Shared Ownership (Single-Threaded)

```rust
// Not used much in Pyfun, but common in other Rust programs:
use std::rc::Rc;

let shared = Rc::new(some_data);
let clone1 = Rc::clone(&shared);  // Increment reference count
let clone2 = Rc::clone(&shared);  // Increment reference count
// When clone2, clone1, and shared are all dropped, the data is freed
```

---

## 13. Iterators and Functional Chains

Rust's iterator API is lazy: nothing happens until you consume the iterator.

### Lazy Evaluation

```rust
pub fn float_literal_spans(types: &[types::TypeSpan]) -> std::collections::HashSet<lexer::Span> {
    types
        .iter()              // Start iterating (lazy)
        .filter(|t| t.ty == "float" || t.ty.starts_with("float<"))  // Filter predicate (lazy)
        .map(|t| t.span)     // Transform (lazy)
        .collect()           // Consume the iterator (executes the chain)
}
```

Nothing runs until `.collect()`. The compiler optimizes this chain into a single efficient loop.

### Collecting into Different Types

```rust
// Collect into a Vec
let vec: Vec<_> = items.iter().map(transform).collect();

// Collect into a HashSet
let set: HashSet<_> = items.iter().map(transform).collect();

// Collect into a HashMap
let map: HashMap<K, V> = items.iter().map(|(k, v)| (k, v)).collect();
```

The type annotation tells `.collect()` what to produce.

---

## 14. Error Messages and Diagnostics

### Using Display and Debug

```rust
pub struct LexError {
    pub message: String,
    pub span: Span,
}

impl std::fmt::Display for LexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (at {}..{})", self.message, self.span.start, self.span.end)
    }
}
```

- `Display` (`{}`) — a user-friendly, concise error
- `Debug` (`{:?}`) — a verbose, developer-friendly error (auto-derived)

```rust
println!("{}", lex_error);     // Calls Display: "unexpected character (at 42..43)"
println!("{:?}", lex_error);   // Calls Debug: "LexError { message: \"unexpected character\", span: Span { start: 42, end: 43 } }"
```

---

## 15. Syntax Fundamentals

### Variables and Mutability

```rust
let x = 5;           // Immutable by default
let mut y = 5;       // Mutable variable
const MAX: usize = 100;  // Compile-time constant

let x = "hello";     // Shadowing: rebind x to a new value (different type OK)
```

**Rust is immutable-by-default** — you must explicitly opt-in to mutability with `mut`. This makes it easier to reason about which values change.

### Type Annotations

```rust
let x: i32 = 5;                    // Type annotation (usually optional—inferred)
let items: Vec<Item> = Vec::new(); // Generic type with type parameter
let f: fn(i32) -> i32 = |x| x * 2; // Function pointer type
let r: &str = "hello";             // Reference to a string literal
```

Type annotations are optional when the compiler can infer them, but required in some contexts (like function parameters and return types).

### Semicolons and Expressions

```rust
let x = {
    let y = 3;
    y + 1   // No semicolon—this is an expression that returns 4
};
assert_eq!(x, 4);

let z = {
    let y = 3;
    y + 1;  // Semicolon—this turns it into a statement, returns ()
};
assert_eq!(z, ());
```

**Rust distinguishes statements from expressions:**
- **Expressions** return a value (no semicolon at the end)
- **Statements** perform an action and return nothing (semicolon at the end)

This is why `let x = if cond { 5 } else { 6 };` works — the `if` is an expression.

### Function Declarations

```rust
fn add(a: i32, b: i32) -> i32 {
    a + b  // Return the expression (no semicolon)
}

fn print_and_return(msg: &str) -> String {
    println!("{}", msg);
    msg.to_string()
}

fn side_effect() {
    println!("Hello!");
    // Returns ()
}
```

**Rust functions always return a value:**
- Explicit `return` statement (with semicolon): `return x;`
- Final expression (without semicolon): `x`
- No explicit return → returns `()` (unit type)

### Operators

```rust
// Arithmetic
let sum = 5 + 6;
let product = 12 / 3;
let remainder = 7 % 3;
let power = 2_i32.pow(3);

// Comparison
let x = 5;
let is_greater = x > 3;      // true
let is_equal = x == 5;       // true
let in_range = x >= 3 && x <= 7;

// Logical
let a = true || false;   // OR
let b = true && false;   // AND
let c = !true;           // NOT

// String/Collection operators
let s = "Hello".to_string() + " " + "World";
let v = vec![1, 2, 3];
let first = v[0];    // Index (panics if out of bounds)
```

### String Types

```rust
let s1 = "hello";          // &str — string literal (immutable, fixed size)
let s2 = String::from("hello");  // String — owned, mutable, heap-allocated
let s3 = "hello".to_string();    // String — owned copy

let mut s = String::new();
s.push_str("hello");       // Append to mutable String
s.push('!');               // Append a character

// String interpolation
let name = "Alice";
let greeting = format!("Hello, {}!", name);
```

**Key distinction:**
- `&str` — a view into existing string data (can't modify)
- `String` — owns the string data (can modify, can grow)

### Collections

```rust
// Vectors (dynamic arrays)
let v: Vec<i32> = vec![1, 2, 3];
let mut items = Vec::new();
items.push(1);
items.push(2);
let first = items[0];
let maybe_first = items.get(0);  // Returns Option

// HashMaps (dictionaries)
use std::collections::HashMap;
let mut map = HashMap::new();
map.insert("key", "value");
map.get("key");  // Returns Option<&V>

// HashSets (unique values)
use std::collections::HashSet;
let mut set = HashSet::new();
set.insert(1);
set.insert(2);
set.contains(&1);  // Returns bool
```

### Control Flow

```rust
// if expressions (return values)
let x = if condition { 5 } else { 6 };

// match (exhaustive pattern matching)
match value {
    1 => println!("one"),
    2 | 3 => println!("two or three"),
    n if n > 10 => println!("big number"),
    _ => println!("something else"),
}

// loops
for i in 0..5 {
    println!("{}", i);  // Prints 0, 1, 2, 3, 4
}

let mut count = 0;
while count < 5 {
    count += 1;
}

loop {
    if should_break { break; }
}

// Named loop breaks
'outer: for i in 0..3 {
    for j in 0..3 {
        if i == 1 && j == 1 {
            break 'outer;  // Break from outer loop
        }
    }
}
```

### Ranges

```rust
let r1 = 0..5;      // [0, 1, 2, 3, 4] — excludes end
let r2 = 0..=5;     // [0, 1, 2, 3, 4, 5] — includes end
let r3 = 0..;       // [0, 1, 2, ...] — infinite range

for i in 0..3 {
    println!("{}", i);
}
```

### Tuples

```rust
let tuple = (5, "hello", true);
let (a, b, c) = tuple;  // Destructure
let first = tuple.0;    // Access by index
```

### Struct Literals

```rust
struct Point { x: i32, y: i32 }

let p = Point { x: 5, y: 10 };
let Point { x, y } = p;  // Destructure

// Shorthand (if variable name matches field name)
let x = 5;
let y = 10;
let p = Point { x, y };  // Same as Point { x: x, y: y }
```

### Comments

```rust
// Single-line comment

/* Multi-line
   comment */

/// Doc comment for the item below (exported in documentation)
fn documented() {}

//! Module-level doc comment (exported in documentation)
```

### Method Chaining Syntax

From `src/lib.rs`:

```rust
let syntax_errors: Vec<_> = lex_errors
    .iter()
    .map(|e| to_type_error(&CompileError::Lex(e.clone())))
    .chain(
        parse_errors
            .iter()
            .map(|e| to_type_error(&CompileError::Parse(e.clone()))),
    )
    .collect();
```

Methods are called with dot notation, and chains can span multiple lines. The `.` operator automatically dereferences and borrows as needed.

### The `?` Operator (Try Operator)

```rust
fn parse_compile_args(args: &[String]) -> Result<(&str, Option<String>, PyTarget), String> {
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--target" => {
                i += 1;
                target = match args.get(i).map(String::as_str) {
                    Some("3.11") => PyTarget::Py311,
                    Some("3.12") => PyTarget::Py312,
                    Some(other) => return Err(format!("`--target` must be 3.11 or 3.12, got `{other}`")),
                    None => return Err("`--target` needs a version (3.11 or 3.12)".to_string()),
                };
            }
            // ...
        }
        i += 1;
    }
    Ok((path.ok_or("`compile` needs a file path")?, out, target))
}
```

The `?` operator:
- On `Result<T, E>`: if `Err`, return immediately with that error; if `Ok(v)`, unwrap to `v`
- On `Option<T>`: if `None`, return immediately with an error; if `Some(v)`, unwrap to `v`

This makes error handling concise without try-catch verbosity.

---

## 16. Common Patterns

### The `match` Guard

```rust
match item {
    Item::Expr(e) if is_side_effect(&e) => {
        // Only match if is_side_effect returns true
    }
    _ => { /* default */ }
}
```

### Destructuring in Function Parameters

```rust
fn render_project_error(entry: &str, error: &ProjectError) -> ExitCode {
    match error {
        ProjectError::Compile { name, error } => {
            // Extract name and error from the variant
            eprintln!("error: in module `{name}`: {}", error.message())
        }
        other => eprintln!("error: {other}"),
    }
}
```

### Early Returns with Explicit Unwrapping

```rust
let Some(source) = read(path) else {
    return ExitCode::FAILURE;
};
```

This pattern (introduced in Rust 1.65) is cleaner than nested if-let.

---

## 17. Key Takeaways

1. **Ownership is enforced at compile-time** — no garbage collector, no panics (usually)
2. **Rust is explicit about failures** — use `Result` and `Option` instead of exceptions/null
3. **Pattern matching is exhaustive** — the compiler ensures you handle all cases
4. **Generics are monomorphic** — each generic is specialized at compile-time (no runtime overhead like Java generics)
5. **Lifetimes prevent dangling references** — the compiler checks that references don't outlive their data
6. **Traits provide shared behavior** — interfaces without inheritance
7. **Iterators are lazy** — chains of operations optimize into single loops
8. **The type system is your friend** — compile-time errors are vastly better than runtime panics

Rust is harder to learn than Python or JavaScript, but the payoff is correctness: if it compiles, it's very likely to work correctly. The compiler is famous for being strict but fair — once you understand the rules, the error messages guide you to the fix.

---

## Next Steps

Now that you're familiar with Rust fundamentals, dive into the numbered chapters to see how these concepts come together in a real compiler. Each chapter focuses on one stage of the pipeline and calls out Rust idioms as they appear in context.
