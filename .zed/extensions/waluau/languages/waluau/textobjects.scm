(function_declaration body: (_)* @function.inside) @function.around
(local_function_declaration body: (_)* @function.inside) @function.around
(const_function_declaration body: (_)* @function.inside) @function.around
(function_expression body: (_)* @function.inside) @function.around
(type_declaration value: (_)* @class.inside) @class.around
(enum_declaration) @class.around
(comment)+ @comment.around
