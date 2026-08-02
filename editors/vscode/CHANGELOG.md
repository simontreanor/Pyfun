# Changelog

The extension's version tracks the compiler's: every Pyfun release ships a matching extension, so
`pyfun --version` and the installed client always agree. A release with no client changes says so.

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
