; Waluau contextual keywords are identifiers in the typed-Luau parser used as
; the structural foundation. Capture them by spelling so the editor still
; presents Waluau syntax even while the LSP owns semantic understanding.
((identifier) @keyword
  (#any-of? @keyword
    "case" "const" "declare" "enum" "export" "extends" "match" "opaque"
    "property" "type"))

[
  "local"
  "while"
  "repeat"
  "until"
  "for"
  "in"
  "if"
  "elseif"
  "else"
  "then"
  "do"
  "function"
  "end"
  "return"
  (continue_statement)
  (break_statement)
] @keyword

(type_alias_declaration
  ["export" "type"] @keyword)

(type_function_declaration
  ["export" "type"] @keyword)

(declare_global_declaration "declare" @keyword)
(declare_global_function_declaration "declare" @keyword)

(declare_class_declaration
  ["declare" "class" "extends"] @keyword)

(declare_extern_type_declaration
  ["declare" "extern" "type" "extends" "with"] @keyword)

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

["==" "~=" "<=" ">="] @operator.comparison
["not" "and" "or"] @operator.logical
["=" "+=" "-=" "*=" "/=" "//=" "%=" "^=" "..="] @operator.assignment
["+" "-" "*" "/" "//" "%" "^"] @operator.arithmetic
["#" "&" "|" "::" ".." "?"] @operator

((identifier) @keyword.operator
  (#eq? @keyword.operator "is"))

(identifier) @variable

((identifier) @type.builtin
  (#any-of? @type.builtin
    "bool" "bytes" "extern" "f32" "f64" "i32" "i64" "number"
    "string" "thread" "u32" "u64" "unit" "unknown" "void"))

(string_interpolation ["{" "}"] @punctuation.special) @embedded

(type_binding (identifier) @variable.parameter)

((identifier) @variable.special
  (#any-of? @variable.special
    "math" "table" "coroutine" "bit32" "utf8" "os" "debug" "buffer"))

((identifier) @variable.special
  (#match? @variable.special "^_[A-Z]*$"))

(table_constructor ["{" "}"] @constructor)
(field_identifier) @property

(nil) @constant.builtin

((identifier) @constant.builtin
  (#eq? @constant.builtin "_VERSION"))

([
  (identifier)
  (field_identifier)
] @constant
  (#match? @constant "^[A-Z][A-Z][A-Z_0-9]*$"))

(number) @number
[(true) (false)] @boolean
(string) @string
(escape_sequence) @string.escape
(interpolated_string "`" @string)
(string_content) @string

(table_property_attribute) @attribute
(typeof_type "typeof" @function.builtin)
(type_identifier) @type

(type_reference prefix: (identifier) @variable.namespace)

(function_declaration
  name: [
    (identifier) @function
    (dot_index_expression field: (field_identifier) @function)
  ])

(method_index_expression method: (field_identifier) @function.method)
(local_function_declaration name: (identifier) @function)
(const_function_declaration ["const" @keyword name: (identifier) @function])
(const_variable_declaration "const" @keyword)
(declare_global_function_declaration name: (identifier) @function)
(class_function name: (identifier) @function)

(parameters
  [
    (binding name: (identifier) @variable.parameter)
    (variadic_parameter "..." @variable.parameter)
  ])

((identifier) @variable.special
  (#eq? @variable.special "self"))

(function_call
  name: [
    (identifier) @function.call
    (dot_index_expression field: (field_identifier) @function.call)
  ])

(function_call
  name: (identifier) @function.builtin
  (#any-of? @function.builtin
    "assert" "error" "getfenv" "getmetatable" "ipairs" "next" "pairs"
    "pcall" "print" "rawequal" "rawget" "rawset" "require" "select"
    "setfenv" "setmetatable" "tonumber" "tostring" "type" "unpack"
    "xpcall"))

(comment) @comment
(hash_bang_line) @preproc

((comment) @comment.doc
  (#match? @comment.doc "^[-][-][-]"))
