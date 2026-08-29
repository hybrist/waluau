// Tree-sitter grammar for Waluau, written against the language as the
// compiler implements it (crates/waluau-lexer, crates/waluau-parser).
//
// Where the compiler resolves a construct contextually (`const`, `type`,
// `enum`, `match`, `declare`, `export`, `opaque`, `case`, `is`, `self`,
// `extends`, `property`), this grammar relies on tree-sitter's keyword
// extraction: the spelling is a keyword exactly in the states where the
// construct can start and an ordinary identifier everywhere else, which
// mirrors the compiler's two-token lookahead.
//
// Deliberate divergences from the compiler, all in the permissive direction
// so the editor keeps a coherent tree while the LSP reports the real error:
// - reserved primitive-type words (`number`, `bool`, ...) parse as plain
//   identifiers in expression positions instead of failing the lexer
// - a trailing-dot number literal (`1.`) is not recognized; the compiler
//   accepts it but no code in this repository uses the form
// - `declare function` signatures accept type parameters and an omitted
//   return type; the compiler parses those too and rejects them afterwards

const PREC = {
  OR: 1,
  AND: 2,
  COMPARE: 3,
  CONCAT: 4,
  ADD: 5,
  MUL: 6,
  CAST: 7,
  UNARY: 8,
  POWER: 9,
  CALL: 10,
};

const TYPE_PREC = {
  UNION: 1,
  EXTENDS: 2,
  NULLABLE: 3,
};

function sep1(rule, separator) {
  return seq(rule, repeat(seq(separator, rule)));
}

function commaSep1(rule) {
  return sep1(rule, ',');
}

module.exports = grammar({
  name: 'waluau',

  // `_const_keyword` implements the compiler's two-token lookahead: `const`
  // begins a declaration only when an identifier or `function` follows;
  // `const = 1` stays an ordinary assignment to a variable named `const`.
  externals: $ => [$.comment, $.long_string, $._const_keyword],

  extras: $ => [/\s/, $.comment],

  word: $ => $.identifier,

  conflicts: $ => [
    // `function f() ... end` at the top level is a module function
    // declaration; the identical token sequence is also a named function
    // expression standing alone as an expression statement. The compiler
    // only produces declarations at the top level, so the declaration wins
    // via dynamic precedence.
    [$.function_declaration, $.function_expression],
    // `f<T>(x)` (generic call) versus comparison chains like `f < T > (x)`;
    // resolved in favor of the call by its dynamic precedence, mirroring
    // the compiler's speculative type-argument parse.
    [$.call_expression, $.unary_expression, $.binary_expression],
    [$.call_expression, $.binary_expression],
    // `if Type(name) = value then` (cast binding) shares its prefix with an
    // ordinary call-expression condition until the `=`.
    [$._cast_binding_name, $._expression],
    // Inside a speculative `<`: the tokens after it may be a comparison
    // right-hand side or the first type argument.
    [$._expression, $.type_reference],
    [$._expression, $.tagged_variant_type],
    [$._expression, $.literal_type],
    // `if c then e else ...` in statement position: the statement reading
    // survives because only it consumes the closing `end`.
    [$._statement, $.if_expression],
    [$.array_literal, $.record_type],
    [$._expression, $.record_field],
    [$._expression, $._parameter_type_entry],
    [$._parameter_type_entry, $.tuple_type],
  ],

  rules: {
    source_file: $ => repeat(choice($._top_level_declaration, $._statement)),

    // ---------------------------------------------------------------------
    // Top-level declarations (the compiler only accepts these at file scope)
    // ---------------------------------------------------------------------

    _top_level_declaration: $ => choice(
      $.function_declaration,
      $.declare_function_declaration,
      $.declare_property_declaration,
      $.declare_const_declaration,
      $.enum_declaration,
      $.type_declaration,
    ),

    function_declaration: $ => prec.dynamic(1, seq(
      optional(field('export', 'export')),
      'function',
      field('name', choice($.identifier, $.dotted_name, $.method_name)),
      $._function_body,
    )),

    dotted_name: $ => seq(
      field('table', $.identifier),
      '.',
      field('member', $.identifier),
    ),

    method_name: $ => seq(
      field('table', $.identifier),
      ':',
      field('method', $.identifier),
    ),

    // `declare function math.abs(n: f64): f64` — a host import signature.
    declare_function_declaration: $ => seq(
      'declare',
      'function',
      field('name', choice($.identifier, $.dotted_name, $.method_name)),
      optional(field('type_parameters', $.type_parameters)),
      field('parameters', $.parameters),
      optional(seq(':', field('return_type', $._return_type))),
    ),

    // `declare property HTMLElement:innerText: string`
    declare_property_declaration: $ => seq(
      'declare',
      'property',
      field('receiver', $.identifier),
      ':',
      field('name', choice($.identifier, alias('not', $.identifier))),
      ':',
      field('type', $._type),
    ),

    // `declare const math.pi: f64 = 3.141592653589793`
    declare_const_declaration: $ => seq(
      'declare',
      'const',
      field('name', $.dotted_name),
      ':',
      field('type', $._type),
      '=',
      field('value', $.number),
    ),

    // `enum Direction { north, east, south, west }`
    enum_declaration: $ => seq(
      optional(field('export', 'export')),
      'enum',
      field('name', $.identifier),
      '{',
      commaSep1(alias($.identifier, $.enum_variant)),
      optional(','),
      '}',
    ),

    // `export opaque type Handle = i32`
    // `type Tally = Counter & { count: i32 }` (interface conformance)
    type_declaration: $ => seq(
      optional(field('export', 'export')),
      optional(field('opaque', 'opaque')),
      'type',
      field('name', $.identifier),
      optional(field('type_parameters', $.type_parameters)),
      '=',
      field('value', choice($._type, $.conformance_type)),
    ),

    conformance_type: $ => seq(
      field('interface', $.type_reference),
      '&',
      field('shape', $._type),
    ),

    // ---------------------------------------------------------------------
    // Statements
    // ---------------------------------------------------------------------

    _statement: $ => choice(
      $.local_declaration,
      $.local_function_declaration,
      $.const_declaration,
      $.const_function_declaration,
      $.if_statement,
      $.while_statement,
      $.repeat_statement,
      $.for_statement,
      $.for_in_statement,
      $.do_statement,
      $.match_statement,
      $.break_statement,
      $.continue_statement,
      $.return_statement,
      $.assignment_statement,
      $._expression,
      ';',
    ),

    block: $ => repeat1($._statement),

    local_declaration: $ => seq(
      'local',
      commaSep1($.binding),
      optional(seq('=', commaSep1(field('value', $._expression)))),
    ),

    binding: $ => seq(
      field('name', $.identifier),
      optional($.attribute),
      optional(seq(':', field('type', $._type))),
    ),

    // `local total <const> = 0`
    attribute: $ => seq('<', $.identifier, '>'),

    local_function_declaration: $ => seq(
      'local',
      'function',
      field('name', $.identifier),
      $._function_body,
    ),

    // `const` bindings must be initialized at declaration.
    const_declaration: $ => seq(
      alias($._const_keyword, 'const'),
      commaSep1($.binding),
      '=',
      commaSep1(field('value', $._expression)),
    ),

    const_function_declaration: $ => seq(
      alias($._const_keyword, 'const'),
      'function',
      field('name', $.identifier),
      $._function_body,
    ),

    if_statement: $ => seq(
      'if',
      field('condition', choice($._expression, $.cast_binding)),
      'then',
      optional(field('consequence', $.block)),
      repeat(field('alternative', $.elseif_clause)),
      optional(field('alternative', $.else_clause)),
      'end',
    ),

    elseif_clause: $ => seq(
      'elseif',
      field('condition', choice($._expression, $.cast_binding)),
      'then',
      optional(field('consequence', $.block)),
    ),

    else_clause: $ => seq('else', optional(field('body', $.block))),

    // `if HTMLInputElement(input) = element then` / `if Left(n) = value then`
    // narrows an extern or tagged-union value and binds it in the branch.
    cast_binding: $ => seq(
      field('type', alias($._cast_binding_name, $.type_reference)),
      '(',
      field('name', $.identifier),
      ')',
      '=',
      field('value', $._expression),
    ),

    _cast_binding_name: $ => seq(
      $.identifier,
      optional(seq('.', $.identifier)),
    ),

    while_statement: $ => seq(
      'while',
      field('condition', $._expression),
      'do',
      optional(field('body', $.block)),
      'end',
    ),

    repeat_statement: $ => seq(
      'repeat',
      optional(field('body', $.block)),
      'until',
      field('condition', $._expression),
    ),

    for_statement: $ => seq(
      'for',
      field('name', $.identifier),
      '=',
      field('start', $._expression),
      ',',
      field('stop', $._expression),
      optional(seq(',', field('step', $._expression))),
      'do',
      optional(field('body', $.block)),
      'end',
    ),

    for_in_statement: $ => seq(
      'for',
      commaSep1(field('name', $.identifier)),
      'in',
      commaSep1(field('iterator', $._expression)),
      'do',
      optional(field('body', $.block)),
      'end',
    ),

    do_statement: $ => seq('do', optional(field('body', $.block)), 'end'),

    // `match kind do case Enum.variant then ... end` — exhaustive over a
    // nominal enum; cases may be module-qualified (`case mod.Enum.variant`).
    match_statement: $ => seq(
      'match',
      field('value', $._expression),
      'do',
      repeat($.match_case),
      'end',
    ),

    match_case: $ => seq(
      'case',
      field('pattern', $.enum_member),
      'then',
      optional(field('body', $.block)),
    ),

    enum_member: $ => seq(
      $.identifier,
      '.',
      $.identifier,
      optional(seq('.', $.identifier)),
    ),

    break_statement: () => 'break',

    continue_statement: () => 'continue',

    return_statement: $ => prec.right(seq(
      'return',
      optional(commaSep1(field('value', $._expression))),
    )),

    assignment_statement: $ => seq(
      commaSep1(field('target', $._lvalue)),
      field('operator', choice(
        '=', '+=', '-=', '*=', '/=', '//=', '%=', '^=', '..=',
      )),
      commaSep1(field('value', $._expression)),
    ),

    _lvalue: $ => choice(
      $.identifier,
      $.field_expression,
      $.index_expression,
    ),

    // ---------------------------------------------------------------------
    // Expressions
    // ---------------------------------------------------------------------

    _expression: $ => choice(
      $.nil,
      $.true,
      $.false,
      $.number,
      $.string,
      $.long_string,
      $.interpolated_string,
      $.bytes,
      $.vararg_expression,
      $.identifier,
      $.function_expression,
      $.array_literal,
      $.table_literal,
      $.parenthesized_expression,
      $.call_expression,
      $.method_call_expression,
      $.field_expression,
      $.not_modifier_expression,
      $.index_expression,
      $.unary_expression,
      $.binary_expression,
      $.is_expression,
      $.cast_expression,
      $.if_expression,
    ),

    nil: () => 'nil',
    true: () => 'true',
    false: () => 'false',

    vararg_expression: () => '...',

    parenthesized_expression: $ => seq('(', $._expression, ')'),

    // `if c then a else b` in expression position always has an else arm.
    if_expression: $ => prec.right(seq(
      'if',
      field('condition', $._expression),
      'then',
      field('consequence', $._expression),
      'else',
      field('alternative', $._expression),
    )),

    function_expression: $ => seq(
      'function',
      optional(field('name', $.identifier)),
      $._function_body,
    ),

    _function_body: $ => seq(
      optional(field('type_parameters', $.type_parameters)),
      field('parameters', $.parameters),
      optional(seq(':', field('return_type', $._return_type))),
      optional(field('body', $.block)),
      'end',
    ),

    parameters: $ => seq(
      '(',
      optional(choice(
        seq(
          commaSep1($.parameter),
          optional(seq(',', $.vararg_parameter)),
        ),
        $.vararg_parameter,
      )),
      ')',
    ),

    parameter: $ => seq(
      field('name', $.identifier),
      optional(seq(':', field('type', $._type))),
    ),

    vararg_parameter: $ => seq('...', optional(seq(':', field('type', $._type)))),

    call_expression: $ => choice(
      prec(PREC.CALL, seq(
        field('function', $._expression),
        field('arguments', $.arguments),
      )),
      // `f<i32>(x)` — like the compiler, the generic-call reading wins over
      // the comparison chain `f < i32 > (x)` when `(` follows the `>`.
      prec.dynamic(1, prec(PREC.CALL, seq(
        field('function', $._expression),
        field('type_arguments', $.type_arguments),
        field('arguments', $.arguments),
      ))),
    ),

    method_call_expression: $ => choice(
      prec(PREC.CALL, seq(
        field('receiver', $._expression),
        ':',
        field('method', $.identifier),
        field('arguments', $.arguments),
      )),
      prec.dynamic(1, prec(PREC.CALL, seq(
        field('receiver', $._expression),
        ':',
        field('method', $.identifier),
        field('type_arguments', $.type_arguments),
        field('arguments', $.arguments),
      ))),
    ),

    // Call-argument sugar: a lone string or brace literal may replace the
    // parenthesized list (`require "./mod"`, `configure { debug = true }`).
    arguments: $ => choice(
      seq('(', optional(commaSep1($._expression)), ')'),
      $.string,
      $.long_string,
      $.array_literal,
      $.table_literal,
    ),

    field_expression: $ => prec(PREC.CALL, seq(
      field('base', $._expression),
      '.',
      field('field', $.identifier),
    )),

    // `expect(x):not:toBe(y)` — `not` is a keyword, so the `:not` negation
    // modifier is a dedicated postfix form rather than a method name.
    not_modifier_expression: $ => prec(PREC.CALL, seq(
      field('base', $._expression),
      ':',
      'not',
    )),

    index_expression: $ => prec(PREC.CALL, seq(
      field('base', $._expression),
      '[',
      field('index', $._expression),
      ']',
    )),

    unary_expression: $ => prec(PREC.UNARY, seq(
      field('operator', choice('not', '-', '#')),
      field('operand', $._expression),
    )),

    binary_expression: $ => {
      const table = [
        [PREC.OR, 'or'],
        [PREC.AND, 'and'],
        [PREC.COMPARE, choice('==', '~=', '<', '<=', '>', '>=')],
        [PREC.CONCAT, '..'],
        [PREC.ADD, choice('+', '-')],
        [PREC.MUL, choice('*', '/', '//', '%')],
      ];
      const left = table.map(([precedence, operator]) =>
        prec.left(precedence, seq(
          field('left', $._expression),
          field('operator', operator),
          field('right', $._expression),
        )));
      return choice(
        ...left,
        // Exponentiation binds tighter than the unary operators and is
        // right-associative: `-2 ^ 2` is `-(2 ^ 2)`.
        prec.right(PREC.POWER, seq(
          field('left', $._expression),
          field('operator', '^'),
          field('right', $._expression),
        )),
      );
    },

    // `value is Tag` tests a tagged-union variant.
    is_expression: $ => prec.left(PREC.COMPARE, seq(
      field('value', $._expression),
      'is',
      field('variant', $.identifier),
    )),

    // `expr :: T` casts; chains left-associatively.
    cast_expression: $ => prec.left(PREC.CAST, seq(
      field('value', $._expression),
      '::',
      field('type', $._type),
    )),

    // `{ 1, 2, 3 }` — element list; `{}` is an empty array literal.
    array_literal: $ => seq(
      '{',
      optional(seq(commaSep1($._expression), optional(','))),
      '}',
    ),

    // `{ x = 1, y = 2 }` — a record-shaped table literal.
    table_literal: $ => seq(
      '{',
      commaSep1($.table_field),
      optional(','),
      '}',
    ),

    table_field: $ => seq(
      field('name', $.identifier),
      '=',
      field('value', $._expression),
    ),

    // ---------------------------------------------------------------------
    // Types
    // ---------------------------------------------------------------------

    _type: $ => choice(
      $.builtin_type,
      $.extern_type,
      $.type_reference,
      $.tagged_variant_type,
      $.array_type,
      $.record_type,
      $.function_type,
      $.tuple_type,
      $.nullable_type,
      $.union_type,
      $.literal_type,
    ),

    builtin_type: () => choice(
      'number', 'u32', 'u64', 'i32', 'i64', 'f32', 'f64',
      'unit', 'void', 'bool', 'unknown', 'string', 'bytes', 'thread',
    ),

    extern_type: $ => prec.right(TYPE_PREC.EXTENDS, seq(
      'extern',
      optional(seq('extends', field('parent', $._type))),
    )),

    // The dotted module prefix and any type arguments bind greedily, like
    // the compiler's parse_type: `x::mod.T` is a cast to the module type,
    // never a field access on the cast result.
    type_reference: $ => prec.right(seq(
      optional(seq(field('module', $.identifier), '.')),
      field('name', $.identifier),
      optional(field('type_arguments', $.type_arguments)),
    )),

    // `Count(i32)` — one member of a tagged union.
    tagged_variant_type: $ => seq(
      field('tag', $.identifier),
      '(',
      field('payload', $._type),
      ')',
    ),

    array_type: $ => seq('{', field('element', $._type), '}'),

    // `{}` is the empty record type; `{ x: T }` a record shape. A field's
    // function type may take a `self` receiver (interface method types).
    record_type: $ => seq(
      '{',
      optional(seq(commaSep1($.record_field), optional(','))),
      '}',
    ),

    record_field: $ => seq(
      field('name', $.identifier),
      ':',
      field('type', $._type),
    ),

    function_type: $ => prec.right(seq(
      field('parameters', $.parameter_types),
      '->',
      field('return_type', $._type),
    )),

    parameter_types: $ => seq(
      '(',
      optional(commaSep1($._parameter_type_entry)),
      ')',
    ),

    _parameter_type_entry: $ => choice(
      alias('self', $.self_parameter),
      // A documentation-only parameter name (`a: i32`); names never affect
      // type identity.
      seq(field('name', $.identifier), ':', $._type),
      $._type,
    ),

    // `()` is unit, `(T)` grouping, `(T1, T2)` a multiple-return list.
    tuple_type: $ => seq('(', optional(commaSep1($._type)), ')'),

    nullable_type: $ => prec(TYPE_PREC.NULLABLE, seq($._type, '?')),

    union_type: $ => prec.left(TYPE_PREC.UNION, seq(
      $._type,
      '|',
      $._type,
    )),

    // `"red"`, `0`, `-1`, `0.5` in type position (literal unions).
    literal_type: $ => choice(
      $.string,
      seq(optional('-'), $.number),
    ),

    // Return annotations may list several types: `: (bool, i32)` or `: bool, i32`.
    _return_type: $ => choice($._type, $.return_type_list),

    return_type_list: $ => seq($._type, repeat1(seq(',', $._type))),

    type_parameters: $ => seq('<', optional(commaSep1($.identifier)), '>'),

    type_arguments: $ => seq('<', optional(commaSep1($._type)), '>'),

    // ---------------------------------------------------------------------
    // Terminals
    // ---------------------------------------------------------------------

    identifier: () => /[A-Za-z_][A-Za-z0-9_]*/,

    // Decimal (`42`, `3.14`, `1_000`) and hex (`0xFF`) literals; underscores
    // are digit separators. No exponent form exists in Waluau.
    number: () => token(choice(
      /0[xX][0-9a-fA-F_]+/,
      /[0-9][0-9_]*(\.[0-9_]+)?/,
    )),

    string: $ => choice(
      seq(
        '"',
        repeat(choice(
          alias(token.immediate(prec(1, /[^"\\\n]+/)), $.string_content),
          $.escape_sequence,
        )),
        '"',
      ),
      seq(
        "'",
        repeat(choice(
          alias(token.immediate(prec(1, /[^'\\\n]+/)), $.string_content),
          $.escape_sequence,
        )),
        "'",
      ),
    ),

    escape_sequence: () => token.immediate(
      /\\(?:z\s*|x[0-9a-fA-F]{2}|u\{[0-9a-fA-F]+\}|[0-9]{1,3}|\r?\n|[^\r\n0-9])/,
    ),

    // `` `hi {name}!` `` — interpolation holes hold full expressions.
    interpolated_string: $ => seq(
      '`',
      repeat(choice(
        alias(token.immediate(prec(1, /[^`\\{]+/)), $.string_content),
        $.escape_sequence,
        $.interpolation,
      )),
      '`',
    ),

    interpolation: $ => seq('{', $._expression, '}'),

    // `b"ABC\x00"` — an ASCII bytes literal.
    bytes: () => token(seq('b"', repeat(choice(/[^"\\\n]/, /\\[^\n]/)), '"')),
  },
});
