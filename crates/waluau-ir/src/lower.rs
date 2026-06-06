pub fn build(program: &Program) -> Result<Module, Diagnostic> {
    let erased = erase_opaque_types(program);
    let monomorphic = Monomorphizer::new(&erased).run(&erased)?;
    let signatures: HashMap<_, _> = monomorphic
        .functions
        .iter()
        .map(|function| {
            let return_type = function.return_type.clone().ok_or_else(|| {
                Diagnostic::new(format!(
                    "function '{}' must have a concrete return type before IR lowering",
                    function.name
                ))
            })?;
            Ok((
                function.name.to_string(),
                (
                    function
                        .params
                        .iter()
                        .map(|param| param.ty.clone())
                        .collect(),
                    return_type,
                ),
            ))
        })
        .collect::<Result<HashMap<_, _>, _>>()?;
    let mut functions = Vec::new();
    for function in &monomorphic.functions {
        let mut lowered = build_function(function, &signatures, &monomorphic.sources)?;
        functions.push(lowered.remove(0));
        functions.extend(lowered);
    }
    let start = functions
        .iter()
        .position(|function| function.name == "__waluau_top_level_init");
    let module = Module { functions, start };
    verify(&module)?;
    Ok(module)
}

fn erase_opaque_types(program: &Program) -> Program {
    Program {
        functions: program.functions.iter().map(erase_function_opaque_types).collect(),
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
        type_params: function.type_params.clone(),
        params: function
            .params
            .iter()
            .map(|param| waluau_ast::Param {
                name: param.name.clone(),
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
            rebindability,
            ty,
            value,
        } => Stmt::Let {
            name: name.clone(),
            rebindability: *rebindability,
            ty: ty.as_ref().map(erase_type_opaque_types),
            value: erase_expr_opaque_types(value),
        },
        Stmt::Assign { op, name, value } => Stmt::Assign {
            op: *op,
            name: name.clone(),
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
            start,
            stop,
            step,
            body,
        } => Stmt::NumericFor {
            name: name.clone(),
            start: erase_expr_opaque_types(start),
            stop: erase_expr_opaque_types(stop),
            step: step.as_ref().map(erase_expr_opaque_types),
            body: body.iter().map(erase_stmt_opaque_types).collect(),
        },
        Stmt::ForIn {
            names,
            iterator,
            body,
        } => Stmt::ForIn {
            names: names.clone(),
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
                    rebindability: binding.rebindability,
                    ty: binding.ty.as_ref().map(erase_type_opaque_types),
                })
                .collect(),
            values: values.iter().map(erase_expr_opaque_types).collect(),
        },
        Stmt::AssignMulti { targets, values } => Stmt::AssignMulti {
            targets: targets.clone(),
            values: values.iter().map(erase_expr_opaque_types).collect(),
        },
        Stmt::Expr(expr) => Stmt::Expr(erase_expr_opaque_types(expr)),
    }
}

fn erase_expr_opaque_types(expr: &Expr) -> Expr {
    match expr {
        Expr::Number(..)
        | Expr::Bool(..)
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
            implicit_self: function.implicit_self.clone(),
            type_params: function.type_params.clone(),
            params: function
                .params
                .iter()
                .map(|param| waluau_ast::Param {
                    name: param.name.clone(),
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

pub(crate) fn build_function(
    function: &AstFunction,
    signatures: &HashMap<String, (Vec<Type>, Type)>,
    sources: &BTreeMap<String, String>,
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
    // Precompute names that are referenced by any nested function so we can
    // represent them as cell-backed storage (1-element arrays) to support
    // mutable closure capture semantics.
    let captured_names: HashSet<String> = collect_nested_function_capture_names(function);

    for (index, (name, ty)) in out.params.clone().into_iter().enumerate() {
        let value = out.next_value();
        block_mut(&mut out, entry)
            .instructions
            .push((value, Instruction::Param(index)));
        // If this parameter is captured by a nested function, wrap it in a 1-element
        // array cell so closures share mutable storage. Otherwise keep the raw value.
        if captured_names.contains(&name) {
            let cell = out.next_value();
            block_mut(&mut out, entry).instructions.push((
                cell,
                Instruction::ArrayNew {
                    element_ty: ty.clone(),
                    elements: vec![value],
                },
            ));
            env.insert(name, cell);
        } else {
            env.insert(name, value);
        }
        type_env.insert(out.params[index].0.clone(), ty);
    }

    let mut builder = Builder {
        function: out,
        current_block: BlockId(0),
        next_block: 1,
        signatures,
        lifted_functions: Vec::new(),
        lambda_counter: 0,
        loop_stack: Vec::new(),
        cell_names: captured_names,
        sources,
        file_path: function.file_path.clone(),
        tag_ids: BTreeMap::new(),
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
    signatures: &'a HashMap<String, (Vec<Type>, Type)>,
    lifted_functions: Vec<Function>,
    lambda_counter: usize,
    loop_stack: Vec<LoopContext>,
    /// Names that are represented as 1-element array "cells" to support mutable capture.
    cell_names: HashSet<String>,
    sources: &'a BTreeMap<String, String>,
    file_path: String,
    /// Stable discriminant IDs for tagged-union variant names, assigned lazily in
    /// encounter order.  Both producers and consumers within the same function share
    /// this map so IDs are always consistent.
    tag_ids: BTreeMap<String, i32>,
}

#[derive(Clone)]
struct LoopContext {
    header: BlockId,
    continue_target: BlockId,
    break_target: BlockId,
    phis: HashMap<String, ValueId>,
}

fn builtin_name(callee: &Expr) -> Option<String> {
    match callee {
        Expr::Name(name, _) => Some(name.clone()),
        Expr::Field { base, name, .. } => match base.as_ref() {
            Expr::Name(namespace, _) => Some(format!("{namespace}.{name}")),
            _ => None,
        },
        _ => None,
    }
}

fn method_signature_name(base: &str, method: &str) -> String {
    format!("{base}.{method}")
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
    let Expr::Name(base, _) = receiver else {
        return None;
    };
    signatures.get(&method_signature_name(base, name)).cloned()
}

fn direct_field_call_name(
    callee: &Expr,
    signatures: &HashMap<String, (Vec<Type>, Type)>,
) -> Option<(String, Vec<Type>, Type)> {
    let Expr::Field { base, name, .. } = callee else {
        return None;
    };
    let Expr::Name(base, _) = base.as_ref() else {
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

    /// Return (and lazily assign) the stable i32 discriminant for a tagged-union variant name.
    fn variant_tag_id(&mut self, name: &str) -> i32 {
        let next = self.tag_ids.len() as i32;
        *self.tag_ids.entry(name.to_string()).or_insert(next)
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

    fn lower_break(&mut self, _env: &HashMap<String, ValueId>) -> Result<(), Diagnostic> {
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

    fn lower_continue(&mut self, env: &HashMap<String, ValueId>) -> Result<(), Diagnostic> {
        let Some(loop_ctx) = self.loop_stack.last() else {
            return Err(Diagnostic::new("continue is only allowed inside loops"));
        };
        if self.current_block == DEAD_BLOCK {
            return Ok(());
        }
        let current = self.current_block;
        for (name, phi) in &loop_ctx.phis {
            if let Some(value) = env.get(name).copied() {
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
        env: &mut HashMap<String, ValueId>,
        types: &mut HashMap<String, Type>,
    ) -> Result<(), Diagnostic> {
        match stmt {
            Stmt::Let {
                name,
                rebindability: _,
                ty,
                value,
            } => {
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
                if self.cell_names.contains(name) {
                    let cell = self.emit(Instruction::ArrayNew {
                        element_ty: inferred_ty.clone(),
                        elements: vec![value],
                    });
                    env.insert(name.clone(), cell);
                    // Keep the declared type as the inner element type for type checking.
                    types.insert(name.clone(), inferred_ty);
                } else {
                    env.insert(name.clone(), value);
                    types.insert(name.clone(), inferred_ty);
                }
            }
            Stmt::Assign { op, name, value } => {
                let ty = types.get(name).cloned().ok_or_else(|| {
                    Diagnostic::new(format!("unknown local '{name}' during IR lowering"))
                })?;
                if self.cell_names.contains(name) {
                    // Captured local: stored in a 1-element array (cell). Perform ArraySet
                    // rather than rebinding the env entry.
                    let cell = env.get(name).copied().ok_or_else(|| {
                        Diagnostic::new(format!("unknown local '{name}' during IR lowering"))
                    })?;
                    let index0 = self.emit(Instruction::Number {
                        ty: NumericType::I32,
                        literal: NumberLiteral { raw: "0".into() },
                    });
                    match op {
                        AssignOp::Set => {
                            let rhs = self.lower_expr(value, env, types, Some(ty.clone()))?;
                            self.emit(Instruction::ArraySet {
                                array: cell,
                                index: index0,
                                value: rhs,
                                element_ty: ty,
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
                            let current = *env.get(name).ok_or_else(|| {
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
                    env.insert(name.clone(), value);
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
                let (base_ty, field_ty) = if let Expr::Name(base_name, _) = base.as_ref() {
                    let Some(Type::Record(mut fields)) = types.get(base_name).cloned() else {
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

                            // Shape transition for incremental record initialization: rebuild
                            // the struct with existing fields + the newly introduced field.
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
                            env.insert(base_name.clone(), rebuilt);
                            types.insert(base_name.clone(), updated_ty);
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
                    if let Expr::Name(name, _) = callee.as_ref() {
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
                        env.insert(binding.name.clone(), value);
                        types.insert(binding.name.clone(), expected_ty);
                    }
                } else {
                    // No explicit type annotations: infer types from the RHS expressions.
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
                        env.insert(binding.name.clone(), value);
                        types.insert(binding.name.clone(), ty);
                    }
                }
            }
            Stmt::AssignMulti { targets, values } => {
                let mut expected = Vec::new();
                for target in targets {
                    let ty = types.get(target).cloned().ok_or_else(|| {
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
                for (target, value) in targets.iter().zip(lowered) {
                    env.insert(target.clone(), value);
                }
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                self.lower_if(condition, then_body, else_body, env, types)?;
            }
            Stmt::While { condition, body } => {
                self.lower_while(condition, body, env, types)?;
            }
            Stmt::Repeat { body, condition } => {
                self.lower_repeat(body, condition, env, types)?;
            }
            Stmt::NumericFor {
                name,
                start,
                stop,
                step,
                body,
            } => {
                self.lower_numeric_for(name, start, stop, step.as_ref(), body, env, types)?;
            }
            Stmt::ForIn {
                names,
                iterator,
                body,
            } => {
                self.lower_for_in(names, iterator, body, env, types)?;
            }
        }
        Ok(())
    }

    fn lower_assert_call(
        &mut self,
        args: &[Expr],
        span: Option<waluau_ast::Span>,
        env: &mut HashMap<String, ValueId>,
        types: &mut HashMap<String, Type>,
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
        types: &HashMap<String, Type>,
    ) -> (HashMap<String, Type>, HashMap<String, Type>) {
        let mut then_types = types.clone();
        let mut else_types = types.clone();
        let Expr::IsVariant { expr, tag, .. } = condition else {
            return (then_types, else_types);
        };
        let Expr::Name(name, _) = expr.as_ref() else {
            return (then_types, else_types);
        };
        let Some(ty) = types.get(name) else {
            return (then_types, else_types);
        };
        if let Some(variant) = ty.tagged_variant(tag) {
            then_types.insert(name.clone(), Type::TaggedVariant(variant));
        }
        if let Some(remaining) = ty.remove_tagged_variant(tag) {
            else_types.insert(name.clone(), remaining);
        }
        (then_types, else_types)
    }

    fn lower_if(
        &mut self,
        condition: &Expr,
        then_body: &[Stmt],
        else_body: &[Stmt],
        env: &mut HashMap<String, ValueId>,
        types: &mut HashMap<String, Type>,
    ) -> Result<(), Diagnostic> {
        let (then_types_init, else_types_init) =
            Self::narrowed_variant_type_scopes(condition, types);
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

        let mut then_env = env.clone();
        let mut then_types = then_types_init;
        self.current_block = then_block;
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
        let mut else_types = else_types_init;
        self.current_block = else_block;
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
                    let mut incoming = Vec::new();
                    if then_exit != DEAD_BLOCK {
                        incoming.push((then_exit, tv));
                    }
                    if else_exit != DEAD_BLOCK {
                        incoming.push((else_exit, ev));
                    }
                    let phi = self.emit(Instruction::Phi(incoming));
                    env.insert(name, phi);
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

    fn lower_while(
        &mut self,
        condition: &Expr,
        body: &[Stmt],
        env: &mut HashMap<String, ValueId>,
        types: &mut HashMap<String, Type>,
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
        for name in &mutated {
            if let Some(initial) = env.get(name).copied() {
                let phi = self.emit(Instruction::Phi(vec![(preheader, initial)]));
                loop_env.insert(name.clone(), phi);
                phis.insert(name.clone(), phi);
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
            for (name, phi) in &phis {
                if let Some(next_value) = body_env.get(name).copied() {
                    add_phi_incoming(&mut self.function, header, *phi, (body_exit, next_value));
                }
            }
        }

        for (name, phi) in phis {
            env.insert(name, phi);
        }
        self.current_block = exit;
        Ok(())
    }

    fn lower_repeat(
        &mut self,
        body: &[Stmt],
        condition: &Expr,
        env: &mut HashMap<String, ValueId>,
        types: &mut HashMap<String, Type>,
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
                loop_env.insert(name.clone(), phi);
                phis.insert(name.clone(), phi);
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
            env.insert(name.clone(), body_env.get(&name).copied().unwrap_or(phi));
        }
        self.current_block = exit;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn lower_numeric_for(
        &mut self,
        name: &str,
        start: &Expr,
        stop: &Expr,
        step: Option<&Expr>,
        body: &[Stmt],
        env: &mut HashMap<String, ValueId>,
        types: &mut HashMap<String, Type>,
    ) -> Result<(), Diagnostic> {
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
        for name in &mutated {
            if let Some(initial) = env.get(name).copied() {
                let phi = self.emit(Instruction::Phi(vec![(preheader, initial)]));
                loop_env.insert(name.clone(), phi);
                phis.insert(name.clone(), phi);
            }
        }
        let stop_phi = self.emit(Instruction::Phi(vec![(preheader, stop_init)]));
        let index_phi = self.emit(Instruction::Phi(vec![(preheader, start_value)]));
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
        body_env.insert(name.to_string(), index_phi);
        body_types.insert(name.to_string(), loop_ty.clone());
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
            for (name, phi) in &phis {
                if let Some(next_value) = body_env.get(name).copied() {
                    add_phi_incoming(&mut self.function, header, *phi, (body_exit, next_value));
                }
            }
        }

        for (name, phi) in phis {
            env.insert(name, phi);
        }
        self.current_block = exit;
        Ok(())
    }

    fn lower_for_in(
        &mut self,
        names: &[String],
        iterator: &Expr,
        body: &[Stmt],
        env: &mut HashMap<String, ValueId>,
        types: &mut HashMap<String, Type>,
    ) -> Result<(), Diagnostic> {
        let iterator_ty = self.infer_expr_type(iterator, types, None)?;
        if let Type::Array(element_ty) = &iterator_ty {
            if names.len() != 1 && names.len() != 2 {
                return Err(Diagnostic::new(format!(
                    "array for-in loop expects 1 or 2 loop variables, got {}",
                    names.len()
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
            for name in &mutated {
                if let Some(initial) = env.get(name).copied() {
                    let phi = self.emit(Instruction::Phi(vec![(preheader, initial)]));
                    loop_env.insert(name.clone(), phi);
                    phis.insert(name.clone(), phi);
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

            if names.len() == 1 {
                body_env.insert(names[0].clone(), element_val);
                body_types.insert(names[0].clone(), *element_ty.clone());
            } else {
                body_env.insert(names[0].clone(), index_phi);
                body_types.insert(names[0].clone(), Type::Numeric(NumericType::I32));
                body_env.insert(names[1].clone(), element_val);
                body_types.insert(names[1].clone(), *element_ty.clone());
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
        if return_values.len() != names.len() + 1 {
            return Err(Diagnostic::new(format!(
                "for-in iterator expects {} return values (bool + {} loop values), got {}",
                names.len() + 1,
                names.len(),
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
            Expr::Name(name, _) => self.signatures.get(name).and_then(|(params, ret)| {
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
        for name in &mutated {
            if let Some(initial) = env.get(name).copied() {
                let phi = self.emit(Instruction::Phi(vec![(preheader, initial)]));
                loop_env.insert(name.clone(), phi);
                phis.insert(name.clone(), phi);
            }
        }

        let call = if let Some(name) = direct_iterator_name {
            self.emit(Instruction::Call {
                name,
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
        for (index, (name, ty)) in names.iter().zip(loop_value_types.iter()).enumerate() {
            let value = self.emit(Instruction::MultiGet {
                value: call,
                index: index + 1,
                ty: ty.clone(),
            });
            body_env.insert(name.clone(), value);
            body_types.insert(name.clone(), ty.clone());
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
            for (name, phi) in &phis {
                if let Some(next_value) = body_env.get(name).copied() {
                    add_phi_incoming(&mut self.function, header, *phi, (body_exit, next_value));
                }
            }
        }

        for (name, phi) in phis {
            env.insert(name, phi);
        }
        self.current_block = exit;
        Ok(())
    }

    fn lower_expr(
        &mut self,
        expr: &Expr,
        env: &HashMap<String, ValueId>,
        types: &HashMap<String, Type>,
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
            Expr::String(value, _) => self.emit(Instruction::String(value.clone())),
            Expr::Bytes(value, _) => self.emit(Instruction::Bytes(value.clone())),
            Expr::Name(name, _) => {
                if let Some(value) = env.get(name).copied() {
                    let actual = types.get(name).cloned().ok_or_else(|| {
                        Diagnostic::new(format!("unknown local '{name}' during IR lowering"))
                    })?;
                    // If this name is represented as a cell (1-element array) then
                    // load the element before coercion so the value reflects mutations.
                    if self.cell_names.contains(name) {
                        let index0 = self.emit(Instruction::Number {
                            ty: NumericType::I32,
                            literal: NumberLiteral { raw: "0".into() },
                        });
                        let val = self.emit(Instruction::ArrayGet {
                            array: value,
                            index: index0,
                            element_ty: actual.clone(),
                        });
                        self.coerce_value(val, actual, expected)?
                    } else {
                        self.coerce_value(value, actual, expected)?
                    }
                } else if let Some((params, return_type)) = self.signatures.get(name).cloned() {
                    // A bare top-level function name used as a value becomes a
                    // capture-free function reference (funcref), enabling it to
                    // be stored, returned, and called indirectly.
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
                let (param_types, return_type) = if let Some(signature) =
                    method_signature(receiver, name, self.signatures)
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
                let value = if let Some(direct_name) = direct_name {
                    self.emit(Instruction::Call {
                        name: direct_name,
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
                            Type::Named { .. } | Type::Opaque { .. } => {
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
                let value = self.lower_expr(expr, env, types, None)?;
                let actual = self.infer_expr_type(expr, types, None)?;
                let cast = self.explicit_cast(value, actual, ty.clone())?;
                self.coerce_value(cast, ty.clone(), expected)?
            }
            Expr::IsVariant { expr, tag, .. } => {
                // Lower the base expression as-is (the underlying IR value is the canonical record).
                let base = self.lower_expr(expr, env, types, None)?;
                let tag_id = self.variant_tag_id(tag);
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
                if let (Expr::Name(tag, _), [arg]) = (callee.as_ref(), args.as_slice()) {
                    if self.signatures.get(tag.as_str()).is_none()
                        && types.get(tag.as_str()).is_none()
                    {
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
                            let tag_id = self.variant_tag_id(tag);
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
                        self.lower_print_builtin_call(&name, args, env, types, expected.clone())
                    {
                        return result;
                    }
                }
                if let Expr::Name(name, _) = callee.as_ref() {
                    if let Some((param_types, _)) = self.signatures.get(name) {
                        let args = args
                            .iter()
                            .zip(param_types.iter())
                            .map(|(arg, param_ty)| {
                                self.lower_expr(arg, env, types, Some(param_ty.clone()))
                            })
                            .collect::<Result<Vec<_>, _>>()?;
                        let value = self.emit(Instruction::Call {
                            name: name.clone(),
                            args,
                        });
                        let actual = self.infer_expr_type(expr, types, None)?;
                        return self.coerce_value(value, actual, expected);
                    }
                }
                if let Some((direct_name, param_types, _)) =
                    direct_field_call_name(callee.as_ref(), self.signatures)
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
                // Special case: `.value` on a narrowed tagged variant — the IR value is a
                // canonical record, so we must StructGet the `unknown` value field and then
                // Cast (unbox) to the payload type.
                if matches!(&base_ty, Type::TaggedVariant(_)) && name == "value" {
                    let payload_ty = base_ty.record_field(name).expect("TaggedVariant has value field");
                    if matches!(&payload_ty, Type::String | Type::Bytes) {
                        return Err(Diagnostic::new(format!(
                            "reading .value of type {payload_ty} is not yet supported \
                             (string/bytes payloads cannot be unboxed from anyref)"
                        )));
                    }
                    // Lower base without expected (avoids Record<->TaggedVariant coerce mismatch).
                    let base_val = self.lower_expr(base, env, types, None)?;
                    let unknown_val = self.emit(Instruction::StructGet {
                        base: base_val,
                        field: "value".to_string(),
                        field_ty: Type::Unknown,
                    });
                    let cast_val = self.emit(Instruction::Cast {
                        value: unknown_val,
                        from: Type::Unknown,
                        to: payload_ty.clone(),
                    });
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
        env: &HashMap<String, ValueId>,
        types: &HashMap<String, Type>,
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
        env: &HashMap<String, ValueId>,
        types: &HashMap<String, Type>,
    ) -> Result<ValueId, Diagnostic> {
        let return_ty = Self::function_expr_return_type(function)?;
        let captures = collect_captures(function, env, types, self.signatures);
        let capture_values = captures
            .iter()
            .map(|(name, _)| {
                env.get(name).copied().ok_or_else(|| {
                    Diagnostic::new(format!("unknown local '{name}' during IR lowering"))
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
        };
        lifted.blocks.insert(
            lifted.entry,
            BasicBlock {
                id: lifted.entry,
                instructions: Vec::new(),
                terminator: Terminator::Unreachable { span: None },
            },
        );
        for (name, ty) in &captures {
            // Captured variables are passed as 1-element array "cells" to nested
            // (lifted) functions so they can observe/mutate shared storage.
            lifted
                .params
                .push((name.clone(), Type::Array(Box::new(ty.clone()))));
        }
        for param in &function.params {
            lifted.params.push((param.name.clone(), param.ty.clone()));
        }

        let mut nested_env = HashMap::new();
        let mut nested_types = HashMap::new();
        let lifted_entry = lifted.entry;
        for (index, (name, ty)) in lifted.params.clone().into_iter().enumerate() {
            let value = lifted.next_value();
            block_mut(&mut lifted, lifted_entry)
                .instructions
                .push((value, Instruction::Param(index)));
            nested_env.insert(name.clone(), value);
            // If the lifted param is an array cell for a captured variable, expose
            // the inner element type within the nested function's type map so that
            // expressions using the name are treated as the element type during lowering.
            if let Some(elem) = ty.element_type() {
                nested_types.insert(name, elem);
            } else {
                nested_types.insert(name, ty);
            }
        }

        // nested builder should treat the capture parameters as cell-backed names
        // so the nested function will access them via ArrayGet/ArraySet.
        let mut capture_param_names: HashSet<String> =
            captures.iter().map(|(n, _)| n.clone()).collect();
        // Also include any names that the nested function's inner nested functions capture.
        let nested_inner_captures = collect_nested_function_capture_names(&waluau_ast::Function {
            name: waluau_ast::FunctionName::Simple(function.name.clone().unwrap_or_default()),
            type_params: function.type_params.clone(),
            params: function.params.clone(),
            return_type: Some(return_ty.clone()),
            body: function.body.clone(),
            file_path: function.file_path.clone(),
        });
        capture_param_names.extend(nested_inner_captures);

        let mut nested = Builder {
            function: lifted,
            current_block: BlockId(0),
            next_block: 1,
            signatures: self.signatures,
            lifted_functions: Vec::new(),
            lambda_counter: 0,
            loop_stack: Vec::new(),
            cell_names: capture_param_names,
            sources: self.sources,
            file_path: function.file_path.clone(),
            tag_ids: BTreeMap::new(),
        };
        if let Some(name) = &function.name {
            let capture_param_values = captures
                .iter()
                .map(|(capture_name, _)| {
                    nested_env.get(capture_name).copied().ok_or_else(|| {
                        Diagnostic::new(format!(
                            "missing capture '{}' in nested function lowering",
                            capture_name
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
            nested_env.insert(name.clone(), self_callee);
            nested_types.insert(
                name.clone(),
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
        types: &HashMap<String, Type>,
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
            Expr::IsVariant { .. } => coerce_type(Type::Bool, expected),
            Expr::Bool(..) => Ok(Type::Bool),
            Expr::String(..) => Ok(Type::String),
            Expr::Bytes(..) => Ok(Type::Bytes),
            Expr::Require(path, _) => Err(Diagnostic::new(format!(
                "unresolved require(\"{path}\") reached IR lowering"
            ))),
            Expr::Name(name, _) => {
                if let Some(ty) = types.get(name) {
                    Ok(ty.clone())
                } else if let Some((params, ret)) = self.signatures.get(name) {
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
                let (params, return_type) = if let Some(signature) =
                    method_signature(receiver, name, self.signatures)
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
                        Type::Named { .. } | Type::Opaque { .. } => {
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
                require_numeric_cast(actual, ty.clone())?;
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
                if let (Expr::Name(tag, _), [_arg]) = (callee.as_ref(), args.as_slice()) {
                    if self.signatures.get(tag.as_str()).is_none()
                        && types.get(tag.as_str()).is_none()
                    {
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
                    if let Some(result) = self.infer_print_builtin_call_type(&name, expr, types) {
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
                for field in fields {
                    let field_ty = self.infer_expr_type(&field.value, types, None)?;
                    record_fields.insert(field.name.clone(), field_ty);
                }
                coerce_type(Type::Record(record_fields), expected)
            }
            Expr::Field { base, name, .. } => {
                let base_ty = self.infer_expr_type(base, types, None)?;
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
        types: &HashMap<String, Type>,
        expected: Option<Type>,
    ) -> Result<Type, Diagnostic> {
        let expected_numeric = match expected {
            Some(Type::Numeric(numeric)) => Some(numeric),
            _ => None,
        };

        match op {
            BinaryOp::And | BinaryOp::Or => Ok(Type::Bool),
            BinaryOp::Eq => {
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
                        from: actual,
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
        types: &HashMap<String, Type>,
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
        env: &HashMap<String, ValueId>,
        types: &HashMap<String, Type>,
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
                    let yielded_tag = self.variant_tag_id("Yielded");
                    let finished_tag = self.variant_tag_id("Finished");
                    let error_tag = self.variant_tag_id("Error");
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
        types: &HashMap<String, Type>,
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
        env: &HashMap<String, ValueId>,
        types: &HashMap<String, Type>,
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
                callee: Box::new(Expr::Name(name.to_string(), None)),
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
        env: &HashMap<String, ValueId>,
        types: &HashMap<String, Type>,
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

    fn lower_print_builtin_call(
        &mut self,
        name: &str,
        args: &[Expr],
        env: &HashMap<String, ValueId>,
        types: &HashMap<String, Type>,
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
        types: &HashMap<String, Type>,
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
        types: &HashMap<String, Type>,
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

    fn infer_print_builtin_call_type(
        &self,
        name: &str,
        call: &Expr,
        types: &HashMap<String, Type>,
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
    _types: &HashMap<String, Type>,
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
