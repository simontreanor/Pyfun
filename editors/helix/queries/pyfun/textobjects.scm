; Helix textobject queries for Pyfun: `f` functions, `t` types and modules,
; `a` parameters and record fields, `e` match arms and variants, `c` comments
; (so `maf`, `mit`, `]f`, and so on).

; A function is a `let` that takes parameters, or a lambda.
(let_binding
  (parameter)
  body: (_) @function.inside) @function.around

(active_pattern_definition
  body: (_) @function.inside) @function.around

(lambda
  body: (_) @function.inside) @function.around

; A type declaration is Pyfun's nearest thing to a class.
(type_definition
  body: (_) @class.inside) @class.around

(opaque_type_definition) @class.around

(module_definition
  body: (_) @class.inside) @class.around

(parameter) @parameter.inside @parameter.around

(field_declaration) @parameter.inside @parameter.around

(variant) @entry.around

(case_clause
  body: (_) @entry.inside) @entry.around

(comment) @comment.inside

(comment)+ @comment.around
