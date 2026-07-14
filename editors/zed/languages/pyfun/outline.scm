(let_binding
  name: (identifier) @name) @item

(type_definition
  name: (type_identifier) @name) @item

(extern_type_definition
  name: (type_identifier) @name) @item

(module_definition
  name: (module_identifier) @name) @item

(extern_declaration
  name: (identifier) @name) @item

(active_pattern_definition
  cases: (active_pattern_cases) @name) @item
