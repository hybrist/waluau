(comment) @annotation

(local_declaration
  "local" @context
  (binding
    name: (identifier) @name
    (#match? @name "^[A-Z][A-Z][A-Z_0-9]*$")) @item)

(const_declaration
  "const" @context
  (binding name: (identifier) @name) @item)

(type_declaration
  "export"? @context
  "opaque"? @context
  "type" @context
  name: (identifier) @name) @item

(enum_declaration
  "export"? @context
  "enum" @context
  name: (identifier) @name) @item

(function_declaration
  "export"? @context
  "function" @context
  name: (_) @name
  parameters: (parameters "(" @context ")" @context)) @item

(local_function_declaration
  "local" @context
  "function" @context
  name: (_) @name
  parameters: (parameters "(" @context ")" @context)) @item

(const_function_declaration
  "const" @context
  "function" @context
  name: (_) @name
  parameters: (parameters "(" @context ")" @context)) @item

(declare_function_declaration
  "declare" @context
  "function" @context
  name: (_) @name
  parameters: (parameters "(" @context ")" @context)) @item

(declare_property_declaration
  "declare" @context
  "property" @context
  receiver: (identifier) @name
  ":" @name
  name: (identifier) @name) @item

(declare_const_declaration
  "declare" @context
  "const" @context
  name: (dotted_name) @name) @item
