(comment) @annotation

(local_variable_declaration
  "local" @context
  (binding_list
    (binding
      name: (identifier) @name
      (#match? @name "^[A-Z][A-Z][A-Z_0-9]*$")) @item))

(const_variable_declaration
  "const" @context
  (binding_list
    (binding name: (identifier) @name) @item))

(type_alias_declaration
  "export"? @context
  "type" @context
  name: (type_identifier) @name) @item

(type_function_declaration
  "export"? @context
  "type" @context
  "function" @context
  name: (type_identifier) @name) @item

(function_declaration
  "function" @context
  name: (_) @name
  (parameters "(" @context ")" @context)) @item

(local_function_declaration
  "local" @context
  "function" @context
  name: (_) @name
  (parameters "(" @context ")" @context)) @item

(const_function_declaration
  "const" @context
  "function" @context
  name: (_) @name
  (parameters "(" @context ")" @context)) @item

(declare_global_declaration
  "declare" @context
  name: (identifier) @name) @item

(declare_global_function_declaration
  "declare" @context
  "function" @context
  name: (identifier) @name) @item
