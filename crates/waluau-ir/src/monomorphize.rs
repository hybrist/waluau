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
    generic_name: String,
    type_args: Vec<Type>,
}

#[derive(Clone, Debug)]
struct ActiveSpecialization {
    generic_name: String,
    type_args: Vec<Type>,
}

pub(crate) struct Monomorphizer<'a> {
    generic_functions: HashMap<String, &'a AstFunction>,
    specialized_names: HashMap<SpecializationKey, String>,
    pending: Vec<SpecializationKey>,
}

impl<'a> Monomorphizer<'a> {
    pub(crate) fn new(program: &'a Program) -> Self {
        let generic_functions = program
            .functions
            .iter()
            .filter(|function| !function.type_params.is_empty())
            .map(|function| (function.name.to_string(), function))
            .collect();
        Self {
            generic_functions,
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
                .get(&key.generic_name)
                .copied()
                .ok_or_else(|| {
                    Diagnostic::new(format!(
                        "missing generic function '{}' during monomorphization",
                        key.generic_name
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
                generic_name: template.name.to_string(),
                type_args: key.type_args.clone(),
            };
            functions.push(self.rewrite_function_with_name(
                template,
                specialized_name,
                &subst,
                Some(&active),
            )?);
        }

        Ok(Program {
            functions,
            top_level: program.top_level.clone(),
            export: program.export.clone(),
            sources: program.sources.clone(),
            entry_file_path: program.entry_file_path.clone(),
        })
    }

    fn rewrite_function(
        &mut self,
        function: &AstFunction,
        subst: &HashMap<String, Type>,
        active: Option<&ActiveSpecialization>,
    ) -> Result<AstFunction, Diagnostic> {
        self.rewrite_function_with_name(function, function.name.to_string(), subst, active)
    }

    fn rewrite_function_with_name(
        &mut self,
        function: &AstFunction,
        name: String,
        subst: &HashMap<String, Type>,
        active: Option<&ActiveSpecialization>,
    ) -> Result<AstFunction, Diagnostic> {
        Ok(AstFunction {
            name: waluau_ast::FunctionName::Simple(name),
            type_params: Vec::new(),
            params: function
                .params
                .iter()
                .map(|param| waluau_ast::Param {
                    name: param.name.clone(),
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
        stmts
            .iter()
            .map(|stmt| self.rewrite_stmt(stmt, subst, active))
            .collect()
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
            } => Stmt::Let {
                name: name.clone(),
                rebindability: *rebindability,
                ty: ty.as_ref().map(|ty| substitute_type(ty, subst)),
                value: self.rewrite_expr(value, subst, active)?,
            },
            Stmt::Assign { op, name, value } => Stmt::Assign {
                op: *op,
                name: name.clone(),
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
            } => Stmt::NumericFor {
                name: name.clone(),
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
            } => Stmt::ForIn {
                names: names.clone(),
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
                        rebindability: binding.rebindability,
                        ty: binding.ty.as_ref().map(|ty| substitute_type(ty, subst)),
                    })
                    .collect(),
                values: values
                    .iter()
                    .map(|expr| self.rewrite_expr(expr, subst, active))
                    .collect::<Result<Vec<_>, _>>()?,
            },
            Stmt::AssignMulti { targets, values } => Stmt::AssignMulti {
                targets: targets.clone(),
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
            | Expr::Name(..)
            | Expr::Require(..) => expr.clone(),
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
            } => self.rewrite_call_expr(callee, type_args, args, *span, subst, active)?,
            Expr::MethodCall { .. } => {
                return Err(Diagnostic::new(
                    "method calls must be desugared before monomorphization",
                ));
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

    fn rewrite_call_expr(
        &mut self,
        callee: &Expr,
        type_args: &[Type],
        args: &[Expr],
        span: Option<waluau_ast::Span>,
        subst: &HashMap<String, Type>,
        active: Option<&ActiveSpecialization>,
    ) -> Result<Expr, Diagnostic> {
        let args = args
            .iter()
            .map(|expr| self.rewrite_expr(expr, subst, active))
            .collect::<Result<Vec<_>, _>>()?;

        if let Expr::Name(name, callee_span) = callee {
            if self.generic_functions.contains_key(name) {
                let concrete_type_args = type_args
                    .iter()
                    .map(|ty| substitute_type(ty, subst))
                    .collect::<Vec<_>>();
                self.check_recursive_specialization(name, &concrete_type_args, active)?;
                let specialized_name =
                    self.ensure_specialization(name, concrete_type_args.clone())?;
                return Ok(Expr::Call {
                    callee: Box::new(Expr::Name(specialized_name, *callee_span)),
                    type_args: Vec::new(),
                    args,
                    span,
                });
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
            type_params: Vec::new(),
            params: function
                .params
                .iter()
                .map(|param| waluau_ast::Param {
                    name: param.name.clone(),
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
            type_params: Vec::new(),
            params: function
                .params
                .iter()
                .map(|param| waluau_ast::Param {
                    name: param.name.clone(),
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
        generic_name: &str,
        type_args: Vec<Type>,
    ) -> Result<String, Diagnostic> {
        let template = self
            .generic_functions
            .get(generic_name)
            .copied()
            .ok_or_else(|| {
                Diagnostic::new(format!(
                    "missing generic function '{}' during monomorphization",
                    generic_name
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
            generic_name: generic_name.to_string(),
            type_args,
        };
        if let Some(existing) = self.specialized_names.get(&key) {
            return Ok(existing.clone());
        }
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
        generic_name: &str,
        type_args: &[Type],
        active: Option<&ActiveSpecialization>,
    ) -> Result<(), Diagnostic> {
        let Some(active) = active else {
            return Ok(());
        };
        if active.generic_name == generic_name && active.type_args != type_args {
            return Err(generic_diagnostic(
                "generic/cross-specialization-recursion",
                format!(
                    "generic function '{generic_name}' cannot recursively instantiate different type arguments in this MVP"
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
