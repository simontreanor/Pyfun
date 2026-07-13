; Highlight query for Pyfun (standard tree-sitter capture names).
; Later patterns win, so defaults come first and refinements after.

; ---- identifiers (defaults) ----

(identifier) @variable
(constructor_identifier) @constructor
(type_identifier) @type
(type_variable) @type.parameter
(module_identifier) @module
(wildcard) @variable.builtin
(hole) @variable.builtin

(parameter) @variable.parameter

; a `let` with parameters defines a function
(let_binding
  name: (identifier) @function
  parameter: (parameter))

(active_pattern_cases
  case: (constructor_identifier) @function)

; qualified access: the field of `Module.member` / record field access
(field_expression
  field: (identifier) @property)
(field_expression
  field: (constructor_identifier) @constructor)

; record fields
(field_declaration name: (identifier) @property)
(field_initializer name: (identifier) @property)
(field_update path: (identifier) @property)
(field_pattern name: (identifier) @property)

; externs bind Python callables
(extern_declaration name: (identifier) @function)
(python_path (identifier) @module)
(extern_kwarg name: (identifier) @variable.parameter)

(measure_definition name: (identifier) @type)
(measure_factor (identifier) @type)
(effect_label) @attribute

(ce_expression
  builder: (module_identifier) @function.macro)

; ---- literals ----

(integer) @number
(float) @number.float
(boolean) @boolean
(string) @string
(raw_string) @string
(fstring) @string
(string_content) @string
(escape_sequence) @string.escape

(interpolation
  "{" @punctuation.special
  "}" @punctuation.special)
(debug_marker) @operator

(comment) @comment

; ---- keywords ----

[
  "let"
  "mut"
  "type"
  "measure"
  "module"
  "extern"
  "fun"
  "with"
  "as"
] @keyword

"pure" @keyword.modifier

[
  "import"
] @keyword.import

[
  "if"
  "then"
  "elif"
  "else"
  "match"
  "case"
] @keyword.conditional

[
  "try"
] @keyword.exception

[
  "return"
  "return!"
  "yield"
  "yield!"
  "let!"
  "do!"
] @keyword.coroutine

[
  "async"
  "seq"
  "result"
] @function.macro

[
  "and"
  "or"
  "not"
] @keyword.operator

; ---- operators & punctuation ----

[
  "|>"
  "<|"
  ">>"
  "<<"
  "->"
  "<-"
  "=="
  "!="
  "<="
  ">="
  "<"
  ">"
  "+"
  "-"
  "*"
  "/"
  "//"
  "%"
  "**"
  "="
  "^"
  "|"
] @operator

[
  "("
  ")"
  "["
  "]"
  "{"
  "}"
] @punctuation.bracket

[
  ","
  "."
  ":"
] @punctuation.delimiter
