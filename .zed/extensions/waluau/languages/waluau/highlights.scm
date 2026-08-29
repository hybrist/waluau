; Highlights for the Waluau grammar (tools/tree-sitter-waluau). Every
; construct below is a first-class node or token in that grammar — no
; spelling-based identifier heuristics are needed for keywords.

[
  "local"
  "function"
  "end"
  "if"
  "then"
  "elseif"
  "else"
  "while"
  "do"
  "repeat"
  "until"
  "for"
  "in"
  "return"
  "match"
  "case"
  "const"
  "export"
  "opaque"
  "type"
  "enum"
  "declare"
  "property"
  "extends"
  (break_statement)
  (continue_statement)
] @keyword

[
  "("
  ")"
  "["
  "]"
  "{"
  "}"
  "<"
  ">"
] @punctuation.bracket

[
  ";"
  ":"
  ","
  "."
  "->"
] @punctuation.delimiter

(binary_expression ["<" ">"] @operator.comparison)
(attribute ["<" ">"] @punctuation.bracket)

["==" "~=" "<=" ">="] @operator.comparison
["not" "and" "or"] @operator.logical
["=" "+=" "-=" "*=" "/=" "//=" "%=" "^=" "..="] @operator.assignment
["+" "-" "*" "/" "//" "%" "^"] @operator.arithmetic
["#" "&" "|" "::" ".." "?" "..."] @operator
"is" @keyword.operator

(identifier) @variable

; Primitive types are dedicated tokens inside builtin_type / extern_type.
[
  "number" "u32" "u64" "i32" "i64" "f32" "f64"
  "unit" "void" "bool" "unknown" "string" "bytes" "thread"
  "extern"
] @type.builtin

(type_reference name: (identifier) @type)
(type_reference module: (identifier) @variable.namespace)
(type_declaration name: (identifier) @type)
(conformance_type interface: (type_reference name: (identifier) @type))
(tagged_variant_type tag: (identifier) @constructor)
(self_parameter) @variable.special
(type_parameters (identifier) @type)

(enum_declaration name: (identifier) @type)
(enum_variant) @constant
(match_case pattern: (enum_member (identifier) @constant))

; `is Variant` tests and `Tag(payload)` constructions name union variants.
(is_expression variant: (identifier) @constructor)

(attribute (identifier) @attribute)

((identifier) @variable.special
  (#any-of? @variable.special
    "math" "table" "coroutine" "string" "bit32" "utf8" "buffer"))

((identifier) @variable.special
  (#eq? @variable.special "self"))

((identifier) @variable.special
  (#match? @variable.special "^_[A-Z]*$"))

(table_literal ["{" "}"] @constructor)
(table_field name: (identifier) @property)
(record_field name: (identifier) @property)
(field_expression field: (identifier) @property)
(declare_property_declaration name: (identifier) @property)
(declare_property_declaration receiver: (identifier) @type)

(nil) @constant.builtin

([
  (identifier)
] @constant
  (#match? @constant "^[A-Z][A-Z][A-Z_0-9]*$"))

(number) @number
(declare_const_declaration value: (number) @number)
[(true) (false)] @boolean
(string) @string
(long_string) @string
(bytes) @string.special
(escape_sequence) @string.escape
(interpolated_string "`" @string)
(string_content) @string
(interpolation ["{" "}"] @punctuation.special) @embedded

(function_declaration
  name: [
    (identifier) @function
    (dotted_name member: (identifier) @function)
    (method_name method: (identifier) @function.method)
  ])
(function_declaration
  name: [
    (dotted_name table: (identifier) @type)
    (method_name table: (identifier) @type)
  ])
(local_function_declaration name: (identifier) @function)
(const_function_declaration name: (identifier) @function)
(function_expression name: (identifier) @function)
(declare_function_declaration
  name: [
    (identifier) @function
    (dotted_name member: (identifier) @function)
    (method_name method: (identifier) @function.method)
  ])

(parameter name: (identifier) @variable.parameter)
(vararg_parameter "..." @variable.parameter)
(binding name: (identifier) @variable)
(cast_binding name: (identifier) @variable)
(cast_binding type: (type_reference (identifier) @type))

(call_expression
  function: [
    (identifier) @function.call
    (field_expression field: (identifier) @function.call)
  ])
(method_call_expression method: (identifier) @function.method)

(call_expression
  function: (identifier) @function.builtin
  (#any-of? @function.builtin
    "assert" "error" "ipairs" "pairs" "print" "require" "select"
    "tonumber" "tostring" "type" "typeof"))

(comment) @comment

((comment) @comment.doc
  (#match? @comment.doc "^[-][-][-]"))
