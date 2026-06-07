use std::collections::HashMap;
use waluau_ast::{Function as AstFunction, Expr, Program, Stmt, SymbolId, Type, MethodCallOrigin, Binding};
use waluau_diagnostics::{Diagnostic, DiagnosticCategory};

fn substitute_type(ty: &Type, subst: &HashMap<String, Type>) -> Type {
    match ty {
        Type::TypeParam(name) => subst
            .get(name)
            .cloned()
            .unwrap_or_else(|| Type::TypeParam(name.clone())),
        Type::Nullable(inner) => Type::Nullable(Box::new(substitute_type(inner, subst))),
        Type::ExternSubtype(parent) => Type::ExternSubtype(Box::new(substitute_type(parent, subst))),
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

fn contains_type_param(ty: &Type) -> bool {
    match ty {
        Type::Named { type_args, .. } => type_args.iter().any(contains_type_param),
        Type::TaggedVariant(variant) => contains_type_param(variant.payload.as_ref()),
        Type::TaggedUnion(variants) => variants
            .iter()
            .any(|variant| contains_type_param(variant.payload.as_ref())),
        Type::TypeParam(_) => true,
        Type::Opaque { ty, .. } => contains_type_param(ty.as_ref()),
        Type::ExternSubtype(parent) => contains_type_param(parent.as_ref()),
        Type::Nullable(inner) => contains_type_param(inner.as_ref()),
        Type::Array(inner) => contains_type_param(inner.as_ref()),
        Type::Record(fields) => fields.values().any(contains_type_param),
        Type::Function {
            params,
            return_type,
        } => params.iter().any(contains_type_param) || contains_type_param(return_type.as_ref()),
        _ => false,
    }
}

fn coerce_type(actual: Type, expected: Option<Type>) -> Result<Type, Diagnostic> {
    match expected {
        None => Ok(actual),
        Some(expected) if actual == expected => Ok(expected),
        Some(Type::Unknown) => Ok(Type::Unknown),
        Some(Type::Numeric(expected_num)) => match actual {
            Type::Numeric(actual_num) if actual_num.can_implicitly_widen_to(expected_num) => {
                Ok(Type::Numeric(expected_num))
            }
            _ => Err(Diagnostic::new(format!(
                "cannot convert {actual} to {}",
                Type::Numeric(expected_num)
            ))),
        },
        _ => Err(Diagnostic::new(format!(
            "cannot convert {actual} to {:?}",
            expected
        ))),
    }
}

fn unify(
    param_ty: &Type,
    actual_ty: &Type,
    subst: &mut HashMap<String, Type>,
) -> Result<(), Diagnostic> {
    match (param_ty, actual_ty) {
        (Type::TypeParam(name), _) => {
            if let Some(existing) = subst.get(name) {
                match (existing, actual_ty) {
                    (Type::Numeric(left), Type::Numeric(right)) => {
                        if let Some(common) = left.common(*right) {
                            subst.insert(name.clone(), Type::Numeric(common));
                            Ok(())
                        } else {
                            Err(Diagnostic::new(format!(
                                "conflicting types inferred for type parameter '{name}': {existing} vs {actual_ty}"
                            )))
                        }
                    }
                    (left, right) if left == right => Ok(()),
                    _ => Err(Diagnostic::new(format!(
                        "conflicting types inferred for type parameter '{name}': {existing} vs {actual_ty}"
                    ))),
                }
            } else {
                subst.insert(name.clone(), actual_ty.clone());
                Ok(())
            }
        }
        (
            Type::Named {
                name: p_name,
                type_args: p_args,
            },
            Type::Named {
                name: a_name,
                type_args: a_args,
            },
        ) => {
            if p_name != a_name || p_args.len() != a_args.len() {
                return Err(Diagnostic::new(format!(
                    "cannot match type {param_ty} with {actual_ty}"
                )));
            }
            for (p_arg, a_arg) in p_args.iter().zip(a_args.iter()) {
                unify(p_arg, a_arg, subst)?;
            }
            Ok(())
        }
        (Type::TaggedVariant(p_var), Type::TaggedVariant(a_var)) => {
            if p_var.tag != a_var.tag {
                return Err(Diagnostic::new(format!(
                    "cannot match type {param_ty} with {actual_ty}"
                )));
            }
            unify(p_var.payload.as_ref(), a_var.payload.as_ref(), subst)
        }
        (Type::TaggedUnion(p_variants), Type::TaggedVariant(a_variant)) => {
            let Some(p_var) = p_variants.iter().find(|v| v.tag == a_variant.tag) else {
                return Err(Diagnostic::new(format!(
                    "cannot match type {param_ty} with {actual_ty}"
                )));
            };
            unify(p_var.payload.as_ref(), a_variant.payload.as_ref(), subst)
        }
        (Type::TaggedVariant(p_variant), Type::TaggedUnion(a_variants)) => {
            let Some(a_var) = a_variants.iter().find(|v| v.tag == p_variant.tag) else {
                return Err(Diagnostic::new(format!(
                    "cannot match type {param_ty} with {actual_ty}"
                )));
            };
            unify(p_variant.payload.as_ref(), a_var.payload.as_ref(), subst)
        }
        (Type::TaggedUnion(p_variants), Type::TaggedUnion(a_variants)) => {
            for a_var in a_variants {
                let Some(p_var) = p_variants.iter().find(|v| v.tag == a_var.tag) else {
                    return Err(Diagnostic::new(format!(
                        "cannot match type {param_ty} with {actual_ty}"
                    )));
                };
                unify(p_var.payload.as_ref(), a_var.payload.as_ref(), subst)?;
            }
            Ok(())
        }
        (
            Type::Opaque {
                name: p_name,
                ty: p_ty,
            },
            Type::Opaque {
                name: a_name,
                ty: a_ty,
            },
        ) => {
            if p_name != a_name {
                return Err(Diagnostic::new(format!(
                    "cannot match type {param_ty} with {actual_ty}"
                )));
            }
            unify(p_ty.as_ref(), a_ty.as_ref(), subst)
        }
        (Type::Array(p_inner), Type::Array(a_inner)) => {
            unify(p_inner.as_ref(), a_inner.as_ref(), subst)
        }
        (Type::Record(p_fields), Type::Record(a_fields)) => {
            for (name, p_ty) in p_fields {
                if let Some(a_ty) = a_fields.get(name) {
                    unify(p_ty, a_ty, subst)?;
                } else {
                    return Err(Diagnostic::new(format!(
                        "missing record field '{name}' in actual type {actual_ty}"
                    )));
                }
            }
            Ok(())
        }
        (
            Type::Function {
                params: p_params,
                return_type: p_ret,
            },
            Type::Function {
                params: a_params,
                return_type: a_ret,
            },
        ) => {
            if p_params.len() != a_params.len() {
                return Err(Diagnostic::new(format!(
                    "cannot match functions of different arity: {param_ty} vs {actual_ty}"
                )));
            }
            for (p_param, a_param) in p_params.iter().zip(a_params.iter()) {
                unify(p_param, a_param, subst)?;
            }
            unify(p_ret.as_ref(), a_ret.as_ref(), subst)
        }
        (p, a) => {
            if !contains_type_param(p) {
                if coerce_type(a.clone(), Some(p.clone())).is_ok() {
                    Ok(())
                } else {
                    Err(Diagnostic::new(format!("cannot match type {p} with {a}")))
                }
            } else {
                Err(Diagnostic::new(format!("cannot match type {p} with {a}")))
            }
        }
    }
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

fn property_getter_name(base: &str, property: &str) -> String {
    format!("{base}.get_{property}")
}

fn type_property_getter_signature(
    receiver_ty: &Type,
    name: &str,
    signatures: &HashMap<String, (Vec<Type>, Type)>,
) -> Option<(Vec<Type>, Type)> {
    if let Type::Opaque {
        name: type_name, ..
    } = receiver_ty
    {
        let direct_name = property_getter_name(type_name, name);
        if let Some(signature) = signatures.get(&direct_name).cloned() {
            return Some(signature);
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
                .then(|| (params.clone(), return_type.clone()))
        })
        .collect::<Vec<_>>();
    (matches.len() == 1).then(|| matches.remove(0))
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
    function_signatures: HashMap<SymbolId, Type>,
    method_signatures: HashMap<(SymbolId, String), Type>,
    property_signatures: HashMap<String, (Vec<Type>, Type)>,
    specialized_names: HashMap<SpecializationKey, String>,
    pending: Vec<SpecializationKey>,
}

impl<'a> Monomorphizer<'a> {
    pub(crate) fn new(program: &'a Program) -> Self {
        let generic_functions: HashMap<SymbolId, &AstFunction> = program
            .functions
            .iter()
            .filter(|function| !function.type_params.is_empty())
            .map(|function| (function.symbol_id.expect("generic function has symbol id"), function))
            .collect();
        let generic_methods: HashMap<(SymbolId, String), &waluau_ast::FunctionExpr> = program
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

        let mut function_signatures = HashMap::new();
        for function in &program.functions {
            let param_types = function.params.iter().map(|p| p.ty.clone()).collect::<Vec<_>>();
            let return_type = function.return_type.clone().unwrap_or(Type::Unit);
            let fn_ty = Type::Function {
                params: param_types,
                return_type: Box::new(return_type),
            };
            if let Some(symbol_id) = function.symbol_id {
                function_signatures.insert(symbol_id, fn_ty);
            }
        }
        let mut property_signatures = HashMap::new();
        for declared in &program.declared_imports {
            let param_types = declared
                .params
                .iter()
                .map(|p| p.ty.clone())
                .collect::<Vec<_>>();
            let return_type = declared.return_type.clone();
            property_signatures.insert(
                declared.name.clone(),
                (param_types.clone(), return_type.clone()),
            );
            let fn_ty = Type::Function {
                params: param_types,
                return_type: Box::new(return_type),
            };
            if let Some(symbol_id) = declared.symbol_id {
                function_signatures.insert(symbol_id, fn_ty);
            }
        }

        let mut method_signatures = HashMap::new();
        if let Some(top_level) = program.functions.iter().find(|function| function.name.to_string() == "__waluau_top_level_init") {
            for stmt in &top_level.body {
                if let Stmt::FieldAssign {
                    base,
                    name,
                    value: Expr::Function(function),
                    ..
                } = stmt {
                    if let Expr::Name(_, Some(table_symbol_id), _) = base.as_ref() {
                        let param_types = function.params.iter().map(|p| p.ty.clone()).collect::<Vec<_>>();
                        let return_type = function.return_type.clone().unwrap_or(Type::Unit);
                        let fn_ty = Type::Function {
                            params: param_types,
                            return_type: Box::new(return_type),
                        };
                        method_signatures.insert((*table_symbol_id, name.clone()), fn_ty);
                    }
                }
            }
        }

        Self {
            generic_functions,
            generic_methods,
            function_signatures,
            method_signatures,
            property_signatures,
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
            declared_imports: program.declared_imports.clone(),
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
        let mut types = HashMap::new();
        for param in &function.params {
            if let Some(symbol_id) = param.symbol_id {
                types.insert(symbol_id, substitute_type(&param.ty, subst));
            }
        }
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
            body: self.rewrite_stmts(&function.body, subst, active, &mut types)?,
            file_path: function.file_path.clone(),
        })
    }

    fn rewrite_stmts(
        &mut self,
        stmts: &[Stmt],
        subst: &HashMap<String, Type>,
        active: Option<&ActiveSpecialization>,
        types: &mut HashMap<SymbolId, Type>,
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
            rewritten.push(self.rewrite_stmt(stmt, subst, active, types)?);
        }
        Ok(rewritten)
    }

    fn rewrite_stmt(
        &mut self,
        stmt: &Stmt,
        subst: &HashMap<String, Type>,
        active: Option<&ActiveSpecialization>,
        types: &mut HashMap<SymbolId, Type>,
    ) -> Result<Stmt, Diagnostic> {
        Ok(match stmt {
            Stmt::Let {
                name,
                symbol_id,
                rebindability,
                ty,
                value,
                ..
            } => {
                let val = self.rewrite_expr(value, subst, active, types)?;
                let inferred_ty = if let Some(ty) = ty {
                    substitute_type(ty, subst)
                } else if matches!(value, Expr::ArrayLiteral { elements, .. } if elements.is_empty()) {
                    Type::Record(std::collections::BTreeMap::new())
                } else {
                    self.infer_expr_type(value, subst, types)?
                };
                if let Some(symbol_id) = symbol_id {
                    types.insert(*symbol_id, inferred_ty.clone());
                }
                Stmt::Let {
                    name: name.clone(),
                    symbol_id: *symbol_id,
                    rebindability: *rebindability,
                    ty: Some(inferred_ty),
                    value: val,
                }
            }
            Stmt::Assign { op, name, symbol_id, value } => Stmt::Assign {
                op: *op,
                name: name.clone(),
                symbol_id: *symbol_id,
                value: self.rewrite_expr(value, subst, active, types)?,
            },
            Stmt::IndexAssign {
                op,
                base,
                index,
                value,
            } => Stmt::IndexAssign {
                op: *op,
                base: Box::new(self.rewrite_expr(base, subst, active, types)?),
                index: Box::new(self.rewrite_expr(index, subst, active, types)?),
                value: self.rewrite_expr(value, subst, active, types)?,
            },
            Stmt::FieldAssign {
                op,
                base,
                name,
                value,
            } => {
                let rewritten_base = self.rewrite_expr(base, subst, active, types)?;
                let rewritten_value = self.rewrite_expr(value, subst, active, types)?;
                if let Expr::Name(_, Some(base_symbol_id), _) = base.as_ref() {
                    if let Some(Type::Record(mut fields)) = types.get(base_symbol_id).cloned() {
                        if !fields.contains_key(name) {
                            let inferred_val_ty = self.infer_expr_type(&rewritten_value, subst, types)?;
                            fields.insert(name.clone(), inferred_val_ty);
                            types.insert(*base_symbol_id, Type::Record(fields));
                        }
                    }
                }
                Stmt::FieldAssign {
                    op: *op,
                    base: Box::new(rewritten_base),
                    name: name.clone(),
                    value: rewritten_value,
                }
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => Stmt::If {
                condition: self.rewrite_expr(condition, subst, active, types)?,
                then_body: self.rewrite_stmts(then_body, subst, active, &mut types.clone())?,
                else_body: self.rewrite_stmts(else_body, subst, active, &mut types.clone())?,
            },
            Stmt::IfCast {
                target_name,
                target_ty,
                binding,
                binding_symbol_id,
                value,
                then_body,
                else_body,
            } => {
                let rewritten_value = self.rewrite_expr(value, subst, active, types)?;
                let mut then_types = types.clone();
                if let Some(symbol_id) = binding_symbol_id {
                    then_types.insert(*symbol_id, substitute_type(target_ty, subst));
                }
                Stmt::IfCast {
                    target_name: target_name.clone(),
                    target_ty: substitute_type(target_ty, subst),
                    binding: binding.clone(),
                    binding_symbol_id: *binding_symbol_id,
                    value: rewritten_value,
                    then_body: self.rewrite_stmts(then_body, subst, active, &mut then_types)?,
                    else_body: self.rewrite_stmts(else_body, subst, active, &mut types.clone())?,
                }
            }
            Stmt::While { condition, body } => Stmt::While {
                condition: self.rewrite_expr(condition, subst, active, types)?,
                body: self.rewrite_stmts(body, subst, active, &mut types.clone())?,
            },
            Stmt::Repeat { body, condition } => Stmt::Repeat {
                body: self.rewrite_stmts(body, subst, active, &mut types.clone())?,
                condition: self.rewrite_expr(condition, subst, active, types)?,
            },
            Stmt::NumericFor {
                name,
                symbol_id,
                start,
                stop,
                step,
                body,
                ..
            } => {
                let rewritten_start = self.rewrite_expr(start, subst, active, types)?;
                let rewritten_stop = self.rewrite_expr(stop, subst, active, types)?;
                let rewritten_step = step
                    .as_ref()
                    .map(|expr| self.rewrite_expr(expr, subst, active, types))
                    .transpose()?;
                let mut body_types = types.clone();
                if let Some(symbol_id) = symbol_id {
                    body_types.insert(*symbol_id, Type::Numeric(waluau_ast::NumericType::F64));
                }
                Stmt::NumericFor {
                    name: name.clone(),
                    symbol_id: *symbol_id,
                    start: rewritten_start,
                    stop: rewritten_stop,
                    step: rewritten_step,
                    body: self.rewrite_stmts(body, subst, active, &mut body_types)?,
                }
            }
            Stmt::ForIn {
                names,
                symbol_ids,
                iterator,
                body,
                ..
            } => {
                let rewritten_iterator = self.rewrite_expr(iterator, subst, active, types)?;
                let iterator_ty = self.infer_expr_type(iterator, subst, types)?;
                let mut body_types = types.clone();
                if let Type::Array(element_ty) = &iterator_ty {
                    if let Some(symbol_ids) = symbol_ids {
                        if symbol_ids.len() == 1 {
                            body_types.insert(symbol_ids[0], *element_ty.clone());
                        } else if symbol_ids.len() == 2 {
                            body_types.insert(symbol_ids[0], Type::Numeric(waluau_ast::NumericType::I32));
                            body_types.insert(symbol_ids[1], *element_ty.clone());
                        }
                    }
                }
                Stmt::ForIn {
                    names: names.clone(),
                    symbol_ids: symbol_ids.clone(),
                    iterator: rewritten_iterator,
                    body: self.rewrite_stmts(body, subst, active, &mut body_types)?,
                }
            }
            Stmt::Break => Stmt::Break,
            Stmt::Continue => Stmt::Continue,
            Stmt::Return(expr) => Stmt::Return(self.rewrite_expr(expr, subst, active, types)?),
            Stmt::ReturnMulti(values) => Stmt::ReturnMulti(
                values
                    .iter()
                    .map(|expr| self.rewrite_expr(expr, subst, active, types))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            Stmt::LetMulti { bindings, values } => {
                let rewritten_values = values
                    .iter()
                    .map(|expr| self.rewrite_expr(expr, subst, active, types))
                    .collect::<Result<Vec<_>, _>>()?;
                let mut value_types = Vec::new();
                for expr in &rewritten_values {
                    if matches!(expr, Expr::ArrayLiteral { elements, .. } if elements.is_empty()) {
                        value_types.push(Type::Record(std::collections::BTreeMap::new()));
                    } else {
                        match self.infer_expr_type(expr, subst, types)? {
                            Type::Multi(tys) => value_types.extend(tys),
                            other => value_types.push(other),
                        }
                    }
                }
                let mut rewritten_bindings = Vec::with_capacity(bindings.len());
                for (i, binding) in bindings.iter().enumerate() {
                    let inferred_ty = if let Some(ty) = &binding.ty {
                        substitute_type(ty, subst)
                    } else {
                        value_types.get(i).cloned().unwrap_or(Type::Unit)
                    };
                    if let Some(symbol_id) = binding.symbol_id {
                        types.insert(symbol_id, inferred_ty.clone());
                    }
                    rewritten_bindings.push(Binding {
                        name: binding.name.clone(),
                        symbol_id: binding.symbol_id,
                        rebindability: binding.rebindability,
                        ty: Some(inferred_ty),
                    });
                }
                Stmt::LetMulti {
                    bindings: rewritten_bindings,
                    values: rewritten_values,
                }
            }
            Stmt::AssignMulti { targets, symbol_ids, values } => Stmt::AssignMulti {
                targets: targets.clone(),
                symbol_ids: symbol_ids.clone(),
                values: values
                    .iter()
                    .map(|expr| self.rewrite_expr(expr, subst, active, types))
                    .collect::<Result<Vec<_>, _>>()?,
            },
            Stmt::Expr(expr) => Stmt::Expr(self.rewrite_expr(expr, subst, active, types)?),
        })
    }

    fn rewrite_expr(
        &mut self,
        expr: &Expr,
        subst: &HashMap<String, Type>,
        active: Option<&ActiveSpecialization>,
        types: &HashMap<SymbolId, Type>,
    ) -> Result<Expr, Diagnostic> {
        Ok(match expr {
            Expr::Number(..)
            | Expr::Bool(..)
            | Expr::Nil(..)
            | Expr::String(..)
            | Expr::Bytes(..)
            | Expr::Require(..) => expr.clone(),
            Expr::Name(name, symbol_id, span) => Expr::Name(name.clone(), *symbol_id, *span),
            Expr::Unary { op, expr, span } => Expr::Unary {
                op: *op,
                expr: Box::new(self.rewrite_expr(expr, subst, active, types)?),
                span: *span,
            },
            Expr::Cast { expr, ty, span } => Expr::Cast {
                expr: Box::new(self.rewrite_expr(expr, subst, active, types)?),
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
                left: Box::new(self.rewrite_expr(left, subst, active, types)?),
                right: Box::new(self.rewrite_expr(right, subst, active, types)?),
                span: *span,
            },
            Expr::IsVariant { expr, tag, span } => Expr::IsVariant {
                expr: Box::new(self.rewrite_expr(expr, subst, active, types)?),
                tag: tag.clone(),
                span: *span,
            },
            Expr::If {
                condition,
                then_expr,
                else_expr,
                span,
            } => Expr::If {
                condition: Box::new(self.rewrite_expr(condition, subst, active, types)?),
                then_expr: Box::new(self.rewrite_expr(then_expr, subst, active, types)?),
                else_expr: Box::new(self.rewrite_expr(else_expr, subst, active, types)?),
                span: *span,
            },
            Expr::Call {
                callee,
                type_args,
                args,
                span,
                method_call_origin,
            } => self.rewrite_call_expr(callee, type_args, args, *span, method_call_origin, subst, active, types)?,
            method_call @ Expr::MethodCall { .. } => {
                self.rewrite_method_call(method_call, subst, active, types)?
            }
            Expr::Function(function) => {
                Expr::Function(self.rewrite_function_expr(function, subst, active, types)?)
            }
            Expr::ArrayLiteral { elements, span } => Expr::ArrayLiteral {
                elements: elements
                    .iter()
                    .map(|expr| self.rewrite_expr(expr, subst, active, types))
                    .collect::<Result<Vec<_>, _>>()?,
                span: *span,
            },
            Expr::TableLiteral { fields, span } => Expr::TableLiteral {
                fields: fields
                    .iter()
                    .map(|field| {
                        Ok(waluau_ast::TableField {
                            name: field.name.clone(),
                            value: self.rewrite_expr(&field.value, subst, active, types)?,
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?,
                span: *span,
            },
            Expr::Field { base, name, span } => Expr::Field {
                base: Box::new(self.rewrite_expr(base, subst, active, types)?),
                name: name.clone(),
                span: *span,
            },
            Expr::Index { base, index, span } => Expr::Index {
                base: Box::new(self.rewrite_expr(base, subst, active, types)?),
                index: Box::new(self.rewrite_expr(index, subst, active, types)?),
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
        types: &HashMap<SymbolId, Type>,
    ) -> Result<Expr, Diagnostic> {
        let args = args
            .iter()
            .map(|expr| self.rewrite_expr(expr, subst, active, types))
            .collect::<Result<Vec<_>, _>>()?;

        if let Expr::Name(_name, Some(symbol_id), callee_span) = callee {
            if self.generic_functions.contains_key(symbol_id) {
                let inferred_type_args;
                let concrete_type_args = if type_args.is_empty() {
                    let template = self.generic_functions.get(symbol_id).unwrap();
                    let params = template.params.iter().map(|p| p.ty.clone()).collect::<Vec<_>>();
                    let ret = template.return_type.clone().unwrap_or(Type::Unit);
                    inferred_type_args = self.infer_type_arguments(
                        &template.type_params,
                        &params,
                        &ret,
                        &args,
                        subst,
                        types,
                    )?;
                    &inferred_type_args
                } else {
                    type_args
                };
                let subst_concrete = concrete_type_args
                    .iter()
                    .map(|ty| substitute_type(ty, subst))
                    .collect::<Vec<_>>();
                self.check_recursive_specialization(*symbol_id, &subst_concrete, active)?;
                let specialized_name =
                    self.ensure_specialization(*symbol_id, subst_concrete)?;
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
                    let inferred_type_args;
                    let concrete_type_args = if type_args.is_empty() && !function.type_params.is_empty() {
                        let params = function.params.iter().map(|p| p.ty.clone()).collect::<Vec<_>>();
                        let ret = function.return_type.clone().unwrap_or(Type::Unit);
                        inferred_type_args = self.infer_type_arguments(
                            &function.type_params,
                            &params,
                            &ret,
                            &args,
                            subst,
                            types,
                        )?;
                        &inferred_type_args
                    } else {
                        type_args
                    };
                    let specialized =
                        self.specialize_function_expr(function, concrete_type_args, subst, active, types)?;
                    
                    // For dot-call form, the first argument is the receiver
                    let receiver_expr = if !args.is_empty() {
                        Some(MethodCallOrigin {
                            original_receiver: Box::new(self.rewrite_expr(&args[0], subst, active, types)?),
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
                let inferred_type_args;
                let concrete_type_args = if type_args.is_empty() {
                    let params = function.params.iter().map(|p| p.ty.clone()).collect::<Vec<_>>();
                    let ret = function.return_type.clone().unwrap_or(Type::Unit);
                    inferred_type_args = self.infer_type_arguments(
                        &function.type_params,
                        &params,
                        &ret,
                        &args,
                        subst,
                        types,
                    )?;
                    &inferred_type_args
                } else {
                    type_args
                };
                let specialized =
                    self.specialize_function_expr(function, concrete_type_args, subst, active, types)?;
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
            callee: Box::new(self.rewrite_expr(callee, subst, active, types)?),
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
        types: &HashMap<SymbolId, Type>,
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
        let rewritten_receiver = self.rewrite_expr(receiver, subst, active, types)?;
        let rewritten_args = args
            .iter()
            .map(|expr| self.rewrite_expr(expr, subst, active, types))
            .collect::<Result<Vec<_>, _>>()?;

        // Specialize generic method calls (`receiver:method<T>(...)`) the same way
        // the dot-call form (`Table.method<T>(receiver, ...)`) is handled in
        // `rewrite_call_expr`: inline the specialized method as a plain call with
        // the receiver threaded in as the explicit `self` argument.
        if let Expr::Name(_, Some(table_symbol_id), _) = receiver.as_ref() {
            let key = (*table_symbol_id, name.clone());
            if let Some(function) = self.generic_methods.get(&key).copied() {
                let inferred_type_args;
                let concrete_type_args = if type_args.is_empty() && !function.type_params.is_empty() {
                    let params = function.params.iter().map(|p| p.ty.clone()).collect::<Vec<_>>();
                    let ret = function.return_type.clone().unwrap_or(Type::Unit);
                    let mut call_args = vec![rewritten_receiver.clone()];
                    call_args.extend(rewritten_args.clone());
                    inferred_type_args = self.infer_type_arguments(
                        &function.type_params,
                        &params,
                        &ret,
                        &call_args,
                        subst,
                        types,
                    )?;
                    &inferred_type_args
                } else {
                    type_args
                };
                if !concrete_type_args.is_empty() {
                    let specialized =
                        self.specialize_function_expr(function, concrete_type_args, subst, active, types)?;
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
        } else {
            let receiver_ty = self.infer_expr_type(receiver, subst, types)?;
            if let Some(function) = self.find_generic_method(&receiver_ty, name) {
                let inferred_type_args;
                let concrete_type_args = if type_args.is_empty() && !function.type_params.is_empty() {
                    let params = function.params.iter().map(|p| p.ty.clone()).collect::<Vec<_>>();
                    let ret = function.return_type.clone().unwrap_or(Type::Unit);
                    let mut call_args = vec![rewritten_receiver.clone()];
                    call_args.extend(rewritten_args.clone());
                    inferred_type_args = self.infer_type_arguments(
                        &function.type_params,
                        &params,
                        &ret,
                        &call_args,
                        subst,
                        types,
                    )?;
                    &inferred_type_args
                } else {
                    type_args
                };
                if !concrete_type_args.is_empty() {
                    let specialized =
                        self.specialize_function_expr(function, concrete_type_args, subst, active, types)?;
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
        types: &HashMap<SymbolId, Type>,
    ) -> Result<waluau_ast::FunctionExpr, Diagnostic> {
        if !function.type_params.is_empty() {
            return Err(generic_diagnostic(
                "generic/uninstantiated-value",
                "generic function expression must be instantiated before IR lowering",
            ));
        }
        let mut function_types = types.clone();
        for param in &function.params {
            if let Some(symbol_id) = param.symbol_id {
                function_types.insert(symbol_id, substitute_type(&param.ty, subst));
            }
        }
        Ok(waluau_ast::FunctionExpr {
            name: function.name.clone(),
            symbol_id: function.symbol_id,
            implicit_self: function.implicit_self.clone(),
            type_params: Vec::new(),
            params: function
                .params
                .iter()
                .map(|param| waluau_ast::Param {
                    name: param.name.clone(),
                    symbol_id: param.symbol_id,
                    ty: substitute_type(&param.ty, subst),
                })
                .collect(),
            return_type: function
                .return_type
                .as_ref()
                .map(|ty| substitute_type(ty, subst)),
            body: self.rewrite_stmts(&function.body, subst, active, &mut function_types)?,
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
        types: &HashMap<SymbolId, Type>,
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
        let mut function_types = types.clone();
        for param in &function.params {
            if let Some(symbol_id) = param.symbol_id {
                function_types.insert(symbol_id, substitute_type(&param.ty, &local_subst));
            }
        }
        Ok(waluau_ast::FunctionExpr {
            name: function.name.clone(),
            symbol_id: function.symbol_id,
            implicit_self: function.implicit_self.clone(),
            type_params: Vec::new(),
            params: function
                .params
                .iter()
                .map(|param| waluau_ast::Param {
                    name: param.name.clone(),
                    symbol_id: param.symbol_id,
                    ty: substitute_type(&param.ty, &local_subst),
                })
                .collect(),
            return_type: function
                .return_type
                .as_ref()
                .map(|ty| substitute_type(ty, &local_subst)),
            body: self.rewrite_stmts(&function.body, &local_subst, active, &mut function_types)?,
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

    fn infer_expr_type(
        &self,
        expr: &Expr,
        subst: &HashMap<String, Type>,
        types: &HashMap<SymbolId, Type>,
    ) -> Result<Type, Diagnostic> {
        match expr {
            Expr::Number(..) => Ok(Type::number()),
            Expr::Bool(..) => Ok(Type::Bool),
            Expr::Nil(..) => Ok(Type::Nil),
            Expr::String(..) => Ok(Type::String),
            Expr::Bytes(..) => Ok(Type::Bytes),
            Expr::Require(..) => Ok(Type::Unknown),
            Expr::Name(_, symbol_id, _) => {
                let symbol_id = symbol_id.ok_or_else(|| {
                    Diagnostic::new("symbol_id should be resolved during monomorphization")
                })?;
                if let Some(ty) = types.get(&symbol_id) {
                    Ok(ty.clone())
                } else if let Some(ty) = self.function_signatures.get(&symbol_id) {
                    Ok(ty.clone())
                } else {
                    Err(Diagnostic::new(format!(
                        "unknown symbol ID {:?} during type inference in monomorphization",
                        symbol_id
                    )))
                }
            }
            Expr::Unary { op, expr, .. } => match op {
                waluau_ast::UnaryOp::Neg => self.infer_expr_type(expr, subst, types),
                waluau_ast::UnaryOp::Not => Ok(Type::Bool),
                waluau_ast::UnaryOp::Len => Ok(Type::Numeric(waluau_ast::NumericType::I32)),
            },
            Expr::Cast { ty, .. } => Ok(substitute_type(ty, subst)),
            Expr::Binary { op, left, right, .. } => match op {
                waluau_ast::BinaryOp::Eq
                | waluau_ast::BinaryOp::Less
                | waluau_ast::BinaryOp::Greater
                | waluau_ast::BinaryOp::And
                | waluau_ast::BinaryOp::Or => Ok(Type::Bool),
                waluau_ast::BinaryOp::Concat => Ok(Type::String),
                _ => {
                    let left_ty = self.infer_expr_type(left, subst, types)?;
                    let right_ty = self.infer_expr_type(right, subst, types)?;
                    match (left_ty, right_ty) {
                        (Type::Numeric(l), Type::Numeric(r)) => {
                            if let Some(common) = l.common(r) {
                                Ok(Type::Numeric(common))
                            } else {
                                Ok(Type::Numeric(l))
                            }
                        }
                        (Type::Numeric(l), _) => Ok(Type::Numeric(l)),
                        (_, Type::Numeric(r)) => Ok(Type::Numeric(r)),
                        _ => Ok(Type::number()),
                    }
                }
            },
            Expr::IsVariant { .. } => Ok(Type::Bool),
            Expr::If { then_expr, .. } => self.infer_expr_type(then_expr, subst, types),
            Expr::Call {
                callee,
                type_args,
                args,
                ..
            } => {
                if let Some(name) = builtin_name(callee) {
                    if let Some(ty) = self.infer_builtin_call_type(&name, args, subst, types)? {
                        return Ok(ty);
                    }
                }
                if let Expr::Name(_, Some(symbol_id), _) = callee.as_ref() {
                    if let Some(function) = self.generic_functions.get(symbol_id) {
                        let params = function
                            .params
                            .iter()
                            .map(|p| p.ty.clone())
                            .collect::<Vec<_>>();
                        let ret = function.return_type.clone().unwrap_or(Type::Unit);
                        return self.infer_generic_call_return_type(
                            &function.type_params,
                            &params,
                            &ret,
                            type_args,
                            args,
                            subst,
                            types,
                        );
                    }
                }
                if let Expr::Field { base, name, .. } = callee.as_ref() {
                    if let Expr::Name(_, Some(table_symbol_id), _) = base.as_ref() {
                        let key = (*table_symbol_id, name.clone());
                        if let Some(function) = self.generic_methods.get(&key) {
                            let params = function
                                .params
                                .iter()
                                .map(|p| p.ty.clone())
                                .collect::<Vec<_>>();
                            let ret = function.return_type.clone().unwrap_or(Type::Unit);
                            return self.infer_generic_call_return_type(
                                &function.type_params,
                                &params,
                                &ret,
                                type_args,
                                args,
                                subst,
                                types,
                            );
                        }
                    }
                }
                if let Expr::Function(function) = callee.as_ref() {
                    if !function.type_params.is_empty() {
                        let params = function
                            .params
                            .iter()
                            .map(|p| p.ty.clone())
                            .collect::<Vec<_>>();
                        let ret = function.return_type.clone().unwrap_or(Type::Unit);
                        return self.infer_generic_call_return_type(
                            &function.type_params,
                            &params,
                            &ret,
                            type_args,
                            args,
                            subst,
                            types,
                        );
                    }
                }

                let callee_ty = self.infer_expr_type(callee, subst, types)?;
                match callee_ty {
                    Type::Function { return_type, .. } => Ok(*return_type),
                    _ => Ok(Type::Unknown),
                }
            }
            Expr::MethodCall {
                receiver,
                name,
                type_args,
                args,
                ..
            } => {
                let receiver_ty = self.infer_expr_type(receiver, subst, types)?;
                if let Expr::Name(_, Some(table_symbol_id), _) = receiver.as_ref() {
                    let key = (*table_symbol_id, name.clone());
                    if let Some(function) = self.generic_methods.get(&key) {
                        let params = function
                            .params
                            .iter()
                            .map(|p| p.ty.clone())
                            .collect::<Vec<_>>();
                        let ret = function.return_type.clone().unwrap_or(Type::Unit);
                        let mut call_args = vec![(**receiver).clone()];
                        call_args.extend(args.clone());
                        return self.infer_generic_call_return_type(
                            &function.type_params,
                            &params,
                            &ret,
                            type_args,
                            &call_args,
                            subst,
                            types,
                        );
                    }
                } else if let Some(function) = self.find_generic_method(&receiver_ty, name) {
                    let params = function
                        .params
                        .iter()
                        .map(|p| p.ty.clone())
                        .collect::<Vec<_>>();
                    let ret = function.return_type.clone().unwrap_or(Type::Unit);
                    let mut call_args = vec![(**receiver).clone()];
                    call_args.extend(args.clone());
                    return self.infer_generic_call_return_type(
                        &function.type_params,
                        &params,
                        &ret,
                        type_args,
                        &call_args,
                        subst,
                        types,
                    );
                }

                let field_ty = if let Some(ty) = self.lookup_method_signature(&receiver_ty, name) {
                    ty
                } else {
                    receiver_ty.record_field(name).unwrap_or(Type::Unknown)
                };
                match field_ty {
                    Type::Function { return_type, .. } => Ok(*return_type),
                    _ => Ok(Type::Unknown),
                }
            }
            Expr::Function(function) => {
                let params = function
                    .params
                    .iter()
                    .map(|p| substitute_type(&p.ty, subst))
                    .collect();
                let return_type = function
                    .return_type
                    .clone()
                    .map(|ty| substitute_type(&ty, subst))
                    .unwrap_or(Type::Unit);
                Ok(Type::Function {
                    params,
                    return_type: Box::new(return_type),
                })
            }
            Expr::ArrayLiteral { elements, .. } => {
                let element_ty = if let Some(first) = elements.first() {
                    self.infer_expr_type(first, subst, types)?
                } else {
                    Type::Unknown
                };
                Ok(Type::Array(Box::new(element_ty)))
            }
            Expr::TableLiteral { fields, .. } => {
                let mut record_fields = std::collections::BTreeMap::new();
                for field in fields {
                    let field_ty = self.infer_expr_type(&field.value, subst, types)?;
                    record_fields.insert(field.name.clone(), field_ty);
                }
                Ok(Type::Record(record_fields))
            }
            Expr::Field { base, name, .. } => {
                let base_ty = self.infer_expr_type(base, subst, types)?;
                if let Some((params, return_type)) =
                    type_property_getter_signature(&base_ty, name, &self.property_signatures)
                {
                    if params.len() == 1 && method_receiver_matches(&params[0], &base_ty) {
                        return Ok(substitute_type(&return_type, subst));
                    }
                }
                base_ty.record_field(name).ok_or_else(|| {
                    Diagnostic::new(format!(
                        "unknown record field '{name}' on type '{base_ty}'"
                    ))
                })
            }
            Expr::Index { base, .. } => {
                let base_ty = self.infer_expr_type(base, subst, types)?;
                base_ty.element_type().ok_or_else(|| {
                    Diagnostic::new(format!("indexing non-array type '{base_ty}'"))
                })
            }
        }
    }

    fn lookup_method_signature(&self, receiver_ty: &Type, name: &str) -> Option<Type> {
        let suffix = name;
        for ((_table_id, m_name), function_ty) in &self.method_signatures {
            if m_name == suffix {
                if let Type::Function { params, .. } = function_ty {
                    if let Some(first_param) = params.first() {
                        if method_receiver_matches(first_param, receiver_ty) {
                            return Some(function_ty.clone());
                        }
                    }
                }
            }
        }
        None
    }

    fn find_generic_method(
        &self,
        receiver_ty: &Type,
        method_name: &str,
    ) -> Option<&'a waluau_ast::FunctionExpr> {
        let suffix = method_name;
        for ((_table_id, m_name), function) in &self.generic_methods {
            if m_name == suffix {
                if let Some(first_param) = function.params.first() {
                    if method_receiver_matches(&first_param.ty, receiver_ty) {
                        return Some(*function);
                    }
                }
            }
        }
        None
    }

    #[allow(clippy::too_many_arguments)]
    fn infer_generic_call_return_type(
        &self,
        type_params: &[String],
        params: &[Type],
        return_type: &Type,
        type_args: &[Type],
        args: &[Expr],
        subst: &HashMap<String, Type>,
        types: &HashMap<SymbolId, Type>,
    ) -> Result<Type, Diagnostic> {
        let inferred_type_args;
        let concrete_type_args = if type_args.is_empty() && !type_params.is_empty() {
            inferred_type_args = self.infer_type_arguments(
                type_params,
                params,
                return_type,
                args,
                subst,
                types,
            )?;
            &inferred_type_args
        } else {
            type_args
        };
        let mut local_subst = subst.clone();
        for (param, ty) in type_params.iter().zip(concrete_type_args.iter()) {
            local_subst.insert(param.clone(), substitute_type(ty, subst));
        }
        Ok(substitute_type(return_type, &local_subst))
    }

    fn infer_type_arguments(
        &self,
        type_params: &[String],
        params: &[Type],
        _return_type: &Type,
        args: &[Expr],
        subst: &HashMap<String, Type>,
        types: &HashMap<SymbolId, Type>,
    ) -> Result<Vec<Type>, Diagnostic> {
        let mut unified_subst = HashMap::new();
        for (param_ty, arg) in params.iter().zip(args.iter()) {
            let substituted_param_ty = substitute_type(param_ty, &unified_subst);
            let actual_arg_ty = self.infer_expr_type(arg, subst, types)?;
            let _ = unify(&substituted_param_ty, &actual_arg_ty, &mut unified_subst);
        }
        let mut inferred_type_args = Vec::new();
        for param in type_params {
            if let Some(ty) = unified_subst.get(param) {
                inferred_type_args.push(ty.clone());
            } else {
                return Err(generic_diagnostic(
                    "generic/missing-type-args",
                    format!("cannot infer type argument for type parameter '{param}'"),
                ));
            }
        }
        Ok(inferred_type_args)
    }

    fn infer_builtin_call_type(
        &self,
        name: &str,
        args: &[Expr],
        subst: &HashMap<String, Type>,
        types: &HashMap<SymbolId, Type>,
    ) -> Result<Option<Type>, Diagnostic> {
        match name {
            "print" | "assert" => Ok(Some(Type::Unit)),
            "tostring" => Ok(Some(Type::String)),
            "coroutine.create" => Ok(Some(Type::Thread)),
            "coroutine.resume" => Ok(Some(Type::Multi(vec![
                Type::Bool,
                Type::Numeric(waluau_ast::NumericType::I32),
            ]))),
            "coroutine.close" => Ok(Some(Type::Bool)),
            "math.abs" | "math.min" | "math.max" | "math.sqrt" | "math.floor" | "math.ceil"
            | "math.trunc" | "math.nearest" | "math.copysign" => {
                if let Some(first) = args.first() {
                    let first_ty = self.infer_expr_type(first, subst, types)?;
                    if let Type::Numeric(num) = first_ty {
                        Ok(Some(Type::Numeric(num)))
                    } else {
                        Ok(Some(Type::number()))
                    }
                } else {
                    Ok(Some(Type::number()))
                }
            }
            _ => Ok(None),
        }
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
