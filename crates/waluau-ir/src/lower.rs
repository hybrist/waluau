pub fn build(program: &Program) -> Result<Module, Diagnostic> {
    let mut resolved = program.clone();
    waluau_ast::resolve_symbols(&mut resolved)?;
    let erased = erase_opaque_types(&resolved);
    let monomorphic = Monomorphizer::new(&erased).run(&erased)?;
    let tag_ids = collect_variant_tag_ids(&monomorphic, &resolved.type_declarations);
    let mut signatures = HashMap::new();
    let mut field_call_signatures = HashMap::new();
    let mut declared_imports = Vec::new();
    for declared in &monomorphic.declared_imports {
        let symbol_id = declared.symbol_id.ok_or_else(|| {
            Diagnostic::new(format!(
                "declared host function '{}' must have a symbol ID resolved",
                declared.name
            ))
        })?;
        let sig = (
            declared
                .params
                .iter()
                .map(|param| param.ty.clone())
                .collect::<Vec<_>>(),
            declared.return_type.clone(),
        );
        signatures.insert(symbol_id, sig.clone());
        field_call_signatures.insert(declared.name.clone(), sig);
        declared_imports.push(DeclaredImport {
            module: "waluau".to_string(),
            name: declared.name.clone(),
            params: declared
                .params
                .iter()
                .map(|param| param.ty.clone())
                .collect(),
            return_type: declared.return_type.clone(),
            symbol_id,
        });
    }
    for function in &monomorphic.functions {
        let symbol_id = function.symbol_id.ok_or_else(|| {
            Diagnostic::new(format!(
                "function '{}' must have a symbol ID resolved",
                function.name
            ))
        })?;
        let return_type = function.return_type.clone().ok_or_else(|| {
            Diagnostic::new(format!(
                "function '{}' must have a concrete return type before IR lowering",
                function.name
            ))
        })?;
        let sig = (
            function.params.iter().map(|param| param.ty.clone()).collect::<Vec<_>>(),
            return_type,
        );
        signatures.insert(symbol_id, sig.clone());
        field_call_signatures.insert(function.name.to_string(), sig);
    }
    let host_import_signatures = declared_imports
        .iter()
        .map(|declared| {
            (
                declared.symbol_id,
                (declared.params.clone(), declared.return_type.clone()),
            )
        })
        .collect::<HashMap<_, _>>();
    let host_import_names = declared_imports
        .iter()
        .map(|declared| (declared.name.clone(), declared.symbol_id))
        .collect::<HashMap<_, _>>();
    let mut functions = Vec::new();
    for function in &monomorphic.functions {
        let mut lowered = build_function(
            function,
            &signatures,
            &host_import_signatures,
            &host_import_names,
            &field_call_signatures,
            &monomorphic.sources,
            &tag_ids,
        )?;
        functions.push(lowered.remove(0));
        functions.extend(lowered);
    }
    let start = functions
        .iter()
        .position(|function| function.name == "__waluau_top_level_init");
    let module = Module {
        functions,
        declared_imports,
        start,
        tag_ids,
    };
    verify(&module)?;
    Ok(module)
}

struct IfCastParts<'a> {
    target_name: &'a str,
    binding_symbol_id: Option<SymbolId>,
    value: &'a Expr,
    then_body: &'a [Stmt],
    else_body: &'a [Stmt],
}

fn collect_variant_tag_ids(program: &Program, type_declarations: &[TypeDeclaration]) -> BTreeMap<String, i32> {
    let mut tag_ids = BTreeMap::new();
    for decl in type_declarations {
        collect_type_variant_tags(&decl.ty, &mut tag_ids);
    }
    for function in &program.functions {
        for param in &function.params {
            collect_type_variant_tags(&param.ty, &mut tag_ids);
        }
        if let Some(return_type) = &function.return_type {
            collect_type_variant_tags(return_type, &mut tag_ids);
        }
        for stmt in &function.body {
            collect_stmt_variant_tags(stmt, &mut tag_ids);
        }
    }
    for stmt in &program.top_level {
        collect_stmt_variant_tags(stmt, &mut tag_ids);
    }
    // Tagged coroutine resume can produce all three runtime variants even when
    // a source annotation only models a subset such as `Finished | Error`.
    insert_variant_tag(&mut tag_ids, "Yielded");
    insert_variant_tag(&mut tag_ids, "Finished");
    insert_variant_tag(&mut tag_ids, "Error");
    tag_ids
}

fn insert_variant_tag(tag_ids: &mut BTreeMap<String, i32>, tag: &str) {
    if !tag_ids.contains_key(tag) {
        tag_ids.insert(tag.to_string(), tag_ids.len() as i32);
    }
}

fn collect_type_variant_tags(ty: &Type, tag_ids: &mut BTreeMap<String, i32>) {
    match ty {
        Type::TaggedVariant(variant) => {
            insert_variant_tag(tag_ids, &variant.tag);
            collect_type_variant_tags(variant.payload.as_ref(), tag_ids);
        }
        Type::TaggedUnion(variants) => {
            for variant in variants {
                insert_variant_tag(tag_ids, &variant.tag);
                collect_type_variant_tags(variant.payload.as_ref(), tag_ids);
            }
        }
        Type::Opaque { ty, .. }
        | Type::Array(ty)
        | Type::Nullable(ty)
        | Type::ExternSubtype(ty) => {
            collect_type_variant_tags(ty, tag_ids)
        }
        Type::Multi(types) => {
            for ty in types {
                collect_type_variant_tags(ty, tag_ids);
            }
        }
        Type::Function {
            params,
            return_type,
        } => {
            for param in params {
                collect_type_variant_tags(param, tag_ids);
            }
            collect_type_variant_tags(return_type, tag_ids);
        }
        Type::Record(fields) => {
            for ty in fields.values() {
                collect_type_variant_tags(ty, tag_ids);
            }
        }
        Type::Named { type_args, .. } => {
            for ty in type_args {
                collect_type_variant_tags(ty, tag_ids);
            }
        }
        Type::Unit
        | Type::Bool
        | Type::Numeric(_)
        | Type::String
        | Type::Bytes
        | Type::Extern
        | Type::Nil
        | Type::Unknown
        | Type::Thread
        | Type::TypeParam(_) => {}
    }
}

fn collect_stmt_variant_tags(stmt: &Stmt, tag_ids: &mut BTreeMap<String, i32>) {
    match stmt {
        Stmt::Let { ty, value, .. } => {
            if let Some(ty) = ty {
                collect_type_variant_tags(ty, tag_ids);
            }
            collect_expr_variant_tags(value, tag_ids);
        }
        Stmt::Assign { value, .. } | Stmt::Return(value) | Stmt::Expr(value) => {
            collect_expr_variant_tags(value, tag_ids);
        }
        Stmt::IndexAssign {
            base,
            index,
            value,
            ..
        } => {
            collect_expr_variant_tags(base, tag_ids);
            collect_expr_variant_tags(index, tag_ids);
            collect_expr_variant_tags(value, tag_ids);
        }
        Stmt::FieldAssign { base, value, .. } => {
            collect_expr_variant_tags(base, tag_ids);
            collect_expr_variant_tags(value, tag_ids);
        }
        Stmt::If {
            condition,
            then_body,
            else_body,
        } => {
            collect_expr_variant_tags(condition, tag_ids);
            for stmt in then_body {
                collect_stmt_variant_tags(stmt, tag_ids);
            }
            for stmt in else_body {
                collect_stmt_variant_tags(stmt, tag_ids);
            }
        }
        Stmt::IfCast {
            target_ty,
            value,
            then_body,
            else_body,
            ..
        } => {
            collect_type_variant_tags(target_ty, tag_ids);
            collect_expr_variant_tags(value, tag_ids);
            for stmt in then_body {
                collect_stmt_variant_tags(stmt, tag_ids);
            }
            for stmt in else_body {
                collect_stmt_variant_tags(stmt, tag_ids);
            }
        }
        Stmt::While { condition, body } => {
            collect_expr_variant_tags(condition, tag_ids);
            for stmt in body {
                collect_stmt_variant_tags(stmt, tag_ids);
            }
        }
        Stmt::Repeat { body, condition } => {
            for stmt in body {
                collect_stmt_variant_tags(stmt, tag_ids);
            }
            collect_expr_variant_tags(condition, tag_ids);
        }
        Stmt::NumericFor {
            start,
            stop,
            step,
            body,
            ..
        } => {
            collect_expr_variant_tags(start, tag_ids);
            collect_expr_variant_tags(stop, tag_ids);
            if let Some(step) = step {
                collect_expr_variant_tags(step, tag_ids);
            }
            for stmt in body {
                collect_stmt_variant_tags(stmt, tag_ids);
            }
        }
        Stmt::ForIn { iterator, body, .. } => {
            collect_expr_variant_tags(iterator, tag_ids);
            for stmt in body {
                collect_stmt_variant_tags(stmt, tag_ids);
            }
        }
        Stmt::ReturnMulti(values) => {
            for value in values {
                collect_expr_variant_tags(value, tag_ids);
            }
        }
        Stmt::LetMulti { bindings, values } => {
            for binding in bindings {
                if let Some(ty) = &binding.ty {
                    collect_type_variant_tags(ty, tag_ids);
                }
            }
            for value in values {
                collect_expr_variant_tags(value, tag_ids);
            }
        }
        Stmt::AssignMulti { values, .. } => {
            for value in values {
                collect_expr_variant_tags(value, tag_ids);
            }
        }
        Stmt::Break | Stmt::Continue => {}
    }
}

fn collect_expr_variant_tags(expr: &Expr, tag_ids: &mut BTreeMap<String, i32>) {
    match expr {
        Expr::Unary { expr, .. } => {
            collect_expr_variant_tags(expr, tag_ids);
        }
        Expr::Cast { expr, ty, .. } => {
            collect_expr_variant_tags(expr, tag_ids);
            collect_type_variant_tags(ty, tag_ids);
        }
        Expr::IsVariant { expr, .. } => collect_expr_variant_tags(expr, tag_ids),
        Expr::Binary { left, right, .. } => {
            collect_expr_variant_tags(left, tag_ids);
            collect_expr_variant_tags(right, tag_ids);
        }
        Expr::If {
            condition,
            then_expr,
            else_expr,
            ..
        } => {
            collect_expr_variant_tags(condition, tag_ids);
            collect_expr_variant_tags(then_expr, tag_ids);
            collect_expr_variant_tags(else_expr, tag_ids);
        }
        Expr::Call {
            callee,
            type_args,
            args,
            ..
        } => {
            collect_expr_variant_tags(callee, tag_ids);
            for ty in type_args {
                collect_type_variant_tags(ty, tag_ids);
            }
            for arg in args {
                collect_expr_variant_tags(arg, tag_ids);
            }
        }
        Expr::MethodCall {
            receiver,
            args,
            type_args,
            ..
        } => {
            collect_expr_variant_tags(receiver, tag_ids);
            for ty in type_args {
                collect_type_variant_tags(ty, tag_ids);
            }
            for arg in args {
                collect_expr_variant_tags(arg, tag_ids);
            }
        }
        Expr::Function(function) => {
            for param in &function.params {
                collect_type_variant_tags(&param.ty, tag_ids);
            }
            if let Some(return_type) = &function.return_type {
                collect_type_variant_tags(return_type, tag_ids);
            }
            for stmt in &function.body {
                collect_stmt_variant_tags(stmt, tag_ids);
            }
        }
        Expr::ArrayLiteral { elements, .. } => {
            for element in elements {
                collect_expr_variant_tags(element, tag_ids);
            }
        }
        Expr::TableLiteral { fields, .. } => {
            for field in fields {
                collect_expr_variant_tags(&field.value, tag_ids);
            }
        }
        Expr::Field { base, .. } => collect_expr_variant_tags(base, tag_ids),
        Expr::Index { base, index, .. } => {
            collect_expr_variant_tags(base, tag_ids);
            collect_expr_variant_tags(index, tag_ids);
        }
        Expr::Number(..)
        | Expr::Bool(..)
        | Expr::Nil(..)
        | Expr::String(..)
        | Expr::Bytes(..)
        | Expr::Name(..)
        | Expr::Require(..) => {}
    }
}

fn erase_opaque_types(program: &Program) -> Program {
    Program {
        functions: program.functions.iter().map(erase_function_opaque_types).collect(),
        declared_imports: program
            .declared_imports
            .iter()
            .map(|declared| waluau_ast::DeclaredImport {
                name: declared.name.clone(),
                symbol_id: declared.symbol_id,
                params: declared
                    .params
                    .iter()
                    .map(|param| waluau_ast::Param {
                        name: param.name.clone(),
                        symbol_id: param.symbol_id,
                        ty: erase_type_opaque_types(&param.ty),
                    })
                    .collect(),
                return_type: erase_type_opaque_types(&declared.return_type),
            })
            .collect(),
        type_declarations: Vec::new(),
        top_level: program.top_level.iter().map(erase_stmt_opaque_types).collect(),
        export: program.export.as_ref().map(erase_expr_opaque_types),
        sources: program.sources.clone(),
        entry_file_path: program.entry_file_path.clone(),
    }
}

fn erase_function_opaque_types(function: &AstFunction) -> AstFunction {
    AstFunction {
        name: function.name.clone(),
        symbol_id: function.symbol_id,
        type_params: function.type_params.clone(),
        params: function
            .params
            .iter()
            .map(|param| waluau_ast::Param {
                name: param.name.clone(),
                symbol_id: param.symbol_id,
                ty: erase_type_opaque_types(&param.ty),
            })
            .collect(),
        return_type: function
            .return_type
            .as_ref()
            .map(erase_type_opaque_types),
        body: function.body.iter().map(erase_stmt_opaque_types).collect(),
        file_path: function.file_path.clone(),
    }
}

fn erase_stmt_opaque_types(stmt: &Stmt) -> Stmt {
    match stmt {
        Stmt::Let {
            name,
            symbol_id,
            rebindability,
            ty,
            value,
        } => Stmt::Let {
            name: name.clone(),
            symbol_id: *symbol_id,
            rebindability: *rebindability,
            ty: ty.as_ref().map(erase_type_opaque_types),
            value: erase_expr_opaque_types(value),
        },
        Stmt::Assign { op, name, symbol_id, value } => Stmt::Assign {
            op: *op,
            name: name.clone(),
            symbol_id: *symbol_id,
            value: erase_expr_opaque_types(value),
        },
        Stmt::IndexAssign {
            op,
            base,
            index,
            value,
        } => Stmt::IndexAssign {
            op: *op,
            base: Box::new(erase_expr_opaque_types(base)),
            index: Box::new(erase_expr_opaque_types(index)),
            value: erase_expr_opaque_types(value),
        },
        Stmt::FieldAssign {
            op,
            base,
            name,
            value,
        } => Stmt::FieldAssign {
            op: *op,
            base: Box::new(erase_expr_opaque_types(base)),
            name: name.clone(),
            value: erase_expr_opaque_types(value),
        },
        Stmt::If {
            condition,
            then_body,
            else_body,
        } => Stmt::If {
            condition: erase_expr_opaque_types(condition),
            then_body: then_body.iter().map(erase_stmt_opaque_types).collect(),
            else_body: else_body.iter().map(erase_stmt_opaque_types).collect(),
        },
        Stmt::IfCast {
            target_name,
            target_ty,
            binding,
            binding_symbol_id,
            value,
            then_body,
            else_body,
        } => Stmt::IfCast {
            target_name: target_name.clone(),
            target_ty: erase_type_opaque_types(target_ty),
            binding: binding.clone(),
            binding_symbol_id: *binding_symbol_id,
            value: erase_expr_opaque_types(value),
            then_body: then_body.iter().map(erase_stmt_opaque_types).collect(),
            else_body: else_body.iter().map(erase_stmt_opaque_types).collect(),
        },
        Stmt::While { condition, body } => Stmt::While {
            condition: erase_expr_opaque_types(condition),
            body: body.iter().map(erase_stmt_opaque_types).collect(),
        },
        Stmt::Repeat { body, condition } => Stmt::Repeat {
            body: body.iter().map(erase_stmt_opaque_types).collect(),
            condition: erase_expr_opaque_types(condition),
        },
        Stmt::NumericFor {
            name,
            symbol_id,
            start,
            stop,
            step,
            body,
        } => Stmt::NumericFor {
            name: name.clone(),
            symbol_id: *symbol_id,
            start: erase_expr_opaque_types(start),
            stop: erase_expr_opaque_types(stop),
            step: step.as_ref().map(erase_expr_opaque_types),
            body: body.iter().map(erase_stmt_opaque_types).collect(),
        },
        Stmt::ForIn {
            names,
            symbol_ids,
            iterator,
            body,
        } => Stmt::ForIn {
            names: names.clone(),
            symbol_ids: symbol_ids.clone(),
            iterator: erase_expr_opaque_types(iterator),
            body: body.iter().map(erase_stmt_opaque_types).collect(),
        },
        Stmt::Break => Stmt::Break,
        Stmt::Continue => Stmt::Continue,
        Stmt::Return(value) => Stmt::Return(erase_expr_opaque_types(value)),
        Stmt::ReturnMulti(values) => {
            Stmt::ReturnMulti(values.iter().map(erase_expr_opaque_types).collect())
        }
        Stmt::LetMulti { bindings, values } => Stmt::LetMulti {
            bindings: bindings
                .iter()
                .map(|binding| waluau_ast::Binding {
                    name: binding.name.clone(),
                    symbol_id: binding.symbol_id,
                    rebindability: binding.rebindability,
                    ty: binding.ty.as_ref().map(erase_type_opaque_types),
                })
                .collect(),
            values: values.iter().map(erase_expr_opaque_types).collect(),
        },
        Stmt::AssignMulti { targets, symbol_ids, values } => Stmt::AssignMulti {
            targets: targets.clone(),
            symbol_ids: symbol_ids.clone(),
            values: values.iter().map(erase_expr_opaque_types).collect(),
        },
        Stmt::Expr(expr) => Stmt::Expr(erase_expr_opaque_types(expr)),
    }
}

fn erase_expr_opaque_types(expr: &Expr) -> Expr {
    match expr {
        Expr::Number(..)
        | Expr::Bool(..)
        | Expr::Nil(..)
        | Expr::String(..)
        | Expr::Bytes(..)
        | Expr::Name(..)
        | Expr::Require(..) => expr.clone(),
        Expr::IsVariant { expr, tag, span } => Expr::IsVariant {
            expr: Box::new(erase_expr_opaque_types(expr)),
            tag: tag.clone(),
            span: *span,
        },
        Expr::Unary { op, expr, span } => Expr::Unary {
            op: *op,
            expr: Box::new(erase_expr_opaque_types(expr)),
            span: *span,
        },
        Expr::Cast { expr, ty, span } => Expr::Cast {
            expr: Box::new(erase_expr_opaque_types(expr)),
            ty: erase_type_opaque_types(ty),
            span: *span,
        },
        Expr::Binary {
            op,
            left,
            right,
            span,
        } => Expr::Binary {
            op: *op,
            left: Box::new(erase_expr_opaque_types(left)),
            right: Box::new(erase_expr_opaque_types(right)),
            span: *span,
        },
        Expr::If {
            condition,
            then_expr,
            else_expr,
            span,
        } => Expr::If {
            condition: Box::new(erase_expr_opaque_types(condition)),
            then_expr: Box::new(erase_expr_opaque_types(then_expr)),
            else_expr: Box::new(erase_expr_opaque_types(else_expr)),
            span: *span,
        },
        Expr::Call {
            callee,
            type_args,
            args,
            span,
            method_call_origin,
        } => Expr::Call {
            callee: Box::new(erase_expr_opaque_types(callee)),
            type_args: type_args.iter().map(erase_type_opaque_types).collect(),
            args: args.iter().map(erase_expr_opaque_types).collect(),
            span: *span,
            method_call_origin: method_call_origin.clone(),
        },
        Expr::MethodCall {
            receiver,
            name,
            args,
            span,
            type_args,
        } => Expr::MethodCall {
            receiver: Box::new(erase_expr_opaque_types(receiver)),
            name: name.clone(),
            args: args.iter().map(erase_expr_opaque_types).collect(),
            span: *span,
            type_args: type_args.clone(),
        },
        Expr::Function(function) => Expr::Function(waluau_ast::FunctionExpr {
            name: function.name.clone(),
            symbol_id: function.symbol_id,
            implicit_self: function.implicit_self.clone(),
            type_params: function.type_params.clone(),
            params: function
                .params
                .iter()
                .map(|param| waluau_ast::Param {
                    name: param.name.clone(),
                    symbol_id: param.symbol_id,
                    ty: erase_type_opaque_types(&param.ty),
                })
                .collect(),
            return_type: function.return_type.as_ref().map(erase_type_opaque_types),
            body: function.body.iter().map(erase_stmt_opaque_types).collect(),
            file_path: function.file_path.clone(),
            span: function.span,
        }),
        Expr::ArrayLiteral { elements, span } => Expr::ArrayLiteral {
            elements: elements.iter().map(erase_expr_opaque_types).collect(),
            span: *span,
        },
        Expr::TableLiteral { fields, span } => Expr::TableLiteral {
            fields: fields
                .iter()
                .map(|field| waluau_ast::TableField {
                    name: field.name.clone(),
                    value: erase_expr_opaque_types(&field.value),
                })
                .collect(),
            span: *span,
        },
        Expr::Field { base, name, span } => Expr::Field {
            base: Box::new(erase_expr_opaque_types(base)),
            name: name.clone(),
            span: *span,
        },
        Expr::Index { base, index, span } => Expr::Index {
            base: Box::new(erase_expr_opaque_types(base)),
            index: Box::new(erase_expr_opaque_types(index)),
            span: *span,
        },
    }
}

fn erase_type_opaque_types(ty: &Type) -> Type {
    match ty {
        Type::Opaque { ty, .. } => erase_type_opaque_types(ty),
        Type::ExternSubtype(_) => Type::Extern,
        Type::Nullable(inner) => Type::Nullable(Box::new(erase_type_opaque_types(inner))),
        Type::Array(inner) => Type::Array(Box::new(erase_type_opaque_types(inner))),
        Type::Multi(types) => Type::Multi(types.iter().map(erase_type_opaque_types).collect()),
        Type::Function {
            params,
            return_type,
        } => Type::Function {
            params: params.iter().map(erase_type_opaque_types).collect(),
            return_type: Box::new(erase_type_opaque_types(return_type)),
        },
        Type::Record(fields) => Type::Record(
            fields
                .iter()
                .map(|(name, ty)| (name.clone(), erase_type_opaque_types(ty)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// `TaggedUnion`/`TaggedVariant` source-level types are represented at the IR
/// level by the canonical `{ tag: i32, value: unknown }` record. Use this
/// whenever a source-level type ends up annotating an IR instruction (array
/// cells, casts, etc.) so the annotation matches the value's actual runtime
/// representation that `verify` checks against.
fn to_runtime_type(ty: &Type) -> Type {
    match ty {
        Type::TaggedUnion(_) | Type::TaggedVariant(_) => Type::canonical_tagged_union_record(),
        other => other.clone(),
    }
}

pub(crate) fn build_function(
    function: &AstFunction,
    signatures: &HashMap<SymbolId, (Vec<Type>, Type)>,
    host_import_signatures: &HashMap<SymbolId, (Vec<Type>, Type)>,
    host_import_names: &HashMap<String, SymbolId>,
    field_call_signatures: &HashMap<String, (Vec<Type>, Type)>,
    sources: &BTreeMap<String, String>,
    tag_ids: &BTreeMap<String, i32>,
) -> Result<Vec<Function>, Diagnostic> {
    let return_type = function.return_type.clone().ok_or_else(|| {
        inference_diagnostic(
            "inference/unsupported",
            DiagnosticCategory::Unsupported,
            format!(
                "function '{}' must have a concrete return type before IR lowering",
                function.name
            ),
            "ensure type inference runs before IR lowering or add an explicit return type",
        )
    })?;
    let mut out = Function {
        name: function.name.to_string(),
        params: function
            .params
            .iter()
            .map(|param| (param.name.clone(), param.ty.clone()))
            .collect(),
        return_type,
        entry: BlockId(0),
        blocks: BTreeMap::new(),
        next_value: 0,
        capture_count: 0,
        value_symbols: BTreeMap::new(),
        symbol_id: function.symbol_id,
    };

    out.blocks.insert(
        out.entry,
        BasicBlock {
            id: out.entry,
            instructions: Vec::new(),
            terminator: Terminator::Unreachable { span: None },
        },
    );

    let mut env = HashMap::new();
    let mut type_env = HashMap::new();
    let entry = out.entry;
    let captured_symbols: HashSet<SymbolId> = collect_nested_function_capture_names(function);

    for (index, param) in function.params.iter().enumerate() {
        let symbol_id = param.symbol_id.expect("param has resolved symbol_id");
        let value = out.next_value();
        block_mut(&mut out, entry)
            .instructions
            .push((value, Instruction::Param(index)));
        if captured_symbols.contains(&symbol_id) {
            let cell = out.next_value();
            block_mut(&mut out, entry).instructions.push((
                cell,
                Instruction::ArrayNew {
                    element_ty: to_runtime_type(&param.ty),
                    elements: vec![value],
                },
            ));
            env.insert(symbol_id, cell);
            out.value_symbols.insert(cell, symbol_id);
        } else {
            env.insert(symbol_id, value);
        }
        out.value_symbols.insert(value, symbol_id);
        type_env.insert(symbol_id, param.ty.clone());
    }

    let mut builder = Builder {
        function: out,
        current_block: BlockId(0),
        next_block: 1,
        signatures,
        host_import_signatures,
        host_import_names,
        field_call_signatures,
        lifted_functions: Vec::new(),
        lambda_counter: 0,
        loop_stack: Vec::new(),
        cell_names: captured_symbols,
        sources,
        file_path: function.file_path.clone(),
        tag_ids,
    };
    for stmt in &function.body {
        if builder.current_block == DEAD_BLOCK {
            break;
        }
        builder.lower_stmt(stmt, &mut env, &mut type_env)?;
    }
    if builder.current_block != DEAD_BLOCK && builder.function.return_type == Type::Unit {
        let value = builder.emit(Instruction::Unit);
        builder.set_terminator(builder.current_block, Terminator::Return(value));
        builder.current_block = DEAD_BLOCK;
    }
    let mut functions = vec![builder.function];
    functions.extend(builder.lifted_functions);
    Ok(functions)
}

const DEAD_BLOCK: BlockId = BlockId(usize::MAX);

struct Builder<'a> {
    function: Function,
    current_block: BlockId,
    next_block: usize,
    signatures: &'a HashMap<SymbolId, (Vec<Type>, Type)>,
    host_import_signatures: &'a HashMap<SymbolId, (Vec<Type>, Type)>,
    host_import_names: &'a HashMap<String, SymbolId>,
    field_call_signatures: &'a HashMap<String, (Vec<Type>, Type)>,
    lifted_functions: Vec<Function>,
    lambda_counter: usize,
    loop_stack: Vec<LoopContext>,
    /// SymbolIds that are represented as 1-element array "cells" to support mutable capture.
    cell_names: HashSet<SymbolId>,
    sources: &'a BTreeMap<String, String>,
    file_path: String,
    /// Stable discriminant IDs for tagged-union variant names, shared across the
    /// whole module so constructors and checks in different functions agree.
    tag_ids: &'a BTreeMap<String, i32>,
}

#[derive(Clone)]
struct LoopContext {
    header: BlockId,
    continue_target: BlockId,
    break_target: BlockId,
    phis: HashMap<SymbolId, ValueId>,
}

fn builtin_name(callee: &Expr) -> Option<String> {
    match callee {
        Expr::Name(name, _, _) => Some(name.clone()),
        Expr::Field { base, name, .. } => match base.as_ref() {
            Expr::Name(namespace, _, _) => Some(format!("{namespace}.{name}")),
            _ => None,
        },
        _ => None,
    }
}

fn method_signature_name(base: &str, method: &str) -> String {
    format!("{base}.{method}")
}

fn property_getter_name(base: &str, property: &str) -> String {
    format!("{base}.get_{property}")
}

fn property_setter_name(base: &str, property: &str) -> String {
    format!("{base}.set_{property}")
}

fn method_receiver_matches(expected: &Type, actual: &Type) -> bool {
    if expected == actual {
        return true;
    }
    match (expected, actual) {
        (Type::Record(expected_fields), Type::Record(actual_fields)) => expected_fields
            .iter()
            .all(|(name, expected_ty)| actual_fields.get(name) == Some(expected_ty)),
        _ => false,
    }
}

fn method_signature(
    receiver: &Expr,
    name: &str,
    signatures: &HashMap<String, (Vec<Type>, Type)>,
) -> Option<(Vec<Type>, Type)> {
    let Expr::Name(base, _, _) = receiver else {
        return None;
    };
    signatures.get(&method_signature_name(base, name)).cloned()
}

fn type_method_signature(
    receiver_ty: &Type,
    name: &str,
    signatures: &HashMap<String, (Vec<Type>, Type)>,
) -> Option<(String, Vec<Type>, Type)> {
    if let Type::Opaque { name: type_name, .. } = receiver_ty {
        let direct_name = method_signature_name(type_name, name);
        return signatures
            .get(&direct_name)
            .cloned()
            .map(|(params, return_type)| (direct_name, params, return_type));
    }

    let suffix = format!(".{name}");
    let mut matches = signatures
        .iter()
        .filter_map(|(direct_name, (params, return_type))| {
            if !direct_name.ends_with(&suffix) {
                return None;
            }
            let receiver_param = params.first()?;
            method_receiver_matches(receiver_param, receiver_ty)
                .then(|| (direct_name.clone(), params.clone(), return_type.clone()))
        })
        .collect::<Vec<_>>();
    (matches.len() == 1).then(|| matches.remove(0))
}

fn type_property_getter_signature(
    receiver_ty: &Type,
    name: &str,
    signatures: &HashMap<String, (Vec<Type>, Type)>,
) -> Option<(String, Vec<Type>, Type)> {
    if let Type::Opaque { name: type_name, .. } = receiver_ty {
        let direct_name = property_getter_name(type_name, name);
        if let Some((params, return_type)) = signatures.get(&direct_name).cloned() {
            return Some((direct_name, params, return_type));
        }
    }

    let suffix = format!(".get_{name}");
    let mut matches = signatures
        .iter()
        .filter_map(|(direct_name, (params, return_type))| {
            if !direct_name.ends_with(&suffix) || params.len() != 1 {
                return None;
            }
            let receiver_param = params.first()?;
            method_receiver_matches(receiver_param, receiver_ty)
                .then(|| (direct_name.clone(), params.clone(), return_type.clone()))
        })
        .collect::<Vec<_>>();
    (matches.len() == 1).then(|| matches.remove(0))
}

fn type_property_setter_signature(
    receiver_ty: &Type,
    name: &str,
    signatures: &HashMap<String, (Vec<Type>, Type)>,
) -> Option<(String, Vec<Type>, Type)> {
    if let Type::Opaque { name: type_name, .. } = receiver_ty {
        let direct_name = property_setter_name(type_name, name);
        if let Some((params, return_type)) = signatures.get(&direct_name).cloned() {
            return Some((direct_name, params, return_type));
        }
    }

    let suffix = format!(".set_{name}");
    let mut matches = signatures
        .iter()
        .filter_map(|(direct_name, (params, return_type))| {
            if !direct_name.ends_with(&suffix) || params.len() != 2 {
                return None;
            }
            let receiver_param = params.first()?;
            method_receiver_matches(receiver_param, receiver_ty)
                .then(|| (direct_name.clone(), params.clone(), return_type.clone()))
        })
        .collect::<Vec<_>>();
    (matches.len() == 1).then(|| matches.remove(0))
}

fn direct_field_call_name(
    callee: &Expr,
    signatures: &HashMap<String, (Vec<Type>, Type)>,
) -> Option<(String, Vec<Type>, Type)> {
    let Expr::Field { base, name, .. } = callee else {
        return None;
    };
    let Expr::Name(base, _, _) = base.as_ref() else {
        return None;
    };
    let direct_name = method_signature_name(base, name);
    signatures
        .get(&direct_name)
        .cloned()
        .map(|(params, return_type)| (direct_name, params, return_type))
}

impl Builder<'_> {
    fn function_expr_return_type(function: &waluau_ast::FunctionExpr) -> Result<Type, Diagnostic> {
        function.return_type.clone().ok_or_else(|| {
            Diagnostic::new(
                "function return inference is only supported for named functions in this MVP",
            )
        })
    }

    fn new_block(&mut self) -> BlockId {
        let id = BlockId(self.next_block);
        self.next_block += 1;
        self.function.blocks.insert(
            id,
            BasicBlock {
                id,
                instructions: Vec::new(),
                terminator: Terminator::Unreachable { span: None },
            },
        );
        id
    }

    fn emit(&mut self, instruction: Instruction) -> ValueId {
        let value = self.function.next_value();
        block_mut(&mut self.function, self.current_block)
            .instructions
            .push((value, instruction));
        value
    }

    /// Return the stable i32 discriminant for a tagged-union variant name.
    fn variant_tag_id(&self, name: &str) -> Result<i32, Diagnostic> {
        self.tag_ids
            .get(name)
            .copied()
            .ok_or_else(|| Diagnostic::new(format!("unknown tagged-union variant '{name}'")))
    }

    /// Unbox the payload of a canonical tagged-union record (`{ tag: i32, value: anyref }`):
    /// reads the `value` field and casts the resulting `unknown` to the payload type.
    fn unbox_tagged_variant_value(
        &mut self,
        base: ValueId,
        payload_ty: &Type,
    ) -> Result<ValueId, Diagnostic> {
        if matches!(payload_ty, Type::String | Type::Bytes) {
            return Err(Diagnostic::new(format!(
                "reading .value of type {payload_ty} is not yet supported \
                 (string/bytes payloads cannot be unboxed from anyref)"
            )));
        }
        let unknown_val = self.emit(Instruction::StructGet {
            base,
            field: "value".to_string(),
            field_ty: Type::Unknown,
        });
        Ok(self.emit(Instruction::Cast {
            value: unknown_val,
            from: Type::Unknown,
            to: payload_ty.clone(),
        }))
    }

    fn instruction(&self, value: ValueId) -> Option<&Instruction> {
        self.function
            .blocks
            .values()
            .flat_map(|block| block.instructions.iter())
            .find_map(|(id, instruction)| (*id == value).then_some(instruction))
    }

    fn direct_record_field_closure_name(&self, base: ValueId, field: &str) -> Option<String> {
        let Instruction::StructNew { struct_ty, fields } = self.instruction(base)? else {
            return None;
        };
        let Type::Record(record_fields) = struct_ty else {
            return None;
        };
        let field_index = record_fields.keys().position(|name| name == field)?;
        let field_value = *fields.get(field_index)?;
        let Instruction::Closure { name, captures, .. } = self.instruction(field_value)? else {
            return None;
        };
        captures.is_empty().then(|| name.clone())
    }

    fn set_terminator(&mut self, block: BlockId, terminator: Terminator) {
        block_mut(&mut self.function, block).terminator = terminator;
    }

    fn lower_break(&mut self, _env: &HashMap<SymbolId, ValueId>) -> Result<(), Diagnostic> {
        let Some(loop_ctx) = self.loop_stack.last() else {
            return Err(Diagnostic::new("break is only allowed inside loops"));
        };
        if self.current_block == DEAD_BLOCK {
            return Ok(());
        }
        let current = self.current_block;
        self.set_terminator(current, Terminator::Jump(loop_ctx.break_target));
        self.current_block = DEAD_BLOCK;
        Ok(())
    }

    fn lower_continue(&mut self, env: &HashMap<SymbolId, ValueId>) -> Result<(), Diagnostic> {
        let Some(loop_ctx) = self.loop_stack.last() else {
            return Err(Diagnostic::new("continue is only allowed inside loops"));
        };
        if self.current_block == DEAD_BLOCK {
            return Ok(());
        }
        let current = self.current_block;
        for (id, phi) in &loop_ctx.phis {
            if let Some(value) = env.get(id).copied() {
                add_phi_incoming(&mut self.function, loop_ctx.header, *phi, (current, value));
            }
        }
        self.set_terminator(current, Terminator::Jump(loop_ctx.continue_target));
        self.current_block = DEAD_BLOCK;
        Ok(())
    }

    fn lower_stmt(
        &mut self,
        stmt: &Stmt,
        env: &mut HashMap<SymbolId, ValueId>,
        types: &mut HashMap<SymbolId, Type>,
    ) -> Result<(), Diagnostic> {
        match stmt {
            Stmt::Let {
                name: _,
                symbol_id,
                rebindability: _,
                ty,
                value,
            } => {
                let symbol_id = symbol_id.expect("symbol_id should be resolved");
                let inferred_ty = if let Some(ty) = ty.clone() {
                    ty
                } else if matches!(value, Expr::ArrayLiteral { elements, .. } if elements.is_empty()) {
                    // Keep parity with HIR inference: bare `{}` in local initialization starts
                    // as an empty record so subsequent `t.field = ...` can shape it.
                    Type::Record(BTreeMap::new())
                } else {
                    self.infer_expr_type(value, types, None)?
                };
                let value = self.lower_expr(value, env, types, Some(inferred_ty.clone()))?;
                // If this local is captured by any nested function, represent it as a 1-element
                // array cell so closures can observe and mutate the same storage location.
                if self.cell_names.contains(&symbol_id) {
                    let cell = self.emit(Instruction::ArrayNew {
                        element_ty: to_runtime_type(&inferred_ty),
                        elements: vec![value],
                    });
                    env.insert(symbol_id, cell);
                    self.function.value_symbols.insert(cell, symbol_id);
                    // Keep the declared type as the inner element type for type checking.
                    types.insert(symbol_id, inferred_ty);
                } else {
                    env.insert(symbol_id, value);
                    types.insert(symbol_id, inferred_ty);
                }
                self.function.value_symbols.insert(value, symbol_id);
            }
            Stmt::Assign { op, name, symbol_id, value } => {
                let symbol_id = symbol_id.expect("symbol_id should be resolved");
                let ty = types.get(&symbol_id).cloned().ok_or_else(|| {
                    Diagnostic::new(format!("unknown local '{name}' during IR lowering"))
                })?;
                if self.cell_names.contains(&symbol_id) {
                    // Captured local: stored in a 1-element array (cell). Perform ArraySet
                    // rather than rebinding the env entry.
                    let cell = env.get(&symbol_id).copied().ok_or_else(|| {
                        Diagnostic::new(format!("unknown local '{name}' during IR lowering"))
                    })?;
                    let index0 = self.emit(Instruction::Number {
                        ty: NumericType::I32,
                        literal: NumberLiteral { raw: "0".into() },
                    });
                    match op {
                        AssignOp::Set => {
                            let rhs = self.lower_expr(value, env, types, Some(ty.clone()))?;
                            self.function.value_symbols.insert(rhs, symbol_id);
                            self.emit(Instruction::ArraySet {
                                array: cell,
                                index: index0,
                                value: rhs,
                                element_ty: to_runtime_type(&ty),
                            });
                        }
                        AssignOp::Add => {
                            if !ty.is_numeric() {
                                return Err(Diagnostic::new(format!(
                                    "compound assignment to '{}' requires a numeric target",
                                    name
                                )));
                            }
                            // load current, add, store back
                            let current = self.emit(Instruction::ArrayGet {
                                array: cell,
                                index: index0,
                                element_ty: ty.clone(),
                            });
                            let rhs = self.lower_expr(value, env, types, Some(ty.clone()))?;
                            let sum = self.emit(Instruction::Binary {
                                op: BinaryOp::Add,
                                left: current,
                                right: rhs,
                                operand_ty: ty.clone(),
                                result_ty: ty.clone(),
                            });
                            self.function.value_symbols.insert(sum, symbol_id);
                            self.emit(Instruction::ArraySet {
                                array: cell,
                                index: index0,
                                value: sum,
                                element_ty: ty.clone(),
                            });
                        }
                    }
                    // Do not replace env entry -- it remains the cell.
                } else {
                    let value = match op {
                        AssignOp::Set => self.lower_expr(value, env, types, Some(ty))?,
                        AssignOp::Add => {
                            if !ty.is_numeric() {
                                return Err(Diagnostic::new(format!(
                                    "compound assignment to '{}' requires a numeric target",
                                    name
                                )));
                            }
                            let current = *env.get(&symbol_id).ok_or_else(|| {
                                Diagnostic::new(format!(
                                    "unknown local '{name}' during IR lowering"
                                ))
                            })?;
                            let rhs = self.lower_expr(value, env, types, Some(ty.clone()))?;
                            self.emit(Instruction::Binary {
                                op: BinaryOp::Add,
                                left: current,
                                right: rhs,
                                operand_ty: ty.clone(),
                                result_ty: ty,
                            })
                        }
                    };
                    env.insert(symbol_id, value);
                    self.function.value_symbols.insert(value, symbol_id);
                }
            }
            Stmt::IndexAssign {
                op,
                base,
                index,
                value,
            } => {
                let base_ty = self.infer_expr_type(base, types, None)?;
                let element_ty = base_ty.element_type().ok_or_else(|| {
                    Diagnostic::new("array element assignment requires an array operand")
                })?;
                let array = self.lower_expr(base, env, types, Some(base_ty))?;
                let index =
                    self.lower_expr(index, env, types, Some(Type::Numeric(NumericType::I32)))?;
                let value = match op {
                    AssignOp::Set => {
                        self.lower_expr(value, env, types, Some(element_ty.clone()))?
                    }
                    AssignOp::Add => {
                        if !element_ty.is_numeric() {
                            return Err(Diagnostic::new(
                                "compound array assignment requires numeric elements",
                            ));
                        }
                        let current = self.emit(Instruction::ArrayGet {
                            array,
                            index,
                            element_ty: element_ty.clone(),
                        });
                        let rhs = self.lower_expr(value, env, types, Some(element_ty.clone()))?;
                        self.emit(Instruction::Binary {
                            op: BinaryOp::Add,
                            left: current,
                            right: rhs,
                            operand_ty: element_ty.clone(),
                            result_ty: element_ty.clone(),
                        })
                    }
                };
                self.emit(Instruction::ArraySet {
                    array,
                    index,
                    value,
                    element_ty,
                });
            }
            Stmt::FieldAssign { .. } => {
                let Stmt::FieldAssign {
                    op,
                    base,
                    name,
                    value,
                } = stmt
                else {
                    unreachable!();
                };
                let base_ty = self.infer_expr_type(base, types, None)?;
                if let Some((setter_name, params, return_type)) =
                    type_property_setter_signature(&base_ty, name, self.field_call_signatures)
                {
                    if *op != AssignOp::Set {
                        return Err(Diagnostic::new(
                            "compound property assignment is not supported",
                        ));
                    }
                    if params.len() != 2 || !method_receiver_matches(&params[0], &base_ty) {
                        return Err(Diagnostic::new(format!(
                            "property setter for '{name}' does not accept receiver {base_ty}"
                        )));
                    }
                    if return_type != Type::Unit {
                        return Err(Diagnostic::new(format!(
                            "property setter for '{name}' must return unit"
                        )));
                    }
                    let receiver =
                        self.lower_expr(base, env, types, Some(params[0].clone()))?;
                    let stored = self.lower_expr(value, env, types, Some(params[1].clone()))?;
                    let symbol_id = self.host_import_names.get(&setter_name).copied().ok_or_else(|| {
                        Diagnostic::new(format!(
                            "declared property setter '{setter_name}' is missing a host import symbol"
                        ))
                    })?;
                    self.emit(Instruction::HostCall {
                        name: setter_name,
                        symbol_id,
                        args: vec![receiver, stored],
                        return_type,
                    });
                    return Ok(());
                }
                let (base_ty, field_ty) = if let Expr::Name(_base_name, Some(base_symbol_id), _) = base.as_ref() {
                    let Some(Type::Record(mut fields)) = types.get(base_symbol_id).cloned() else {
                        return Err(Diagnostic::new("field assignment requires a record base"));
                    };
                    let existing_field = fields.get(name).cloned();
                    match existing_field {
                        Some(existing) => (Type::Record(fields), existing),
                        None => {
                            let inferred = self.infer_expr_type(value, types, None)?;
                            let previous_ty = Type::Record(fields.clone());
                            fields.insert(name.clone(), inferred.clone());
                            let updated_ty = Type::Record(fields.clone());
                            let base_value =
                                self.lower_expr(base, env, types, Some(previous_ty.clone()))?;
                            let new_field_value =
                                self.lower_expr(value, env, types, Some(inferred.clone()))?;

                            let mut lowered_fields = Vec::with_capacity(fields.len());
                            for (field_name, field_ty) in &fields {
                                if field_name == name {
                                    lowered_fields.push(new_field_value);
                                } else {
                                    lowered_fields.push(self.emit(Instruction::StructGet {
                                        base: base_value,
                                        field: field_name.clone(),
                                        field_ty: field_ty.clone(),
                                    }));
                                }
                            }
                            let rebuilt = self.emit(Instruction::StructNew {
                                struct_ty: updated_ty.clone(),
                                fields: lowered_fields,
                            });
                            env.insert(*base_symbol_id, rebuilt);
                            self.function.value_symbols.insert(rebuilt, *base_symbol_id);
                            types.insert(*base_symbol_id, updated_ty);
                            return Ok(());
                        }
                    }
                } else {
                    let field_ty = base_ty.record_field(name).ok_or_else(|| {
                        Diagnostic::new(format!("unknown record field '{name}'"))
                    })?;
                    (base_ty, field_ty)
                };
                let base = self.lower_expr(base, env, types, Some(base_ty))?;
                let value = match op {
                    AssignOp::Set => self.lower_expr(value, env, types, Some(field_ty.clone()))?,
                    AssignOp::Add => {
                        if !field_ty.is_numeric() {
                            return Err(Diagnostic::new(
                                "compound field assignment requires a numeric field",
                            ));
                        }
                        let current = self.emit(Instruction::StructGet {
                            base,
                            field: name.clone(),
                            field_ty: field_ty.clone(),
                        });
                        let rhs = self.lower_expr(value, env, types, Some(field_ty.clone()))?;
                        self.emit(Instruction::Binary {
                            op: BinaryOp::Add,
                            left: current,
                            right: rhs,
                            operand_ty: field_ty.clone(),
                            result_ty: field_ty.clone(),
                        })
                    }
                };
                self.emit(Instruction::StructSet {
                    base,
                    field: name.clone(),
                    value,
                });
            }
            Stmt::Expr(expr) => {
                if let Expr::Call {
                    callee,
                    type_args: _,
                    args,
                    span,
                    ..
                } = expr
                {
                    if let Expr::Name(name, _, _) = callee.as_ref() {
                        if name == ASSERT {
                            self.lower_assert_call(args, *span, env, types)?;
                            return Ok(());
                        }
                    }
                }
                let _ = self.lower_expr(expr, env, types, None)?;
            }
            Stmt::Break => {
                self.lower_break(env)?;
            }
            Stmt::Continue => {
                self.lower_continue(env)?;
            }
            Stmt::Return(expr) => {
                let value =
                    self.lower_expr(expr, env, types, Some(self.function.return_type.clone()))?;
                self.set_terminator(self.current_block, Terminator::Return(value));
                self.current_block = DEAD_BLOCK;
            }
            Stmt::ReturnMulti(values) => {
                let expected = match &self.function.return_type {
                    Type::Multi(types) => types.clone(),
                    other => vec![other.clone()],
                };
                let lowered = self.lower_expr_list(values, env, types, Some(&expected))?;
                if lowered.len() != expected.len() {
                    return Err(Diagnostic::new(format!(
                        "return expects {} values, got {}",
                        expected.len(),
                        lowered.len()
                    )));
                }
                let packed = self.emit(Instruction::PackMulti {
                    values: lowered,
                    types: expected,
                });
                self.set_terminator(self.current_block, Terminator::Return(packed));
                self.current_block = DEAD_BLOCK;
            }
            Stmt::LetMulti { bindings, values } => {
                let all_typed = bindings.iter().all(|binding| binding.ty.is_some());
                let any_typed = bindings.iter().any(|binding| binding.ty.is_some());
                if any_typed && !all_typed {
                    return Err(Diagnostic::new(
                        "multi-binding declaration must either annotate all bindings or none",
                    ));
                }
                if all_typed {
                    let expected: Vec<Type> = bindings
                        .iter()
                        .map(|binding| binding.ty.clone().expect("checked above"))
                        .collect();
                    let lowered = self.lower_expr_list(values, env, types, Some(&expected))?;
                    if lowered.len() != expected.len() {
                        return Err(Diagnostic::new(format!(
                            "multi-binding declaration expects {} values, got {}",
                            expected.len(),
                            lowered.len()
                        )));
                    }
                    for ((binding, value), expected_ty) in
                        bindings.iter().zip(lowered).zip(expected)
                    {
                        let symbol_id = binding.symbol_id.expect("resolved symbol_id");
                        env.insert(symbol_id, value);
                        self.function.value_symbols.insert(value, symbol_id);
                        types.insert(symbol_id, expected_ty);
                    }
                } else {
                    let mut inferred_types = Vec::new();
                    for expr in values {
                        let ty = self.infer_expr_type(expr, types, None)?;
                        match ty {
                            Type::Multi(types_for_expr) => {
                                inferred_types.extend(types_for_expr);
                            }
                            other => inferred_types.push(other),
                        }
                    }
                    if inferred_types.len() != bindings.len() {
                        return Err(Diagnostic::new(format!(
                            "multi-binding declaration expects {} values, got {}",
                            bindings.len(),
                            inferred_types.len()
                        )));
                    }
                    let lowered = self.lower_expr_list(values, env, types, None)?;
                    if lowered.len() != inferred_types.len() {
                        return Err(Diagnostic::new(format!(
                            "multi-binding declaration expects {} values, got {}",
                            inferred_types.len(),
                            lowered.len()
                        )));
                    }
                    for ((binding, value), ty) in bindings.iter().zip(lowered).zip(inferred_types) {
                        let symbol_id = binding.symbol_id.expect("resolved symbol_id");
                        env.insert(symbol_id, value);
                        self.function.value_symbols.insert(value, symbol_id);
                        types.insert(symbol_id, ty);
                    }
                }
            }
            Stmt::AssignMulti { targets, symbol_ids, values } => {
                let ids = symbol_ids.as_ref().expect("symbol_ids should be resolved");
                let mut expected = Vec::new();
                for (target, id) in targets.iter().zip(ids) {
                    let ty = types.get(id).cloned().ok_or_else(|| {
                        Diagnostic::new(format!("unknown local '{target}' during IR lowering"))
                    })?;
                    expected.push(ty);
                }
                let lowered = self.lower_expr_list(values, env, types, Some(&expected))?;
                if lowered.len() != expected.len() {
                    return Err(Diagnostic::new(format!(
                        "multi-assignment expects {} values, got {}",
                        expected.len(),
                        lowered.len()
                    )));
                }
                for (id, value) in ids.iter().zip(lowered) {
                    env.insert(*id, value);
                    self.function.value_symbols.insert(value, *id);
                }
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                self.lower_if(condition, then_body, else_body, env, types)?;
            }
            Stmt::IfCast {
                target_name,
                binding_symbol_id,
                value,
                then_body,
                else_body,
                ..
            } => {
                self.lower_if_cast(
                    IfCastParts {
                        target_name,
                        binding_symbol_id: *binding_symbol_id,
                        value,
                        then_body,
                        else_body,
                    },
                    env,
                    types,
                )?;
            }
            Stmt::While { condition, body } => {
                self.lower_while(condition, body, env, types)?;
            }
            Stmt::Repeat { body, condition } => {
                self.lower_repeat(body, condition, env, types)?;
            }
            Stmt::NumericFor {
                symbol_id,
                start,
                stop,
                step,
                body,
                ..
            } => {
                self.lower_numeric_for(symbol_id, start, stop, step.as_ref(), body, env, types)?;
            }
            Stmt::ForIn {
                symbol_ids,
                iterator,
                body,
                ..
            } => {
                self.lower_for_in(symbol_ids, iterator, body, env, types)?;
            }
        }
        Ok(())
    }

    fn lower_assert_call(
        &mut self,
        args: &[Expr],
        span: Option<waluau_ast::Span>,
        env: &mut HashMap<SymbolId, ValueId>,
        types: &mut HashMap<SymbolId, Type>,
    ) -> Result<(), Diagnostic> {
        if args.len() != 1 {
            return Err(Diagnostic::new(format!(
                "{ASSERT} expects 1 argument, got {}",
                args.len()
            )));
        }
        let condition = self.lower_expr(&args[0], env, types, Some(Type::Bool))?;
        let continue_block = self.new_block();
        let trap_block = self.new_block();
        self.set_terminator(
            self.current_block,
            Terminator::Branch {
                condition,
                then_block: continue_block,
                else_block: trap_block,
            },
        );
        self.current_block = trap_block;

        let assert_span = args[0].span().or(span);
        let msg_str = if let Some(sp) = assert_span {
            if let Some(source) = self.sources.get(&self.file_path) {
                let (line, expr_text) = resolve_span_to_line_and_text(source, sp);
                format!(
                    "Assertion failed: {} at {}:{}",
                    expr_text, self.file_path, line
                )
            } else {
                format!("Assertion failed at {}:0", self.file_path)
            }
        } else {
            "Assertion failed".to_string()
        };

        let message = self.emit(Instruction::String(msg_str));
        self.emit(Instruction::Print { value: message });
        self.set_terminator(trap_block, Terminator::Unreachable { span });
        self.current_block = continue_block;
        Ok(())
    }

    /// Compute narrowed type scopes for `if result is Variant then ... else ... end`.
    /// Returns `(then_types, else_types)` — clones of `types` with the narrowed type
    /// applied to the named variable in each branch.
    fn narrowed_variant_type_scopes(
        condition: &Expr,
        types: &HashMap<SymbolId, Type>,
    ) -> (HashMap<SymbolId, Type>, HashMap<SymbolId, Type>) {
        let mut then_types = types.clone();
        let mut else_types = types.clone();
        let Expr::IsVariant { expr, tag, .. } = condition else {
            return (then_types, else_types);
        };
        let Expr::Name(_, Some(symbol_id), _) = expr.as_ref() else {
            return (then_types, else_types);
        };
        let Some(ty) = types.get(symbol_id) else {
            return (then_types, else_types);
        };
        if let Some(variant) = ty.tagged_variant(tag) {
            then_types.insert(*symbol_id, Type::TaggedVariant(variant));
        }
        if let Some(remaining) = ty.remove_tagged_variant(tag) {
            else_types.insert(*symbol_id, remaining);
        }
        (then_types, else_types)
    }

    fn nil_test_subject(condition: &Expr) -> Option<(SymbolId, bool)> {
        let Expr::Binary {
            op, left, right, ..
        } = condition
        else {
            return None;
        };
        let non_null_when_true = match op {
            BinaryOp::Eq => false,
            BinaryOp::NotEq => true,
            _ => return None,
        };
        match (left.as_ref(), right.as_ref()) {
            (Expr::Name(_, Some(symbol_id), _), Expr::Nil(..))
            | (Expr::Nil(..), Expr::Name(_, Some(symbol_id), _)) => {
                Some((*symbol_id, non_null_when_true))
            }
            _ => None,
        }
    }

    fn narrowed_type_scopes(
        condition: &Expr,
        types: &HashMap<SymbolId, Type>,
    ) -> (HashMap<SymbolId, Type>, HashMap<SymbolId, Type>) {
        let (mut then_types, mut else_types) =
            Self::narrowed_variant_type_scopes(condition, types);
        let Some((symbol_id, non_null_when_true)) = Self::nil_test_subject(condition) else {
            return (then_types, else_types);
        };
        let Some(inner) = types.get(&symbol_id).and_then(Type::nullable_inner) else {
            return (then_types, else_types);
        };
        if non_null_when_true {
            then_types.insert(symbol_id, inner);
        } else {
            else_types.insert(symbol_id, inner);
        }
        (then_types, else_types)
    }

    fn lower_if(
        &mut self,
        condition: &Expr,
        then_body: &[Stmt],
        else_body: &[Stmt],
        env: &mut HashMap<SymbolId, ValueId>,
        types: &mut HashMap<SymbolId, Type>,
    ) -> Result<(), Diagnostic> {
        let (then_types_init, else_types_init) = Self::narrowed_type_scopes(condition, types);
        let condition_value = self.lower_expr(condition, env, types, Some(Type::Bool))?;
        self.lower_if_branches(
            condition_value,
            then_body,
            else_body,
            env,
            types,
            then_types_init,
            else_types_init,
            None,
        )
    }

    /// Lowers the branches of an `if`, given the already-lowered boolean `condition`
    /// value and the narrowed type scopes for each branch. `pattern_binding`, when
    /// present, additionally unboxes the tagged-variant payload from `base` and binds
    /// it to `symbol_id` at the start of the `then` branch (for `if Tag(x) = expr then`).
    #[allow(clippy::too_many_arguments)]
    fn lower_if_branches(
        &mut self,
        condition: ValueId,
        then_body: &[Stmt],
        else_body: &[Stmt],
        env: &mut HashMap<SymbolId, ValueId>,
        types: &mut HashMap<SymbolId, Type>,
        then_types_init: HashMap<SymbolId, Type>,
        else_types_init: HashMap<SymbolId, Type>,
        pattern_binding: Option<(SymbolId, ValueId, Type)>,
    ) -> Result<(), Diagnostic> {
        let then_block = self.new_block();
        let else_block = self.new_block();
        let merge_block = self.new_block();
        self.set_terminator(
            self.current_block,
            Terminator::Branch {
                condition,
                then_block,
                else_block,
            },
        );

        let mut then_env = env.clone();
        let mut then_types = types.clone();
        let mut then_narrowed_values = HashMap::new();
        self.current_block = then_block;
        for (symbol_id, narrowed_ty) in then_types_init {
            if let (Some(original_ty), Some(original_value)) =
                (types.get(&symbol_id), then_env.get(&symbol_id).copied())
            {
                if original_ty != &narrowed_ty
                    && original_ty.nullable_inner().as_ref() == Some(&narrowed_ty)
                {
                    let narrowed_value = self.emit(Instruction::Cast {
                        value: original_value,
                        from: original_ty.clone(),
                        to: narrowed_ty.clone(),
                    });
                    then_env.insert(symbol_id, narrowed_value);
                    then_narrowed_values.insert(symbol_id, narrowed_value);
                    self.function
                        .value_symbols
                        .insert(narrowed_value, symbol_id);
                }
            }
            then_types.insert(symbol_id, narrowed_ty);
        }
        if let Some((symbol_id, base, payload_ty)) = &pattern_binding {
            let unboxed = self.unbox_tagged_variant_value(*base, payload_ty)?;
            if self.cell_names.contains(symbol_id) {
                let cell = self.emit(Instruction::ArrayNew {
                    element_ty: to_runtime_type(payload_ty),
                    elements: vec![unboxed],
                });
                then_env.insert(*symbol_id, cell);
                self.function.value_symbols.insert(cell, *symbol_id);
            } else {
                then_env.insert(*symbol_id, unboxed);
                self.function.value_symbols.insert(unboxed, *symbol_id);
            }
        }
        for stmt in then_body {
            if self.current_block == DEAD_BLOCK {
                break;
            }
            self.lower_stmt(stmt, &mut then_env, &mut then_types)?;
        }
        let then_exit = self.current_block;
        if then_exit != DEAD_BLOCK {
            self.set_terminator(then_exit, Terminator::Jump(merge_block));
        }

        let mut else_env = env.clone();
        let mut else_types = types.clone();
        let mut else_narrowed_values = HashMap::new();
        self.current_block = else_block;
        for (symbol_id, narrowed_ty) in else_types_init {
            if let (Some(original_ty), Some(original_value)) =
                (types.get(&symbol_id), else_env.get(&symbol_id).copied())
            {
                if original_ty != &narrowed_ty
                    && original_ty.nullable_inner().as_ref() == Some(&narrowed_ty)
                {
                    let narrowed_value = self.emit(Instruction::Cast {
                        value: original_value,
                        from: original_ty.clone(),
                        to: narrowed_ty.clone(),
                    });
                    else_env.insert(symbol_id, narrowed_value);
                    else_narrowed_values.insert(symbol_id, narrowed_value);
                    self.function
                        .value_symbols
                        .insert(narrowed_value, symbol_id);
                }
            }
            else_types.insert(symbol_id, narrowed_ty);
        }
        for stmt in else_body {
            if self.current_block == DEAD_BLOCK {
                break;
            }
            self.lower_stmt(stmt, &mut else_env, &mut else_types)?;
        }
        let else_exit = self.current_block;
        if else_exit != DEAD_BLOCK {
            self.set_terminator(else_exit, Terminator::Jump(merge_block));
        }

        if then_exit == DEAD_BLOCK && else_exit == DEAD_BLOCK {
            self.current_block = DEAD_BLOCK;
            return Ok(());
        }

        self.current_block = merge_block;
        for name in env.keys().cloned().collect::<Vec<_>>() {
            let t = then_env
                .get(&name)
                .copied()
                .or_else(|| env.get(&name).copied());
            let e = else_env
                .get(&name)
                .copied()
                .or_else(|| env.get(&name).copied());
            if let (Some(tv), Some(ev)) = (t, e) {
                if tv != ev {
                    let original = env.get(&name).copied();
                    let then_is_only_narrowed =
                        then_narrowed_values.get(&name).copied() == Some(tv) && original == Some(ev);
                    let else_is_only_narrowed =
                        else_narrowed_values.get(&name).copied() == Some(ev) && original == Some(tv);
                    if then_is_only_narrowed || else_is_only_narrowed {
                        continue;
                    }
                    let mut incoming = Vec::new();
                    if then_exit != DEAD_BLOCK {
                        incoming.push((then_exit, tv));
                    }
                    if else_exit != DEAD_BLOCK {
                        incoming.push((else_exit, ev));
                    }
                    let phi = self.emit(Instruction::Phi(incoming));
                    env.insert(name, phi);
                    self.function.value_symbols.insert(phi, name);
                }
            }
        }
        // Propagate narrowed types from the surviving branch when one side diverges.
        // This enables narrowing to persist after `if result is X then return ... end`.
        if then_exit == DEAD_BLOCK {
            for name in types.keys().cloned().collect::<Vec<_>>() {
                if let Some(narrowed) = else_types.get(&name) {
                    types.insert(name, narrowed.clone());
                }
            }
        } else if else_exit == DEAD_BLOCK {
            for name in types.keys().cloned().collect::<Vec<_>>() {
                if let Some(narrowed) = then_types.get(&name) {
                    types.insert(name, narrowed.clone());
                }
            }
        }
        Ok(())
    }

    /// Lowers the dual-purpose `if Name(binding) = expr then ... end` syntax.
    /// HIR type-checking has already validated which interpretation applies and
    /// scoped `binding` accordingly, but the IR re-derives the same dispatch
    /// (tagged-union pattern match vs. extern safe-cast) from the scrutinee's
    /// inferred type, since a single parsed `Stmt::IfCast` shape backs both:
    ///
    ///   - Tagged-union pattern match with binding (`target_name` matches a
    ///     variant tag of the scrutinee's type): emits `StructGet tag` + `Eq`
    ///     and unboxes the payload into `binding`, scoped to `then`.
    ///   - Extern safe-cast (`target_name` names an extern type): emits
    ///     `ExternCastTest` and binds the whole tested value to `binding` with
    ///     type `Type::Extern`, scoped to `then`.
    fn lower_if_cast(
        &mut self,
        parts: IfCastParts<'_>,
        env: &mut HashMap<SymbolId, ValueId>,
        types: &mut HashMap<SymbolId, Type>,
    ) -> Result<(), Diagnostic> {
        let binding_symbol_id = parts
            .binding_symbol_id
            .ok_or_else(|| Diagnostic::new("if-cast binding must have a symbol ID resolved"))?;

        let scrutinee_ty = self.infer_expr_type(parts.value, types, None)?;
        if let Some(variant) = scrutinee_ty.tagged_variant(parts.target_name) {
            let payload_ty = (*variant.payload).clone();

            // Lower the scrutinee once; the underlying IR value is the canonical record.
            let base = self.lower_expr(parts.value, env, types, None)?;
            let tag_id = self.variant_tag_id(parts.target_name)?;
            let tag_field_ty = Type::Numeric(NumericType::I32);
            let tag_val = self.emit(Instruction::StructGet {
                base,
                field: "tag".to_string(),
                field_ty: tag_field_ty.clone(),
            });
            let expected_tag = self.emit(Instruction::Number {
                ty: NumericType::I32,
                literal: NumberLiteral {
                    raw: tag_id.to_string(),
                },
            });
            let condition_value = self.emit(Instruction::Binary {
                op: BinaryOp::Eq,
                left: tag_val,
                right: expected_tag,
                operand_ty: tag_field_ty,
                result_ty: Type::Bool,
            });

            let mut then_types_init = types.clone();
            then_types_init.insert(binding_symbol_id, payload_ty.clone());
            let else_types_init = types.clone();
            return self.lower_if_branches(
                condition_value,
                parts.then_body,
                parts.else_body,
                env,
                types,
                then_types_init,
                else_types_init,
                Some((binding_symbol_id, base, payload_ty)),
            );
        }

        let tested = self.lower_expr(parts.value, env, types, None)?;
        let condition = self.emit(Instruction::ExternCastTest {
            value: tested,
            target_name: parts.target_name.to_string(),
        });
        let then_block = self.new_block();
        let else_block = self.new_block();
        let merge_block = self.new_block();
        self.set_terminator(
            self.current_block,
            Terminator::Branch {
                condition,
                then_block,
                else_block,
            },
        );

        let mut then_env = env.clone();
        let mut then_types = types.clone();
        then_env.insert(binding_symbol_id, tested);
        then_types.insert(binding_symbol_id, Type::Extern);
        self.current_block = then_block;
        for stmt in parts.then_body {
            if self.current_block == DEAD_BLOCK {
                break;
            }
            self.lower_stmt(stmt, &mut then_env, &mut then_types)?;
        }
        let then_exit = self.current_block;
        if then_exit != DEAD_BLOCK {
            self.set_terminator(then_exit, Terminator::Jump(merge_block));
        }

        let mut else_env = env.clone();
        let mut else_types = types.clone();
        self.current_block = else_block;
        for stmt in parts.else_body {
            if self.current_block == DEAD_BLOCK {
                break;
            }
            self.lower_stmt(stmt, &mut else_env, &mut else_types)?;
        }
        let else_exit = self.current_block;
        if else_exit != DEAD_BLOCK {
            self.set_terminator(else_exit, Terminator::Jump(merge_block));
        }

        if then_exit == DEAD_BLOCK && else_exit == DEAD_BLOCK {
            self.current_block = DEAD_BLOCK;
            return Ok(());
        }

        self.current_block = merge_block;
        for name in env.keys().cloned().collect::<Vec<_>>() {
            let t = then_env
                .get(&name)
                .copied()
                .or_else(|| env.get(&name).copied());
            let e = else_env
                .get(&name)
                .copied()
                .or_else(|| env.get(&name).copied());
            if let (Some(tv), Some(ev)) = (t, e)
                && tv != ev
            {
                let mut incoming = Vec::new();
                if then_exit != DEAD_BLOCK {
                    incoming.push((then_exit, tv));
                }
                if else_exit != DEAD_BLOCK {
                    incoming.push((else_exit, ev));
                }
                let phi = self.emit(Instruction::Phi(incoming));
                env.insert(name, phi);
                self.function.value_symbols.insert(phi, name);
            }
        }
        Ok(())
    }

    fn lower_while(
        &mut self,
        condition: &Expr,
        body: &[Stmt],
        env: &mut HashMap<SymbolId, ValueId>,
        types: &mut HashMap<SymbolId, Type>,
    ) -> Result<(), Diagnostic> {
        let preheader = self.current_block;
        let header = self.new_block();
        let loop_body = self.new_block();
        let exit = self.new_block();
        self.set_terminator(preheader, Terminator::Jump(header));

        let mutated = collect_assigned_names(body);
        self.current_block = header;
        let mut loop_env = env.clone();
        let loop_types = types.clone();
        let mut phis = HashMap::new();
        for id in &mutated {
            if let Some(initial) = env.get(id).copied() {
                let phi = self.emit(Instruction::Phi(vec![(preheader, initial)]));
                loop_env.insert(*id, phi);
                self.function.value_symbols.insert(phi, *id);
                phis.insert(*id, phi);
            }
        }

        self.loop_stack.push(LoopContext {
            header,
            continue_target: header,
            break_target: exit,
            phis: phis.clone(),
        });

        let cond_value = self.lower_expr(condition, &loop_env, &loop_types, Some(Type::Bool))?;
        self.set_terminator(
            header,
            Terminator::Branch {
                condition: cond_value,
                then_block: loop_body,
                else_block: exit,
            },
        );

        self.current_block = loop_body;
        let mut body_env = loop_env.clone();
        let mut body_types = loop_types.clone();
        for stmt in body {
            if self.current_block == DEAD_BLOCK {
                break;
            }
            self.lower_stmt(stmt, &mut body_env, &mut body_types)?;
        }
        let loop_ctx = self
            .loop_stack
            .pop()
            .expect("loop stack must contain entry for while loop");
        let phis = loop_ctx.phis;
        let body_exit = self.current_block;
        if body_exit != DEAD_BLOCK {
            self.set_terminator(body_exit, Terminator::Jump(header));
            for (id, phi) in &phis {
                if let Some(next_value) = body_env.get(id).copied() {
                    add_phi_incoming(&mut self.function, header, *phi, (body_exit, next_value));
                }
            }
        }

        for (id, phi) in phis {
            env.insert(id, phi);
            self.function.value_symbols.insert(phi, id);
        }
        self.current_block = exit;
        Ok(())
    }

    fn lower_repeat(
        &mut self,
        body: &[Stmt],
        condition: &Expr,
        env: &mut HashMap<SymbolId, ValueId>,
        types: &mut HashMap<SymbolId, Type>,
    ) -> Result<(), Diagnostic> {
        let preheader = self.current_block;
        let loop_body = self.new_block();
        let check = self.new_block();
        let exit = self.new_block();
        self.set_terminator(preheader, Terminator::Jump(loop_body));

        let mutated = collect_assigned_names(body);
        self.current_block = loop_body;
        let mut loop_env = env.clone();
        let loop_types = types.clone();
        let mut phis = HashMap::new();
        for name in &mutated {
            if let Some(initial) = env.get(name).copied() {
                let phi = self.emit(Instruction::Phi(vec![(preheader, initial)]));
                loop_env.insert(*name, phi);
                self.function.value_symbols.insert(phi, *name);
                phis.insert(*name, phi);
            }
        }

        self.loop_stack.push(LoopContext {
            header: loop_body,
            continue_target: check,
            break_target: exit,
            phis: phis.clone(),
        });

        let mut body_env = loop_env.clone();
        let mut body_types = loop_types.clone();
        for stmt in body {
            if self.current_block == DEAD_BLOCK {
                break;
            }
            self.lower_stmt(stmt, &mut body_env, &mut body_types)?;
        }
        let loop_ctx = self
            .loop_stack
            .pop()
            .expect("loop stack must contain entry for repeat-until loop");
        let phis = loop_ctx.phis;
        let body_exit = self.current_block;
        if body_exit == DEAD_BLOCK {
            for (name, phi) in phis {
                env.insert(name, phi);
                self.function.value_symbols.insert(phi, name);
            }
            self.current_block = exit;
            return Ok(());
        }
        self.set_terminator(body_exit, Terminator::Jump(check));

        self.current_block = check;
        let cond_value = self.lower_expr(condition, &body_env, &body_types, Some(Type::Bool))?;
        self.set_terminator(
            check,
            Terminator::Branch {
                condition: cond_value,
                then_block: exit,
                else_block: loop_body,
            },
        );

        for (name, phi) in &phis {
            if let Some(next_value) = body_env.get(name).copied() {
                add_phi_incoming(&mut self.function, loop_body, *phi, (check, next_value));
            }
        }

        for (name, phi) in phis {
            let val = body_env.get(&name).copied().unwrap_or(phi);
            env.insert(name, val);
            self.function.value_symbols.insert(val, name);
        }
        self.current_block = exit;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn lower_numeric_for(
        &mut self,
        symbol_id: &Option<SymbolId>,
        start: &Expr,
        stop: &Expr,
        step: Option<&Expr>,
        body: &[Stmt],
        env: &mut HashMap<SymbolId, ValueId>,
        types: &mut HashMap<SymbolId, Type>,
    ) -> Result<(), Diagnostic> {
        let symbol_id = symbol_id.expect("symbol_id should be resolved");
        let start_ty = self.infer_expr_type(start, types, None)?;
        let stop_ty = self.infer_expr_type(stop, types, None)?;
        let mut loop_ty = common_numeric_type(start_ty, stop_ty)?;
        if let Some(step_expr) = step {
            let step_ty = self.infer_expr_type(step_expr, types, None)?;
            loop_ty = common_numeric_type(loop_ty, step_ty)?;
        }
        let Type::Numeric(numeric_ty) = loop_ty else {
            return Err(Diagnostic::new("numeric for-loop bounds must be numeric"));
        };
        let loop_ty = Type::Numeric(numeric_ty);
        let start_value = self.lower_expr(start, env, types, Some(loop_ty.clone()))?;
        let stop_init = self.lower_expr(stop, env, types, Some(loop_ty.clone()))?;
        let zero_value = self.emit(Instruction::Number {
            ty: numeric_ty,
            literal: NumberLiteral { raw: "0".into() },
        });
        let default_step_value = if step.is_none() {
            let default_step_expr = Expr::Cast {
                expr: Box::new(Expr::Number(NumberLiteral { raw: "1".into() }, None)),
                ty: loop_ty.clone(),
                span: None,
            };
            Some(self.lower_expr(&default_step_expr, env, types, Some(loop_ty.clone()))?)
        } else {
            None
        };

        let preheader = self.current_block;
        let header = self.new_block();
        let loop_body = self.new_block();
        let exit = self.new_block();
        self.set_terminator(preheader, Terminator::Jump(header));

        let mutated = collect_assigned_names(body);
        self.current_block = header;
        let mut loop_env = env.clone();
        let loop_types = types.clone();
        let mut phis = HashMap::new();
        for id in &mutated {
            if let Some(initial) = env.get(id).copied() {
                let phi = self.emit(Instruction::Phi(vec![(preheader, initial)]));
                loop_env.insert(*id, phi);
                self.function.value_symbols.insert(phi, *id);
                phis.insert(*id, phi);
            }
        }
        let stop_phi = self.emit(Instruction::Phi(vec![(preheader, stop_init)]));
        let index_phi = self.emit(Instruction::Phi(vec![(preheader, start_value)]));
        self.function.value_symbols.insert(index_phi, symbol_id);
        let step_value = if let Some(step_expr) = step {
            self.lower_expr(step_expr, &loop_env, &loop_types, Some(loop_ty.clone()))?
        } else {
            default_step_value.expect("precomputed default step")
        };

        let step_positive = self.emit(Instruction::Binary {
            op: BinaryOp::Greater,
            left: step_value,
            right: zero_value,
            operand_ty: loop_ty.clone(),
            result_ty: Type::Bool,
        });
        let step_negative = self.emit(Instruction::Binary {
            op: BinaryOp::Less,
            left: step_value,
            right: zero_value,
            operand_ty: loop_ty.clone(),
            result_ty: Type::Bool,
        });
        let i_lt_stop = self.emit(Instruction::Binary {
            op: BinaryOp::Less,
            left: index_phi,
            right: stop_phi,
            operand_ty: loop_ty.clone(),
            result_ty: Type::Bool,
        });
        let i_gt_stop = self.emit(Instruction::Binary {
            op: BinaryOp::Greater,
            left: index_phi,
            right: stop_phi,
            operand_ty: loop_ty.clone(),
            result_ty: Type::Bool,
        });
        let i_eq_stop_for_le = self.emit(Instruction::Binary {
            op: BinaryOp::Eq,
            left: index_phi,
            right: stop_phi,
            operand_ty: loop_ty.clone(),
            result_ty: Type::Bool,
        });
        let i_eq_stop_for_ge = self.emit(Instruction::Binary {
            op: BinaryOp::Eq,
            left: index_phi,
            right: stop_phi,
            operand_ty: loop_ty.clone(),
            result_ty: Type::Bool,
        });
        let i_le_stop = self.emit(Instruction::Binary {
            op: BinaryOp::Or,
            left: i_lt_stop,
            right: i_eq_stop_for_le,
            operand_ty: Type::Bool,
            result_ty: Type::Bool,
        });
        let i_ge_stop = self.emit(Instruction::Binary {
            op: BinaryOp::Or,
            left: i_gt_stop,
            right: i_eq_stop_for_ge,
            operand_ty: Type::Bool,
            result_ty: Type::Bool,
        });
        let forward_ok = self.emit(Instruction::Binary {
            op: BinaryOp::And,
            left: step_positive,
            right: i_le_stop,
            operand_ty: Type::Bool,
            result_ty: Type::Bool,
        });
        let backward_ok = self.emit(Instruction::Binary {
            op: BinaryOp::And,
            left: step_negative,
            right: i_ge_stop,
            operand_ty: Type::Bool,
            result_ty: Type::Bool,
        });
        let loop_cond = self.emit(Instruction::Binary {
            op: BinaryOp::Or,
            left: forward_ok,
            right: backward_ok,
            operand_ty: Type::Bool,
            result_ty: Type::Bool,
        });

        self.loop_stack.push(LoopContext {
            header,
            continue_target: header,
            break_target: exit,
            phis: phis.clone(),
        });
        self.set_terminator(
            header,
            Terminator::Branch {
                condition: loop_cond,
                then_block: loop_body,
                else_block: exit,
            },
        );

        self.current_block = loop_body;
        let mut body_env = loop_env.clone();
        let mut body_types = loop_types.clone();
        body_env.insert(symbol_id, index_phi);
        self.function.value_symbols.insert(index_phi, symbol_id);
        body_types.insert(symbol_id, loop_ty.clone());
        for stmt in body {
            if self.current_block == DEAD_BLOCK {
                break;
            }
            self.lower_stmt(stmt, &mut body_env, &mut body_types)?;
        }
        let loop_ctx = self
            .loop_stack
            .pop()
            .expect("loop stack must contain entry for numeric for loop");
        let phis = loop_ctx.phis;
        let body_exit = self.current_block;
        if body_exit != DEAD_BLOCK {
            let stop_next = self.emit(Instruction::Binary {
                op: BinaryOp::Add,
                left: stop_phi,
                right: zero_value,
                operand_ty: loop_ty.clone(),
                result_ty: loop_ty.clone(),
            });
            let next_index = self.emit(Instruction::Binary {
                op: BinaryOp::Add,
                left: index_phi,
                right: step_value,
                operand_ty: loop_ty.clone(),
                result_ty: loop_ty.clone(),
            });
            self.set_terminator(body_exit, Terminator::Jump(header));
            add_phi_incoming(&mut self.function, header, stop_phi, (body_exit, stop_next));
            add_phi_incoming(
                &mut self.function,
                header,
                index_phi,
                (body_exit, next_index),
            );
            for (id, phi) in &phis {
                if let Some(next_value) = body_env.get(id).copied() {
                    add_phi_incoming(&mut self.function, header, *phi, (body_exit, next_value));
                }
            }
        }

        for (id, phi) in phis {
            env.insert(id, phi);
            self.function.value_symbols.insert(phi, id);
        }
        self.current_block = exit;
        Ok(())
    }

    fn lower_for_in(
        &mut self,
        symbol_ids: &Option<Vec<SymbolId>>,
        iterator: &Expr,
        body: &[Stmt],
        env: &mut HashMap<SymbolId, ValueId>,
        types: &mut HashMap<SymbolId, Type>,
    ) -> Result<(), Diagnostic> {
        let ids = symbol_ids.as_ref().expect("symbol_ids should be resolved");
        let iterator_ty = self.infer_expr_type(iterator, types, None)?;
        if let Type::Array(element_ty) = &iterator_ty {
            if ids.len() != 1 && ids.len() != 2 {
                return Err(Diagnostic::new(format!(
                    "array for-in loop expects 1 or 2 loop variables, got {}",
                    ids.len()
                )));
            }
            let array_val = self.lower_expr(iterator, env, types, Some(iterator_ty.clone()))?;
            let array_len_init = self.emit(Instruction::ArrayLen { array: array_val });
            let const_zero = self.emit(Instruction::Number {
                ty: NumericType::I32,
                literal: NumberLiteral { raw: "0".into() },
            });
            let const_one = self.emit(Instruction::Number {
                ty: NumericType::I32,
                literal: NumberLiteral { raw: "1".into() },
            });

            let preheader = self.current_block;
            let header = self.new_block();
            let loop_body = self.new_block();
            let exit = self.new_block();
            self.set_terminator(preheader, Terminator::Jump(header));

            let mutated = collect_assigned_names(body);
            self.current_block = header;
            let mut loop_env = env.clone();
            let loop_types = types.clone();
            let mut phis = HashMap::new();
            for id in &mutated {
                if let Some(initial) = env.get(id).copied() {
                    let phi = self.emit(Instruction::Phi(vec![(preheader, initial)]));
                    loop_env.insert(*id, phi);
                    self.function.value_symbols.insert(phi, *id);
                    phis.insert(*id, phi);
                }
            }
            let array_len_phi = self.emit(Instruction::Phi(vec![(preheader, array_len_init)]));
            let index_phi = self.emit(Instruction::Phi(vec![(preheader, const_zero)]));

            let loop_cond = self.emit(Instruction::Binary {
                op: BinaryOp::Less,
                left: index_phi,
                right: array_len_phi,
                operand_ty: Type::Numeric(NumericType::I32),
                result_ty: Type::Bool,
            });
            self.loop_stack.push(LoopContext {
                header,
                continue_target: header,
                break_target: exit,
                phis: phis.clone(),
            });
            self.set_terminator(
                header,
                Terminator::Branch {
                    condition: loop_cond,
                    then_block: loop_body,
                    else_block: exit,
                },
            );

            self.current_block = loop_body;
            let mut body_env = loop_env.clone();
            let mut body_types = loop_types.clone();

            let element_val = self.emit(Instruction::ArrayGet {
                array: array_val,
                index: index_phi,
                element_ty: *element_ty.clone(),
            });

            if ids.len() == 1 {
                body_env.insert(ids[0], element_val);
                self.function.value_symbols.insert(element_val, ids[0]);
                body_types.insert(ids[0], *element_ty.clone());
            } else {
                body_env.insert(ids[0], index_phi);
                self.function.value_symbols.insert(index_phi, ids[0]);
                body_types.insert(ids[0], Type::Numeric(NumericType::I32));
                body_env.insert(ids[1], element_val);
                self.function.value_symbols.insert(element_val, ids[1]);
                body_types.insert(ids[1], *element_ty.clone());
            }

            for stmt in body {
                if self.current_block == DEAD_BLOCK {
                    break;
                }
                self.lower_stmt(stmt, &mut body_env, &mut body_types)?;
            }

            let loop_ctx = self
                .loop_stack
                .pop()
                .expect("loop stack must contain entry for array for loop");
            let phis = loop_ctx.phis;
            let body_exit = self.current_block;
            if body_exit != DEAD_BLOCK {
                let array_len_next = self.emit(Instruction::Binary {
                    op: BinaryOp::Add,
                    left: array_len_phi,
                    right: const_zero,
                    operand_ty: Type::Numeric(NumericType::I32),
                    result_ty: Type::Numeric(NumericType::I32),
                });
                let next_index = self.emit(Instruction::Binary {
                    op: BinaryOp::Add,
                    left: index_phi,
                    right: const_one,
                    operand_ty: Type::Numeric(NumericType::I32),
                    result_ty: Type::Numeric(NumericType::I32),
                });
                self.set_terminator(body_exit, Terminator::Jump(header));
                add_phi_incoming(
                    &mut self.function,
                    header,
                    array_len_phi,
                    (body_exit, array_len_next),
                );
                add_phi_incoming(
                    &mut self.function,
                    header,
                    index_phi,
                    (body_exit, next_index),
                );
                for (name, phi) in &phis {
                    if let Some(next_value) = body_env.get(name).copied() {
                        add_phi_incoming(&mut self.function, header, *phi, (body_exit, next_value));
                    }
                }
            }

            for (name, phi) in phis {
                env.insert(name, phi);
                self.function.value_symbols.insert(phi, name);
            }
            self.current_block = exit;
            return Ok(());
        }

        let Type::Function {
            params,
            return_type,
        } = iterator_ty
        else {
            return Err(Diagnostic::new("for-in iterator must be a function"));
        };
        if !params.is_empty() {
            return Err(Diagnostic::new(
                "for-in iterator function must not require parameters",
            ));
        }
        let return_values = match *return_type {
            Type::Multi(values) => values,
            other => vec![other],
        };
        if return_values.len() != ids.len() + 1 {
            return Err(Diagnostic::new(format!(
                "for-in iterator expects {} return values (bool + {} loop values), got {}",
                ids.len() + 1,
                ids.len(),
                return_values.len()
            )));
        }
        if return_values[0] != Type::Bool {
            return Err(Diagnostic::new(
                "for-in iterator first return value must be bool",
            ));
        }
        let loop_value_types = return_values.into_iter().skip(1).collect::<Vec<_>>();
        let return_ty = Type::Multi(
            std::iter::once(Type::Bool)
                .chain(loop_value_types.clone())
                .collect(),
        );
        let direct_iterator_name = match iterator {
            Expr::Name(name, Some(symbol_id), _) => self.signatures.get(symbol_id).and_then(|(params, ret)| {
                if params.is_empty() && *ret == return_ty {
                    Some(name.clone())
                } else {
                    None
                }
            }),
            _ => None,
        };
        let iterator_value = if direct_iterator_name.is_none() {
            Some(self.lower_expr(iterator, env, types, None)?)
        } else {
            None
        };

        let preheader = self.current_block;
        let header = self.new_block();
        let loop_body = self.new_block();
        let exit = self.new_block();
        self.set_terminator(preheader, Terminator::Jump(header));

        let mutated = collect_assigned_names(body);
        self.current_block = header;
        let mut loop_env = env.clone();
        let loop_types = types.clone();
        let mut phis = HashMap::new();
        for id in &mutated {
            if let Some(initial) = env.get(id).copied() {
                let phi = self.emit(Instruction::Phi(vec![(preheader, initial)]));
                loop_env.insert(*id, phi);
                self.function.value_symbols.insert(phi, *id);
                phis.insert(*id, phi);
            }
        }

        let call = if let Some(name) = direct_iterator_name {
            self.emit(Instruction::Call {
                name,
                symbol_id: None,
                args: Vec::new(),
            })
        } else {
            self.emit(Instruction::CallValue {
                callee: iterator_value.expect("lowered above when not direct"),
                args: Vec::new(),
                params: Vec::new(),
                return_type: return_ty,
            })
        };
        let continue_value = self.emit(Instruction::MultiGet {
            value: call,
            index: 0,
            ty: Type::Bool,
        });
        self.loop_stack.push(LoopContext {
            header,
            continue_target: header,
            break_target: exit,
            phis: phis.clone(),
        });
        self.set_terminator(
            header,
            Terminator::Branch {
                condition: continue_value,
                then_block: loop_body,
                else_block: exit,
            },
        );

        self.current_block = loop_body;
        let mut body_env = loop_env.clone();
        let mut body_types = loop_types.clone();
        for (index, (id, ty)) in ids.iter().zip(loop_value_types.iter()).enumerate() {
            let value = self.emit(Instruction::MultiGet {
                value: call,
                index: index + 1,
                ty: ty.clone(),
            });
            body_env.insert(*id, value);
            self.function.value_symbols.insert(value, *id);
            body_types.insert(*id, ty.clone());
        }
        for stmt in body {
            if self.current_block == DEAD_BLOCK {
                break;
            }
            self.lower_stmt(stmt, &mut body_env, &mut body_types)?;
        }

        let loop_ctx = self
            .loop_stack
            .pop()
            .expect("loop stack must contain entry for for-in loop");
        let phis = loop_ctx.phis;
        let body_exit = self.current_block;
        if body_exit != DEAD_BLOCK {
            self.set_terminator(body_exit, Terminator::Jump(header));
            for (id, phi) in &phis {
                if let Some(next_value) = body_env.get(id).copied() {
                    add_phi_incoming(&mut self.function, header, *phi, (body_exit, next_value));
                }
            }
        }

        for (id, phi) in phis {
            env.insert(id, phi);
            self.function.value_symbols.insert(phi, id);
        }
        self.current_block = exit;
        Ok(())
    }

    fn lower_expr(
        &mut self,
        expr: &Expr,
        env: &HashMap<SymbolId, ValueId>,
        types: &HashMap<SymbolId, Type>,
        expected: Option<Type>,
    ) -> Result<ValueId, Diagnostic> {
        let value = match expr {
            Expr::Number(number, _) => {
                let ty = match self.infer_expr_type(expr, types, expected)? {
                    Type::Numeric(ty) => ty,
                    Type::Bool => unreachable!("number literal cannot lower as bool"),
                    Type::Unit => {
                        return Err(Diagnostic::new("numeric literal is not assignable to unit"));
                    }
                    Type::String => {
                        return Err(Diagnostic::new(
                            "numeric literal is not assignable to string",
                        ));
                    }
                    Type::Bytes => {
                        return Err(Diagnostic::new(
                            "numeric literal is not assignable to bytes",
                        ));
                    }
                    Type::Extern | Type::ExternSubtype(_) => {
                        return Err(Diagnostic::new(
                            "numeric literal is not assignable to extern",
                        ));
                    }
                    Type::Nil | Type::Nullable(_) => {
                        return Err(Diagnostic::new(
                            "numeric literal is not assignable to nullable extern",
                        ));
                    }
                    Type::Named { name, .. } => {
                        return Err(Diagnostic::new(format!(
                            "numeric literal is not assignable to {name}",
                        )));
                    }
                    Type::Opaque { name, .. } => {
                        return Err(Diagnostic::new(format!(
                            "numeric literal is not assignable to {name}",
                        )));
                    }
                    Type::Array(_) => {
                        return Err(Diagnostic::new(
                            "numeric literal is not assignable to array",
                        ));
                    }
                    Type::Function { .. } => {
                        return Err(Diagnostic::new(
                            "numeric literal is not assignable to function",
                        ));
                    }
                    Type::Multi(_) => {
                        return Err(Diagnostic::new(
                            "numeric literal is not assignable to multiple values",
                        ));
                    }
                    Type::Record(_) => {
                        return Err(Diagnostic::new(
                            "numeric literal is not assignable to namespace",
                        ));
                    }
                    Type::TypeParam(_) => {
                        return Err(Diagnostic::new(
                            "generic type parameters must be specialized before IR lowering",
                        ));
                    }
                    Type::Thread => {
                        return Err(Diagnostic::new(
                            "numeric literal is not assignable to thread",
                        ));
                    }
                    Type::TaggedVariant(_) | Type::TaggedUnion(_) => {
                        return Err(Diagnostic::new(
                            "numeric literal is not assignable to tagged union type",
                        ));
                    }
                    Type::Unknown => {
                        // Boxing a bare literal into `unknown`: lower it at its
                        // default numeric type, then box the result into anyref.
                        let ty = match self.infer_expr_type(expr, types, None)? {
                            Type::Numeric(ty) => ty,
                            other => {
                                return Err(Diagnostic::new(format!(
                                    "cannot box numeric literal as unknown: {other}"
                                )));
                            }
                        };
                        let value = self.emit(Instruction::Number {
                            ty,
                            literal: number.clone(),
                        });
                        return self.coerce_value(value, Type::Numeric(ty), Some(Type::Unknown));
                    }
                };
                self.emit(Instruction::Number {
                    ty,
                    literal: number.clone(),
                })
            }
            Expr::Bool(value, _) => self.emit(Instruction::Bool(*value)),
            Expr::Nil(_) => {
                let ty = match expected.clone() {
                    Some(Type::Nullable(inner)) => *inner,
                    Some(Type::Extern) => Type::Extern,
                    Some(other) => {
                        return Err(Diagnostic::new(format!(
                            "nil is only assignable to nullable extern, got {other}"
                        )));
                    }
                    None => Type::Extern,
                };
                let value = self.emit(Instruction::Null { ty: ty.clone() });
                self.coerce_value(value, ty, expected)?
            }
            Expr::String(value, _) => self.emit(Instruction::String(value.clone())),
            Expr::Bytes(value, _) => self.emit(Instruction::Bytes(value.clone())),
            Expr::Name(name, symbol_id, _) => {
                let symbol_id = symbol_id.expect("symbol_id should be resolved");
                if let Some(value) = env.get(&symbol_id).copied() {
                    let actual = types.get(&symbol_id).cloned().ok_or_else(|| {
                        Diagnostic::new(format!("unknown local '{name}' during IR lowering"))
                    })?;
                    if self.cell_names.contains(&symbol_id) {
                        let index0 = self.emit(Instruction::Number {
                            ty: NumericType::I32,
                            literal: NumberLiteral { raw: "0".into() },
                        });
                        let val = self.emit(Instruction::ArrayGet {
                            array: value,
                            index: index0,
                            element_ty: to_runtime_type(&actual),
                        });
                        self.coerce_value(val, actual, expected)?
                    } else {
                        self.coerce_value(value, actual, expected)?
                    }
                } else if let Some((params, return_type)) = self.signatures.get(&symbol_id).cloned() {
                    let value = self.emit(Instruction::Closure {
                        name: name.clone(),
                        captures: Vec::new(),
                        params: params.clone(),
                        return_type: return_type.clone(),
                    });
                    let actual = Type::Function {
                        params,
                        return_type: Box::new(return_type),
                    };
                    self.coerce_value(value, actual, expected)?
                } else {
                    return Err(Diagnostic::new(format!(
                        "unknown local '{name}' during IR lowering"
                    )));
                }
            }
            Expr::MethodCall {
                receiver,
                name,
                args,
                ..
            } => {
                let receiver_ty = self.infer_expr_type(receiver, types, None)?;
                let type_method =
                    type_method_signature(&receiver_ty, name, self.field_call_signatures);
                let (param_types, return_type) = if let Some((_, params, return_type)) =
                    type_method.clone()
                {
                    (params, Box::new(return_type))
                } else if let Some(signature) =
                    method_signature(receiver, name, self.field_call_signatures)
                {
                    let (params, return_type) = signature;
                    (params, Box::new(return_type))
                } else {
                    let field_ty = receiver_ty
                        .record_field(name)
                        .ok_or_else(|| Diagnostic::new(format!("unknown record field '{name}'")))?;
                    let Type::Function {
                        params,
                        return_type,
                    } = field_ty
                    else {
                        return Err(Diagnostic::new("attempt to call non-function value"));
                    };
                    (params, return_type)
                };
                if param_types.is_empty() {
                    return Err(Diagnostic::new(format!(
                        "function expects 0 arguments, got {}",
                        args.len() + 1
                    )));
                }
                if !method_receiver_matches(&param_types[0], &receiver_ty) {
                    return Err(Diagnostic::new(format!(
                        "call expected {}, got {}",
                        param_types[0], receiver_ty
                    )));
                }
                let receiver_value =
                    self.lower_expr(receiver, env, types, Some(receiver_ty.clone()))?;
                let direct_name = self.direct_record_field_closure_name(receiver_value, name);
                let mut lowered_args = Vec::with_capacity(args.len() + 1);
                let lowered_receiver = self.coerce_method_receiver(
                    receiver_value,
                    &receiver_ty,
                    &param_types[0],
                )?;
                lowered_args.push(lowered_receiver);
                for (arg, param_ty) in args.iter().zip(param_types.iter().skip(1)) {
                    lowered_args.push(self.lower_expr(arg, env, types, Some(param_ty.clone()))?);
                }
                if param_types.len() != lowered_args.len() {
                    return Err(Diagnostic::new(format!(
                        "function expects {} arguments, got {}",
                        param_types.len(),
                        lowered_args.len()
                    )));
                }
                let value = if let Some((direct_name, _, return_type)) = type_method {
                    if let Some(symbol_id) = self.host_import_names.get(&direct_name) {
                        self.emit(Instruction::HostCall {
                            name: direct_name,
                            symbol_id: *symbol_id,
                            args: lowered_args,
                            return_type,
                        })
                    } else {
                        self.emit(Instruction::Call {
                            name: direct_name,
                            symbol_id: None,
                            args: lowered_args,
                        })
                    }
                } else if let Some(direct_name) = direct_name {
                    self.emit(Instruction::Call {
                        name: direct_name,
                        symbol_id: None,
                        args: lowered_args,
                    })
                } else {
                    let callee_value = self.emit(Instruction::StructGet {
                        base: receiver_value,
                        field: name.clone(),
                        field_ty: Type::Function {
                            params: param_types.clone(),
                            return_type: return_type.clone(),
                        },
                    });
                    self.emit(Instruction::CallValue {
                        callee: callee_value,
                        args: lowered_args,
                        params: param_types.clone(),
                        return_type: *return_type,
                    })
                };
                self.write_back_method_receiver_mutations(
                    receiver_value,
                    lowered_receiver,
                    &receiver_ty,
                    &param_types[0],
                )?;
                let actual = self.infer_expr_type(expr, types, None)?;
                self.coerce_value(value, actual, expected)?
            }
            Expr::Unary { op, expr, .. } => {
                let actual = self.infer_expr_type(expr, types, None)?;
                match op {
                    UnaryOp::Neg => {
                        let operand_ty = match actual {
                            Type::Numeric(ty) => ty,
                            Type::Bool => {
                                return Err(Diagnostic::new(
                                    "unary '-' requires a numeric operand",
                                ));
                            }
                            Type::Unit => {
                                return Err(Diagnostic::new(
                                    "unary '-' requires a numeric operand",
                                ));
                            }
                            Type::String => {
                                return Err(Diagnostic::new(
                                    "unary '-' requires a numeric operand",
                                ));
                            }
                            Type::Bytes => {
                                return Err(Diagnostic::new(
                                    "unary '-' requires a numeric operand",
                                ));
                            }
                            Type::Extern | Type::ExternSubtype(_) => {
                                return Err(Diagnostic::new(
                                    "unary '-' requires a numeric operand",
                                ));
                            }
                            Type::Nil | Type::Nullable(_) | Type::Named { .. } | Type::Opaque { .. } => {
                                return Err(Diagnostic::new(
                                    "unary '-' requires a numeric operand",
                                ));
                            }
                            Type::Array(_) => {
                                return Err(Diagnostic::new(
                                    "unary '-' requires a numeric operand",
                                ));
                            }
                            Type::Function { .. } | Type::Record(_) => {
                                return Err(Diagnostic::new(
                                    "unary '-' requires a numeric operand",
                                ));
                            }
                            Type::Multi(_) => {
                                return Err(Diagnostic::new(
                                    "unary '-' requires a numeric operand",
                                ));
                            }
                            Type::TypeParam(_) => {
                                return Err(Diagnostic::new(
                                    "unary '-' requires a numeric operand",
                                ));
                            }
                            Type::Thread
                            | Type::Unknown
                            | Type::TaggedVariant(_)
                            | Type::TaggedUnion(_) => {
                                return Err(Diagnostic::new(
                                    "unary '-' requires a numeric operand",
                                ));
                            }
                        };
                        let zero = self.emit(Instruction::Number {
                            ty: operand_ty,
                            literal: NumberLiteral {
                                raw: "0".to_string(),
                            },
                        });
                        let operand =
                            self.lower_expr(expr, env, types, Some(Type::Numeric(operand_ty)))?;
                        let value = self.emit(Instruction::Binary {
                            op: BinaryOp::Sub,
                            left: zero,
                            right: operand,
                            operand_ty: Type::Numeric(operand_ty),
                            result_ty: Type::Numeric(operand_ty),
                        });
                        self.coerce_value(value, Type::Numeric(operand_ty), expected)?
                    }
                    UnaryOp::Not => {
                        let operand = self.lower_expr(expr, env, types, Some(Type::Bool))?;
                        let false_value = self.emit(Instruction::Bool(false));
                        let value = self.emit(Instruction::Binary {
                            op: BinaryOp::Eq,
                            left: operand,
                            right: false_value,
                            operand_ty: Type::Bool,
                            result_ty: Type::Bool,
                        });
                        self.coerce_value(value, Type::Bool, expected)?
                    }
                    UnaryOp::Len => {
                        let actual = self.infer_expr_type(expr, types, None)?;
                        if actual == Type::Bytes {
                            let bytes = self.lower_expr(expr, env, types, Some(Type::Bytes))?;
                            let len = self.emit(Instruction::BytesLen { bytes });
                            return self.coerce_value(
                                len,
                                Type::Numeric(NumericType::I32),
                                expected,
                            );
                        }
                        if !actual.is_array() {
                            return Err(Diagnostic::new("# requires an array or bytes operand"));
                        }
                        let array = self.lower_expr(expr, env, types, Some(actual))?;
                        let len = self.emit(Instruction::ArrayLen { array });
                        self.coerce_value(len, Type::Numeric(NumericType::I32), expected)?
                    }
                }
            }
            Expr::Cast { expr, ty, .. } => {
                let actual = self.infer_expr_type(expr, types, None)?;
                let cast = if require_numeric_cast(actual.clone(), ty.clone()).is_ok() {
                    let value = self.lower_expr(expr, env, types, None)?;
                    self.explicit_cast(value, actual, ty.clone())?
                } else {
                    let typed_actual = self.infer_expr_type(expr, types, Some(ty.clone()))?;
                    let value = self.lower_expr(expr, env, types, Some(ty.clone()))?;
                    self.coerce_value(value, typed_actual, Some(ty.clone()))?
                };
                self.coerce_value(cast, ty.clone(), expected)?
            }
            Expr::IsVariant { expr, tag, .. } => {
                // Lower the base expression as-is (the underlying IR value is the canonical record).
                let base = self.lower_expr(expr, env, types, None)?;
                let tag_id = self.variant_tag_id(tag)?;
                let tag_field_ty = Type::Numeric(NumericType::I32);
                // StructGet "tag" field from the canonical record
                let tag_val = self.emit(Instruction::StructGet {
                    base,
                    field: "tag".to_string(),
                    field_ty: tag_field_ty.clone(),
                });
                let expected_tag = self.emit(Instruction::Number {
                    ty: NumericType::I32,
                    literal: NumberLiteral {
                        raw: tag_id.to_string(),
                    },
                });
                let result = self.emit(Instruction::Binary {
                    op: BinaryOp::Eq,
                    left: tag_val,
                    right: expected_tag,
                    operand_ty: tag_field_ty,
                    result_ty: Type::Bool,
                });
                self.coerce_value(result, Type::Bool, expected)?
            }
            Expr::If {
                condition,
                then_expr,
                else_expr,
                ..
            } => {
                let result_ty = self.infer_expr_type(expr, types, expected.clone())?;
                let condition = self.lower_expr(condition, env, types, Some(Type::Bool))?;
                let then_block = self.new_block();
                let else_block = self.new_block();
                let merge_block = self.new_block();
                self.set_terminator(
                    self.current_block,
                    Terminator::Branch {
                        condition,
                        then_block,
                        else_block,
                    },
                );

                self.current_block = then_block;
                let then_value = self.lower_expr(then_expr, env, types, Some(result_ty.clone()))?;
                let then_exit = self.current_block;
                self.set_terminator(then_exit, Terminator::Jump(merge_block));

                self.current_block = else_block;
                let else_value = self.lower_expr(else_expr, env, types, Some(result_ty.clone()))?;
                let else_exit = self.current_block;
                self.set_terminator(else_exit, Terminator::Jump(merge_block));

                self.current_block = merge_block;
                self.emit(Instruction::Phi(vec![
                    (then_exit, then_value),
                    (else_exit, else_value),
                ]))
            }
            Expr::Binary {
                op, left, right, ..
            } => match op {
                BinaryOp::And | BinaryOp::Or => {
                    let result_ty = self.infer_expr_type(expr, types, expected.clone())?;
                    let left_value = self.lower_expr(left, env, types, Some(Type::Bool))?;
                    let rhs_block = self.new_block();
                    let short_block = self.new_block();
                    let merge_block = self.new_block();

                    match op {
                        BinaryOp::And => {
                            self.set_terminator(
                                self.current_block,
                                Terminator::Branch {
                                    condition: left_value,
                                    then_block: rhs_block,
                                    else_block: short_block,
                                },
                            );
                        }
                        BinaryOp::Or => {
                            self.set_terminator(
                                self.current_block,
                                Terminator::Branch {
                                    condition: left_value,
                                    then_block: short_block,
                                    else_block: rhs_block,
                                },
                            );
                        }
                        _ => unreachable!(),
                    }

                    self.current_block = short_block;
                    let short_circuit_value =
                        self.emit(Instruction::Bool(matches!(op, BinaryOp::Or)));
                    let short_exit = self.current_block;
                    self.set_terminator(short_exit, Terminator::Jump(merge_block));

                    self.current_block = rhs_block;
                    let rhs_value = self.lower_expr(right, env, types, Some(Type::Bool))?;
                    let rhs_exit = self.current_block;
                    self.set_terminator(rhs_exit, Terminator::Jump(merge_block));

                    self.current_block = merge_block;
                    let mut incoming =
                        vec![(short_exit, short_circuit_value), (rhs_exit, rhs_value)];
                    incoming.sort_by_key(|(pred, _)| *pred);
                    let value = self.emit(Instruction::Phi(incoming));
                    self.coerce_value(value, result_ty, expected)?
                }
                _ => {
                    if matches!(op, BinaryOp::Eq | BinaryOp::NotEq)
                        && (matches!(left.as_ref(), Expr::Nil(..))
                            || matches!(right.as_ref(), Expr::Nil(..)))
                    {
                        let value_expr = if matches!(left.as_ref(), Expr::Nil(..)) {
                            right
                        } else {
                            left
                        };
                        let nullable_ty = self.infer_expr_type(value_expr, types, None)?;
                        let inner_ty = nullable_ty.nullable_inner().ok_or_else(|| {
                            Diagnostic::new("nil comparison requires a nullable extern operand")
                        })?;
                        let lowered =
                            self.lower_expr(value_expr, env, types, Some(nullable_ty))?;
                        let mut is_null = self.emit(Instruction::IsNull {
                            value: lowered,
                            ty: inner_ty,
                        });
                        if matches!(op, BinaryOp::NotEq) {
                            let false_value = self.emit(Instruction::Bool(false));
                            is_null = self.emit(Instruction::Binary {
                                op: BinaryOp::Eq,
                                left: is_null,
                                right: false_value,
                                operand_ty: Type::Bool,
                                result_ty: Type::Bool,
                            });
                        }
                        return self.coerce_value(is_null, Type::Bool, expected);
                    }
                    let operand_ty =
                        self.infer_binary_operand_type(left, right, op, types, expected.clone())?;
                    let left = self.lower_expr(left, env, types, Some(operand_ty.clone()))?;
                    let right = self.lower_expr(right, env, types, Some(operand_ty.clone()))?;
                    let raw_result_ty = self.infer_expr_type(expr, types, expected.clone())?;
                    let value = self.emit(Instruction::Binary {
                        op: *op,
                        left,
                        right,
                        operand_ty,
                        result_ty: raw_result_ty.clone(),
                    });
                    self.coerce_value(value, raw_result_ty, expected)?
                }
            },
            Expr::Call {
                callee,
                type_args: _,
                args,
                method_call_origin,
                ..
            } => {
                // Tagged-union constructor: Tag(expr) — emit StructNew for the canonical record.
                if let (Expr::Name(tag, symbol_id, _), [arg]) = (callee.as_ref(), args.as_slice())
                {
                    // symbol_id is None for names that are not resolved to a local or function,
                    // i.e. potential tagged-union constructor names like `Num` or `Flag`.
                    if symbol_id.is_none() {
                        if let Some(variant) =
                            expected.as_ref().and_then(|e| e.tagged_variant(tag))
                        {
                            let payload_ty = *variant.payload;
                            if matches!(&payload_ty, Type::String | Type::Bytes) {
                                return Err(Diagnostic::new(format!(
                                    "tagged union constructor {}({}) is not yet supported: \
                                     string/bytes payloads cannot be boxed into anyref",
                                    tag, payload_ty
                                )));
                            }
                            let payload_val =
                                self.lower_expr(arg, env, types, Some(payload_ty.clone()))?;
                            let boxed_val = self.emit(Instruction::Cast {
                                value: payload_val,
                                from: payload_ty,
                                to: Type::Unknown,
                            });
                            let tag_id = self.variant_tag_id(tag)?;
                            let tag_val = self.emit(Instruction::Number {
                                ty: NumericType::I32,
                                literal: NumberLiteral {
                                    raw: tag_id.to_string(),
                                },
                            });
                            let record_ty = Type::canonical_tagged_union_record();
                            let value = self.emit(Instruction::StructNew {
                                struct_ty: record_ty.clone(),
                                fields: vec![tag_val, boxed_val],
                            });
                            // The canonical record IS the runtime representation of any
                            // tagged-union value, so TaggedUnion/TaggedVariant expected types
                            // are satisfied directly.  Only coerce for other targets (e.g. unknown).
                            let coerce_target = match &expected {
                                Some(Type::TaggedUnion(_) | Type::TaggedVariant(_)) => None,
                                other => other.clone(),
                            };
                            return self.coerce_value(value, record_ty, coerce_target);
                        }
                    }
                }
                if let Some(name) = builtin_name(callee.as_ref()) {
                    if let Some(result) =
                        self.lower_math_builtin_call(&name, args, env, types, expected.clone())
                    {
                        return result;
                    }
                    if let Some(result) = self.lower_coroutine_builtin_call(
                        &name,
                        args,
                        env,
                        types,
                        expected.clone(),
                    ) {
                        return result;
                    }
                    if let Some(result) =
                        self.lower_tostring_builtin_call(&name, args, env, types, expected.clone())
                    {
                        return result;
                    }
                    if let Some(result) =
                        self.lower_table_builtin_call(&name, args, env, types, expected.clone())
                    {
                        return result;
                    }
                    if let Some(result) =
                        self.lower_print_builtin_call(&name, args, env, types, expected.clone())
                    {
                        return result;
                    }
                    if let Some(result) =
                        self.lower_string_builtin_call(&name, args, env, types, expected.clone())
                    {
                        return result;
                    }
                }
                if let Expr::Name(name, Some(symbol_id), _) = callee.as_ref() {
                    if let Some((param_types, return_type)) =
                        self.host_import_signatures.get(symbol_id)
                    {
                        let args = args
                            .iter()
                            .zip(param_types.iter())
                            .map(|(arg, param_ty)| {
                                self.lower_expr(arg, env, types, Some(param_ty.clone()))
                            })
                            .collect::<Result<Vec<_>, _>>()?;
                        let value = self.emit(Instruction::HostCall {
                            name: name.clone(),
                            symbol_id: *symbol_id,
                            args,
                            return_type: return_type.clone(),
                        });
                        let actual = self.infer_expr_type(expr, types, None)?;
                        return self.coerce_value(value, actual, expected);
                    }
                    if let Some((param_types, _)) = self.signatures.get(symbol_id) {
                        let args = args
                            .iter()
                            .zip(param_types.iter())
                            .map(|(arg, param_ty)| {
                                self.lower_expr(arg, env, types, Some(param_ty.clone()))
                            })
                            .collect::<Result<Vec<_>, _>>()?;
                        let value = self.emit(Instruction::Call {
                            name: name.clone(),
                            symbol_id: Some(*symbol_id),
                            args,
                        });
                        let actual = self.infer_expr_type(expr, types, None)?;
                        return self.coerce_value(value, actual, expected);
                    }
                }
                if let Some((direct_name, param_types, _)) =
                    direct_field_call_name(callee.as_ref(), self.field_call_signatures)
                {
                    let args = args
                        .iter()
                        .zip(param_types.iter())
                        .map(|(arg, param_ty)| {
                            self.lower_expr(arg, env, types, Some(param_ty.clone()))
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    let value = self.emit(Instruction::Call {
                        name: direct_name,
                        symbol_id: None,
                        args,
                    });
                    let actual = self.infer_expr_type(expr, types, None)?;
                    return self.coerce_value(value, actual, expected);
                }
                let callee_ty = self.infer_expr_type(callee, types, None)?;
                let Type::Function {
                    params: param_types,
                    return_type,
                } = callee_ty
                else {
                    return Err(Diagnostic::new("attempt to call non-function value"));
                };
                let callee_value = self.lower_expr(
                    callee,
                    env,
                    types,
                    Some(Type::Function {
                        params: param_types.clone(),
                        return_type: return_type.clone(),
                    }),
                )?;
                let args = args
                    .iter()
                    .zip(param_types.iter())
                    .map(|(arg, param_ty)| self.lower_expr(arg, env, types, Some(param_ty.clone())))
                    .collect::<Result<Vec<_>, _>>()?;
                let value = self.emit(Instruction::CallValue {
                    callee: callee_value,
                    args: args.clone(),
                    params: param_types.clone(),
                    return_type: *return_type,
                });

                // Handle method call writeback if this call originated from a generic method
                if let Some(method_call_origin) = method_call_origin {
                    if !args.is_empty() && !param_types.is_empty() {
                        // Lower the original receiver expression to get the "before coercion" value  
                        let original_receiver_type = self.infer_expr_type(&method_call_origin.original_receiver, types, None)?;
                        let original_receiver_value = self.lower_expr(
                            &method_call_origin.original_receiver,
                            env,
                            types,
                            Some(original_receiver_type.clone()),
                        )?;
                        
                        // The first argument is the coerced receiver that was passed to the method
                        let coerced_receiver_value = args[0];
                        let expected_receiver_type = &param_types[0];
                        
                        // Apply the same writeback logic as used in MethodCall
                        self.write_back_method_receiver_mutations(
                            original_receiver_value,
                            coerced_receiver_value,
                            &original_receiver_type,
                            expected_receiver_type,
                        )?;
                    }
                }

                let actual = self.infer_expr_type(expr, types, None)?;
                self.coerce_value(value, actual, expected)?
            }
            Expr::Function(function) => {
                let value = self.lower_function_expr(function, env, types)?;
                let actual = self.infer_expr_type(expr, types, None)?;
                self.coerce_value(value, actual, expected)?
            }
            Expr::Require(path, _) => {
                return Err(Diagnostic::new(format!(
                    "unresolved require(\"{path}\") reached IR lowering"
                )));
            }
            Expr::ArrayLiteral { elements, .. } => {
                if elements.is_empty()
                    && matches!(expected.as_ref(), Some(Type::Record(_)))
                {
                    let struct_ty = expected.expect("checked above");
                    let Type::Record(record_fields) = &struct_ty else {
                        unreachable!("checked above");
                    };
                    let value = self.emit(Instruction::StructNew {
                        struct_ty: struct_ty.clone(),
                        fields: Vec::with_capacity(record_fields.len()),
                    });
                    return self.coerce_value(value, struct_ty, None);
                }
                let array_ty = self.infer_array_literal_type(elements, types, expected.clone())?;
                let element_ty = array_ty
                    .element_type()
                    .expect("array literal must have element type");
                let lowered = elements
                    .iter()
                    .map(|element| self.lower_expr(element, env, types, Some(element_ty.clone())))
                    .collect::<Result<Vec<_>, _>>()?;
                let value = self.emit(Instruction::ArrayNew {
                    element_ty,
                    elements: lowered,
                });
                self.coerce_value(value, array_ty, expected)?
            }
            Expr::Index { base, index, .. } => {
                let base_ty = self.infer_expr_type(base, types, None)?;
                if base_ty == Type::Bytes {
                    let bytes = self.lower_expr(base, env, types, Some(Type::Bytes))?;
                    let index =
                        self.lower_expr(index, env, types, Some(Type::Numeric(NumericType::I32)))?;
                    let value = self.emit(Instruction::BytesGet { bytes, index });
                    return self.coerce_value(value, Type::Numeric(NumericType::I32), expected);
                }
                let element_ty = base_ty
                    .element_type()
                    .ok_or_else(|| Diagnostic::new("indexing requires an array or bytes operand"))?;
                let array = self.lower_expr(base, env, types, Some(base_ty))?;
                let index =
                    self.lower_expr(index, env, types, Some(Type::Numeric(NumericType::I32)))?;
                let value = self.emit(Instruction::ArrayGet {
                    array,
                    index,
                    element_ty: element_ty.clone(),
                });
                self.coerce_value(value, element_ty, expected)?
            }
            Expr::TableLiteral { fields, .. } => {
                let struct_ty = self.infer_expr_type(expr, types, expected.clone())?;
                let Type::Record(record_fields) = &struct_ty else {
                    return Err(Diagnostic::new(
                        "table literal lowering requires a record type",
                    ));
                };
                let lowered_fields = record_fields
                    .iter()
                    .map(|(name, field_ty)| {
                        let field_expr = fields
                            .iter()
                            .find(|field| field.name == *name)
                            .ok_or_else(|| {
                                Diagnostic::new(format!(
                                    "missing table literal field '{name}' during lowering"
                                ))
                            })?;
                        self.lower_expr(&field_expr.value, env, types, Some(field_ty.clone()))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let value = self.emit(Instruction::StructNew {
                    struct_ty: struct_ty.clone(),
                    fields: lowered_fields,
                });
                self.coerce_value(value, struct_ty, expected)?
            }
            Expr::Field { base, name, .. } => {
                let base_ty = self.infer_expr_type(base, types, None)?;
                if let Some((getter_name, params, return_type)) =
                    type_property_getter_signature(&base_ty, name, self.field_call_signatures)
                {
                    if params.len() != 1 || !method_receiver_matches(&params[0], &base_ty) {
                        return Err(Diagnostic::new(format!(
                            "property getter for '{name}' does not accept receiver {base_ty}"
                        )));
                    }
                    let receiver =
                        self.lower_expr(base, env, types, Some(params[0].clone()))?;
                    let symbol_id = self.host_import_names.get(&getter_name).copied().ok_or_else(|| {
                        Diagnostic::new(format!(
                            "declared property getter '{getter_name}' is missing a host import symbol"
                        ))
                    })?;
                    let value = self.emit(Instruction::HostCall {
                        name: getter_name,
                        symbol_id,
                        args: vec![receiver],
                        return_type: return_type.clone(),
                    });
                    return self.coerce_value(value, return_type, expected);
                }
                // Special case: `.value` on a narrowed tagged variant — the IR value is a
                // canonical record, so we must StructGet the `unknown` value field and then
                // Cast (unbox) to the payload type.
                if matches!(&base_ty, Type::TaggedVariant(_)) && name == "value" {
                    let payload_ty = base_ty.record_field(name).expect("TaggedVariant has value field");
                    // Lower base without expected (avoids Record<->TaggedVariant coerce mismatch).
                    let base_val = self.lower_expr(base, env, types, None)?;
                    let cast_val = self.unbox_tagged_variant_value(base_val, &payload_ty)?;
                    return self.coerce_value(cast_val, payload_ty, expected);
                }
                let field_ty = base_ty
                    .record_field(name)
                    .ok_or_else(|| Diagnostic::new(format!("unknown record field '{name}'")))?;
                let base = self.lower_expr(base, env, types, Some(base_ty))?;
                let value = self.emit(Instruction::StructGet {
                    base,
                    field: name.clone(),
                    field_ty: field_ty.clone(),
                });
                self.coerce_value(value, field_ty, expected)?
            }
        };
        Ok(value)
    }

    fn lower_expr_list(
        &mut self,
        exprs: &[Expr],
        env: &HashMap<SymbolId, ValueId>,
        types: &HashMap<SymbolId, Type>,
        expected: Option<&[Type]>,
    ) -> Result<Vec<ValueId>, Diagnostic> {
        let mut out = Vec::new();
        for expr in exprs {
            let slot_expected = expected.and_then(|types| types.get(out.len()).cloned());
        let ty = if matches!(expr, Expr::Call { .. } | Expr::MethodCall { .. }) {
            self.infer_expr_type(expr, types, None)?
        } else {
            self.infer_expr_type(expr, types, slot_expected.clone())?
        };
            match ty {
                Type::Multi(multi_types) => {
                    let tuple = self.lower_expr(expr, env, types, None)?;
                    for (index, part) in multi_types.into_iter().enumerate() {
                        let coerced =
                            if let Some(exp) = expected.and_then(|types| types.get(out.len())) {
                                coerce_type(part, Some(exp.clone()))?
                            } else {
                                part
                            };
                        let value = self.emit(Instruction::MultiGet {
                            value: tuple,
                            index,
                            ty: coerced,
                        });
                        out.push(value);
                    }
                }
                scalar => {
                    let coerced = if let Some(exp) = expected.and_then(|types| types.get(out.len()))
                    {
                        coerce_type(scalar, Some(exp.clone()))?
                    } else {
                        scalar
                    };
                    let value = self.lower_expr(expr, env, types, Some(coerced))?;
                    out.push(value);
                }
            }
        }
        Ok(out)
    }

    fn lower_function_expr(
        &mut self,
        function: &waluau_ast::FunctionExpr,
        env: &HashMap<SymbolId, ValueId>,
        types: &HashMap<SymbolId, Type>,
    ) -> Result<ValueId, Diagnostic> {
        let return_ty = Self::function_expr_return_type(function)?;
        let captures = collect_captures(function, env, types, self.signatures);
        let capture_values = captures
            .iter()
            .map(|(symbol_id, _)| {
                env.get(symbol_id).copied().ok_or_else(|| {
                    Diagnostic::new(format!("unknown local symbol ID {:?} during IR lowering", symbol_id))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let lifted_name = format!("{}$lambda{}", self.function.name, self.lambda_counter);
        self.lambda_counter += 1;

        let capture_count = captures.len();
        let mut lifted = Function {
            name: lifted_name.clone(),
            params: Vec::new(),
            return_type: return_ty.clone(),
            entry: BlockId(0),
            blocks: BTreeMap::new(),
            next_value: 0,
            capture_count,
            value_symbols: BTreeMap::new(),
            symbol_id: function.symbol_id,
        };
        lifted.blocks.insert(
            lifted.entry,
            BasicBlock {
                id: lifted.entry,
                instructions: Vec::new(),
                terminator: Terminator::Unreachable { span: None },
            },
        );
        for (symbol_id, ty) in &captures {
            // Captured variables are passed as 1-element array "cells" to nested
            // (lifted) functions so they can observe/mutate shared storage.
            lifted.params.push((
                format!("capture_{}", symbol_id.0),
                Type::Array(Box::new(to_runtime_type(ty))),
            ));
        }
        for param in &function.params {
            lifted.params.push((param.name.clone(), param.ty.clone()));
        }

        let mut nested_env = HashMap::new();
        let mut nested_types = HashMap::new();
        let lifted_entry = lifted.entry;
        for (index, (symbol_id, ty)) in captures.iter().cloned().enumerate() {
            let value = lifted.next_value();
            block_mut(&mut lifted, lifted_entry)
                .instructions
                .push((value, Instruction::Param(index)));
            nested_env.insert(symbol_id, value);
            lifted.value_symbols.insert(value, symbol_id);
            // If the lifted param is an array cell for a captured variable, expose
            // the inner element type within the nested function's type map so that
            // expressions using the name are treated as the element type during lowering.
            if let Some(elem) = ty.element_type() {
                nested_types.insert(symbol_id, elem);
            } else {
                nested_types.insert(symbol_id, ty);
            }
        }
        let captures_count = captures.len();
        for (index, param) in function.params.iter().enumerate() {
            let symbol_id = param.symbol_id.expect("param has resolved symbol_id");
            let value = lifted.next_value();
            block_mut(&mut lifted, lifted_entry)
                .instructions
                .push((value, Instruction::Param(captures_count + index)));
            nested_env.insert(symbol_id, value);
            lifted.value_symbols.insert(value, symbol_id);
            nested_types.insert(symbol_id, param.ty.clone());
        }

        // nested builder should treat the capture parameters as cell-backed names
        // so the nested function will access them via ArrayGet/ArraySet.
        let mut capture_param_symbols: HashSet<SymbolId> =
            captures.iter().map(|(id, _)| *id).collect();
        // Also include any names that the nested function's inner nested functions capture.
        let nested_inner_captures = collect_nested_function_capture_names(&waluau_ast::Function {
            name: waluau_ast::FunctionName::Simple(function.name.clone().unwrap_or_default()),
            symbol_id: function.symbol_id,
            type_params: function.type_params.clone(),
            params: function.params.clone(),
            return_type: Some(return_ty.clone()),
            body: function.body.clone(),
            file_path: function.file_path.clone(),
        });
        capture_param_symbols.extend(nested_inner_captures);

        let mut nested = Builder {
            function: lifted,
            current_block: BlockId(0),
            next_block: 1,
            signatures: self.signatures,
            host_import_signatures: self.host_import_signatures,
            host_import_names: self.host_import_names,
            field_call_signatures: self.field_call_signatures,
            lifted_functions: Vec::new(),
            lambda_counter: 0,
            loop_stack: Vec::new(),
            cell_names: capture_param_symbols,
            sources: self.sources,
            file_path: function.file_path.clone(),
            tag_ids: self.tag_ids,
        };
        if let Some(_name) = &function.name {
            let symbol_id = function.symbol_id.expect("resolved symbol_id");
            let capture_param_values = captures
                .iter()
                .map(|(capture_symbol_id, _)| {
                    nested_env.get(capture_symbol_id).copied().ok_or_else(|| {
                        Diagnostic::new(format!(
                            "missing capture symbol ID {:?} in nested function lowering",
                            capture_symbol_id
                        ))
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            let self_callee = nested.emit(Instruction::Closure {
                name: lifted_name.clone(),
                captures: capture_param_values,
                params: function
                    .params
                    .iter()
                    .map(|param| param.ty.clone())
                    .collect(),
                return_type: return_ty.clone(),
            });
            nested_env.insert(symbol_id, self_callee);
            nested.function.value_symbols.insert(self_callee, symbol_id);
            nested_types.insert(
                symbol_id,
                Type::Function {
                    params: function
                        .params
                        .iter()
                        .map(|param| param.ty.clone())
                        .collect(),
                    return_type: Box::new(return_ty.clone()),
                },
            );
        }
        for stmt in &function.body {
            if nested.current_block == DEAD_BLOCK {
                break;
            }
            nested.lower_stmt(stmt, &mut nested_env, &mut nested_types)?;
        }
        if nested.current_block != DEAD_BLOCK && nested.function.return_type == Type::Unit {
            let value = nested.emit(Instruction::Unit);
            nested.set_terminator(nested.current_block, Terminator::Return(value));
            nested.current_block = DEAD_BLOCK;
        }
        self.lifted_functions.push(nested.function);
        self.lifted_functions.extend(nested.lifted_functions);

        Ok(self.emit(Instruction::Closure {
            name: lifted_name,
            captures: capture_values,
            params: function
                .params
                .iter()
                .map(|param| param.ty.clone())
                .collect(),
            return_type: return_ty,
        }))
    }

    fn infer_expr_type(
        &self,
        expr: &Expr,
        types: &HashMap<SymbolId, Type>,
        expected: Option<Type>,
    ) -> Result<Type, Diagnostic> {
        match expr {
            Expr::Number(..) => match expected {
                Some(Type::Numeric(ty)) => Ok(Type::Numeric(ty)),
                Some(Type::Bool) => {
                    Err(Diagnostic::new("numeric literal is not assignable to bool"))
                }
                Some(Type::Unit) => {
                    Err(Diagnostic::new("numeric literal is not assignable to unit"))
                }
                Some(Type::String) => Err(Diagnostic::new(
                    "numeric literal is not assignable to string",
                )),
                Some(Type::Bytes) => Err(Diagnostic::new(
                    "numeric literal is not assignable to bytes",
                )),
                Some(Type::Extern) | Some(Type::ExternSubtype(_)) => Err(Diagnostic::new(
                    "numeric literal is not assignable to extern",
                )),
                Some(Type::Nil) => Err(Diagnostic::new(
                    "numeric literal is not assignable to nil",
                )),
                Some(Type::Nullable(_)) => Err(Diagnostic::new(
                    "numeric literal is not assignable to nullable extern",
                )),
                Some(Type::Named { name, .. }) => Err(Diagnostic::new(format!(
                    "numeric literal is not assignable to {name}",
                ))),
                Some(Type::Opaque { name, .. }) => Err(Diagnostic::new(format!(
                    "numeric literal is not assignable to {name}",
                ))),
                Some(Type::Array(_)) => Err(Diagnostic::new(
                    "numeric literal is not assignable to array",
                )),
                Some(Type::Function { .. }) => Err(Diagnostic::new(
                    "numeric literal is not assignable to function",
                )),
                Some(Type::Multi(_)) => Err(Diagnostic::new(
                    "numeric literal is not assignable to multiple values",
                )),
                Some(Type::Record(_)) => Err(Diagnostic::new(
                    "numeric literal is not assignable to namespace",
                )),
                Some(Type::TypeParam(_)) => Err(Diagnostic::new(
                    "numeric literal is not assignable to generic type parameter",
                )),
                Some(Type::Thread) => Err(Diagnostic::new(
                    "numeric literal is not assignable to thread",
                )),
                Some(Type::TaggedVariant(_)) | Some(Type::TaggedUnion(_)) => Err(
                    Diagnostic::new("numeric literal is not assignable to tagged union type"),
                ),
                // A literal coerced to `unknown` is boxed; report `unknown` as its type.
                Some(Type::Unknown) => Ok(Type::Unknown),
                None => Ok(Type::number()),
            },
            Expr::Nil(..) => coerce_type(Type::Nil, expected),
            Expr::IsVariant { .. } => coerce_type(Type::Bool, expected),
            Expr::Bool(..) => Ok(Type::Bool),
            Expr::String(..) => Ok(Type::String),
            Expr::Bytes(..) => Ok(Type::Bytes),
            Expr::Require(path, _) => Err(Diagnostic::new(format!(
                "unresolved require(\"{path}\") reached IR lowering"
            ))),
            Expr::Name(name, symbol_id, _) => {
                let symbol_id = symbol_id.expect("symbol_id should be resolved");
                if let Some(ty) = types.get(&symbol_id) {
                    Ok(ty.clone())
                } else if let Some((params, ret)) = self.signatures.get(&symbol_id) {
                    Ok(Type::Function {
                        params: params.clone(),
                        return_type: Box::new(ret.clone()),
                    })
                } else {
                    Err(Diagnostic::new(format!(
                        "unknown local '{name}' during IR lowering"
                    )))
                }
            }
            Expr::MethodCall {
                receiver,
                name,
                args,
                ..
            } => {
                let receiver_ty = self.infer_expr_type(receiver, types, None)?;
                let (params, return_type) = if let Some((_, params, return_type)) =
                    type_method_signature(&receiver_ty, name, self.field_call_signatures)
                {
                    (params, Box::new(return_type))
                } else if let Some(signature) =
                    method_signature(receiver, name, self.field_call_signatures)
                {
                    let (params, return_type) = signature;
                    (params, Box::new(return_type))
                } else {
                    let field_ty = receiver_ty
                        .record_field(name)
                        .ok_or_else(|| Diagnostic::new(format!("unknown record field '{name}'")))?;
                    let Type::Function {
                        params,
                        return_type,
                    } = field_ty
                    else {
                        return Err(Diagnostic::new("attempt to call non-function value"));
                    };
                    (params, return_type)
                };
                if params.is_empty() {
                    return Err(Diagnostic::new(format!(
                        "function expects 0 arguments, got {}",
                        args.len() + 1
                    )));
                }
                if !method_receiver_matches(&params[0], &receiver_ty) {
                    return Err(Diagnostic::new(format!(
                        "call expected {}, got {}",
                        params[0], receiver_ty
                    )));
                }
                let actual_args = args
                    .iter()
                    .map(|arg| self.infer_expr_type(arg, types, None))
                    .collect::<Result<Vec<_>, _>>()?;
                if params.len() != actual_args.len() + 1 {
                    return Err(Diagnostic::new(format!(
                        "function expects {} arguments, got {}",
                        params.len(),
                        actual_args.len() + 1
                    )));
                }
                for (expected_param, actual) in params.iter().skip(1).zip(actual_args.iter()) {
                    if expected_param != actual {
                        return Err(Diagnostic::new(format!(
                            "call expected {}, got {}",
                            expected_param, actual
                        )));
                    }
                }
                coerce_type(*return_type, expected)
            }
            Expr::Unary { op, expr, .. } => match op {
                UnaryOp::Neg => {
                    let actual = self.infer_expr_type(expr, types, expected.clone())?;
                    match actual {
                        Type::Numeric(_) => coerce_type(actual, expected),
                        Type::Bool => Err(Diagnostic::new("unary '-' requires a numeric operand")),
                        Type::Unit => Err(Diagnostic::new("unary '-' requires a numeric operand")),
                        Type::String => {
                            Err(Diagnostic::new("unary '-' requires a numeric operand"))
                        }
                        Type::Bytes => {
                            Err(Diagnostic::new("unary '-' requires a numeric operand"))
                        }
                        Type::Extern | Type::ExternSubtype(_) => {
                            Err(Diagnostic::new("unary '-' requires a numeric operand"))
                        }
                        Type::Nil | Type::Nullable(_) | Type::Named { .. } | Type::Opaque { .. } => {
                            Err(Diagnostic::new("unary '-' requires a numeric operand"))
                        }
                        Type::Array(_) => {
                            Err(Diagnostic::new("unary '-' requires a numeric operand"))
                        }
                        Type::Function { .. } | Type::Record(_) => {
                            Err(Diagnostic::new("unary '-' requires a numeric operand"))
                        }
                        Type::Multi(_) => {
                            Err(Diagnostic::new("unary '-' requires a numeric operand"))
                        }
                        Type::TypeParam(_) => {
                            Err(Diagnostic::new("unary '-' requires a numeric operand"))
                        }
                        Type::Thread
                        | Type::Unknown
                        | Type::TaggedVariant(_)
                        | Type::TaggedUnion(_) => {
                            Err(Diagnostic::new("unary '-' requires a numeric operand"))
                        }
                    }
                }
                UnaryOp::Not => {
                    let actual = self.infer_expr_type(expr, types, Some(Type::Bool))?;
                    if actual == Type::Bool {
                        Ok(Type::Bool)
                    } else {
                        Err(Diagnostic::new("unary 'not' requires a bool operand"))
                    }
                }
                UnaryOp::Len => {
                    let actual = self.infer_expr_type(expr, types, None)?;
                    if actual == Type::Bytes || actual.is_array() {
                        coerce_type(Type::Numeric(NumericType::I32), expected)
                    } else {
                        Err(Diagnostic::new("# requires an array or bytes operand"))
                    }
                }
            },
            Expr::Cast { expr, ty, .. } => {
                let actual = self.infer_expr_type(expr, types, None)?;
                if require_numeric_cast(actual, ty.clone()).is_err() {
                    self.infer_expr_type(expr, types, Some(ty.clone()))?;
                }
                Ok(ty.clone())
            }
            Expr::If {
                condition,
                then_expr,
                else_expr,
                ..
            } => {
                let condition_ty = self.infer_expr_type(condition, types, Some(Type::Bool))?;
                if condition_ty != Type::Bool {
                    return Err(Diagnostic::new("if expression condition must be bool"));
                }
                let then_ty = self.infer_expr_type(then_expr, types, expected.clone())?;
                let else_ty = self.infer_expr_type(else_expr, types, expected.clone())?;
                if then_ty == else_ty {
                    Ok(then_ty)
                } else {
                    Err(Diagnostic::new(
                        "if expression branches must resolve to the same type",
                    ))
                }
            }
            Expr::Call { callee, args, .. } => {
                // Tagged-union constructor type inference mirrors lower_expr detection.
                if let (Expr::Name(tag, symbol_id, _), [_arg]) = (callee.as_ref(), args.as_slice())
                {
                    if symbol_id.is_none() {
                        if let Some(variant) =
                            expected.as_ref().and_then(|e| e.tagged_variant(tag))
                        {
                            let result_ty = Type::TaggedVariant(TaggedVariant {
                                tag: variant.tag.clone(),
                                payload: variant.payload.clone(),
                            });
                            return coerce_type(result_ty, expected);
                        }
                    }
                }
                if let Some(name) = builtin_name(callee.as_ref()) {
                    if let Some(result) = self.infer_math_builtin_call_type(&name, expr, types) {
                        return result;
                    }
                    if let Some(result) =
                        self.infer_coroutine_builtin_call_type(&name, expr, types, expected.clone())
                    {
                        return result;
                    }
                    if let Some(result) = self.infer_tostring_builtin_call_type(&name, expr, types)
                    {
                        return result;
                    }
                    if let Some(result) = self.infer_table_builtin_call_type(&name, expr, types) {
                        return result;
                    }
                    if let Some(result) = self.infer_print_builtin_call_type(&name, expr, types) {
                        return result;
                    }
                    if let Some(result) = self.infer_string_builtin_call_type(&name, expr, types) {
                        return result;
                    }
                }
                let callee_ty = self.infer_expr_type(callee, types, None)?;
                match callee_ty {
                    Type::Function { return_type, .. } => Ok(*return_type),
                    other => Err(Diagnostic::new(format!(
                        "attempt to call non-function value of type {other}",
                    ))),
                }
            }
            Expr::Function(function) => Ok(Type::Function {
                return_type: Box::new(Self::function_expr_return_type(function)?),
                params: function
                    .params
                    .iter()
                    .map(|param| param.ty.clone())
                    .collect(),
            }),
            Expr::ArrayLiteral { elements, .. } => {
                self.infer_array_literal_type(elements, types, expected)
            }
            Expr::TableLiteral { fields, .. } => {
                let mut record_fields = BTreeMap::new();
                let expected_fields = match &expected {
                    Some(Type::Record(fields)) => Some(fields),
                    _ => None,
                };
                for field in fields {
                    let expected_field_ty = expected_fields
                        .and_then(|fields| fields.get(&field.name))
                        .cloned();
                    let field_ty = self.infer_expr_type(&field.value, types, expected_field_ty)?;
                    record_fields.insert(field.name.clone(), field_ty);
                }
                coerce_type(Type::Record(record_fields), expected)
            }
            Expr::Field { base, name, .. } => {
                let base_ty = self.infer_expr_type(base, types, None)?;
                if let Some((_, params, return_type)) =
                    type_property_getter_signature(&base_ty, name, self.field_call_signatures)
                {
                    if params.len() == 1 && method_receiver_matches(&params[0], &base_ty) {
                        return coerce_type(return_type, expected);
                    }
                }
                let field_ty = base_ty
                    .record_field(name)
                    .ok_or_else(|| Diagnostic::new(format!("unknown record field '{name}'")))?;
                coerce_type(field_ty, expected)
            }
            Expr::Index { base, index, .. } => {
                let base_ty = self.infer_expr_type(base, types, None)?;
                let element_ty = if base_ty == Type::Bytes {
                    Type::Numeric(NumericType::I32)
                } else {
                    base_ty
                        .element_type()
                        .ok_or_else(|| Diagnostic::new("indexing requires an array or bytes operand"))?
                };
                let index_ty =
                    self.infer_expr_type(index, types, Some(Type::Numeric(NumericType::I32)))?;
                if index_ty != Type::Numeric(NumericType::I32) {
                    return Err(Diagnostic::new("index must be i32"));
                }
                coerce_type(element_ty, expected)
            }
            Expr::Binary {
                op, left, right, ..
            } => match op {
                BinaryOp::Add
                | BinaryOp::Sub
                | BinaryOp::Mul
                | BinaryOp::Div
                | BinaryOp::FloorDiv
                | BinaryOp::Mod
                | BinaryOp::Concat => {
                    let raw = self.infer_binary_operand_type(left, right, op, types, expected.clone())?;
                    coerce_type(raw, expected)
                }
                BinaryOp::Less
                | BinaryOp::Greater
                | BinaryOp::Eq
                | BinaryOp::NotEq
                | BinaryOp::And
                | BinaryOp::Or => Ok(Type::Bool),
            },
        }
    }

    fn infer_binary_operand_type(
        &self,
        left: &Expr,
        right: &Expr,
        op: &BinaryOp,
        types: &HashMap<SymbolId, Type>,
        expected: Option<Type>,
    ) -> Result<Type, Diagnostic> {
        let expected_numeric = match expected {
            Some(Type::Numeric(numeric)) => Some(numeric),
            _ => None,
        };

        match op {
            BinaryOp::And | BinaryOp::Or => Ok(Type::Bool),
            BinaryOp::Eq | BinaryOp::NotEq => {
                if matches!(left, Expr::Nil(..)) || matches!(right, Expr::Nil(..)) {
                    let value = if matches!(left, Expr::Nil(..)) {
                        right
                    } else {
                        left
                    };
                    let value_ty = self.infer_expr_type(value, types, None)?;
                    if matches!(value_ty, Type::Nullable(_)) {
                        return Ok(Type::Bool);
                    }
                    return Err(Diagnostic::new(
                        "nil comparison requires a nullable extern operand",
                    ));
                }
                let left_ty = self.infer_expr_type(left, types, None)?;
                if left_ty == Type::Bool {
                    let right_ty = self.infer_expr_type(right, types, Some(Type::Bool))?;
                    if right_ty == Type::Bool {
                        Ok(Type::Bool)
                    } else {
                        Err(Diagnostic::new(
                            "could not resolve operand type during IR lowering",
                        ))
                    }
                } else if left_ty == Type::String {
                    let right_ty = self.infer_expr_type(right, types, Some(Type::String))?;
                    if right_ty == Type::String {
                        Ok(Type::String)
                    } else {
                        Err(Diagnostic::new(
                            "could not resolve operand type during IR lowering",
                        ))
                    }
                } else if left_ty == Type::Bytes {
                    let right_ty = self.infer_expr_type(right, types, Some(Type::Bytes))?;
                    if right_ty == Type::Bytes {
                        Ok(Type::Bytes)
                    } else {
                        Err(Diagnostic::new(
                            "could not resolve operand type during IR lowering",
                        ))
                    }
                } else {
                    infer_numeric_common_type(
                        left,
                        right,
                        types,
                        expected_numeric,
                        |expr, expected| self.infer_expr_type(expr, types, expected),
                    )
                }
            }
            BinaryOp::Concat => {
                let left_ty = self.infer_expr_type(left, types, None)?;
                if left_ty == Type::String {
                    let right_ty = self.infer_expr_type(right, types, Some(Type::String))?;
                    if right_ty == Type::String {
                        Ok(Type::String)
                    } else {
                        Err(Diagnostic::new(
                            "could not resolve operand type during IR lowering",
                        ))
                    }
                } else if left_ty == Type::Bytes {
                    let right_ty = self.infer_expr_type(right, types, Some(Type::Bytes))?;
                    if right_ty == Type::Bytes {
                        Ok(Type::Bytes)
                    } else {
                        Err(Diagnostic::new(
                            "could not resolve operand type during IR lowering",
                        ))
                    }
                } else {
                    Err(Diagnostic::new(
                        "could not resolve operand type during IR lowering",
                    ))
                }
            }
            BinaryOp::Add => {
                let left_ty = self.infer_expr_type(left, types, None)?;
                if left_ty == Type::String {
                    Err(Diagnostic::new(
                        "could not resolve operand type during IR lowering",
                    ))
                } else {
                    infer_numeric_common_type(
                        left,
                        right,
                        types,
                        expected_numeric,
                        |expr, expected| self.infer_expr_type(expr, types, expected),
                    )
                }
            }
            BinaryOp::Sub
            | BinaryOp::Mul
            | BinaryOp::Div
            | BinaryOp::FloorDiv
            | BinaryOp::Mod
            | BinaryOp::Less
            | BinaryOp::Greater => {
                if matches!(op, BinaryOp::Less | BinaryOp::Greater) {
                    let left_ty = self.infer_expr_type(left, types, None)?;
                    if left_ty == Type::Bytes {
                        let right_ty = self.infer_expr_type(right, types, Some(Type::Bytes))?;
                        if right_ty == Type::Bytes {
                            return Ok(Type::Bytes);
                        }
                    } else if left_ty == Type::String {
                        let right_ty = self.infer_expr_type(right, types, Some(Type::String))?;
                        if right_ty == Type::String {
                            return Ok(Type::String);
                        }
                    }
                }
                infer_numeric_common_type(left, right, types, expected_numeric, |expr, expected| {
                    self.infer_expr_type(expr, types, expected)
                })
            }
        }
    }

    fn coerce_value(
        &mut self,
        value: ValueId,
        actual: Type,
        expected: Option<Type>,
    ) -> Result<ValueId, Diagnostic> {
        match expected {
            None => Ok(value),
            Some(expected) if actual == expected => Ok(value),
            Some(expected) => {
                let target = coerce_type(actual.clone(), Some(expected.clone()))?;
                if target == actual {
                    Ok(value)
                } else {
                    Ok(self.emit(Instruction::Cast {
                        value,
                        // The value's IR-level type is the canonical record for
                        // TaggedUnion/TaggedVariant; reflect that in the cast so it
                        // matches what `verify` infers for `value`.
                        from: to_runtime_type(&actual),
                        to: target,
                    }))
                }
            }
        }
    }

    fn coerce_method_receiver(
        &mut self,
        value: ValueId,
        actual: &Type,
        expected: &Type,
    ) -> Result<ValueId, Diagnostic> {
        if actual == expected {
            return Ok(value);
        }
        match (actual, expected) {
            (Type::Record(actual_fields), Type::Record(expected_fields))
                if expected_fields
                    .iter()
                    .all(|(name, expected_ty)| actual_fields.get(name) == Some(expected_ty)) =>
            {
                let fields = expected_fields
                    .iter()
                    .map(|(name, field_ty)| {
                        self.emit(Instruction::StructGet {
                            base: value,
                            field: name.clone(),
                            field_ty: field_ty.clone(),
                        })
                    })
                    .collect();
                Ok(self.emit(Instruction::StructNew {
                    struct_ty: expected.clone(),
                    fields,
                }))
            }
            _ => self.coerce_value(value, actual.clone(), Some(expected.clone())),
        }
    }

    fn write_back_method_receiver_mutations(
        &mut self,
        original: ValueId,
        projected: ValueId,
        actual: &Type,
        expected: &Type,
    ) -> Result<(), Diagnostic> {
        if actual == expected {
            return Ok(());
        }
        match (actual, expected) {
            (Type::Record(actual_fields), Type::Record(expected_fields))
                if expected_fields
                    .iter()
                    .all(|(name, expected_ty)| actual_fields.get(name) == Some(expected_ty)) =>
            {
                for (field, field_ty) in expected_fields {
                    let updated = self.emit(Instruction::StructGet {
                        base: projected,
                        field: field.clone(),
                        field_ty: field_ty.clone(),
                    });
                    self.emit(Instruction::StructSet {
                        base: original,
                        field: field.clone(),
                        value: updated,
                    });
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn explicit_cast(
        &mut self,
        value: ValueId,
        from: Type,
        to: Type,
    ) -> Result<ValueId, Diagnostic> {
        require_numeric_cast(from.clone(), to.clone())?;
        if from == to {
            Ok(value)
        } else {
            Ok(self.emit(Instruction::Cast { value, from, to }))
        }
    }

    fn infer_array_literal_type(
        &self,
        elements: &[Expr],
        types: &HashMap<SymbolId, Type>,
        expected: Option<Type>,
    ) -> Result<Type, Diagnostic> {
        if elements.is_empty() {
            return Err(inference_diagnostic(
                "inference/missing-context",
                DiagnosticCategory::MissingContext,
                "empty array literal requires explicit element type",
                "add an explicit element type annotation, e.g. local xs: {i32} = {}",
            ));
        }

        let expected_element = expected.as_ref().and_then(Type::element_type);
        let mut iter = elements.iter();
        let first = iter.next().expect("non-empty array literal");
        let mut element_ty = self.infer_expr_type(first, types, expected_element.clone())?;
        for element in iter {
            let actual = self.infer_expr_type(element, types, Some(element_ty.clone()))?;
            element_ty = common_element_type(element_ty, actual)?;
        }

        coerce_type(Type::Array(Box::new(element_ty)), expected)
    }

    fn lower_coroutine_builtin_call(
        &mut self,
        name: &str,
        args: &[Expr],
        env: &HashMap<SymbolId, ValueId>,
        types: &HashMap<SymbolId, Type>,
        expected: Option<Type>,
    ) -> Option<Result<ValueId, Diagnostic>> {
        let i32_ty = Type::Numeric(NumericType::I32);
        match name {
            COROUTINE_CREATE => {
                if args.len() != 1 {
                    return Some(Err(Diagnostic::new(format!(
                        "{COROUTINE_CREATE} expects 1 argument, got {}",
                        args.len()
                    ))));
                }
                let callee_ty = Type::Function {
                    params: Vec::new(),
                    return_type: Box::new(i32_ty.clone()),
                };
                let coroutine_ty = match self.infer_expr_type(&args[0], types, None) {
                    Ok(ty) => ty,
                    Err(error) => return Some(Err(error)),
                };
                if coroutine_ty != callee_ty {
                    return Some(Err(Diagnostic::new(
                        "coroutine.create expects a zero-argument i32-returning function",
                    )));
                }
                let callee = match self.lower_expr(&args[0], env, types, Some(callee_ty)) {
                    Ok(value) => value,
                    Err(error) => return Some(Err(error)),
                };
                let value = self.emit(Instruction::CoroutineCreate { callee });
                Some(self.coerce_value(value, Type::Thread, expected))
            }
            COROUTINE_RESUME => {
                if args.len() != 1 {
                    return Some(Err(Diagnostic::new(format!(
                        "{COROUTINE_RESUME} expects 1 argument, got {}",
                        args.len()
                    ))));
                }
                let coroutine = match self.lower_expr(&args[0], env, types, Some(Type::Thread)) {
                    Ok(value) => value,
                    Err(error) => return Some(Err(error)),
                };
                // When the expected type is a tagged union, emit the tagged-resume instruction
                // that returns a canonical `{ tag: i32, value: unknown }` record.
                if matches!(&expected, Some(Type::TaggedUnion(_)) | Some(Type::TaggedVariant(_))) {
                    let yielded_tag = match self.variant_tag_id("Yielded") {
                        Ok(tag) => tag,
                        Err(error) => return Some(Err(error)),
                    };
                    let finished_tag = match self.variant_tag_id("Finished") {
                        Ok(tag) => tag,
                        Err(error) => return Some(Err(error)),
                    };
                    let error_tag = match self.variant_tag_id("Error") {
                        Ok(tag) => tag,
                        Err(error) => return Some(Err(error)),
                    };
                    let value = self.emit(Instruction::CoroutineResumeTagged {
                        coroutine,
                        yielded_tag,
                        finished_tag,
                        error_tag,
                    });
                    // Return the canonical record value; the source-level TaggedUnion type is
                    // maintained by the caller via the explicit annotation in types[name].
                    return Some(Ok(value));
                }
                let value = self.emit(Instruction::CoroutineResume { coroutine });
                Some(self.coerce_value(value, Type::Multi(vec![Type::Bool, i32_ty]), expected))
            }
            COROUTINE_CLOSE => {
                if args.len() != 1 {
                    return Some(Err(Diagnostic::new(format!(
                        "{COROUTINE_CLOSE} expects 1 argument, got {}",
                        args.len()
                    ))));
                }
                let coroutine = match self.lower_expr(&args[0], env, types, Some(Type::Thread)) {
                    Ok(value) => value,
                    Err(error) => return Some(Err(error)),
                };
                let value = self.emit(Instruction::CoroutineClose { coroutine });
                Some(self.coerce_value(value, Type::Bool, expected))
            }
            COROUTINE_YIELD => {
                if args.len() != 1 {
                    return Some(Err(Diagnostic::new(format!(
                        "{COROUTINE_YIELD} expects 1 argument, got {}",
                        args.len()
                    ))));
                }
                let yield_value = match self.lower_expr(&args[0], env, types, Some(i32_ty)) {
                    Ok(value) => value,
                    Err(error) => return Some(Err(error)),
                };
                let resume_block = self.new_block();
                self.set_terminator(
                    self.current_block,
                    Terminator::CoroutineYield {
                        value: yield_value,
                        resume_block,
                    },
                );
                self.current_block = resume_block;
                let value = self.emit(Instruction::Unit);
                Some(self.coerce_value(value, Type::Unit, expected))
            }
            _ => None,
        }
    }

    fn infer_coroutine_builtin_call_type(
        &self,
        name: &str,
        call: &Expr,
        types: &HashMap<SymbolId, Type>,
        expected: Option<Type>,
    ) -> Option<Result<Type, Diagnostic>> {
        let Expr::Call { args, .. } = call else {
            return None;
        };
        match name {
            COROUTINE_CREATE => {
                if args.len() != 1 {
                    return Some(Err(Diagnostic::new(format!(
                        "{COROUTINE_CREATE} expects 1 argument, got {}",
                        args.len()
                    ))));
                }
                let coroutine_ty = match self.infer_expr_type(&args[0], types, None) {
                    Ok(ty) => ty,
                    Err(error) => return Some(Err(error)),
                };
                match &coroutine_ty {
                    Type::Function {
                        params,
                        return_type,
                    } if params.is_empty() && **return_type == Type::Numeric(NumericType::I32) => {
                        Some(Ok(Type::Thread))
                    }
                    _ => Some(Err(Diagnostic::new(
                        "coroutine.create expects a zero-argument i32-returning function",
                    ))),
                }
            }
            COROUTINE_RESUME => {
                if args.len() != 1 {
                    return Some(Err(Diagnostic::new(format!(
                        "{COROUTINE_RESUME} expects 1 argument, got {}",
                        args.len()
                    ))));
                }
                let coroutine_ty = match self.infer_expr_type(&args[0], types, None) {
                    Ok(ty) => ty,
                    Err(error) => return Some(Err(error)),
                };
                match coroutine_ty {
                    Type::Thread => {
                        // Tagged-union expected type → result is canonical record (IR level).
                        if matches!(&expected, Some(Type::TaggedUnion(_)) | Some(Type::TaggedVariant(_))) {
                            return Some(Ok(Type::canonical_tagged_union_record()));
                        }
                        Some(Ok(Type::Multi(vec![
                            Type::Bool,
                            Type::Numeric(NumericType::I32),
                        ])))
                    }
                    _ => Some(Err(Diagnostic::new("coroutine.resume expects a thread"))),
                }
            }
            COROUTINE_CLOSE => {
                if args.len() != 1 {
                    return Some(Err(Diagnostic::new(format!(
                        "{COROUTINE_CLOSE} expects 1 argument, got {}",
                        args.len()
                    ))));
                }
                let coroutine_ty = match self.infer_expr_type(&args[0], types, None) {
                    Ok(ty) => ty,
                    Err(error) => return Some(Err(error)),
                };
                match coroutine_ty {
                    Type::Thread => Some(Ok(Type::Bool)),
                    _ => Some(Err(Diagnostic::new("coroutine.close expects a thread"))),
                }
            }
            COROUTINE_YIELD => {
                if args.len() != 1 {
                    return Some(Err(Diagnostic::new(format!(
                        "{COROUTINE_YIELD} expects 1 argument, got {}",
                        args.len()
                    ))));
                }
                let _ = match self.infer_expr_type(&args[0], types, None) {
                    Ok(ty) => ty,
                    Err(error) => return Some(Err(error)),
                };
                Some(Ok(Type::Unit))
            }
            _ => None,
        }
    }

    fn lower_math_builtin_call(
        &mut self,
        name: &str,
        args: &[Expr],
        env: &HashMap<SymbolId, ValueId>,
        types: &HashMap<SymbolId, Type>,
        expected: Option<Type>,
    ) -> Option<Result<ValueId, Diagnostic>> {
        let (intrinsic, arity) = match name {
            MATH_ABS => (MathIntrinsic::Abs, 1),
            MATH_MIN => (MathIntrinsic::Min, 2),
            MATH_MAX => (MathIntrinsic::Max, 2),
            MATH_SQRT => (MathIntrinsic::Sqrt, 1),
            MATH_FLOOR => (MathIntrinsic::Floor, 1),
            MATH_CEIL => (MathIntrinsic::Ceil, 1),
            MATH_TRUNC => (MathIntrinsic::Trunc, 1),
            MATH_NEAREST => (MathIntrinsic::Nearest, 1),
            MATH_COPYSIGN => (MathIntrinsic::Copysign, 2),
            _ => return None,
        };
        if args.len() != arity {
            return Some(Err(Diagnostic::new(format!(
                "{name} expects {arity} argument{}, got {}",
                if arity == 1 { "" } else { "s" },
                args.len()
            ))));
        }
        let operand_ty = match self.infer_math_builtin_call_type(
            name,
            &Expr::Call {
                callee: Box::new(Expr::Name(name.to_string(), None, None)),
                type_args: Vec::new(),
                args: args.to_vec(),
                span: None,
                method_call_origin: None,
            },
            types,
        ) {
            Some(Ok(Type::Numeric(ty))) => Type::Numeric(ty),
            Some(Ok(_)) => unreachable!(),
            Some(Err(error)) => return Some(Err(error)),
            None => return None,
        };
        let mut lowered = Vec::with_capacity(args.len());
        for arg in args {
            match self.lower_expr(arg, env, types, Some(operand_ty.clone())) {
                Ok(value) => lowered.push(value),
                Err(error) => return Some(Err(error)),
            }
        }
        let value = self.emit(Instruction::MathIntrinsic {
            intrinsic,
            args: lowered,
            operand_ty: operand_ty.clone(),
            result_ty: operand_ty.clone(),
        });
        Some(self.coerce_value(value, operand_ty, expected))
    }

    fn lower_tostring_builtin_call(
        &mut self,
        name: &str,
        args: &[Expr],
        env: &HashMap<SymbolId, ValueId>,
        types: &HashMap<SymbolId, Type>,
        expected: Option<Type>,
    ) -> Option<Result<ValueId, Diagnostic>> {
        if name != TO_STRING {
            return None;
        }
        if args.len() != 1 {
            return Some(Err(Diagnostic::new(format!(
                "{TO_STRING} expects 1 argument, got {}",
                args.len()
            ))));
        }
        let arg_ty = match self.infer_expr_type(&args[0], types, None) {
            Ok(ty) => ty,
            Err(error) => return Some(Err(error)),
        };
        if !(arg_ty.is_numeric() || arg_ty == Type::Bool || arg_ty == Type::String) {
            return Some(Err(Diagnostic::new(format!(
                "{TO_STRING} expects a primitive argument (numeric, bool, or string), got {arg_ty}",
            ))));
        }
        let lowered = match self.lower_expr(&args[0], env, types, Some(arg_ty.clone())) {
            Ok(value) => value,
            Err(error) => return Some(Err(error)),
        };
        let value = if arg_ty == Type::String {
            lowered
        } else {
            self.emit(Instruction::ToString {
                value: lowered,
                from: arg_ty,
            })
        };
        Some(self.coerce_value(value, Type::String, expected))
    }

    fn lower_table_builtin_call(
        &mut self,
        name: &str,
        args: &[Expr],
        env: &HashMap<SymbolId, ValueId>,
        types: &HashMap<SymbolId, Type>,
        expected: Option<Type>,
    ) -> Option<Result<ValueId, Diagnostic>> {
        if name != TABLE_CONCAT {
            return None;
        }
        if args.is_empty() || args.len() > 2 {
            return Some(Err(Diagnostic::new(format!(
                "{TABLE_CONCAT} expects 1 or 2 arguments, got {}",
                args.len()
            ))));
        }
        let array_ty = Type::Array(Box::new(Type::String));
        let list_ty = match self.infer_expr_type(&args[0], types, None) {
            Ok(ty) => ty,
            Err(error) => return Some(Err(error)),
        };
        if list_ty != array_ty {
            return Some(Err(Diagnostic::new(format!(
                "{TABLE_CONCAT} expects an array of strings, got {list_ty}"
            ))));
        }
        let array_value = match self.lower_expr(&args[0], env, types, Some(array_ty)) {
            Ok(value) => value,
            Err(error) => return Some(Err(error)),
        };
        let empty_string = self.emit(Instruction::String(String::new()));
        let separator = if let Some(separator_expr) = args.get(1) {
            match self.infer_expr_type(separator_expr, types, None) {
                Ok(Type::String) => {}
                Ok(ty) => {
                    return Some(Err(Diagnostic::new(format!(
                        "{TABLE_CONCAT} expects a string separator, got {ty}"
                    ))));
                }
                Err(error) => return Some(Err(error)),
            }
            match self.lower_expr(separator_expr, env, types, Some(Type::String)) {
                Ok(value) => value,
                Err(error) => return Some(Err(error)),
            }
        } else {
            empty_string
        };

        let len = self.emit(Instruction::ArrayLen { array: array_value });
        let zero = self.emit(Instruction::Number {
            ty: NumericType::I32,
            literal: NumberLiteral { raw: "0".into() },
        });
        let one = self.emit(Instruction::Number {
            ty: NumericType::I32,
            literal: NumberLiteral { raw: "1".into() },
        });

        // Naive lowering: result = "" then for each element, result = result .. prefix .. element,
        // where prefix starts as "" and becomes the separator after the first element. This avoids
        // a conditional inside the loop body while still placing the separator only between items.
        let preheader = self.current_block;
        let header = self.new_block();
        let loop_body = self.new_block();
        let exit = self.new_block();
        self.set_terminator(preheader, Terminator::Jump(header));

        self.current_block = header;
        let index_phi = self.emit(Instruction::Phi(vec![(preheader, zero)]));
        let acc_phi = self.emit(Instruction::Phi(vec![(preheader, empty_string)]));
        let prefix_phi = self.emit(Instruction::Phi(vec![(preheader, empty_string)]));
        // `len` is loop-invariant, but it must still be threaded through a phi (with a
        // trivial `+ 0` self-edge) so the local allocator's liveness analysis treats it as
        // live across the back-edge — mirrors `array_len_phi` in `lower_for_in`.
        let len_phi = self.emit(Instruction::Phi(vec![(preheader, len)]));
        let cond = self.emit(Instruction::Binary {
            op: BinaryOp::Less,
            left: index_phi,
            right: len_phi,
            operand_ty: Type::Numeric(NumericType::I32),
            result_ty: Type::Bool,
        });
        self.set_terminator(
            header,
            Terminator::Branch {
                condition: cond,
                then_block: loop_body,
                else_block: exit,
            },
        );

        self.current_block = loop_body;
        let element = self.emit(Instruction::ArrayGet {
            array: array_value,
            index: index_phi,
            element_ty: Type::String,
        });
        let with_prefix = self.emit(Instruction::Binary {
            op: BinaryOp::Concat,
            left: acc_phi,
            right: prefix_phi,
            operand_ty: Type::String,
            result_ty: Type::String,
        });
        let next_acc = self.emit(Instruction::Binary {
            op: BinaryOp::Concat,
            left: with_prefix,
            right: element,
            operand_ty: Type::String,
            result_ty: Type::String,
        });
        let next_index = self.emit(Instruction::Binary {
            op: BinaryOp::Add,
            left: index_phi,
            right: one,
            operand_ty: Type::Numeric(NumericType::I32),
            result_ty: Type::Numeric(NumericType::I32),
        });
        let next_len = self.emit(Instruction::Binary {
            op: BinaryOp::Add,
            left: len_phi,
            right: zero,
            operand_ty: Type::Numeric(NumericType::I32),
            result_ty: Type::Numeric(NumericType::I32),
        });
        let body_exit = self.current_block;
        self.set_terminator(body_exit, Terminator::Jump(header));
        add_phi_incoming(&mut self.function, header, index_phi, (body_exit, next_index));
        add_phi_incoming(&mut self.function, header, acc_phi, (body_exit, next_acc));
        add_phi_incoming(&mut self.function, header, prefix_phi, (body_exit, separator));
        add_phi_incoming(&mut self.function, header, len_phi, (body_exit, next_len));

        self.current_block = exit;
        Some(self.coerce_value(acc_phi, Type::String, expected))
    }

    fn lower_print_builtin_call(
        &mut self,
        name: &str,
        args: &[Expr],
        env: &HashMap<SymbolId, ValueId>,
        types: &HashMap<SymbolId, Type>,
        expected: Option<Type>,
    ) -> Option<Result<ValueId, Diagnostic>> {
        if name != PRINT {
            return None;
        }
        if args.len() != 1 {
            return Some(Err(Diagnostic::new(format!(
                "{PRINT} expects 1 argument, got {}",
                args.len()
            ))));
        }
        let value = match self.lower_expr(&args[0], env, types, Some(Type::String)) {
            Ok(value) => value,
            Err(error) => return Some(Err(error)),
        };
        let print_value = self.emit(Instruction::Print { value });
        Some(self.coerce_value(print_value, Type::Unit, expected))
    }

    fn infer_math_builtin_call_type(
        &self,
        name: &str,
        call: &Expr,
        types: &HashMap<SymbolId, Type>,
    ) -> Option<Result<Type, Diagnostic>> {
        let Expr::Call { args, .. } = call else {
            return None;
        };
        let (intrinsic, arity) = match name {
            MATH_ABS => (MathIntrinsic::Abs, 1),
            MATH_MIN => (MathIntrinsic::Min, 2),
            MATH_MAX => (MathIntrinsic::Max, 2),
            MATH_SQRT => (MathIntrinsic::Sqrt, 1),
            MATH_FLOOR => (MathIntrinsic::Floor, 1),
            MATH_CEIL => (MathIntrinsic::Ceil, 1),
            MATH_TRUNC => (MathIntrinsic::Trunc, 1),
            MATH_NEAREST => (MathIntrinsic::Nearest, 1),
            MATH_COPYSIGN => (MathIntrinsic::Copysign, 2),
            _ => return None,
        };
        if args.len() != arity {
            return Some(Err(Diagnostic::new(format!(
                "{name} expects {arity} argument{}, got {}",
                if arity == 1 { "" } else { "s" },
                args.len()
            ))));
        }

        let first_ty = match self.infer_expr_type(&args[0], types, None) {
            Ok(ty) => ty,
            Err(error) => return Some(Err(error)),
        };
        let Type::Numeric(first_numeric) = first_ty else {
            return Some(Err(Diagnostic::new(format!(
                "{name} expects numeric arguments"
            ))));
        };
        if arity == 2 {
            let second =
                match self.infer_expr_type(&args[1], types, Some(Type::Numeric(first_numeric))) {
                    Ok(ty) => ty,
                    Err(error) => return Some(Err(error)),
                };
            if second != Type::Numeric(first_numeric) {
                return Some(Err(Diagnostic::new(format!(
                    "{name} requires both arguments to have the same numeric type"
                ))));
            }
        }

        let supports = match intrinsic {
            MathIntrinsic::Min | MathIntrinsic::Max => {
                matches!(first_numeric, NumericType::F32 | NumericType::F64)
            }
            MathIntrinsic::Abs
            | MathIntrinsic::Sqrt
            | MathIntrinsic::Floor
            | MathIntrinsic::Ceil
            | MathIntrinsic::Trunc
            | MathIntrinsic::Nearest
            | MathIntrinsic::Copysign => {
                matches!(first_numeric, NumericType::F32 | NumericType::F64)
            }
        };
        if !supports {
            return Some(Err(Diagnostic::new(format!(
                "{name} does not support {}",
                Type::Numeric(first_numeric)
            ))));
        }

        Some(Ok(Type::Numeric(first_numeric)))
    }

    fn infer_tostring_builtin_call_type(
        &self,
        name: &str,
        call: &Expr,
        types: &HashMap<SymbolId, Type>,
    ) -> Option<Result<Type, Diagnostic>> {
        if name != TO_STRING {
            return None;
        }
        let Expr::Call { args, .. } = call else {
            return None;
        };
        if args.len() != 1 {
            return Some(Err(Diagnostic::new(format!(
                "{TO_STRING} expects 1 argument, got {}",
                args.len()
            ))));
        }
        let arg_ty = match self.infer_expr_type(&args[0], types, None) {
            Ok(ty) => ty,
            Err(error) => return Some(Err(error)),
        };
        if arg_ty.is_numeric() || arg_ty == Type::Bool || arg_ty == Type::String {
            Some(Ok(Type::String))
        } else {
            Some(Err(Diagnostic::new(format!(
                "{TO_STRING} expects a primitive argument (numeric, bool, or string), got {arg_ty}",
            ))))
        }
    }

    fn infer_table_builtin_call_type(
        &self,
        name: &str,
        call: &Expr,
        types: &HashMap<SymbolId, Type>,
    ) -> Option<Result<Type, Diagnostic>> {
        if name != TABLE_CONCAT {
            return None;
        }
        let Expr::Call { args, .. } = call else {
            return None;
        };
        if args.is_empty() || args.len() > 2 {
            return Some(Err(Diagnostic::new(format!(
                "{TABLE_CONCAT} expects 1 or 2 arguments, got {}",
                args.len()
            ))));
        }
        let list_ty = match self.infer_expr_type(&args[0], types, None) {
            Ok(ty) => ty,
            Err(error) => return Some(Err(error)),
        };
        if list_ty != Type::Array(Box::new(Type::String)) {
            return Some(Err(Diagnostic::new(format!(
                "{TABLE_CONCAT} expects an array of strings, got {list_ty}"
            ))));
        }
        if let Some(separator) = args.get(1) {
            match self.infer_expr_type(separator, types, None) {
                Ok(Type::String) => {}
                Ok(ty) => {
                    return Some(Err(Diagnostic::new(format!(
                        "{TABLE_CONCAT} expects a string separator, got {ty}"
                    ))));
                }
                Err(error) => return Some(Err(error)),
            }
        }
        Some(Ok(Type::String))
    }

    fn infer_print_builtin_call_type(
        &self,
        name: &str,
        call: &Expr,
        types: &HashMap<SymbolId, Type>,
    ) -> Option<Result<Type, Diagnostic>> {
        if name != PRINT {
            return None;
        }
        let Expr::Call { args, .. } = call else {
            return None;
        };
        if args.len() != 1 {
            return Some(Err(Diagnostic::new(format!(
                "{PRINT} expects 1 argument, got {}",
                args.len()
            ))));
        }
        match self.infer_expr_type(&args[0], types, Some(Type::String)) {
            Ok(Type::String) => Some(Ok(Type::Unit)),
            Ok(actual) => Some(Err(Diagnostic::new(format!(
                "{PRINT} expects string, got {actual}",
            )))),
            Err(error) => Some(Err(error)),
        }
    }

    fn lower_string_builtin_call(
        &mut self,
        name: &str,
        args: &[Expr],
        env: &HashMap<SymbolId, ValueId>,
        types: &HashMap<SymbolId, Type>,
        expected: Option<Type>,
    ) -> Option<Result<ValueId, Diagnostic>> {
        if name != STRING_FIND {
            return None;
        }
        
        // Validate argument count
        if args.len() != 2 {
            return Some(Err(Diagnostic::new(format!(
                "{STRING_FIND} expects 2 arguments, got {}",
                args.len()
            ))));
        }

        // Lower haystack argument
        let haystack = match self.lower_expr(&args[0], env, types, Some(Type::String)) {
            Ok(val) => val,
            Err(error) => return Some(Err(error)),
        };

        // Lower needle argument  
        let needle = match self.lower_expr(&args[1], env, types, Some(Type::String)) {
            Ok(val) => val,
            Err(error) => return Some(Err(error)),
        };

        // Call the string_find host function
        let call_args = vec![haystack, needle];

        // The result type: i32 (position or -1 if not found)
        let result_ty = Type::Numeric(NumericType::I32);

        // Get the symbol_id for the string_find host function
        let symbol_id = self.host_import_names.get(STRING_FIND).copied().ok_or_else(|| {
            Diagnostic::new(format!(
                "declared function '{STRING_FIND}' is missing a host import symbol"
            ))
        });
        let symbol_id = match symbol_id {
            Ok(id) => id,
            Err(error) => return Some(Err(error)),
        };

        let result_value = self.emit(Instruction::HostCall {
            name: STRING_FIND.to_string(),
            symbol_id,
            args: call_args,
            return_type: result_ty.clone(),
        });
        
        Some(self.coerce_value(result_value, result_ty, expected))
    }

    fn infer_string_builtin_call_type(
        &self,
        name: &str,
        call: &Expr,
        types: &HashMap<SymbolId, Type>,
    ) -> Option<Result<Type, Diagnostic>> {
        if name != STRING_FIND {
            return None;
        }
        let Expr::Call { args, .. } = call else {
            return None;
        };
        if args.len() != 2 {
            return Some(Err(Diagnostic::new(format!(
                "{STRING_FIND} expects 2 arguments, got {}",
                args.len()
            ))));
        }

        // Check argument types
        match self.infer_expr_type(&args[0], types, Some(Type::String)) {
            Ok(Type::String) => {},
            Ok(actual) => return Some(Err(Diagnostic::new(format!(
                "{STRING_FIND} expects haystack to be string, got {actual}",
            )))),
            Err(error) => return Some(Err(error)),
        }

        match self.infer_expr_type(&args[1], types, Some(Type::String)) {
            Ok(Type::String) => {},
            Ok(actual) => return Some(Err(Diagnostic::new(format!(
                "{STRING_FIND} expects needle to be string, got {actual}",
            )))),
            Err(error) => return Some(Err(error)),
        }

        // Return type: i32 (position or -1 if not found)
        Some(Ok(Type::Numeric(NumericType::I32)))
    }
}

fn common_element_type(left: Type, right: Type) -> Result<Type, Diagnostic> {
    match (left, right) {
        (Type::Numeric(left), Type::Numeric(right)) => {
            left.common(right).map(Type::Numeric).ok_or_else(|| {
                inference_diagnostic(
                    "inference/conflict",
                    DiagnosticCategory::Conflict,
                    "array literal elements must share a common type",
                    "cast elements to a common numeric type or annotate the array type",
                )
            })
        }
        (left, right) if left == right => Ok(left),
        _ => Err(inference_diagnostic(
            "inference/conflict",
            DiagnosticCategory::Conflict,
            "array literal elements must share a common type",
            "cast elements to a common type or split values into separate arrays",
        )),
    }
}

fn infer_numeric_common_type(
    left: &Expr,
    right: &Expr,
    _types: &HashMap<SymbolId, Type>,
    expected: Option<NumericType>,
    infer: impl Fn(&Expr, Option<Type>) -> Result<Type, Diagnostic>,
) -> Result<Type, Diagnostic> {
    match (
        matches!(left, Expr::Number(..)),
        matches!(right, Expr::Number(..)),
    ) {
        (true, true) => {
            let ty = Type::Numeric(expected.unwrap_or(NumericType::F64));
            let left_ty = infer(left, Some(ty.clone()))?;
            let right_ty = infer(right, Some(ty))?;
            if left_ty == right_ty {
                Ok(left_ty)
            } else {
                Err(inference_diagnostic(
                    "inference/ambiguous",
                    DiagnosticCategory::Ambiguous,
                    "could not resolve operand type during IR lowering",
                    "add an explicit cast to disambiguate numeric operand types",
                ))
            }
        }
        (true, false) => {
            let right_ty = infer(right, None)?;
            let left_ty = infer(left, Some(right_ty.clone()))?;
            common_numeric_type(left_ty, right_ty)
        }
        (false, true) => {
            let left_ty = infer(left, None)?;
            let right_ty = infer(right, Some(left_ty.clone()))?;
            common_numeric_type(left_ty, right_ty)
        }
        (false, false) => {
            let left_ty = infer(left, None)?;
            let right_ty = infer(right, None)?;
            common_numeric_type(left_ty, right_ty)
        }
    }
}

fn common_numeric_type(left: Type, right: Type) -> Result<Type, Diagnostic> {
    match (left, right) {
        (Type::Numeric(left), Type::Numeric(right)) => {
            left.common(right).map(Type::Numeric).ok_or_else(|| {
                inference_diagnostic(
                    "inference/ambiguous",
                    DiagnosticCategory::Ambiguous,
                    "could not resolve operand type during IR lowering",
                    "add an explicit cast to disambiguate numeric operand types",
                )
            })
        }
        _ => Err(inference_diagnostic(
            "inference/conflict",
            DiagnosticCategory::Conflict,
            "could not resolve operand type during IR lowering",
            "change one operand type or cast explicitly",
        )),
    }
}

fn coerce_type(actual: Type, expected: Option<Type>) -> Result<Type, Diagnostic> {
    match expected {
        None => Ok(actual),
        Some(expected) if actual == expected => Ok(expected),
        // Any value implicitly boxes into `unknown` (anyref). Unboxing is explicit-only.
        Some(Type::Unknown) => Ok(Type::Unknown),
        Some(Type::Nullable(expected_inner)) => match actual {
            Type::Nil => Ok(Type::Nullable(expected_inner)),
            Type::Nullable(actual_inner) if actual_inner == expected_inner => {
                Ok(Type::Nullable(expected_inner))
            }
            other if other == *expected_inner => Ok(Type::Nullable(expected_inner)),
            other => Err(Diagnostic::new(format!(
                "cannot implicitly convert {other} to {}?",
                expected_inner
            ))),
        },
        // Records coerce field-by-field so a field value can box into an `unknown`
        // field. Lowering then targets the expected field types and inserts boxes.
        Some(Type::Record(expected_fields)) => {
            let Type::Record(actual_fields) = &actual else {
                return Err(Diagnostic::new(format!(
                    "cannot implicitly convert {actual} to {}",
                    Type::Record(expected_fields)
                )));
            };
            for (name, expected_ty) in &expected_fields {
                let Some(actual_ty) = actual_fields.get(name) else {
                    return Err(Diagnostic::new(format!("missing record field '{name}'")));
                };
                coerce_type(actual_ty.clone(), Some(expected_ty.clone())).map_err(|_| {
                    Diagnostic::new(format!(
                        "record field '{name}' expects {expected_ty}, got {actual_ty}"
                    ))
                })?;
            }
            Ok(Type::Record(expected_fields))
        }
        Some(Type::Numeric(expected_numeric)) => match actual {
            Type::Numeric(actual_numeric)
                if actual_numeric.can_implicitly_widen_to(expected_numeric) =>
            {
                Ok(Type::Numeric(expected_numeric))
            }
            Type::Numeric(actual_numeric) => Err(Diagnostic::new(format!(
                "cannot implicitly convert {actual_numeric} to {expected_numeric}",
            ))),
            Type::Bool => Err(Diagnostic::new(format!(
                "cannot implicitly convert bool to {expected_numeric}",
            ))),
            Type::Unit => Err(Diagnostic::new(format!(
                "cannot implicitly convert unit to {expected_numeric}",
            ))),
            Type::String => Err(Diagnostic::new(format!(
                "cannot implicitly convert string to {expected_numeric}",
            ))),
            Type::Bytes => Err(Diagnostic::new(format!(
                "cannot implicitly convert bytes to {expected_numeric}",
            ))),
            Type::Extern | Type::ExternSubtype(_) => Err(Diagnostic::new(format!(
                "cannot implicitly convert extern to {expected_numeric}",
            ))),
            Type::Nil => Err(Diagnostic::new(format!(
                "cannot implicitly convert nil to {expected_numeric}",
            ))),
            Type::Nullable(_) => Err(Diagnostic::new(format!(
                "cannot implicitly convert nullable value to {expected_numeric}",
            ))),
            Type::Named { name, .. } => Err(Diagnostic::new(format!(
                "cannot implicitly convert {name} to {expected_numeric}",
            ))),
            Type::Opaque { name, .. } => Err(Diagnostic::new(format!(
                "cannot implicitly convert {name} to {expected_numeric}",
            ))),
            Type::Array(_) => Err(Diagnostic::new(format!(
                "cannot implicitly convert array to {expected_numeric}",
            ))),
            Type::Multi(_) => Err(Diagnostic::new(format!(
                "cannot implicitly convert multiple values to {expected_numeric}",
            ))),
            Type::Function { .. } => Err(Diagnostic::new(format!(
                "cannot implicitly convert function to {expected_numeric}",
            ))),
            Type::Record(_) => Err(Diagnostic::new(format!(
                "cannot implicitly convert namespace to {expected_numeric}",
            ))),
            Type::TypeParam(_) => Err(Diagnostic::new(format!(
                "cannot implicitly convert generic type parameter to {expected_numeric}",
            ))),
            Type::Thread => Err(Diagnostic::new(format!(
                "cannot implicitly convert thread to {expected_numeric}",
            ))),
            Type::Unknown => Err(Diagnostic::new(format!(
                "cannot implicitly convert unknown to {expected_numeric}; use an explicit cast",
            ))),
            Type::TaggedVariant(_) | Type::TaggedUnion(_) => Err(Diagnostic::new(format!(
                "cannot implicitly convert {actual} to {expected_numeric}",
            ))),
        },
        Some(Type::Bool) => Err(Diagnostic::new(format!(
            "cannot implicitly convert {actual} to bool",
        ))),
        Some(Type::Unit) => Err(Diagnostic::new(format!(
            "cannot implicitly convert {actual} to unit",
        ))),
        Some(expected) => Err(Diagnostic::new(format!(
            "cannot implicitly convert {actual} to {expected}",
        ))),
    }
}

pub(crate) fn require_numeric_cast(actual: Type, target: Type) -> Result<(), Diagnostic> {
    match (&actual, &target) {
        (Type::Opaque { ty, .. }, target) if ty.as_ref() == target => Ok(()),
        (actual, Type::Opaque { ty, .. }) if actual == ty.as_ref() => Ok(()),
        // Boxing into / unboxing out of `unknown` (anyref) is an explicit cast.
        (_, Type::Unknown) | (Type::Unknown, _) => Ok(()),
        _ => match (actual, target) {
        (Type::Numeric(_), Type::Numeric(_)) => Ok(()),
        _ => Err(Diagnostic::new(
            "casts require numeric source and destination types",
        )),
        },
    }
}

fn block_mut(function: &mut Function, block: BlockId) -> &mut BasicBlock {
    function
        .blocks
        .get_mut(&block)
        .expect("block must exist when mutating")
}

fn add_phi_incoming(
    function: &mut Function,
    block: BlockId,
    phi: ValueId,
    incoming: (BlockId, ValueId),
) {
    let bb = block_mut(function, block);
    let (_, instruction) = bb
        .instructions
        .iter_mut()
        .find(|(value, _)| *value == phi)
        .expect("phi value must exist in header block");
    if let Instruction::Phi(values) = instruction {
        values.push(incoming);
    }
}
