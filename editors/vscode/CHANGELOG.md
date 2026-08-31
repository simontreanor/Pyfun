# Changelog

The extension's version tracks the compiler's: every Pyfun release ships a matching extension, so
`pyfun --version` and the installed client always agree. A release with no client changes says so.

## 0.8.1

- Version aligned to the compiler's 0.8.1 release. No client changes; the compiler it drives fixes a
  silent miscompile where a module `mut` assigned inside an `async`/`seq`/`result` block bound a
  local instead of flipping the module binding, and generalizes unit-parameter erasure so a `unit`
  argument is dropped wherever it sits in an extern's parameter list, not only when it is the sole
  argument. One breaking change: an extern's mid-list `unit` argument no longer arrives as Python
  `None`; it is dropped from the call entirely.

## 0.8.0

- Version aligned to the compiler's 0.8.0 release. No client changes; the compiler it drives adds
  the `Async`/`Task`/`Encode` modules and `Decode.auto`, `for` and bare expression items in
  computation expressions, async self tail calls that loop instead of recursing, and `pyfun bundle`
  for a static Pyodide page. One breaking change: an `->{async}` extern must return `Async`.

## 0.7.0

- Version aligned to the compiler's 0.7.0 release. No client changes; the compiler it drives emits
  faster Python for `Option`/`Result` matches, nullary constructors, lookups consumed by a match and
  equality searches, folds over destructured elements are linear, every emitted class uses
  `__slots__`, and four cases where a program passed checking and failed at runtime (a block-local
  partial application, and three shadowing shapes that rebound a Python function-wide local) are
  fixed.

## 0.6.0

- Version aligned to the compiler's 0.6.0 release. No client changes; the language server it drives
  now publishes bare diagnostic text, so the `parse error:` prefix and the trailing byte span no
  longer sit beside the range the editor already has. A misplaced line inside a computation
  expression is reported at that line, naming the item it continues, and `extern type` names and
  record fields that reach a module through another module's exports now resolve.

## 0.5.0

- Version aligned to the compiler's 0.5.0 release. No client changes; the language server it drives
  now understands destructuring `let` bindings (`let (r, c) = …`, `let Point { x, y } = …`, and the
  same targets on a computation expression's `let`/`let!`), offering hover, go-to-definition,
  find-references and rename per bound name rather than per binding, and it completes the new
  `option { }` computation expression alongside `async`/`seq`/`result`. The bundled syntax grammar
  highlights both.

## 0.4.0

- Version aligned to the compiler's 0.4.0 release. No client changes; the language server it drives
  gains hover and completion documentation for all 190-odd built-in members (including their
  complexity), destructuring parameters, module-qualified type names, and the rest of the 0.4.0
  language and standard-library work.

## 0.3.0

- Version aligned to the compiler's 0.3.0 release. No client changes; the language server it drives
  gains `input`, caller-supplied `extern` keyword slots, and fixes to cross-module inference,
  effectful recursion, `Option` matching, and emitted-name collisions.

## 0.2.0

- Version aligned to the compiler's 0.2.0 release, with `opaque` added to the syntax palette.

## 0.1.0

- Version aligned to the 0.1.0 milestone, matching the compiler and the other editor integrations.

## 0.0.12

- Darkened the light-theme syntax palette so every role meets WCAG-AA contrast on white.
- Preserved line breaks in multi-line doc-comment hovers.
- Pinned control-flow and logical keywords to bracket-gold; CE builders to purple; units to orange.

Earlier versions established the core client: diagnostics, hover (type + effect), go-to-definition,
find-references, project-wide rename, completion, and document/workspace symbols over resilient
analysis, plus the role-based TextMate syntax palette.
