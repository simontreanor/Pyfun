/**
 * Tree-sitter grammar for Pyfun (https://github.com/simontreanor/Pyfun)
 *
 * Mirrors the Rust reference implementation:
 *   - lexer:  src/lexer/mod.rs   (offside rule: Indent/Dedent/Sep, block
 *             openers `=` `->` `then` `else` `:`, continuation-lead lines)
 *   - parser: src/parser/mod.rs  (precedence table, item/expression grammar)
 *
 * Layout is produced by the external scanner (src/scanner.c) as three
 * zero-width tokens. Inside brackets the scanner never fires, which gives
 * implicit line continuation exactly like the reference lexer.
 *
 * Known, deliberate simplifications (all strictly more permissive):
 *   - Triple-quoted f-strings are opaque tokens (no hole highlighting).
 */

/// <reference types="tree-sitter-cli/dsl" />
// @ts-check

function sep1(rule, separator) {
  return seq(rule, repeat(seq(separator, rule)));
}

const IDENT_LOWER = /[_a-z][_a-zA-Z0-9]*/;
const IDENT_UPPER = /[A-Z][_a-zA-Z0-9]*/;

module.exports = grammar({
  name: 'pyfun',

  externals: $ => [$._indent, $._dedent, $._sep],

  extras: $ => [/\s/, $.comment],

  word: $ => $.identifier,

  conflicts: $ => [
    // `Upper {` is a tagged record or a CE, never a constructor applied to a
    // record update — the content after `{` disambiguates (GLR).
    [$._atom_expression, $._constructor_path, $._module_identifier],
    // `Upper.` is either qualified access (field_expression) or the module
    // prefix of a qualified record tag.
    [$._atom_expression, $._module_identifier],
  ],

  supertypes: $ => [$._expression, $._pattern],

  rules: {
    // ==================== top level ====================

    source_file: $ => optional(seq(
      sep1($._item, $._sep),
      optional($._sep),
    )),

    _item: $ => choice(
      $.type_definition,
      $.extern_type_definition,
      $.measure_definition,
      $.module_definition,
      $.import_declaration,
      $.extern_import_declaration,
      $.extern_declaration,
      $.active_pattern_definition,
      $.let_binding,
      $._expression,
    ),

    // ==================== items ====================

    let_binding: $ => seq(
      'let',
      repeat(choice('mut', 'pure')),
      field('name', choice($.identifier, $.wildcard)),
      repeat(field('parameter', $.parameter)),
      '=',
      field('body', $._body),
    ),

    parameter: $ => $.identifier,

    active_pattern_definition: $ => seq(
      'let',
      field('cases', $.active_pattern_cases),
      repeat(field('parameter', $.parameter)),
      '=',
      field('body', $._body),
    ),

    active_pattern_cases: $ => seq(
      '(', '|',
      sep1(field('case', $.constructor_identifier), '|'),
      optional(seq('|', $.wildcard)),
      '|', ')',
    ),

    type_definition: $ => seq(
      'type',
      field('name', $._type_identifier),
      repeat(field('type_parameter', $._type_variable)),
      '=',
      field('body', choice(
        $.record_declaration,
        $._variant_list,
        $._variant_block,
      )),
    ),

    _type_identifier: $ => alias($.constructor_identifier, $.type_identifier),
    _type_variable: $ => alias($.identifier, $.type_variable),
    _module_identifier: $ => alias($.constructor_identifier, $.module_identifier),

    record_declaration: $ => seq(
      '{',
      sep1($.field_declaration, ','),
      '}',
    ),

    field_declaration: $ => seq(
      field('name', $.identifier),
      ':',
      field('type', $._type),
    ),

    _variant_list: $ => seq(
      optional('|'),
      sep1($.variant, '|'),
    ),

    _variant_block: $ => seq(
      $._indent,
      optional('|'),
      sep1($.variant, choice('|', seq($._sep, optional('|')))),
      $._dedent,
    ),

    variant: $ => prec.right(seq(
      field('name', $.constructor_identifier),
      repeat(field('field', $._type_atom)),
    )),

    extern_type_definition: $ => seq(
      'extern', 'type',
      field('name', $._type_identifier),
      repeat(field('type_parameter', $._type_variable)),
    ),

    measure_definition: $ => seq(
      'measure',
      field('name', $._path_component),
      optional(seq('=', field('body', $.measure))),
    ),

    module_definition: $ => seq(
      'module',
      field('name', $._module_identifier),
      '=',
      field('body', $._block),
    ),

    import_declaration: $ => seq(
      'import',
      field('module', $._module_identifier),
    ),

    extern_import_declaration: $ => seq(
      'extern', 'import',
      field('module', $.python_path),
      optional(seq('as', field('alias', $.identifier))),
    ),

    python_path: $ => sep1($._path_component, '.'),

    _path_component: $ => choice($.identifier, alias($.constructor_identifier, $.identifier)),

    extern_declaration: $ => seq(
      'extern',
      optional('pure'),
      field('name', $.identifier),
      ':',
      field('type', $._type),
      optional(seq('=', field('target', $.extern_target))),
    ),

    extern_target: $ => choice(
      // dotted Python path, optionally with pinned keyword arguments
      seq($.python_path, optional($.extern_kwargs)),
      // instance-access receiver: `.weekday()` (method) or `.days` (property)
      seq('.', $._path_component, optional($.extern_kwargs)),
    ),

    extern_kwargs: $ => seq(
      '(',
      optional(sep1($.extern_kwarg, ',')),
      ')',
    ),

    extern_kwarg: $ => seq(
      field('name', $.identifier),
      '=',
      field('value', $._extern_literal),
    ),

    _extern_literal: $ => choice(
      $.string,
      $.boolean,
      seq(optional('-'), choice($.integer, $.float)),
    ),

    // ==================== statements & blocks ====================

    _body: $ => choice($._expression, $._block),

    _block: $ => seq(
      $._indent,
      sep1($._statement, $._sep),
      $._dedent,
    ),

    _statement: $ => choice(
      $.let_binding,
      $._expression,
    ),

    // ==================== expressions ====================

    _expression: $ => choice(
      $.assignment,
      $.lambda,
      $.if_expression,
      $.match_expression,
      $._pipe_expression,
    ),

    assignment: $ => seq(
      field('target', $.identifier),
      '<-',
      field('value', $._expression),
    ),

    lambda: $ => seq(
      'fun',
      repeat1(field('parameter', $.parameter)),
      '->',
      field('body', $._body),
    ),

    if_expression: $ => seq(
      'if',
      field('condition', $._expression),
      'then',
      field('consequence', $._body),
      repeat(field('alternative', $.elif_clause)),
      'else',
      field('alternative', $._body),
    ),

    elif_clause: $ => seq(
      'elif',
      field('condition', $._expression),
      'then',
      field('consequence', $._body),
    ),

    match_expression: $ => prec.right(seq(
      'match',
      field('subject', $._expression),
      ':',
      choice(
        // offside arm list
        seq(
          $._indent,
          sep1(field('arm', $.case_clause), $._sep),
          $._dedent,
        ),
        // layout-free arm list (inside brackets, or all on one line)
        repeat1(field('arm', $.case_clause)),
      ),
    )),

    case_clause: $ => seq(
      'case',
      field('pattern', $._pattern),
      optional(seq('if', field('guard', $._expression))),
      ':',
      field('body', $._body),
    ),

    // --- precedence ladder (parser/mod.rs, lowest -> highest) ---

    _pipe_expression: $ => choice(
      $.pipe_expression,
      $._compose_expression,
    ),

    pipe_expression: $ => choice(
      prec.left(1, seq(
        field('left', $._pipe_expression),
        field('operator', '|>'),
        field('right', $._compose_expression),
      )),
      prec.right(1, seq(
        field('left', $._compose_expression),
        field('operator', '<|'),
        field('right', $._pipe_expression),
      )),
    ),

    _compose_expression: $ => choice(
      $.compose_expression,
      $._or_expression,
    ),

    compose_expression: $ => prec.left(2, seq(
      field('left', $._compose_expression),
      field('operator', choice('>>', '<<')),
      field('right', $._or_expression),
    )),

    _or_expression: $ => choice($.or_expression, $._and_expression),

    or_expression: $ => prec.left(3, seq(
      field('left', $._or_expression),
      field('operator', 'or'),
      field('right', $._and_expression),
    )),

    _and_expression: $ => choice($.and_expression, $._not_expression),

    and_expression: $ => prec.left(4, seq(
      field('left', $._and_expression),
      field('operator', 'and'),
      field('right', $._not_expression),
    )),

    _not_expression: $ => choice(
      $.not_expression,
      $.try_expression,
      $._comparison_expression,
    ),

    not_expression: $ => seq('not', $._not_expression),

    try_expression: $ => seq('try', $._not_expression),

    _comparison_expression: $ => choice(
      $.comparison_expression,
      $._additive_expression,
    ),

    comparison_expression: $ => prec.left(6, seq(
      field('left', $._comparison_expression),
      field('operator', choice('==', '!=', '<', '>', '<=', '>=')),
      field('right', $._additive_expression),
    )),

    _additive_expression: $ => choice(
      $.additive_expression,
      $._multiplicative_expression,
    ),

    additive_expression: $ => prec.left(7, seq(
      field('left', $._additive_expression),
      field('operator', choice('+', '-')),
      field('right', $._multiplicative_expression),
    )),

    _multiplicative_expression: $ => choice(
      $.multiplicative_expression,
      $._unary_expression,
    ),

    multiplicative_expression: $ => prec.left(8, seq(
      field('left', $._multiplicative_expression),
      field('operator', choice('*', '/', '//', '%')),
      field('right', $._unary_expression),
    )),

    _unary_expression: $ => choice(
      $.unary_expression,
      $._power_expression,
    ),

    unary_expression: $ => seq('-', $._unary_expression),

    _power_expression: $ => choice($.power_expression, $._application_expression),

    power_expression: $ => prec.right(10, seq(
      field('left', $._application_expression),
      field('operator', '**'),
      field('right', $._unary_expression),
    )),

    _application_expression: $ => choice(
      $.application,
      $._postfix_expression,
    ),

    application: $ => prec.left(11, seq(
      field('function', $._application_expression),
      field('argument', $._postfix_expression),
    )),

    _postfix_expression: $ => choice(
      $.field_expression,
      $._atom_expression,
    ),

    field_expression: $ => prec.left(12, seq(
      field('value', $._postfix_expression),
      '.',
      field('field', choice($.identifier, $.constructor_identifier)),
    )),

    // ==================== atoms ====================

    _atom_expression: $ => choice(
      $.integer,
      $.float,
      $.unit_literal,
      $.boolean,
      $.string,
      $.raw_string,
      $.fstring,
      $.hole,
      $.identifier,
      $.constructor_identifier,
      $.unit,
      $.parenthesized_expression,
      $.tuple_expression,
      $.list_expression,
      $.record_expression,
      $.record_update_expression,
      $.ce_expression,
      $.operator_section,
    ),

    unit: _ => seq('(', ')'),

    parenthesized_expression: $ => seq('(', $._expression, ')'),

    tuple_expression: $ => seq(
      '(',
      $._expression,
      ',',
      sep1($._expression, ','),
      optional(','),
      ')',
    ),

    list_expression: $ => seq(
      '[',
      optional(seq(sep1($._expression, ','), optional(','))),
      ']',
    ),

    record_expression: $ => seq(
      field('tag', $._constructor_path),
      '{',
      sep1($.field_initializer, ','),
      '}',
    ),

    _constructor_path: $ => choice(
      $.constructor_identifier,
      seq($._module_identifier, '.', $.constructor_identifier),
    ),

    field_initializer: $ => seq(
      field('name', $.identifier),
      '=',
      field('value', $._expression),
    ),

    record_update_expression: $ => seq(
      '{',
      field('record', $._expression),
      'with',
      sep1($.field_update, ','),
      '}',
    ),

    field_update: $ => seq(
      field('path', sep1($.identifier, '.')),
      '=',
      field('value', $._expression),
    ),

    ce_expression: $ => seq(
      field('builder', choice('async', 'seq', 'result', $._module_identifier)),
      '{',
      repeat1($._ce_item),
      '}',
    ),

    _ce_item: $ => choice(
      $.ce_let,
      $.ce_bind,
      $.ce_do,
      $.ce_return,
      $.ce_yield,
    ),

    ce_let: $ => seq('let', field('name', $.identifier), '=', $._expression),
    ce_bind: $ => seq('let!', field('name', $.identifier), '=', $._expression),
    ce_do: $ => seq('do!', $._expression),
    ce_return: $ => seq(choice('return', 'return!'), $._expression),
    ce_yield: $ => seq(choice('yield', 'yield!'), $._expression),

    operator_section: _ => seq(
      '(',
      choice(
        '+', '-', '*', '/', '//', '%', '**',
        '==', '!=', '<', '>', '<=', '>=',
      ),
      ')',
    ),

    // ==================== patterns ====================

    _pattern: $ => choice(
      $.as_pattern,
      $.or_pattern,
      $.constructor_pattern,
      $._atom_pattern,
    ),

    as_pattern: $ => prec.left(1, seq(
      field('pattern', $._pattern),
      'as',
      field('name', $.identifier),
    )),

    or_pattern: $ => prec.left(2, seq(
      field('left', $._pattern),
      '|',
      field('right', $._pattern),
    )),

    constructor_pattern: $ => prec.right(3, seq(
      field('constructor', $._constructor_path),
      repeat1(field('argument', $._atom_pattern)),
    )),

    _atom_pattern: $ => choice(
      $.wildcard,
      $.identifier,
      $.integer,
      $.negative_integer,
      $.string,
      $.boolean,
      alias($._constructor_path, $.constructor_pattern_name),
      $.parenthesized_pattern,
      $.tuple_pattern,
      $.list_pattern,
      $.record_pattern,
    ),

    wildcard: _ => '_',

    negative_integer: $ => seq('-', $.integer),

    parenthesized_pattern: $ => seq('(', $._pattern, ')'),

    tuple_pattern: $ => seq(
      '(',
      $._pattern,
      ',',
      sep1($._pattern, ','),
      ')',
    ),

    list_pattern: $ => seq(
      '[',
      optional(sep1(choice($._pattern, $.rest_pattern), ',')),
      ']',
    ),

    rest_pattern: $ => seq('*', choice($.identifier, $.wildcard)),

    record_pattern: $ => seq(
      field('tag', $._constructor_path),
      '{',
      sep1($.field_pattern, ','),
      '}',
    ),

    field_pattern: $ => seq(
      field('name', $.identifier),
      optional(seq('=', field('pattern', $._pattern))),
    ),

    // ==================== types ====================

    _type: $ => choice($.function_type, $._type_app),

    function_type: $ => prec.right(seq(
      field('parameter', $._type_app),
      '->',
      optional(field('effects', $.effect_annotation)),
      field('result', $._type),
    )),

    effect_annotation: $ => seq(
      '{',
      sep1($.effect_label, ','),
      '}',
    ),

    effect_label: $ => choice($.identifier, alias('async', $.identifier)),

    _type_app: $ => choice($.type_application, $._type_atom),

    type_application: $ => seq(
      field('constructor', $._type_identifier),
      repeat1(field('argument', $._type_atom)),
    ),

    _type_atom: $ => choice(
      $._type_variable,
      $._type_identifier,
      seq('(', $._type, ')'),
      $.tuple_type,
    ),

    tuple_type: $ => seq(
      '(',
      $._type,
      ',',
      sep1($._type, ','),
      ')',
    ),

    // ==================== measures ====================

    measure: $ => seq(
      choice(alias($.integer, $.dimensionless), repeat1($.measure_factor)),
      optional(seq('/', repeat1($.measure_factor))),
    ),

    measure_factor: $ => seq(
      $._path_component,
      optional(seq('^', $.integer)),
    ),

    // ==================== literals & terminals ====================

    unit_literal: $ => seq(
      choice($.integer, $.float),
      token.immediate('<'),
      $.measure,
      '>',
    ),

    boolean: _ => choice('true', 'false'),

    integer: _ => token(choice(
      /0[xX][0-9a-fA-F][0-9a-fA-F_]*/,
      /0[oO][0-7][0-7_]*/,
      /0[bB][01][01_]*/,
      /[0-9][0-9_]*/,
    )),

    float: _ => token(choice(
      /[0-9][0-9_]*\.[0-9][0-9_]*([eE][+-]?[0-9]+)?/,
      /[0-9][0-9_]*[eE][+-]?[0-9]+/,
    )),

    string: $ => choice(
      seq('"""', repeat(choice($.escape_sequence, $._triple_content)), token.immediate('"""')),
      seq('"', repeat(choice($.escape_sequence, $._string_content)), token.immediate('"')),
    ),

    _string_content: _ => token.immediate(prec(1, /[^"\\\n]+/)),
    _triple_content: _ => token.immediate(prec(1, choice(/[^"\\]+/, /"[^"]/, /""[^"]/))),

    escape_sequence: _ => token.immediate(/\\(u\{[0-9a-fA-F]{1,6}\}|.)/),

    raw_string: _ => token(choice(
      seq('r"""', repeat(choice(/[^"\\]/, /"[^"]/, /""[^"]/, /\\./)), '"""'),
      seq('r"', repeat(choice(/[^"\\\n]/, /\\./)), '"'),
    )),

    fstring: $ => choice(
      // triple-quoted f-strings are opaque (no hole highlighting)
      token(seq('f"""', repeat(choice(/[^"\\]/, /"[^"]/, /""[^"]/, /\\./)), '"""')),
      seq(
        'f"',
        repeat(choice(
          $.escape_sequence,
          alias(token.immediate(prec(1, /[^"\\{}\n]+|\{\{|\}\}/)), $.string_content),
          $.interpolation,
        )),
        token.immediate('"'),
      ),
    ),

    interpolation: $ => seq(
      token.immediate('{'),
      $._expression,
      optional(alias('=', $.debug_marker)),
      '}',
    ),

    hole: _ => token(/\?[_a-zA-Z][_a-zA-Z0-9]*|\?/),

    identifier: _ => IDENT_LOWER,
    constructor_identifier: _ => IDENT_UPPER,

    comment: _ => token(seq('#', /.*/)),
  },
});
