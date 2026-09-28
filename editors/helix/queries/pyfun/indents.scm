; Helix indent queries for Pyfun.
;
; Pyfun is offside-ruled like Python, and its grammar keeps blocks as hidden
; nodes, so indentation keys off the construct that opens a block: a `let`
; whose body follows `=`, a `case` arm after `:`, a lambda after `->`, and so
; on. `@extend` lets a construct keep its indent for the lines that follow it,
; which is how Helix's own Python queries handle an offside body.

[
  (let_binding)
  (active_pattern_definition)
  (module_definition)
  (type_definition)
  (lambda)
  (if_expression)
  (elif_clause)
  (match_expression)
  (case_clause)
  (ce_expression)
  (record_expression)
  (record_declaration)
  (record_update_expression)
  (list_expression)
  (tuple_expression)
  (parenthesized_expression)
] @indent

[
  (let_binding)
  (active_pattern_definition)
  (module_definition)
  (type_definition)
  (lambda)
  (if_expression)
  (elif_clause)
  (match_expression)
  (case_clause)
] @extend

[
  "}"
  "]"
  ")"
] @outdent

(if_expression "else" @outdent)
(elif_clause "elif" @outdent)
