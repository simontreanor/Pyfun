; Highlight query for Pyfun, Zed capture dialect (adapted from
; editors/tree-sitter-pyfun/queries/highlights.scm — keep the two in sync).
; Later patterns win.

; ---- identifiers (defaults) ----

(identifier) @variable
(constructor_identifier) @constructor
(type_identifier) @type
(type_variable) @type
(module_identifier) @type
(wildcard) @variable.special
(hole) @variable.special

(parameter (identifier) @variable.special)

(let_binding
  name: (identifier) @function
  parameter: (parameter))

(active_pattern_cases
  case: (constructor_identifier) @function)

(field_expression
  field: (identifier) @property)
(field_expression
  field: (constructor_identifier) @constructor)

(field_declaration name: (identifier) @property)
(field_initializer name: (identifier) @property)
(field_update path: (identifier) @property)
(field_pattern name: (identifier) @property)

(extern_declaration name: (identifier) @function)
(python_path (identifier) @type)
(extern_kwarg name: (identifier) @variable.special)

(measure_definition name: (identifier) @type)
(measure_factor (identifier) @type)
(effect_label) @attribute

(ce_expression
  builder: (module_identifier) @keyword)

; ---- literals ----

(integer) @number
(float) @number
(dimensionless) @number
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
  "pure"
  "type"
  "measure"
  "module"
  "extern"
  "fun"
  "with"
  "as"
  "import"
  "if"
  "then"
  "elif"
  "else"
  "match"
  "case"
  "try"
  "return"
  "return!"
  "yield"
  "yield!"
  "let!"
  "do!"
  "async"
  "seq"
  "result"
] @keyword

[
  "and"
  "or"
  "not"
] @operator

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
