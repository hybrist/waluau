use std::collections::HashMap;
use waluau_ast::{Function as AstFunction, Expr, Program, Stmt, SymbolId, Type, MethodCallOrigin};
use waluau_diagnostics::{Diagnostic, DiagnosticCategory};

fn substitute_type(ty: &Type, subst: &HashMap<String, Type>) -> Type {
    match ty {
        Type::TypeParam(name) => subst
            .get(name)
            .cloned()
            .unwrap_or_else(|| Type::TypeParam(name.clone())),
        Type::Array(inner) => Type::Array(Box::new(substitute_type(inner, subst))),
        Type::Multi(types) => {
            Type::Multi(types.iter().map(|ty| substitute_type(ty, subst)).collect())
        }
        Type::Function {
            params,
            return_type,
        } => Type::Function {
            params: params.iter().map(|ty| substitute_type(ty, subst)).collect(),
            return_type: Box::new(substitute_type(return_type, subst)),
        },
        Type::Record(fields) => Type::Record(
            fields
                .iter()
                .map(|(name, ty)| (name.clone(), substitute_type(ty, subst)))
                .collect(),
        ),
        other => other.clone(),
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct SpecializationKey {
    generic_symbol_id: SymbolId,
    type_args: Vec<Type>,
}

#[derive(Clone, Debug)]
struct ActiveSpecialization {
    generic_symbol_id: SymbolId,
    type_args: Vec<Type>,
}

pub(crate) struct Monomorphizer<'a> {
    generic_functions: HashMap<SymbolId, &'a AstFunction>,
    generic_methods: HashMap<(SymbolId, String), &'a waluau_ast::FunctionExpr>,
    specialized_names: HashMap<SpecializationKey, String>,
    pending: Vec<SpecializationKey>,
}

impl<'a> Monomorphizer<'a> {
    pub(crate) fn new(program: &'a Program) -> Self {
        let generic_functions = program
            .functions
            .iter()
            .filter(|function| !function.type_params.is_empty())
            .map(|function| (function.symbol_id.expect("generic function has symbol id"), function))
            .collect();
        let generic_methods = program
            .functions
            .iter()
            .find(|function| function.name.to_string() == "__waluau_top_level_init")
            .map(|function| {
                function
                    .body
                    .iter()
                    .filter_map(|stmt| match stmt {
                        Stmt::FieldAssign {
                            base,
                            name,
                            value: Expr::Function(function),
                            ..
                        } if !function.type_params.is_empty() => match base.as_ref() {
                            Expr::Name(_, Some(table_symbol_id), _) => {
                                Some(((*table_symbol_id, name.clone()), function))
                            }
                            _ => None,
                        },
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            generic_functions,
            generic_methods,
            specialized_names: HashMap::new(),
            pending: Vec::new(),
        }
    }

    pub(crate) fn run(&mut self, program: &Program) -> Result<Program, Diagnostic> {
        let mut functions = program
            .functions
            .iter()
            .filter(|function| function.type_params.is_empty())
            .map(|function| self.rewrite_function(function, &HashMap::new(), None))
            .collect::<Result<Vec<_>, _>>()?;

        while let Some(key) = self.pending.pop() {
            let template = self
                .generic_functions
                .get(&key.generic_symbol_id)
                .copied()
                .ok_or_else(|| {
                    Diagnostic::new(format!(
                        "missing generic function with symbol ID {:?} during monomorphization",
                        key.generic_symbol_id
                    ))
                })?;
            let specialized_name = self
                .specialized_names
                .get(&key)
                .cloned()
                .expect("specialization key should have a generated name");
            let subst = template
                .type_params
                .iter()
                .cloned()
                .zip(key.type_args.iter().cloned())
                .collect::<HashMap<_, _>>();
            let active = ActiveSpecialization {
                generic_symbol_id: key.generic_symbol_id,
                type_args: key.type_args.clone(),
            };
            functions.push(self.rewrite_function_with_name(
                template,
                waluau_ast::FunctionName::Simple(specialized_name),
                &subst,
                Some(&active),
            )?);
        }

        let mut specialized_program = Program {
            functions,
            type_declarations: program.type_declarations.clone(),
            top_level: program.top_level.clone(),
            export: program.export.clone(),
            sources: program.sources.clone(),
            entry_file_path: program.entry_file_path.clone(),
        };

        waluau_ast::resolve_symbols(&mut specialized_program)?;
        Ok(specialized_program)
    }

    fn rewrite_function(
        &mut self,
        function: &AstFunction,
        subst: &HashMap<String, Type>,
        active: Option<&ActiveSpecialization>,
    ) -> Result<AstFunction, Diagnostic> {
        self.rewrite_function_with_name(
            function,
            waluau_ast::FunctionName::Simple(function.name.to_string()),
            subst,
            active,
        )
    }

    fn rewrite_function_with_name(
        &mut self,
        function: &AstFunction,
        name: waluau_ast::FunctionName,
        subst: &HashMap<String, Type>,
        active: Option<&ActiveSpecialization>,
    ) -> Result<AstFunction, Diagnostic> {
        Ok(AstFunction {
            name,
            symbol_id: None,
            type_params: Vec::new(),
            params: function
                .params
                .iter()
                .map(|param| waluau_ast::Param {
                    name: param.name.clone(),
                    symbol_id: None,
                    ty: substitute_type(&param.ty, subst),
                })
                .collect(),
            return_type: function
                .return_type
                .as_ref()
                .map(|ty| substitute_type(ty, subst)),
            body: self.rewrite_stmts(&function.body, subst, active)?,
            file_path: function.file_path.clone(),
        })
    }

    fn rewrite_stmts(
        &mut self,
        stmts: &[Stmt],
        subst: &HashMap<String, Type>,
        active: Option<&ActiveSpecialization>,
    ) -> Result<Vec<Stmt>, Diagnostic> {
        let mut rewritten = Vec::with_capacity(stmts.len());
        for stmt in stmts {
            if let Stmt::FieldAssign {
                value: Expr::Function(function),
                ..
            } = stmt
            {
                if !function.type_params.is_empty() {
                    continue;
                }
            }
            rewritten.push(self.rewrite_stmt(stmt, subst, active)?);
        }
        Ok(rewritten)
    }

    fn rewrite_stmt(
        &mut self,
        stmt: &Stmt,
        subst: &HashMap<String, Type>,
        active: Option<&ActiveSpecialization>,
    ) -> Result<Stmt, Diagnostic> {
        Ok(match stmt {
            Stmt::Let {
                name,
                rebindability,
                ty,
                value,
                ..
            } => Stmt::Let {
                name: name.clone(),
                symbol_id: None,
                rebindability: *rebindability,
                ty: ty.as_ref().map(|ty| substitute_type(ty, subst)),
                value: self.rewrite_expr(value, subst, active)?,
            },
            Stmt::Assign { op, name, value, .. } => Stmt::Assign {
                op: *op,
                name: name.clone(),
                symbol_id: None,
                value: self.rewrite_expr(value, subst, active)?,
            },
            Stmt::IndexAssign {
                op,
                base,
                index,
                value,
            } => Stmt::IndexAssign {
                op: *op,
                base: Box::new(self.rewrite_expr(base, subst, active)?),
                index: Box::new(self.rewrite_expr(index, subst, active)?),
                value: self.rewrite_expr(value, subst, active)?,
            },
            Stmt::FieldAssign {
                op,
                base,
                name,
                value,
            } => Stmt::FieldAssign {
                op: *op,
                base: Box::new(self.rewrite_expr(base, subst, active)?),
                name: name.clone(),
                value: self.rewrite_expr(value, subst, active)?,
            },
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => Stmt::If {
                condition: self.rewrite_expr(condition, subst, active)?,
                then_body: self.rewrite_stmts(then_body, subst, active)?,
                else_body: self.rewrite_stmts(else_body, subst, active)?,
            },
            Stmt::While { condition, body } => Stmt::While {
                condition: self.rewrite_expr(condition, subst, active)?,
                body: self.rewrite_stmts(body, subst, active)?,
            },
            Stmt::Repeat { body, condition } => Stmt::Repeat {
                body: self.rewrite_stmts(body, subst, active)?,
                condition: self.rewrite_expr(condition, subst, active)?,
            },
            Stmt::NumericFor {
                name,
                start,
                stop,
                step,
                body,
                ..
            } => Stmt::NumericFor {
                name: name.clone(),
                symbol_id: None,
                start: self.rewrite_expr(start, subst, active)?,
                stop: self.rewrite_expr(stop, subst, active)?,
                step: step
                    .as_ref()
                    .map(|expr| self.rewrite_expr(expr, subst, active))
                    .transpose()?,
                body: self.rewrite_stmts(body, subst, active)?,
            },
            Stmt::ForIn {
                names,
                iterator,
                body,
                ..
            } => Stmt::ForIn {
                names: names.clone(),
                symbol_ids: None,
                iterator: self.rewrite_expr(iterator, subst, active)?,
                body: self.rewrite_stmts(body, subst, active)?,
            },
            Stmt::Break => Stmt::Break,
            Stmt::Continue => Stmt::Continue,
            Stmt::Return(expr) => Stmt::Return(self.rewrite_expr(expr, subst, active)?),
            Stmt::ReturnMulti(values) => Stmt::ReturnMulti(
                values
                    .iter()
                    .map(|expr| self.rewrite_expr(expr, subst, active))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            Stmt::LetMulti { bindings, values } => Stmt::LetMulti {
                bindings: bindings
                    .iter()
                    .map(|binding| waluau_ast::Binding {
                        name: binding.name.clone(),
                        symbol_id: None,
                        rebindability: binding.rebindability,
                        ty: binding.ty.as_ref().map(|ty| substitute_type(ty, subst)),
                    })
                    .collect(),
                values: values
                    .iter()
                    .map(|expr| self.rewrite_expr(expr, subst, active))
                    .collect::<Result<Vec<_>, _>>()?,
            },
            Stmt::AssignMulti { targets, values, .. } => Stmt::AssignMulti {
                targets: targets.clone(),
                symbol_ids: None,
                values: values
                    .iter()
                    .map(|expr| self.rewrite_expr(expr, subst, active))
                    .collect::<Result<Vec<_>, _>>()?,
            },
            Stmt::Expr(expr) => Stmt::Expr(self.rewrite_expr(expr, subst, active)?),
        })
    }

    fn rewrite_expr(
        &mut self,
        expr: &Expr,
        subst: &HashMap<String, Type>,
        active: Option<&ActiveSpecialization>,
    ) -> Result<Expr, Diagnostic> {
        Ok(match expr {
            Expr::Number(..)
            | Expr::Bool(..)
            | Expr::String(..)
            | Expr::Bytes(..)
            | Expr::Require(..) => expr.clone(),
            Expr::Name(name, _, span) => Expr::Name(name.clone(), None, *span),
            Expr::Unary { op, expr, span } => Expr::Unary {
                op: *op,
                expr: Box::new(self.rewrite_expr(expr, subst, active)?),
                span: *span,
            },
            Expr::Cast { expr, ty, span } => Expr::Cast {
                expr: Box::new(self.rewrite_expr(expr, subst, active)?),
                ty: substitute_type(ty, subst),
                span: *span,
            },
            Expr::Binary {
                op,
                left,
                right,
                span,
            } => Expr::Binary {
                op: *op,
                left: Box::new(self.rewrite_expr(left, subst, active)?),
                right: Box::new(self.rewrite_expr(right, subst, active)?),
                span: *span,
            },
            Expr::IsVariant { expr, tag, span } => Expr::IsVariant {
                expr: Box::new(self.rewrite_expr(expr, subst, active)?),
                tag: tag.clone(),
                span: *span,
            },
            Expr::If {
                condition,
                then_expr,
                else_expr,
                span,
            } => Expr::If {
                condition: Box::new(self.rewrite_expr(condition, subst, active)?),
                then_expr: Box::new(self.rewrite_expr(then_expr, subst, active)?),
                else_expr: Box::new(self.rewrite_expr(else_expr, subst, active)?),
                span: *span,
            },
            Expr::Call {
                callee,
                type_args,
                args,
                span,
                method_call_origin,
            } => self.rewrite_call_expr(callee, type_args, args, *span, method_call_origin, subst, active)?,
            method_call @ Expr::MethodCall { .. } => {
                self.rewrite_method_call(method_call, subst, active)?
            }
            Expr::Function(function) => {
                Expr::Function(self.rewrite_function_expr(function, subst, active)?)
            }
            Expr::ArrayLiteral { elements, span } => Expr::ArrayLiteral {
                elements: elements
                    .iter()
                    .map(|expr| self.rewrite_expr(expr, subst, active))
                    .collect::<Result<Vec<_>, _>>()?,
                span: *span,
            },
            Expr::TableLiteral { fields, span } => Expr::TableLiteral {
                fields: fields
                    .iter()
                    .map(|field| {
                        Ok(waluau_ast::TableField {
                            name: field.name.clone(),
                            value: self.rewrite_expr(&field.value, subst, active)?,
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?,
                span: *span,
            },
            Expr::Field { base, name, span } => Expr::Field {
                base: Box::new(self.rewrite_expr(base, subst, active)?),
                name: name.clone(),
                span: *span,
            },
            Expr::Index { base, index, span } => Expr::Index {
                base: Box::new(self.rewrite_expr(base, subst, active)?),
                index: Box::new(self.rewrite_expr(index, subst, active)?),
                span: *span,
            },
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn rewrite_call_expr(
        &mut self,
        callee: &Expr,
        type_args: &[Type],
        args: &[Expr],
        span: Option<waluau_ast::Span>,
        method_call_origin: &Option<MethodCallOrigin>,
        subst: &HashMap<String, Type>,
        active: Option<&ActiveSpecialization>,
    ) -> Result<Expr, Diagnostic> {
        let args = args
            .iter()
            .map(|expr| self.rewrite_expr(expr, subst, active))
            .collect::<Result<Vec<_>, _>>()?;

        if let Expr::Name(_name, Some(symbol_id), callee_span) = callee {
            if self.generic_functions.contains_key(symbol_id) {
                let concrete_type_args = type_args
                    .iter()
                    .map(|ty| substitute_type(ty, subst))
                    .collect::<Vec<_>>();
                self.check_recursive_specialization(*symbol_id, &concrete_type_args, active)?;
                let specialized_name =
                    self.ensure_specialization(*symbol_id, concrete_type_args.clone())?;
                return Ok(Expr::Call {
                    callee: Box::new(Expr::Name(specialized_name, None, *callee_span)),
                    type_args: Vec::new(),
                    args,
                    span,
                    method_call_origin: method_call_origin.clone(),
                });
            }
        }

        if let Expr::Field { base, name, span } = callee {
            if let Expr::Name(_, Some(table_symbol_id), _) = base.as_ref() {
                let key = (*table_symbol_id, name.clone());
                if let Some(function) = self.generic_methods.get(&key).copied() {
                    let specialized =
                        self.specialize_function_expr(function, type_args, subst, active)?;
                    
                    // For dot-call form, the first argument is the receiver
                    let receiver_expr = if !args.is_empty() {
                        Some(MethodCallOrigin {
                            original_receiver: Box::new(self.rewrite_expr(&args[0], subst, active)?),
                            method_name: name.clone(),
                        })
                    } else {
                        None
                    };
                    
                    return Ok(Expr::Call {
                        callee: Box::new(Expr::Function(specialized)),
                        type_args: Vec::new(),
                        args,
                        span: *span,
                        method_call_origin: receiver_expr,
                    });
                }
            }
        }

        if let Expr::Function(function) = callee {
            if !function.type_params.is_empty() {
                let specialized =
                    self.specialize_function_expr(function, type_args, subst, active)?;
                return Ok(Expr::Call {
                    callee: Box::new(Expr::Function(specialized)),
                    type_args: Vec::new(),
                    args,
                    span,
                    method_call_origin: method_call_origin.clone(),
                });
            }
        }

        Ok(Expr::Call {
            callee: Box::new(self.rewrite_expr(callee, subst, active)?),
            type_args: type_args
                .iter()
                .map(|ty| substitute_type(ty, subst))
                .collect(),
            args,
            span,
            method_call_origin: method_call_origin.clone(),
        })
    }

    fn rewrite_method_call(
        &mut self,
        method_call: &Expr,
        subst: &HashMap<String, Type>,
        active: Option<&ActiveSpecialization>,
    ) -> Result<Expr, Diagnostic> {
        let Expr::MethodCall {
            receiver,
            name,
            args,
            span,
            type_args,
        } = method_call
        else {
            unreachable!("rewrite_method_call requires a MethodCall expression");
        };
        let span = *span;
        let rewritten_receiver = self.rewrite_expr(receiver, subst, active)?;
        let rewritten_args = args
            .iter()
            .map(|expr| self.rewrite_expr(expr, subst, active))
            .collect::<Result<Vec<_>, _>>()?;

        // Specialize generic method calls (`receiver:method<T>(...)`) the same way
        // the dot-call form (`Table.method<T>(receiver, ...)`) is handled in
        // `rewrite_call_expr`: inline the specialized method as a plain call with
        // the receiver threaded in as the explicit `self` argument.
        if !type_args.is_empty() {
            if let Expr::Name(_, Some(table_symbol_id), _) = receiver.as_ref() {
                let key = (*table_symbol_id, name.clone());
                if let Some(function) = self.generic_methods.get(&key).copied() {
                    let specialized =
                        self.specialize_function_expr(function, type_args, subst, active)?;
                    let mut call_args = Vec::with_capacity(rewritten_args.len() + 1);
                    call_args.push(rewritten_receiver);
                    call_args.extend(rewritten_args);
                    return Ok(Expr::Call {
                        callee: Box::new(Expr::Function(specialized)),
                        type_args: Vec::new(),
                        args: call_args,
                        span,
                        method_call_origin: Some(MethodCallOrigin {
                            original_receiver: receiver.clone(),
                            method_name: name.clone(),
                        }),
                    });
                }
            }
        }

        Ok(Expr::MethodCall {
            receiver: Box::new(rewritten_receiver),
            name: name.to_string(),
            args: rewritten_args,
            span,
            type_args: type_args
                .iter()
                .map(|ty| substitute_type(ty, subst))
                .collect(),
        })
    }

    fn rewrite_function_expr(
        &mut self,
        function: &waluau_ast::FunctionExpr,
        subst: &HashMap<String, Type>,
        active: Option<&ActiveSpecialization>,
    ) -> Result<waluau_ast::FunctionExpr, Diagnostic> {
        if !function.type_params.is_empty() {
            return Err(generic_diagnostic(
                "generic/uninstantiated-value",
                "generic function expression must be instantiated before IR lowering",
            ));
        }
        Ok(waluau_ast::FunctionExpr {
            name: function.name.clone(),
            symbol_id: None,
            implicit_self: function.implicit_self.clone(),
            type_params: Vec::new(),
            params: function
                .params
                .iter()
                .map(|param| waluau_ast::Param {
                    name: param.name.clone(),
                    symbol_id: None,
                    ty: substitute_type(&param.ty, subst),
                })
                .collect(),
            return_type: function
                .return_type
                .as_ref()
                .map(|ty| substitute_type(ty, subst)),
            body: self.rewrite_stmts(&function.body, subst, active)?,
            file_path: function.file_path.clone(),
            span: function.span,
        })
    }

    fn specialize_function_expr(
        &mut self,
        function: &waluau_ast::FunctionExpr,
        type_args: &[Type],
        subst: &HashMap<String, Type>,
        active: Option<&ActiveSpecialization>,
    ) -> Result<waluau_ast::FunctionExpr, Diagnostic> {
        if type_args.len() != function.type_params.len() {
            return Err(generic_diagnostic(
                "generic/type-arg-count",
                format!(
                    "generic function expects {} type argument{}, got {}",
                    function.type_params.len(),
                    if function.type_params.len() == 1 {
                        ""
                    } else {
                        "s"
                    },
                    type_args.len()
                ),
            ));
        }
        let mut local_subst = subst.clone();
        for param in &function.type_params {
            local_subst.remove(param);
        }
        for (param, ty) in function.type_params.iter().zip(type_args.iter()) {
            local_subst.insert(param.clone(), substitute_type(ty, subst));
        }
        Ok(waluau_ast::FunctionExpr {
            name: function.name.clone(),
            symbol_id: None,
            implicit_self: function.implicit_self.clone(),
            type_params: Vec::new(),
            params: function
                .params
                .iter()
                .map(|param| waluau_ast::Param {
                    name: param.name.clone(),
                    symbol_id: None,
                    ty: substitute_type(&param.ty, &local_subst),
                })
                .collect(),
            return_type: function
                .return_type
                .as_ref()
                .map(|ty| substitute_type(ty, &local_subst)),
            body: self.rewrite_stmts(&function.body, &local_subst, active)?,
            file_path: function.file_path.clone(),
            span: function.span,
        })
    }

    fn ensure_specialization(
        &mut self,
        generic_symbol_id: SymbolId,
        type_args: Vec<Type>,
    ) -> Result<String, Diagnostic> {
        let template = self
            .generic_functions
            .get(&generic_symbol_id)
            .copied()
            .ok_or_else(|| {
                Diagnostic::new(format!(
                    "missing generic function with symbol ID {:?} during monomorphization",
                    generic_symbol_id
                ))
            })?;
        if type_args.len() != template.type_params.len() {
            return Err(generic_diagnostic(
                "generic/type-arg-count",
                format!(
                    "generic function expects {} type argument{}, got {}",
                    template.type_params.len(),
                    if template.type_params.len() == 1 {
                        ""
                    } else {
                        "s"
                    },
                    type_args.len()
                ),
            ));
        }
        let key = SpecializationKey {
            generic_symbol_id,
            type_args,
        };
        if let Some(existing) = self.specialized_names.get(&key) {
            return Ok(existing.clone());
        }
        let generic_name = template.name.to_string();
        let name = format!(
            "__waluau_generic${}${}",
            generic_name,
            key.type_args
                .iter()
                .map(mangle_type)
                .collect::<Vec<_>>()
                .join("")
        );
        self.specialized_names.insert(key.clone(), name.clone());
        self.pending.push(key);
        Ok(name)
    }

    fn check_recursive_specialization(
        &self,
        generic_symbol_id: SymbolId,
        type_args: &[Type],
        active: Option<&ActiveSpecialization>,
    ) -> Result<(), Diagnostic> {
        let Some(active) = active else {
            return Ok(());
        };
        if active.generic_symbol_id == generic_symbol_id && active.type_args != type_args {
            let template = self.generic_functions.get(&generic_symbol_id).unwrap();
            return Err(generic_diagnostic(
                "generic/cross-specialization-recursion",
                format!(
                    "generic function '{}' cannot recursively instantiate different type arguments in this MVP",
                    template.name
                ),
            ));
        }
        Ok(())
    }
}

fn mangle_type(ty: &Type) -> String {
    match ty {
        Type::Numeric(numeric) => format!("$n{numeric}"),
        Type::Bool => "$bbool".to_string(),
        Type::String => "$sstring".to_string(),
        Type::Array(inner) => format!("$a{}", mangle_type(inner)),
        Type::Multi(types) => format!(
            "$m{}",
            types.iter().map(mangle_type).collect::<Vec<_>>().join("")
        ),
        Type::Function {
            params,
            return_type,
        } => format!(
            "$f{}$r{}",
            params.iter().map(mangle_type).collect::<Vec<_>>().join(""),
            mangle_type(return_type)
        ),
        Type::Record(fields) => format!(
            "$r{}",
            fields
                .iter()
                .map(|(name, ty)| format!("${}${}", name.len(), name) + &mangle_type(ty))
                .collect::<Vec<_>>()
                .join("")
        ),
        Type::TypeParam(name) => format!("$p{name}"),
        #[allow(unreachable_patterns)]
        _ => format!("$u{}", ty),
    }
}

fn generic_diagnostic(code: &'static str, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(format!("[{code}] {}", message.into()))
        .with_code(code)
        .with_category(DiagnosticCategory::Unsupported)
}
