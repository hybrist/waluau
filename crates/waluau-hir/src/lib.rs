use std::collections::{BTreeMap, HashMap, HashSet};

use waluau_ast::{
    AssignOp, Expr, Function, FunctionExpr, FunctionName, NumberLiteral, NumericType, Param,
    Program, Rebindability, Stmt, Type,
};
use waluau_diagnostics::{Diagnostic, DiagnosticCategory};

struct CompilerTimer {
    #[cfg(not(target_family = "wasm"))]
    started: std::time::Instant,
}

impl CompilerTimer {
    fn start() -> Self {
        Self {
            #[cfg(not(target_family = "wasm"))]
            started: std::time::Instant::now(),
        }
    }

    fn elapsed(&self) -> std::time::Duration {
        #[cfg(not(target_family = "wasm"))]
        return self.started.elapsed();
        #[cfg(target_family = "wasm")]
        return std::time::Duration::ZERO;
    }

    fn enabled() -> bool {
        #[cfg(not(target_family = "wasm"))]
        return std::env::var_os("WALUAU_TIMINGS").is_some();
        #[cfg(target_family = "wasm")]
        return false;
    }
}

mod builtins;
mod expressions;
mod numeric;
mod signatures;
mod statements;

use expressions::{
    infer_expr, resolve_operator_overload, resolved_type_method_call_name,
    resolved_type_property_getter_name, select_overload,
};
use signatures::{
    FnSignature, GenericScheme, OverloadVariant, active_type_param_set,
    infer_function_expr_return_type, infer_top_level_function_return_type, inference_diagnostic,
};
use statements::check_stmt;
use statements::{checked_if_cast_scopes, narrowed_scopes, resolved_type_property_setter_name};

#[derive(Clone)]
struct Binding {
    ty: Type,
    rebindability: Rebindability,
    record_open: bool,
    pcall_link: Option<PcallLink>,
}

/// Ties the two bindings of `local ok, v = pcall(...)` together so branching
/// on `ok` (or `assert(ok)`) can narrow `v` to the protected function's return
/// type on the success path and to the error payload type on the failure path.
/// The link is bidirectional — narrowing only fires while both bindings still
/// point at each other, so shadowing either name severs it.
#[derive(Clone, PartialEq)]
enum PcallLink {
    Discriminant {
        payload: String,
        when_true: Type,
        when_false: Type,
    },
    Payload {
        discriminant: String,
    },
}

fn binding_for(ty: Type, rebindability: Rebindability) -> Binding {
    let record_open = matches!(ty, Type::Record(_));
    Binding {
        ty,
        rebindability,
        record_open,
        pcall_link: None,
    }
}

fn collect_module_bindings(
    top_level: &[Stmt],
    fn_signatures: &HashMap<String, FnSignature>,
) -> Result<HashMap<String, Binding>, Diagnostic> {
    let mut bindings = HashMap::new();
    for stmt in top_level {
        match stmt {
            Stmt::Let {
                name,
                rebindability,
                ty,
                value,
                ..
            } => {
                let ty = match ty {
                    Some(ty) => ty.clone(),
                    None => infer_expr(value, &bindings, fn_signatures, &HashSet::new(), None)
                        .unwrap_or(Type::Unknown),
                };
                bindings.insert(name.clone(), binding_for(ty, *rebindability));
            }
            Stmt::LetMulti { bindings: lets, .. } => {
                for binding in lets {
                    if let Some(ty) = &binding.ty {
                        bindings.insert(
                            binding.name.clone(),
                            binding_for(ty.clone(), binding.rebindability),
                        );
                    }
                }
            }
            _ => {}
        }
    }
    Ok(bindings)
}

fn function_module_bindings<'a>(
    function: &Function,
    bindings: &'a HashMap<String, Binding>,
) -> &'a HashMap<String, Binding> {
    if function.name.to_string() == "__waluau_top_level_init" {
        static EMPTY: std::sync::LazyLock<HashMap<String, Binding>> =
            std::sync::LazyLock::new(HashMap::new);
        &EMPTY
    } else {
        bindings
    }
}

fn is_builtin_callee(expr: &Expr) -> bool {
    match expr {
        Expr::Name(name, _, _) => matches!(
            name.as_str(),
            "assert"
                | "error"
                | "pcall"
                | "print"
                | "select"
                | "tonumber"
                | "tostring"
                | "type"
                | "typeof"
        ),
        // `math.*` is intentionally absent: math builtins are declared host
        // imports (builtins/math.walu) resolved through overload selection.
        Expr::Field { base, .. } => matches!(
            base.as_ref(),
            Expr::Name(namespace, _, _)
                if matches!(
                    namespace.as_str(),
                    "bit32" | "coroutine" | "promise" | "string" | "table"
                )
        ),
        _ => false,
    }
}

fn annotate_inferred_expr_locals(
    expr: &mut Expr,
    vars: &HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
) -> Result<(), Diagnostic> {
    match expr {
        Expr::Unary { expr, .. } | Expr::Cast { expr, .. } | Expr::IsVariant { expr, .. } => {
            annotate_inferred_expr_locals(expr, vars, fn_signatures, active_type_params)
        }
        Expr::Binary { left, right, .. } => {
            annotate_inferred_expr_locals(left, vars, fn_signatures, active_type_params)?;
            annotate_inferred_expr_locals(right, vars, fn_signatures, active_type_params)
        }
        Expr::If {
            condition,
            then_expr,
            else_expr,
            ..
        } => {
            annotate_inferred_expr_locals(condition, vars, fn_signatures, active_type_params)?;
            annotate_inferred_expr_locals(then_expr, vars, fn_signatures, active_type_params)?;
            annotate_inferred_expr_locals(else_expr, vars, fn_signatures, active_type_params)
        }
        Expr::Call { callee, args, .. } => {
            if !is_builtin_callee(callee) {
                annotate_inferred_expr_locals(callee, vars, fn_signatures, active_type_params)?;
            }
            for arg in args {
                annotate_inferred_expr_locals(arg, vars, fn_signatures, active_type_params)?;
            }
            Ok(())
        }
        Expr::MethodCall { receiver, args, .. } => {
            annotate_inferred_expr_locals(receiver, vars, fn_signatures, active_type_params)?;
            for arg in args {
                annotate_inferred_expr_locals(arg, vars, fn_signatures, active_type_params)?;
            }
            Ok(())
        }
        Expr::Function(function) => {
            let mut function_type_params = active_type_params.clone();
            function_type_params.extend(active_type_param_set(&function.type_params));
            // Backfill the inferred return type for non-generic function
            // expressions that omit one, so IR lowering (which reads
            // `function.return_type`) can lower anonymous/IIFE functions.
            // Generic function expressions still require an explicit return type.
            if function.return_type.is_none() && function.type_params.is_empty() {
                let inferred = infer_function_expr_return_type(
                    function,
                    vars,
                    fn_signatures,
                    &function_type_params,
                )?;
                function.return_type = Some(inferred);
            }
            let mut scope = vars.clone();
            for param in &function.params {
                scope.insert(
                    param.name.clone(),
                    binding_for(param.ty.clone(), Rebindability::Const),
                );
            }
            annotate_inferred_stmt_locals(
                &mut function.body,
                &mut scope,
                fn_signatures,
                &function_type_params,
            )
        }
        Expr::ArrayLiteral { elements, .. } => {
            for element in elements {
                annotate_inferred_expr_locals(element, vars, fn_signatures, active_type_params)?;
            }
            Ok(())
        }
        Expr::TableLiteral { fields, .. } => {
            for field in fields {
                annotate_inferred_expr_locals(
                    &mut field.value,
                    vars,
                    fn_signatures,
                    active_type_params,
                )?;
            }
            Ok(())
        }
        Expr::Field { base, .. } => {
            annotate_inferred_expr_locals(base, vars, fn_signatures, active_type_params)
        }
        Expr::Index { base, index, .. } => {
            annotate_inferred_expr_locals(base, vars, fn_signatures, active_type_params)?;
            annotate_inferred_expr_locals(index, vars, fn_signatures, active_type_params)
        }
        Expr::Number(..)
        | Expr::Bool(..)
        | Expr::Nil(..)
        | Expr::String(..)
        | Expr::Bytes(..)
        | Expr::Vararg(..)
        | Expr::Name(..)
        | Expr::Require(..) => Ok(()),
    }
}

fn annotate_inferred_stmt_locals(
    body: &mut [Stmt],
    vars: &mut HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
) -> Result<(), Diagnostic> {
    for stmt in body {
        match stmt {
            Stmt::Let {
                name,
                rebindability,
                ty,
                value,
                ..
            } => {
                annotate_inferred_expr_locals(value, vars, fn_signatures, active_type_params)?;
                let inferred_ty = if let Some(expected_ty) = ty.clone() {
                    expected_ty
                } else if matches!(value, Expr::ArrayLiteral { elements, .. } if elements.is_empty())
                {
                    Type::Record(BTreeMap::new())
                } else {
                    let inferred =
                        infer_expr(value, vars, fn_signatures, active_type_params, None)?;
                    *ty = Some(inferred.clone());
                    inferred
                };
                vars.insert(name.clone(), binding_for(inferred_ty, *rebindability));
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                annotate_inferred_expr_locals(condition, vars, fn_signatures, active_type_params)?;
                annotate_inferred_stmt_locals(
                    then_body,
                    &mut vars.clone(),
                    fn_signatures,
                    active_type_params,
                )?;
                annotate_inferred_stmt_locals(
                    else_body,
                    &mut vars.clone(),
                    fn_signatures,
                    active_type_params,
                )?;
            }
            Stmt::Match { value, arms, .. } => {
                annotate_inferred_expr_locals(value, vars, fn_signatures, active_type_params)?;
                for arm in arms {
                    annotate_inferred_stmt_locals(
                        &mut arm.body,
                        &mut vars.clone(),
                        fn_signatures,
                        active_type_params,
                    )?;
                }
            }
            Stmt::While { condition, body } => {
                annotate_inferred_expr_locals(condition, vars, fn_signatures, active_type_params)?;
                annotate_inferred_stmt_locals(
                    body,
                    &mut vars.clone(),
                    fn_signatures,
                    active_type_params,
                )?;
            }
            Stmt::Repeat { body, condition } => {
                // Lua scoping: the until-condition sees the body's locals.
                let mut loop_scope = vars.clone();
                annotate_inferred_stmt_locals(
                    body,
                    &mut loop_scope,
                    fn_signatures,
                    active_type_params,
                )?;
                annotate_inferred_expr_locals(
                    condition,
                    &loop_scope,
                    fn_signatures,
                    active_type_params,
                )?;
            }
            Stmt::Return(expr) | Stmt::Expr(expr) => {
                annotate_inferred_expr_locals(expr, vars, fn_signatures, active_type_params)?;
            }
            Stmt::ReturnMulti(exprs) => {
                for expr in exprs {
                    annotate_inferred_expr_locals(expr, vars, fn_signatures, active_type_params)?;
                }
            }
            Stmt::Assign { value, .. } => {
                annotate_inferred_expr_locals(value, vars, fn_signatures, active_type_params)?;
            }
            Stmt::IndexAssign {
                base, index, value, ..
            } => {
                annotate_inferred_expr_locals(base, vars, fn_signatures, active_type_params)?;
                annotate_inferred_expr_locals(index, vars, fn_signatures, active_type_params)?;
                annotate_inferred_expr_locals(value, vars, fn_signatures, active_type_params)?;
            }
            Stmt::FieldAssign { base, value, .. } => {
                annotate_inferred_expr_locals(base, vars, fn_signatures, active_type_params)?;
                annotate_inferred_expr_locals(value, vars, fn_signatures, active_type_params)?;
            }
            Stmt::LetMulti { values, .. } | Stmt::AssignMulti { values, .. } => {
                for value in values {
                    annotate_inferred_expr_locals(value, vars, fn_signatures, active_type_params)?;
                }
            }
            Stmt::IfCast {
                value,
                then_body,
                else_body,
                ..
            } => {
                annotate_inferred_expr_locals(value, vars, fn_signatures, active_type_params)?;
                annotate_inferred_stmt_locals(
                    then_body,
                    &mut vars.clone(),
                    fn_signatures,
                    active_type_params,
                )?;
                annotate_inferred_stmt_locals(
                    else_body,
                    &mut vars.clone(),
                    fn_signatures,
                    active_type_params,
                )?;
            }
            Stmt::NumericFor {
                name,
                start,
                stop,
                step,
                body,
                ..
            } => {
                annotate_inferred_expr_locals(start, vars, fn_signatures, active_type_params)?;
                annotate_inferred_expr_locals(stop, vars, fn_signatures, active_type_params)?;
                if let Some(step) = step {
                    annotate_inferred_expr_locals(step, vars, fn_signatures, active_type_params)?;
                }
                // Bind the loop variable so `local x = <expr using it>` in the
                // body can be inferred; the strict passes validate types later.
                let mut loop_scope = vars.clone();
                let mut bounds = vec![&*start, &*stop];
                if let Some(step) = step {
                    bounds.push(step);
                }
                if let Ok(loop_ty) =
                    numeric::infer_numeric_for_loop_type(&bounds, |expr, expected| {
                        infer_expr(expr, vars, fn_signatures, active_type_params, expected)
                    })
                {
                    loop_scope.insert(name.clone(), binding_for(loop_ty, Rebindability::Const));
                }
                annotate_inferred_stmt_locals(
                    body,
                    &mut loop_scope,
                    fn_signatures,
                    active_type_params,
                )?;
            }
            Stmt::ForIn {
                names,
                iterator,
                body,
                ..
            } => {
                annotate_inferred_expr_locals(iterator, vars, fn_signatures, active_type_params)?;
                let mut loop_scope = vars.clone();
                if let Ok(Type::Array(element_ty)) =
                    infer_expr(iterator, vars, fn_signatures, active_type_params, None)
                {
                    if names.len() == 1 {
                        loop_scope.insert(
                            names[0].clone(),
                            binding_for(*element_ty, Rebindability::Const),
                        );
                    } else if names.len() == 2 {
                        loop_scope.insert(
                            names[0].clone(),
                            binding_for(Type::Numeric(NumericType::I32), Rebindability::Const),
                        );
                        loop_scope.insert(
                            names[1].clone(),
                            binding_for(*element_ty, Rebindability::Const),
                        );
                    }
                }
                annotate_inferred_stmt_locals(
                    body,
                    &mut loop_scope,
                    fn_signatures,
                    active_type_params,
                )?;
            }
            Stmt::Break | Stmt::Continue => {}
        }
    }
    Ok(())
}

fn is_extern_opaque_type(ty: &Type) -> bool {
    matches!(
        ty,
        Type::Opaque { ty, .. }
            if matches!(ty.as_ref(), Type::Extern | Type::ExternSubtype(_))
    )
}

fn is_nullable_host_ref_type(ty: &Type) -> bool {
    matches!(ty, Type::String) || is_extern_opaque_type(ty)
}

fn require_nullable_host_ref_type(ty: &Type) -> Result<(), Diagnostic> {
    if is_nullable_host_ref_type(ty) {
        Ok(())
    } else {
        Err(Diagnostic::new(format!(
            "nullable modifier '?' is only supported on host reference types, got {ty}"
        )))
    }
}

fn is_nullable_inner_type(ty: &Type) -> bool {
    match ty {
        // Primitive value types are supported through typed nullable boxes
        // (a per-primitive GC struct whose null reference stands for nil).
        Type::Numeric(_) | Type::Bool => true,
        Type::String
        | Type::Bytes
        | Type::Array(_)
        | Type::Record(_)
        | Type::Function { .. }
        | Type::Thread
        | Type::TaggedVariant(_)
        | Type::TaggedUnion(_) => true,
        Type::Opaque { ty, .. } => {
            matches!(ty.as_ref(), Type::Extern | Type::ExternSubtype(_))
                || is_nullable_inner_type(ty)
        }
        _ => false,
    }
}

fn require_nullable_inner_type(ty: &Type) -> Result<(), Diagnostic> {
    if is_nullable_inner_type(ty) {
        Ok(())
    } else {
        Err(Diagnostic::new(format!(
            "nullable modifier '?' is not supported on {ty}"
        )))
    }
}

#[derive(Clone)]
struct GenericTypeDecl {
    type_params: Vec<String>,
    ty: Type,
}

fn substitute_type_params(ty: &Type, subst: &HashMap<String, Type>) -> Type {
    match ty {
        Type::Named { name, type_args } => Type::Named {
            name: name.clone(),
            type_args: type_args
                .iter()
                .map(|arg| substitute_type_params(arg, subst))
                .collect(),
        },
        Type::TaggedVariant(variant) => Type::TaggedVariant(waluau_ast::TaggedVariant {
            tag: variant.tag.clone(),
            payload: Box::new(substitute_type_params(variant.payload.as_ref(), subst)),
        }),
        Type::TaggedUnion(variants) => Type::TaggedUnion(
            variants
                .iter()
                .map(|variant| waluau_ast::TaggedVariant {
                    tag: variant.tag.clone(),
                    payload: Box::new(substitute_type_params(variant.payload.as_ref(), subst)),
                })
                .collect(),
        ),
        Type::Opaque { name, ty } => Type::Opaque {
            name: name.clone(),
            ty: Box::new(substitute_type_params(ty, subst)),
        },
        Type::ExternSubtype(parent) => {
            Type::ExternSubtype(Box::new(substitute_type_params(parent, subst)))
        }
        Type::Nullable(inner) => Type::Nullable(Box::new(substitute_type_params(inner, subst))),
        Type::TypeParam(name) => subst
            .get(name)
            .cloned()
            .unwrap_or_else(|| Type::TypeParam(name.clone())),
        Type::Array(inner) => Type::Array(Box::new(substitute_type_params(inner, subst))),
        Type::Multi(types) => Type::Multi(
            types
                .iter()
                .map(|inner| substitute_type_params(inner, subst))
                .collect(),
        ),
        Type::Function {
            params,
            return_type,
        } => Type::Function {
            params: params
                .iter()
                .map(|param| substitute_type_params(param, subst))
                .collect(),
            return_type: Box::new(substitute_type_params(return_type, subst)),
        },
        Type::Record(fields) => Type::Record(
            fields
                .iter()
                .map(|(name, ty)| (name.clone(), substitute_type_params(ty, subst)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn generic_instantiation_name(name: &str, type_args: &[Type]) -> String {
    if type_args.is_empty() {
        return name.to_string();
    }

    let rendered_args = type_args
        .iter()
        .map(std::string::ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    format!("{name}<{rendered_args}>")
}

fn specialize_generic_extern_constructor(name: &str, type_args: &[Type], resolved: Type) -> Type {
    match resolved {
        Type::Extern | Type::ExternSubtype(_) => Type::Opaque {
            name: generic_instantiation_name(name, type_args),
            ty: Box::new(resolved),
        },
        other => other,
    }
}

fn resolve_decl_type_allowing_forward_refs(
    name: &str,
    raw_opaque: &HashMap<String, Type>,
    generic: &HashMap<String, GenericTypeDecl>,
    opaque_cache: &mut HashMap<String, Type>,
    stack: &mut Vec<String>,
) -> Result<Type, Diagnostic> {
    // Check for direct cycles (type A = A)
    if stack.iter().any(|entry| entry == name) {
        let cycle = stack
            .iter()
            .cloned()
            .chain(std::iter::once(name.to_string()))
            .collect::<Vec<_>>()
            .join(" -> ");
        return Err(Diagnostic::new(format!(
            "cyclic type declaration detected: {cycle}"
        )));
    }

    let raw_ty = raw_opaque
        .get(name)
        .cloned()
        .ok_or_else(|| Diagnostic::new(format!("unknown type '{name}'")))?;

    stack.push(name.to_string());
    let resolved_underlying = resolve_type_refs_allowing_forward_refs(
        &raw_ty,
        &HashSet::new(),
        raw_opaque,
        generic,
        opaque_cache,
        stack,
        false,
    )?;
    stack.pop();

    let opaque = Type::Opaque {
        name: name.to_string(),
        ty: Box::new(resolved_underlying),
    };
    Ok(opaque)
}

fn resolve_type_refs_allowing_forward_refs(
    ty: &Type,
    active_type_params: &HashSet<String>,
    raw_opaque: &HashMap<String, Type>,
    generic: &HashMap<String, GenericTypeDecl>,
    opaque_cache: &mut HashMap<String, Type>,
    stack: &mut Vec<String>,
    guarded: bool,
) -> Result<Type, Diagnostic> {
    match ty {
        Type::Named { name, type_args } => {
            if active_type_params.contains(name) {
                if type_args.is_empty() {
                    return Ok(Type::TypeParam(name.clone()));
                }
                return Err(Diagnostic::new(format!(
                    "type parameter '{name}' cannot be used with type arguments"
                )));
            }
            let resolved_args = type_args
                .iter()
                .map(|arg| {
                    resolve_type_refs_allowing_forward_refs(
                        arg,
                        active_type_params,
                        raw_opaque,
                        generic,
                        opaque_cache,
                        stack,
                        guarded,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            if let Some(decl) = generic.get(name) {
                if resolved_args.len() != decl.type_params.len() {
                    return Err(Diagnostic::new(format!(
                        "type declaration '{name}' expects {} type argument{}, got {}",
                        decl.type_params.len(),
                        if decl.type_params.len() == 1 { "" } else { "s" },
                        resolved_args.len()
                    )));
                }
                // Handle generic type instantiation - this can still have cycles
                if stack.iter().any(|entry| entry == name) {
                    // For mutually recursive generics, return a placeholder
                    return Ok(Type::Named {
                        name: name.clone(),
                        type_args: resolved_args,
                    });
                }
                let subst = decl
                    .type_params
                    .iter()
                    .cloned()
                    .zip(resolved_args.iter().cloned())
                    .collect::<HashMap<_, _>>();
                stack.push(name.clone());
                let instantiated = substitute_type_params(&decl.ty, &subst);
                let resolved = resolve_type_refs_allowing_forward_refs(
                    &instantiated,
                    active_type_params,
                    raw_opaque,
                    generic,
                    opaque_cache,
                    stack,
                    guarded,
                );
                stack.pop();
                return resolved.map(|resolved| {
                    specialize_generic_extern_constructor(name, &resolved_args, resolved)
                });
            }
            if !raw_opaque.contains_key(name) {
                return Err(Diagnostic::new(format!("unknown type '{name}'")));
            }
            if !resolved_args.is_empty() {
                return Err(Diagnostic::new(format!(
                    "non-generic type declaration '{name}' does not accept type arguments"
                )));
            }
            // For forward references, return the current opaque type from cache
            // But first check if this is a direct self-reference in the resolution stack
            if stack.iter().any(|entry| entry == name) {
                if guarded {
                    // Rust's owned Type tree cannot contain a literal cycle.
                    // Preserve the alias identity at the recursive edge and
                    // use `unknown` as its finite runtime anchor. HIR coercion
                    // treats same-named opaque aliases nominally, while IR
                    // erasure turns this edge into an anyref plus checked casts.
                    return Ok(Type::Opaque {
                        name: name.to_string(),
                        ty: Box::new(Type::Unknown),
                    });
                }
                let cycle = stack
                    .iter()
                    .cloned()
                    .chain(std::iter::once(name.to_string()))
                    .collect::<Vec<_>>()
                    .join(" -> ");
                return Err(Diagnostic::new(format!(
                    "cyclic type declaration detected: {cycle}"
                )));
            }

            Ok(opaque_cache
                .get(name)
                .cloned()
                .unwrap_or_else(|| Type::Opaque {
                    name: name.to_string(),
                    ty: Box::new(Type::Unit),
                }))
        }
        Type::Opaque { name, ty } => Ok(Type::Opaque {
            name: name.clone(),
            ty: Box::new(resolve_type_refs_allowing_forward_refs(
                ty,
                active_type_params,
                raw_opaque,
                generic,
                opaque_cache,
                stack,
                guarded,
            )?),
        }),
        Type::ExternSubtype(parent) => {
            let parent = resolve_type_refs_allowing_forward_refs(
                parent,
                active_type_params,
                raw_opaque,
                generic,
                opaque_cache,
                stack,
                guarded,
            )?;
            require_nullable_host_ref_type(&parent)?;
            Ok(Type::ExternSubtype(Box::new(parent)))
        }
        Type::Nullable(inner) => {
            let inner = resolve_type_refs_allowing_forward_refs(
                inner,
                active_type_params,
                raw_opaque,
                generic,
                opaque_cache,
                stack,
                guarded,
            )?;
            require_nullable_inner_type(&inner)?;
            Ok(Type::Nullable(Box::new(inner)))
        }
        Type::Array(inner) => Ok(Type::Array(Box::new(
            resolve_type_refs_allowing_forward_refs(
                inner,
                active_type_params,
                raw_opaque,
                generic,
                opaque_cache,
                stack,
                true,
            )?,
        ))),
        Type::Multi(types) => Ok(Type::Multi(
            types
                .iter()
                .map(|ty| {
                    resolve_type_refs_allowing_forward_refs(
                        ty,
                        active_type_params,
                        raw_opaque,
                        generic,
                        opaque_cache,
                        stack,
                        guarded,
                    )
                })
                .collect::<Result<_, _>>()?,
        )),
        Type::Function {
            params,
            return_type,
        } => Ok(Type::Function {
            params: params
                .iter()
                .map(|param| {
                    resolve_type_refs_allowing_forward_refs(
                        param,
                        active_type_params,
                        raw_opaque,
                        generic,
                        opaque_cache,
                        stack,
                        guarded,
                    )
                })
                .collect::<Result<_, _>>()?,
            return_type: Box::new(resolve_type_refs_allowing_forward_refs(
                return_type,
                active_type_params,
                raw_opaque,
                generic,
                opaque_cache,
                stack,
                guarded,
            )?),
        }),
        Type::Record(fields) => Ok(Type::Record(
            fields
                .iter()
                .map(|(name, ty)| {
                    Ok((
                        name.clone(),
                        resolve_type_refs_allowing_forward_refs(
                            ty,
                            active_type_params,
                            raw_opaque,
                            generic,
                            opaque_cache,
                            stack,
                            true,
                        )?,
                    ))
                })
                .collect::<Result<_, Diagnostic>>()?,
        )),
        other => Ok(other.clone()),
    }
}

fn resolve_program_types(program: &mut Program) -> Result<(), Diagnostic> {
    let mut seen = HashSet::new();
    let mut raw_opaque = HashMap::new();
    let mut generic = HashMap::new();
    for decl in &program.type_declarations {
        if !seen.insert(decl.name.clone()) {
            return Err(Diagnostic::new(format!(
                "duplicate type declaration '{}'",
                decl.name
            )));
        }
        if decl.type_params.is_empty() {
            raw_opaque.insert(decl.name.clone(), decl.ty.clone());
        } else {
            generic.insert(
                decl.name.clone(),
                GenericTypeDecl {
                    type_params: decl.type_params.clone(),
                    ty: decl.ty.clone(),
                },
            );
        }
    }

    // Alias-only cycles have no runtime constructor to tie the knot and no
    // finite structural shape to check. Reject them before forward-reference
    // placeholders can hide the cycle. A record/array boundary is guarded and
    // is resolved below using a finite opaque anchor.
    for decl in &program.type_declarations {
        if decl.type_params.is_empty() {
            validate_unguarded_alias_cycle(&decl.name, &raw_opaque, &generic, &mut Vec::new())?;
        }
    }

    // Initialize opaque cache with all declared types to enable forward references
    let mut opaque_cache = HashMap::new();
    for name in raw_opaque.keys() {
        let placeholder = Type::Opaque {
            name: name.clone(),
            ty: Box::new(Type::Unit), // Will be replaced
        };
        opaque_cache.insert(name.clone(), placeholder);
    }

    // Resolve all types allowing forward references
    let mut stack = Vec::new();
    for decl in &program.type_declarations {
        if decl.type_params.is_empty() {
            let resolved_ty = resolve_decl_type_allowing_forward_refs(
                &decl.name,
                &raw_opaque,
                &generic,
                &mut opaque_cache,
                &mut stack,
            )?;
            opaque_cache.insert(decl.name.clone(), resolved_ty);
        }
    }

    for decl in &mut program.type_declarations {
        let active = active_type_param_set(&decl.type_params);
        decl.ty = resolve_type_refs(
            &decl.ty,
            &active,
            &raw_opaque,
            &generic,
            &mut opaque_cache,
            &mut vec![decl.name.clone()],
        )?;
    }
    for function in &mut program.functions {
        resolve_function_type_refs(
            function,
            &raw_opaque,
            &generic,
            &mut opaque_cache,
            &HashSet::new(),
        )?;
    }
    for declared in &mut program.declared_imports {
        for param in &mut declared.params {
            param.ty = resolve_type_refs(
                &param.ty,
                &HashSet::new(),
                &raw_opaque,
                &generic,
                &mut opaque_cache,
                &mut Vec::new(),
            )?;
        }
        declared.return_type = resolve_type_refs(
            &declared.return_type,
            &HashSet::new(),
            &raw_opaque,
            &generic,
            &mut opaque_cache,
            &mut Vec::new(),
        )?;
    }
    for stmt in &mut program.top_level {
        resolve_stmt_type_refs(
            stmt,
            &raw_opaque,
            &generic,
            &mut opaque_cache,
            &HashSet::new(),
        )?;
    }
    if let Some(export) = &mut program.export {
        resolve_expr_type_refs(
            export,
            &raw_opaque,
            &generic,
            &mut opaque_cache,
            &HashSet::new(),
        )?;
    }
    Ok(())
}

fn validate_unguarded_alias_cycle(
    name: &str,
    raw_opaque: &HashMap<String, Type>,
    generic: &HashMap<String, GenericTypeDecl>,
    stack: &mut Vec<String>,
) -> Result<(), Diagnostic> {
    if let Some(start) = stack.iter().position(|entry| entry == name) {
        let cycle = stack[start..]
            .iter()
            .cloned()
            .chain(std::iter::once(name.to_string()))
            .collect::<Vec<_>>()
            .join(" -> ");
        return Err(Diagnostic::new(format!(
            "cyclic type declaration detected: {cycle}"
        )));
    }

    let Some(Type::Named {
        name: target,
        type_args,
    }) = raw_opaque.get(name)
    else {
        return Ok(());
    };
    if !type_args.is_empty() || generic.contains_key(target) || !raw_opaque.contains_key(target) {
        return Ok(());
    }

    stack.push(name.to_string());
    let result = validate_unguarded_alias_cycle(target, raw_opaque, generic, stack);
    stack.pop();
    result
}

fn resolve_type_refs(
    ty: &Type,
    active_type_params: &HashSet<String>,
    raw_opaque: &HashMap<String, Type>,
    generic: &HashMap<String, GenericTypeDecl>,
    opaque_cache: &mut HashMap<String, Type>,
    stack: &mut Vec<String>,
) -> Result<Type, Diagnostic> {
    resolve_type_refs_fixpoint(
        ty,
        active_type_params,
        raw_opaque,
        generic,
        opaque_cache,
        stack,
        false,
    )
}

fn resolve_type_refs_fixpoint(
    ty: &Type,
    active_type_params: &HashSet<String>,
    raw_opaque: &HashMap<String, Type>,
    generic: &HashMap<String, GenericTypeDecl>,
    opaque_cache: &mut HashMap<String, Type>,
    stack: &mut Vec<String>,
    fixpoint_mode: bool,
) -> Result<Type, Diagnostic> {
    match ty {
        Type::Named { name, type_args } => {
            if active_type_params.contains(name) {
                if type_args.is_empty() {
                    return Ok(Type::TypeParam(name.clone()));
                }
                return Err(Diagnostic::new(format!(
                    "type parameter '{name}' cannot be used with type arguments"
                )));
            }
            let resolved_args = type_args
                .iter()
                .map(|arg| {
                    resolve_type_refs_fixpoint(
                        arg,
                        active_type_params,
                        raw_opaque,
                        generic,
                        opaque_cache,
                        stack,
                        fixpoint_mode,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            if let Some(decl) = generic.get(name) {
                if resolved_args.len() != decl.type_params.len() {
                    return Err(Diagnostic::new(format!(
                        "type declaration '{name}' expects {} type argument{}, got {}",
                        decl.type_params.len(),
                        if decl.type_params.len() == 1 { "" } else { "s" },
                        resolved_args.len()
                    )));
                }
                if stack.iter().any(|entry| entry == name) {
                    if fixpoint_mode {
                        // In fixpoint mode, return a placeholder when we hit a cycle
                        return Ok(Type::Named {
                            name: name.clone(),
                            type_args: resolved_args,
                        });
                    } else {
                        let cycle = stack
                            .iter()
                            .cloned()
                            .chain(std::iter::once(name.clone()))
                            .collect::<Vec<_>>()
                            .join(" -> ");
                        return Err(Diagnostic::new(format!(
                            "cyclic type declaration detected: {cycle}"
                        )));
                    }
                }
                let subst = decl
                    .type_params
                    .iter()
                    .cloned()
                    .zip(resolved_args.iter().cloned())
                    .collect::<HashMap<_, _>>();
                stack.push(name.clone());
                let instantiated = substitute_type_params(&decl.ty, &subst);
                let resolved = resolve_type_refs_fixpoint(
                    &instantiated,
                    active_type_params,
                    raw_opaque,
                    generic,
                    opaque_cache,
                    stack,
                    fixpoint_mode,
                );
                stack.pop();
                return resolved.map(|resolved| {
                    specialize_generic_extern_constructor(name, &resolved_args, resolved)
                });
            }
            if !raw_opaque.contains_key(name) {
                return Err(Diagnostic::new(format!("unknown type '{name}'")));
            }
            if !resolved_args.is_empty() {
                return Err(Diagnostic::new(format!(
                    "non-generic type declaration '{name}' does not accept type arguments"
                )));
            }
            if fixpoint_mode {
                // In fixpoint mode, just return a reference to the opaque type
                // Don't try to resolve it further to avoid infinite expansion
                Ok(opaque_cache
                    .get(name)
                    .cloned()
                    .unwrap_or_else(|| Type::Opaque {
                        name: name.to_string(),
                        ty: Box::new(Type::Unit),
                    }))
            } else {
                resolve_decl_type_fixpoint(
                    name,
                    raw_opaque,
                    generic,
                    opaque_cache,
                    stack,
                    fixpoint_mode,
                )
            }
        }
        Type::Opaque { name, ty } => Ok(Type::Opaque {
            name: name.clone(),
            ty: Box::new(resolve_type_refs_fixpoint(
                ty,
                active_type_params,
                raw_opaque,
                generic,
                opaque_cache,
                stack,
                fixpoint_mode,
            )?),
        }),
        Type::ExternSubtype(parent) => {
            let parent = resolve_type_refs_fixpoint(
                parent,
                active_type_params,
                raw_opaque,
                generic,
                opaque_cache,
                stack,
                fixpoint_mode,
            )?;
            require_nullable_host_ref_type(&parent)?;
            Ok(Type::ExternSubtype(Box::new(parent)))
        }
        Type::Nullable(inner) => {
            let inner = resolve_type_refs_fixpoint(
                inner,
                active_type_params,
                raw_opaque,
                generic,
                opaque_cache,
                stack,
                fixpoint_mode,
            )?;
            require_nullable_inner_type(&inner)?;
            Ok(Type::Nullable(Box::new(inner)))
        }
        Type::Array(inner) => Ok(Type::Array(Box::new(resolve_type_refs_fixpoint(
            inner,
            active_type_params,
            raw_opaque,
            generic,
            opaque_cache,
            stack,
            fixpoint_mode,
        )?))),
        Type::Multi(types) => Ok(Type::Multi(
            types
                .iter()
                .map(|ty| {
                    resolve_type_refs_fixpoint(
                        ty,
                        active_type_params,
                        raw_opaque,
                        generic,
                        opaque_cache,
                        stack,
                        fixpoint_mode,
                    )
                })
                .collect::<Result<_, _>>()?,
        )),
        Type::Function {
            params,
            return_type,
        } => Ok(Type::Function {
            params: params
                .iter()
                .map(|param| {
                    resolve_type_refs_fixpoint(
                        param,
                        active_type_params,
                        raw_opaque,
                        generic,
                        opaque_cache,
                        stack,
                        fixpoint_mode,
                    )
                })
                .collect::<Result<_, _>>()?,
            return_type: Box::new(resolve_type_refs_fixpoint(
                return_type,
                active_type_params,
                raw_opaque,
                generic,
                opaque_cache,
                stack,
                fixpoint_mode,
            )?),
        }),
        Type::Record(fields) => Ok(Type::Record(
            fields
                .iter()
                .map(|(name, ty)| {
                    Ok((
                        name.clone(),
                        resolve_type_refs_fixpoint(
                            ty,
                            active_type_params,
                            raw_opaque,
                            generic,
                            opaque_cache,
                            stack,
                            fixpoint_mode,
                        )?,
                    ))
                })
                .collect::<Result<_, Diagnostic>>()?,
        )),
        other => Ok(other.clone()),
    }
}

fn resolve_decl_type_fixpoint(
    name: &str,
    raw_opaque: &HashMap<String, Type>,
    generic: &HashMap<String, GenericTypeDecl>,
    opaque_cache: &mut HashMap<String, Type>,
    stack: &mut Vec<String>,
    fixpoint_mode: bool,
) -> Result<Type, Diagnostic> {
    if let Some(ty) = opaque_cache.get(name) {
        return Ok(ty.clone());
    }
    if stack.iter().any(|entry| entry == name) {
        if fixpoint_mode {
            // In fixpoint mode, return the current cached value if available,
            // or create a placeholder
            return Ok(opaque_cache
                .get(name)
                .cloned()
                .unwrap_or_else(|| Type::Opaque {
                    name: name.to_string(),
                    ty: Box::new(Type::Unit),
                }));
        } else {
            let cycle = stack
                .iter()
                .cloned()
                .chain(std::iter::once(name.to_string()))
                .collect::<Vec<_>>()
                .join(" -> ");
            return Err(Diagnostic::new(format!(
                "cyclic type declaration detected: {cycle}"
            )));
        }
    }
    let raw_ty = raw_opaque
        .get(name)
        .cloned()
        .ok_or_else(|| Diagnostic::new(format!("unknown type '{name}'")))?;
    stack.push(name.to_string());
    let resolved_underlying = resolve_type_refs_fixpoint(
        &raw_ty,
        &HashSet::new(),
        raw_opaque,
        generic,
        opaque_cache,
        stack,
        fixpoint_mode,
    )?;
    stack.pop();
    let opaque = Type::Opaque {
        name: name.to_string(),
        ty: Box::new(resolved_underlying),
    };
    opaque_cache.insert(name.to_string(), opaque.clone());
    Ok(opaque)
}

fn resolve_function_type_refs(
    function: &mut Function,
    raw_opaque: &HashMap<String, Type>,
    generic: &HashMap<String, GenericTypeDecl>,
    opaque_cache: &mut HashMap<String, Type>,
    outer_type_params: &HashSet<String>,
) -> Result<(), Diagnostic> {
    let mut active = outer_type_params.clone();
    active.extend(active_type_param_set(&function.type_params));
    for param in &mut function.params {
        param.ty = resolve_type_refs(
            &param.ty,
            &active,
            raw_opaque,
            generic,
            opaque_cache,
            &mut Vec::new(),
        )?;
    }
    if let Some(return_type) = &mut function.return_type {
        *return_type = resolve_type_refs(
            return_type,
            &active,
            raw_opaque,
            generic,
            opaque_cache,
            &mut Vec::new(),
        )?;
    }
    for stmt in &mut function.body {
        resolve_stmt_type_refs(stmt, raw_opaque, generic, opaque_cache, &active)?;
    }
    Ok(())
}

fn resolve_stmt_type_refs(
    stmt: &mut Stmt,
    raw_opaque: &HashMap<String, Type>,
    generic: &HashMap<String, GenericTypeDecl>,
    opaque_cache: &mut HashMap<String, Type>,
    active_type_params: &HashSet<String>,
) -> Result<(), Diagnostic> {
    match stmt {
        Stmt::Match {
            value,
            enum_ty,
            arms,
        } => {
            *enum_ty = resolve_type_refs(
                enum_ty,
                active_type_params,
                raw_opaque,
                generic,
                opaque_cache,
                &mut Vec::new(),
            )?;
            resolve_expr_type_refs(value, raw_opaque, generic, opaque_cache, active_type_params)?;
            for arm in arms {
                for stmt in &mut arm.body {
                    resolve_stmt_type_refs(
                        stmt,
                        raw_opaque,
                        generic,
                        opaque_cache,
                        active_type_params,
                    )?;
                }
            }
            Ok(())
        }
        Stmt::Let { ty, value, .. } => {
            if let Some(local_ty) = ty {
                *local_ty = resolve_type_refs(
                    local_ty,
                    active_type_params,
                    raw_opaque,
                    generic,
                    opaque_cache,
                    &mut Vec::new(),
                )?;
            }
            resolve_expr_type_refs(value, raw_opaque, generic, opaque_cache, active_type_params)
        }
        Stmt::Assign { value, .. } | Stmt::Expr(value) | Stmt::Return(value) => {
            resolve_expr_type_refs(value, raw_opaque, generic, opaque_cache, active_type_params)
        }
        Stmt::IndexAssign {
            base, index, value, ..
        } => {
            resolve_expr_type_refs(base, raw_opaque, generic, opaque_cache, active_type_params)?;
            resolve_expr_type_refs(index, raw_opaque, generic, opaque_cache, active_type_params)?;
            resolve_expr_type_refs(value, raw_opaque, generic, opaque_cache, active_type_params)
        }
        Stmt::FieldAssign { base, value, .. } => {
            resolve_expr_type_refs(base, raw_opaque, generic, opaque_cache, active_type_params)?;
            resolve_expr_type_refs(value, raw_opaque, generic, opaque_cache, active_type_params)
        }
        Stmt::If {
            condition,
            then_body,
            else_body,
        } => {
            resolve_expr_type_refs(
                condition,
                raw_opaque,
                generic,
                opaque_cache,
                active_type_params,
            )?;
            for stmt in then_body {
                resolve_stmt_type_refs(
                    stmt,
                    raw_opaque,
                    generic,
                    opaque_cache,
                    active_type_params,
                )?;
            }
            for stmt in else_body {
                resolve_stmt_type_refs(
                    stmt,
                    raw_opaque,
                    generic,
                    opaque_cache,
                    active_type_params,
                )?;
            }
            Ok(())
        }
        Stmt::IfCast {
            target_ty,
            value,
            then_body,
            else_body,
            ..
        } => {
            // `target_name` may name either a separately-declared (extern) type or a
            // tagged-union variant tag of the scrutinee's type (e.g. `if Left(n) = value`).
            // Variant tags aren't resolvable type names, so tolerate resolution failure
            // here and leave `target_ty` as an unresolved `Type::Named` — the dispatcher
            // in `check_stmt`/`checked_if_cast_scopes` decides which interpretation
            // applies once it has the inferred scrutinee type.
            *target_ty = resolve_type_refs(
                target_ty,
                active_type_params,
                raw_opaque,
                generic,
                opaque_cache,
                &mut Vec::new(),
            )
            .unwrap_or_else(|_| target_ty.clone());
            resolve_expr_type_refs(value, raw_opaque, generic, opaque_cache, active_type_params)?;
            for stmt in then_body {
                resolve_stmt_type_refs(
                    stmt,
                    raw_opaque,
                    generic,
                    opaque_cache,
                    active_type_params,
                )?;
            }
            for stmt in else_body {
                resolve_stmt_type_refs(
                    stmt,
                    raw_opaque,
                    generic,
                    opaque_cache,
                    active_type_params,
                )?;
            }
            Ok(())
        }
        Stmt::While { condition, body } => {
            resolve_expr_type_refs(
                condition,
                raw_opaque,
                generic,
                opaque_cache,
                active_type_params,
            )?;
            for stmt in body {
                resolve_stmt_type_refs(
                    stmt,
                    raw_opaque,
                    generic,
                    opaque_cache,
                    active_type_params,
                )?;
            }
            Ok(())
        }
        Stmt::Repeat { body, condition } => {
            for stmt in body {
                resolve_stmt_type_refs(
                    stmt,
                    raw_opaque,
                    generic,
                    opaque_cache,
                    active_type_params,
                )?;
            }
            resolve_expr_type_refs(
                condition,
                raw_opaque,
                generic,
                opaque_cache,
                active_type_params,
            )
        }
        Stmt::NumericFor {
            start,
            stop,
            step,
            body,
            ..
        } => {
            resolve_expr_type_refs(start, raw_opaque, generic, opaque_cache, active_type_params)?;
            resolve_expr_type_refs(stop, raw_opaque, generic, opaque_cache, active_type_params)?;
            if let Some(step) = step {
                resolve_expr_type_refs(
                    step,
                    raw_opaque,
                    generic,
                    opaque_cache,
                    active_type_params,
                )?;
            }
            for stmt in body {
                resolve_stmt_type_refs(
                    stmt,
                    raw_opaque,
                    generic,
                    opaque_cache,
                    active_type_params,
                )?;
            }
            Ok(())
        }
        Stmt::ForIn { iterator, body, .. } => {
            resolve_expr_type_refs(
                iterator,
                raw_opaque,
                generic,
                opaque_cache,
                active_type_params,
            )?;
            for stmt in body {
                resolve_stmt_type_refs(
                    stmt,
                    raw_opaque,
                    generic,
                    opaque_cache,
                    active_type_params,
                )?;
            }
            Ok(())
        }
        Stmt::ReturnMulti(values) | Stmt::AssignMulti { values, .. } => {
            for value in values {
                resolve_expr_type_refs(
                    value,
                    raw_opaque,
                    generic,
                    opaque_cache,
                    active_type_params,
                )?;
            }
            Ok(())
        }
        Stmt::LetMulti { bindings, values } => {
            for binding in bindings {
                if let Some(ty) = &mut binding.ty {
                    *ty = resolve_type_refs(
                        ty,
                        active_type_params,
                        raw_opaque,
                        generic,
                        opaque_cache,
                        &mut Vec::new(),
                    )?;
                }
            }
            for value in values {
                resolve_expr_type_refs(
                    value,
                    raw_opaque,
                    generic,
                    opaque_cache,
                    active_type_params,
                )?;
            }
            Ok(())
        }
        Stmt::Break | Stmt::Continue => Ok(()),
    }
}

fn resolve_expr_type_refs(
    expr: &mut Expr,
    raw_opaque: &HashMap<String, Type>,
    generic: &HashMap<String, GenericTypeDecl>,
    opaque_cache: &mut HashMap<String, Type>,
    active_type_params: &HashSet<String>,
) -> Result<(), Diagnostic> {
    match expr {
        Expr::Unary { expr, .. } => {
            resolve_expr_type_refs(expr, raw_opaque, generic, opaque_cache, active_type_params)
        }
        Expr::Cast { expr, ty, .. } => {
            resolve_expr_type_refs(expr, raw_opaque, generic, opaque_cache, active_type_params)?;
            *ty = resolve_type_refs(
                ty,
                active_type_params,
                raw_opaque,
                generic,
                opaque_cache,
                &mut Vec::new(),
            )?;
            Ok(())
        }
        Expr::Binary { left, right, .. } => {
            resolve_expr_type_refs(left, raw_opaque, generic, opaque_cache, active_type_params)?;
            resolve_expr_type_refs(right, raw_opaque, generic, opaque_cache, active_type_params)
        }
        Expr::If {
            condition,
            then_expr,
            else_expr,
            ..
        } => {
            resolve_expr_type_refs(
                condition,
                raw_opaque,
                generic,
                opaque_cache,
                active_type_params,
            )?;
            resolve_expr_type_refs(
                then_expr,
                raw_opaque,
                generic,
                opaque_cache,
                active_type_params,
            )?;
            resolve_expr_type_refs(
                else_expr,
                raw_opaque,
                generic,
                opaque_cache,
                active_type_params,
            )
        }
        Expr::Call {
            callee,
            type_args,
            args,
            ..
        } => {
            resolve_expr_type_refs(
                callee,
                raw_opaque,
                generic,
                opaque_cache,
                active_type_params,
            )?;
            for ty in type_args {
                *ty = resolve_type_refs(
                    ty,
                    active_type_params,
                    raw_opaque,
                    generic,
                    opaque_cache,
                    &mut Vec::new(),
                )?;
            }
            for arg in args {
                resolve_expr_type_refs(arg, raw_opaque, generic, opaque_cache, active_type_params)?;
            }
            Ok(())
        }
        Expr::MethodCall { receiver, args, .. } => {
            resolve_expr_type_refs(
                receiver,
                raw_opaque,
                generic,
                opaque_cache,
                active_type_params,
            )?;
            for arg in args {
                resolve_expr_type_refs(arg, raw_opaque, generic, opaque_cache, active_type_params)?;
            }
            Ok(())
        }
        Expr::Function(function) => resolve_function_expr_type_refs(
            function,
            raw_opaque,
            generic,
            opaque_cache,
            active_type_params,
        ),
        Expr::ArrayLiteral { elements, .. } => {
            for element in elements {
                resolve_expr_type_refs(
                    element,
                    raw_opaque,
                    generic,
                    opaque_cache,
                    active_type_params,
                )?;
            }
            Ok(())
        }
        Expr::TableLiteral { fields, .. } => {
            for field in fields {
                resolve_expr_type_refs(
                    &mut field.value,
                    raw_opaque,
                    generic,
                    opaque_cache,
                    active_type_params,
                )?;
            }
            Ok(())
        }
        Expr::Field { base, .. } => {
            resolve_expr_type_refs(base, raw_opaque, generic, opaque_cache, active_type_params)
        }
        Expr::Index { base, index, .. } => {
            resolve_expr_type_refs(base, raw_opaque, generic, opaque_cache, active_type_params)?;
            resolve_expr_type_refs(index, raw_opaque, generic, opaque_cache, active_type_params)
        }
        Expr::Number(..)
        | Expr::Bool(..)
        | Expr::Nil(..)
        | Expr::String(..)
        | Expr::Bytes(..)
        | Expr::Vararg(..)
        | Expr::Name(..)
        | Expr::Require(..)
        | Expr::IsVariant { .. } => Ok(()),
    }
}

fn resolve_function_expr_type_refs(
    function: &mut FunctionExpr,
    raw_opaque: &HashMap<String, Type>,
    generic: &HashMap<String, GenericTypeDecl>,
    opaque_cache: &mut HashMap<String, Type>,
    outer_type_params: &HashSet<String>,
) -> Result<(), Diagnostic> {
    let mut active = outer_type_params.clone();
    active.extend(active_type_param_set(&function.type_params));
    for param in &mut function.params {
        param.ty = resolve_type_refs(
            &param.ty,
            &active,
            raw_opaque,
            generic,
            opaque_cache,
            &mut Vec::new(),
        )?;
    }
    if let Some(return_type) = &mut function.return_type {
        *return_type = resolve_type_refs(
            return_type,
            &active,
            raw_opaque,
            generic,
            opaque_cache,
            &mut Vec::new(),
        )?;
    }
    for stmt in &mut function.body {
        resolve_stmt_type_refs(stmt, raw_opaque, generic, opaque_cache, &active)?;
    }
    Ok(())
}

fn desugar_method_declarations(program: &Program) -> Result<Program, Diagnostic> {
    let mut rewritten = program.clone();
    rewritten.functions.clear();
    rewritten.top_level.clear();
    let mut pending_methods: Vec<(String, Stmt)> = Vec::new();
    let type_names = program
        .type_declarations
        .iter()
        .map(|decl| decl.name.clone())
        .collect::<HashSet<_>>();

    for function in &program.functions {
        match &function.name {
            FunctionName::Simple(_) => rewritten.functions.push(function.clone()),
            FunctionName::Method { table, method } if type_names.contains(table) => {
                let mut params = Vec::with_capacity(function.params.len() + 1);
                params.push(Param {
                    name: "self".to_string(),
                    symbol_id: None,
                    ty: Type::Named {
                        name: table.clone(),
                        type_args: Vec::new(),
                    },
                });
                params.extend(function.params.clone());
                rewritten.functions.push(Function {
                    name: FunctionName::Simple(method_signature_name(table, method)),
                    symbol_id: None,
                    type_params: function.type_params.clone(),
                    params,
                    vararg: function.vararg,
                    return_type: function.return_type.clone(),
                    body: function.body.clone(),
                    file_path: function.file_path.clone(),
                });
            }
            FunctionName::Method { table, method } => {
                let mut params = Vec::with_capacity(function.params.len() + 1);
                params.push(Param {
                    name: "self".to_string(),
                    symbol_id: None,
                    ty: Type::Unit,
                });
                params.extend(function.params.clone());
                pending_methods.push((
                    table.clone(),
                    Stmt::FieldAssign {
                        op: AssignOp::Set,
                        base: Box::new(Expr::Name(table.clone(), None, None)),
                        name: method.clone(),
                        resolved_name: None,
                        value: Expr::Function(FunctionExpr {
                            name: None,
                            symbol_id: None,
                            implicit_self: Some(table.clone()),
                            type_params: function.type_params.clone(),
                            params,
                            vararg: function.vararg,
                            return_type: function.return_type.clone(),
                            body: function.body.clone(),
                            file_path: function.file_path.clone(),
                            span: None,
                        }),
                    },
                ));
            }
        }
    }

    for stmt in &program.top_level {
        rewritten.top_level.push(stmt.clone());
        let Stmt::Let { name, .. } = stmt else {
            continue;
        };
        let mut remaining = Vec::with_capacity(pending_methods.len());
        for (table, method_stmt) in pending_methods.drain(..) {
            if table == *name {
                rewritten.top_level.push(method_stmt);
            } else {
                remaining.push((table, method_stmt));
            }
        }
        pending_methods = remaining;
    }
    rewritten
        .top_level
        .extend(pending_methods.into_iter().map(|(_, stmt)| stmt));

    Ok(rewritten)
}

fn initial_top_level_names(program: &Program) -> HashSet<String> {
    let mut names = HashSet::from([
        "print".to_string(),
        "assert".to_string(),
        "error".to_string(),
        "pcall".to_string(),
        "type".to_string(),
        "typeof".to_string(),
        "tostring".to_string(),
        "tonumber".to_string(),
        "select".to_string(),
        "math".to_string(),
        "coroutine".to_string(),
        "promise".to_string(),
        "table".to_string(),
        "string".to_string(),
        "bit32".to_string(),
    ]);
    names.extend(
        program
            .declared_imports
            .iter()
            .map(|decl| decl.name.clone()),
    );
    names.extend(program.functions.iter().filter_map(|function| {
        function
            .name
            .simple_name()
            .map(std::borrow::ToOwned::to_owned)
    }));
    names
}

fn fresh_implicit_multi_temp(declared: &HashSet<String>, counter: &mut usize) -> String {
    loop {
        let name = format!("__waluau$implicit_multi${}", *counter);
        *counter += 1;
        if !declared.contains(&name) {
            return name;
        }
    }
}

fn desugar_implicit_top_level_declarations(program: &mut Program) {
    let mut declared = initial_top_level_names(program);
    let mut rewritten = Vec::with_capacity(program.top_level.len());
    let mut temp_counter = 0;

    for stmt in std::mem::take(&mut program.top_level) {
        match stmt {
            Stmt::Let {
                name,
                symbol_id,
                rebindability,
                ty,
                value,
            } => {
                declared.insert(name.clone());
                rewritten.push(Stmt::Let {
                    name,
                    symbol_id,
                    rebindability,
                    ty,
                    value,
                });
            }
            Stmt::LetMulti { bindings, values } => {
                for binding in &bindings {
                    declared.insert(binding.name.clone());
                }
                rewritten.push(Stmt::LetMulti { bindings, values });
            }
            Stmt::Assign {
                op: AssignOp::Set,
                name,
                symbol_id,
                value,
            } if !declared.contains(&name) => {
                declared.insert(name.clone());
                rewritten.push(Stmt::Let {
                    name,
                    symbol_id,
                    rebindability: Rebindability::Rebindable,
                    ty: None,
                    value,
                });
            }
            Stmt::AssignMulti {
                targets,
                symbol_ids: _,
                values,
            } if targets.iter().any(|target| !declared.contains(target)) => {
                if targets.iter().all(|target| !declared.contains(target)) {
                    let bindings = targets
                        .into_iter()
                        .map(|name| {
                            declared.insert(name.clone());
                            waluau_ast::Binding {
                                name,
                                symbol_id: None,
                                rebindability: Rebindability::Rebindable,
                                ty: None,
                            }
                        })
                        .collect();
                    rewritten.push(Stmt::LetMulti { bindings, values });
                } else {
                    let temps = (0..targets.len())
                        .map(|_| {
                            let name = fresh_implicit_multi_temp(&declared, &mut temp_counter);
                            declared.insert(name.clone());
                            name
                        })
                        .collect::<Vec<_>>();
                    let temp_bindings = temps
                        .iter()
                        .map(|name| waluau_ast::Binding {
                            name: name.clone(),
                            symbol_id: None,
                            rebindability: Rebindability::Rebindable,
                            ty: None,
                        })
                        .collect();
                    rewritten.push(Stmt::LetMulti {
                        bindings: temp_bindings,
                        values,
                    });
                    for (target, temp) in targets.into_iter().zip(temps) {
                        let value = Expr::Name(temp, None, None);
                        if declared.contains(&target) {
                            rewritten.push(Stmt::Assign {
                                op: AssignOp::Set,
                                name: target,
                                symbol_id: None,
                                value,
                            });
                        } else {
                            declared.insert(target.clone());
                            rewritten.push(Stmt::Let {
                                name: target,
                                symbol_id: None,
                                rebindability: Rebindability::Rebindable,
                                ty: None,
                                value,
                            });
                        }
                    }
                }
            }
            other => rewritten.push(other),
        }
    }

    program.top_level = rewritten;
}

fn method_signature_name(table: &str, method: &str) -> String {
    format!("{table}.{method}")
}

fn signature_from_function_expr(function: &FunctionExpr) -> Option<FnSignature> {
    let return_type = function.return_type.clone()?;
    let params = function
        .params
        .iter()
        .map(|param| param.ty.clone())
        .collect();
    Some(if function.type_params.is_empty() {
        FnSignature::Mono {
            params,
            vararg: function.vararg,
            return_type,
        }
    } else {
        FnSignature::Generic(GenericScheme {
            type_params: function.type_params.clone(),
            params,
            return_type,
        })
    })
}

fn resolve_implicit_self_functions(
    stmts: &mut [Stmt],
    fn_signatures: &mut HashMap<String, FnSignature>,
) -> Result<(), Diagnostic> {
    let active_type_params = HashSet::new();
    let mut vars: HashMap<String, Binding> = HashMap::new();

    for stmt in stmts.iter_mut() {
        resolve_stmt_implicit_self(stmt, &vars, fn_signatures, &active_type_params)?;
        if let Stmt::FieldAssign {
            base,
            name,
            value: Expr::Function(function),
            ..
        } = stmt
        {
            if let Expr::Name(table, _, _) = base.as_ref() {
                if let Some(signature) = signature_from_function_expr(function) {
                    fn_signatures.insert(method_signature_name(table, name), signature);
                }
            }
        }
        let _ = check_stmt(
            stmt,
            &mut vars,
            fn_signatures,
            &active_type_params,
            &Type::number(),
            false,
        )?;
    }

    Ok(())
}

fn resolve_stmt_implicit_self(
    stmt: &mut Stmt,
    vars: &HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
) -> Result<(), Diagnostic> {
    match stmt {
        Stmt::Match { value, arms, .. } => {
            resolve_expr_implicit_self(value, vars, fn_signatures, active_type_params)?;
            for arm in arms {
                for stmt in &mut arm.body {
                    resolve_stmt_implicit_self(stmt, vars, fn_signatures, active_type_params)?;
                }
            }
            Ok(())
        }
        Stmt::FieldAssign { value, .. }
        | Stmt::Let { value, .. }
        | Stmt::Assign { value, .. }
        | Stmt::Expr(value)
        | Stmt::Return(value) => {
            resolve_expr_implicit_self(value, vars, fn_signatures, active_type_params)
        }
        Stmt::IndexAssign {
            base, index, value, ..
        } => {
            resolve_expr_implicit_self(base, vars, fn_signatures, active_type_params)?;
            resolve_expr_implicit_self(index, vars, fn_signatures, active_type_params)?;
            resolve_expr_implicit_self(value, vars, fn_signatures, active_type_params)
        }
        Stmt::If {
            condition,
            then_body,
            else_body,
        } => {
            resolve_expr_implicit_self(condition, vars, fn_signatures, active_type_params)?;
            for stmt in then_body {
                resolve_stmt_implicit_self(stmt, vars, fn_signatures, active_type_params)?;
            }
            for stmt in else_body {
                resolve_stmt_implicit_self(stmt, vars, fn_signatures, active_type_params)?;
            }
            Ok(())
        }
        Stmt::IfCast {
            value,
            then_body,
            else_body,
            ..
        } => {
            resolve_expr_implicit_self(value, vars, fn_signatures, active_type_params)?;
            for stmt in then_body {
                resolve_stmt_implicit_self(stmt, vars, fn_signatures, active_type_params)?;
            }
            for stmt in else_body {
                resolve_stmt_implicit_self(stmt, vars, fn_signatures, active_type_params)?;
            }
            Ok(())
        }
        Stmt::While { condition, body } => {
            resolve_expr_implicit_self(condition, vars, fn_signatures, active_type_params)?;
            for stmt in body {
                resolve_stmt_implicit_self(stmt, vars, fn_signatures, active_type_params)?;
            }
            Ok(())
        }
        Stmt::Repeat { body, condition } => {
            for stmt in body {
                resolve_stmt_implicit_self(stmt, vars, fn_signatures, active_type_params)?;
            }
            resolve_expr_implicit_self(condition, vars, fn_signatures, active_type_params)
        }
        Stmt::NumericFor {
            start,
            stop,
            step,
            body,
            ..
        } => {
            resolve_expr_implicit_self(start, vars, fn_signatures, active_type_params)?;
            resolve_expr_implicit_self(stop, vars, fn_signatures, active_type_params)?;
            if let Some(step) = step {
                resolve_expr_implicit_self(step, vars, fn_signatures, active_type_params)?;
            }
            for stmt in body {
                resolve_stmt_implicit_self(stmt, vars, fn_signatures, active_type_params)?;
            }
            Ok(())
        }
        Stmt::ForIn { iterator, body, .. } => {
            resolve_expr_implicit_self(iterator, vars, fn_signatures, active_type_params)?;
            for stmt in body {
                resolve_stmt_implicit_self(stmt, vars, fn_signatures, active_type_params)?;
            }
            Ok(())
        }
        Stmt::ReturnMulti(values)
        | Stmt::LetMulti { values, .. }
        | Stmt::AssignMulti { values, .. } => {
            for value in values {
                resolve_expr_implicit_self(value, vars, fn_signatures, active_type_params)?;
            }
            Ok(())
        }
        Stmt::Break | Stmt::Continue => Ok(()),
    }
}

fn resolve_expr_implicit_self(
    expr: &mut Expr,
    vars: &HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
) -> Result<(), Diagnostic> {
    match expr {
        Expr::Function(function) => {
            let mut function_type_params = active_type_params.clone();
            function_type_params.extend(active_type_param_set(&function.type_params));
            if let Some(table_name) = function.implicit_self.clone() {
                let table_ty = vars
                    .get(&table_name)
                    .map(|binding| binding.ty.clone())
                    .ok_or_else(|| Diagnostic::new(format!("unknown name '{table_name}'")))?;
                if function.params.first().map(|param| param.name.as_str()) != Some("self") {
                    return Err(Diagnostic::new("desugared method is missing implicit self"));
                }
                function.params[0].ty = table_ty;
                if function.return_type.is_none() {
                    function.return_type = Some(infer_function_expr_return_type(
                        function,
                        vars,
                        fn_signatures,
                        &function_type_params,
                    )?);
                }
                function.implicit_self = None;
            }
            for stmt in &mut function.body {
                resolve_stmt_implicit_self(stmt, vars, fn_signatures, &function_type_params)?;
            }
            Ok(())
        }
        Expr::Unary { expr, .. } | Expr::Cast { expr, .. } => {
            resolve_expr_implicit_self(expr, vars, fn_signatures, active_type_params)
        }
        Expr::Binary { left, right, .. } => {
            resolve_expr_implicit_self(left, vars, fn_signatures, active_type_params)?;
            resolve_expr_implicit_self(right, vars, fn_signatures, active_type_params)
        }
        Expr::If {
            condition,
            then_expr,
            else_expr,
            ..
        } => {
            resolve_expr_implicit_self(condition, vars, fn_signatures, active_type_params)?;
            resolve_expr_implicit_self(then_expr, vars, fn_signatures, active_type_params)?;
            resolve_expr_implicit_self(else_expr, vars, fn_signatures, active_type_params)
        }
        Expr::Call { callee, args, .. } => {
            resolve_expr_implicit_self(callee, vars, fn_signatures, active_type_params)?;
            for arg in args {
                resolve_expr_implicit_self(arg, vars, fn_signatures, active_type_params)?;
            }
            Ok(())
        }
        Expr::MethodCall { receiver, args, .. } => {
            resolve_expr_implicit_self(receiver, vars, fn_signatures, active_type_params)?;
            for arg in args {
                resolve_expr_implicit_self(arg, vars, fn_signatures, active_type_params)?;
            }
            Ok(())
        }
        Expr::ArrayLiteral { elements, .. } => {
            for element in elements {
                resolve_expr_implicit_self(element, vars, fn_signatures, active_type_params)?;
            }
            Ok(())
        }
        Expr::TableLiteral { fields, .. } => {
            for field in fields {
                resolve_expr_implicit_self(
                    &mut field.value,
                    vars,
                    fn_signatures,
                    active_type_params,
                )?;
            }
            Ok(())
        }
        Expr::Field { base, .. } => {
            resolve_expr_implicit_self(base, vars, fn_signatures, active_type_params)
        }
        Expr::Index { base, index, .. } => {
            resolve_expr_implicit_self(base, vars, fn_signatures, active_type_params)?;
            resolve_expr_implicit_self(index, vars, fn_signatures, active_type_params)
        }
        Expr::Name(..)
        | Expr::Number(..)
        | Expr::Bool(..)
        | Expr::Nil(..)
        | Expr::String(..)
        | Expr::Bytes(..)
        | Expr::Vararg(..)
        | Expr::Require(..)
        | Expr::IsVariant { .. } => Ok(()),
    }
}

fn annotate_resolved_extern_members(
    program: &mut Program,
    fn_signatures: &HashMap<String, FnSignature>,
    module_bindings: &HashMap<String, Binding>,
    reusable: &[bool],
) -> Result<(), Diagnostic> {
    #[cfg(target_family = "wasm")]
    let results = program
        .functions
        .iter_mut()
        .zip(reusable)
        .map(|(function, reusable)| {
            if *reusable {
                Ok(())
            } else {
                annotate_function_resolved_members(function, fn_signatures, module_bindings)
            }
        })
        .collect::<Vec<_>>();
    #[cfg(not(target_family = "wasm"))]
    let results = {
        let workers = std::thread::available_parallelism()
            .map_or(1, std::num::NonZeroUsize::get)
            .min(program.functions.len().max(1));
        let chunk_size = program.functions.len().max(1).div_ceil(workers);
        std::thread::scope(|scope| {
            let handles = program
                .functions
                .chunks_mut(chunk_size)
                .zip(reusable.chunks(chunk_size))
                .map(|(chunk, reusable)| {
                    scope.spawn(move || {
                        chunk
                            .iter_mut()
                            .zip(reusable)
                            .map(|(function, reusable)| {
                                if *reusable {
                                    Ok(())
                                } else {
                                    annotate_function_resolved_members(
                                        function,
                                        fn_signatures,
                                        module_bindings,
                                    )
                                }
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .collect::<Vec<_>>();
            handles
                .into_iter()
                .flat_map(|handle| handle.join().expect("HIR member worker panicked"))
                .collect::<Vec<_>>()
        })
    };
    for result in results {
        result?;
    }
    if let Some(export) = &mut program.export {
        annotate_expr_resolved_members(export, &HashMap::new(), fn_signatures, &HashSet::new())?;
    }
    Ok(())
}

fn annotate_function_resolved_members(
    function: &mut Function,
    fn_signatures: &HashMap<String, FnSignature>,
    module_bindings: &HashMap<String, Binding>,
) -> Result<(), Diagnostic> {
    let active_type_params = active_type_param_set(&function.type_params);
    let mut vars = function_module_bindings(function, module_bindings).clone();
    for param in &function.params {
        vars.insert(
            param.name.clone(),
            binding_for(param.ty.clone(), Rebindability::Rebindable),
        );
    }
    annotate_stmts_resolved_members(
        &mut function.body,
        &mut vars,
        fn_signatures,
        &active_type_params,
    )
}

fn annotate_function_expr_resolved_members(
    function: &mut FunctionExpr,
    parent_vars: &HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    parent_type_params: &HashSet<String>,
) -> Result<(), Diagnostic> {
    let mut active_type_params = parent_type_params.clone();
    active_type_params.extend(active_type_param_set(&function.type_params));
    let mut vars = parent_vars.clone();
    for param in &function.params {
        vars.insert(
            param.name.clone(),
            binding_for(param.ty.clone(), Rebindability::Rebindable),
        );
    }
    annotate_stmts_resolved_members(
        &mut function.body,
        &mut vars,
        fn_signatures,
        &active_type_params,
    )
}

fn annotate_stmts_resolved_members(
    stmts: &mut [Stmt],
    vars: &mut HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
) -> Result<(), Diagnostic> {
    for stmt in stmts {
        annotate_stmt_resolved_members(stmt, vars, fn_signatures, active_type_params)?;
    }
    Ok(())
}

fn annotate_stmt_resolved_members(
    stmt: &mut Stmt,
    vars: &mut HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
) -> Result<(), Diagnostic> {
    match stmt {
        Stmt::Match { value, arms, .. } => {
            annotate_expr_resolved_members(value, vars, fn_signatures, active_type_params)?;
            for arm in arms {
                annotate_stmts_resolved_members(
                    &mut arm.body,
                    &mut vars.clone(),
                    fn_signatures,
                    active_type_params,
                )?;
            }
        }
        Stmt::Let {
            name,
            rebindability,
            ty,
            value,
            ..
        } => {
            annotate_expr_resolved_members(value, vars, fn_signatures, active_type_params)?;
            let inferred_ty = if let Some(expected_ty) = ty {
                infer_expr(
                    value,
                    vars,
                    fn_signatures,
                    active_type_params,
                    Some(expected_ty.clone()),
                )
                .unwrap_or_else(|_| expected_ty.clone())
            } else if matches!(value, Expr::ArrayLiteral { elements, .. } if elements.is_empty()) {
                Type::Record(BTreeMap::new())
            } else {
                infer_expr(value, vars, fn_signatures, active_type_params, None)
                    .unwrap_or(Type::Unknown)
            };
            vars.insert(name.clone(), binding_for(inferred_ty, *rebindability));
        }
        Stmt::Assign { value, .. } | Stmt::Expr(value) | Stmt::Return(value) => {
            annotate_expr_resolved_members(value, vars, fn_signatures, active_type_params)?;
        }
        Stmt::IndexAssign {
            base, index, value, ..
        } => {
            annotate_expr_resolved_members(base, vars, fn_signatures, active_type_params)?;
            annotate_expr_resolved_members(index, vars, fn_signatures, active_type_params)?;
            annotate_expr_resolved_members(value, vars, fn_signatures, active_type_params)?;
        }
        Stmt::FieldAssign {
            base,
            name,
            resolved_name,
            value,
            ..
        } => {
            annotate_expr_resolved_members(base, vars, fn_signatures, active_type_params)?;
            annotate_expr_resolved_members(value, vars, fn_signatures, active_type_params)?;
            if let Ok(base_ty) = infer_expr(base, vars, fn_signatures, active_type_params, None) {
                *resolved_name = resolved_type_property_setter_name(&base_ty, name, fn_signatures);
            }

            if let Expr::Name(base_name, _, _) = base.as_ref() {
                let binding = vars
                    .get(base_name)
                    .cloned()
                    .ok_or_else(|| Diagnostic::new(format!("unknown local '{base_name}'")))?;
                if let Type::Record(mut fields) = binding.ty {
                    let existing_field = fields.get(name).cloned();
                    if let Ok(value_ty) = infer_expr(
                        value,
                        vars,
                        fn_signatures,
                        active_type_params,
                        existing_field.clone(),
                    ) {
                        if existing_field.is_none() && binding.record_open {
                            fields.insert(name.clone(), value_ty);
                        }
                    }
                    let mut updated = binding_for(Type::Record(fields), binding.rebindability);
                    if !binding.record_open {
                        updated.record_open = false;
                    }
                    vars.insert(base_name.clone(), updated);
                }
            }
        }
        Stmt::If {
            condition,
            then_body,
            else_body,
        } => {
            annotate_expr_resolved_members(condition, vars, fn_signatures, active_type_params)?;
            let (mut then_scope, mut else_scope) = narrowed_scopes(condition, vars);
            annotate_stmts_resolved_members(
                then_body,
                &mut then_scope,
                fn_signatures,
                active_type_params,
            )?;
            annotate_stmts_resolved_members(
                else_body,
                &mut else_scope,
                fn_signatures,
                active_type_params,
            )?;
        }
        Stmt::IfCast {
            target_name,
            target_ty,
            binding,
            value,
            then_body,
            else_body,
            ..
        } => {
            annotate_expr_resolved_members(value, vars, fn_signatures, active_type_params)?;
            let branch_scopes = checked_if_cast_scopes(
                target_name,
                target_ty,
                binding,
                value,
                vars,
                fn_signatures,
                active_type_params,
            )?;
            let mut then_scope = branch_scopes.then_scope;
            let mut else_scope = branch_scopes.else_scope;
            annotate_stmts_resolved_members(
                then_body,
                &mut then_scope,
                fn_signatures,
                active_type_params,
            )?;
            annotate_stmts_resolved_members(
                else_body,
                &mut else_scope,
                fn_signatures,
                active_type_params,
            )?;
        }
        Stmt::While { condition, body } => {
            annotate_expr_resolved_members(condition, vars, fn_signatures, active_type_params)?;
            let mut loop_scope = vars.clone();
            annotate_stmts_resolved_members(
                body,
                &mut loop_scope,
                fn_signatures,
                active_type_params,
            )?;
        }
        Stmt::Repeat { body, condition } => {
            let mut loop_scope = vars.clone();
            annotate_stmts_resolved_members(
                body,
                &mut loop_scope,
                fn_signatures,
                active_type_params,
            )?;
            annotate_expr_resolved_members(condition, vars, fn_signatures, active_type_params)?;
        }
        Stmt::NumericFor {
            name,
            start,
            stop,
            step,
            body,
            ..
        } => {
            annotate_expr_resolved_members(start, vars, fn_signatures, active_type_params)?;
            annotate_expr_resolved_members(stop, vars, fn_signatures, active_type_params)?;
            if let Some(step) = step {
                annotate_expr_resolved_members(step, vars, fn_signatures, active_type_params)?;
            }
            let mut loop_scope = vars.clone();
            let mut bounds = vec![&*start, &*stop];
            if let Some(step) = step {
                bounds.push(step);
            }
            if let Ok(loop_ty) = numeric::infer_numeric_for_loop_type(&bounds, |expr, expected| {
                infer_expr(expr, vars, fn_signatures, active_type_params, expected)
            }) {
                loop_scope.insert(name.clone(), binding_for(loop_ty, Rebindability::Const));
            }
            annotate_stmts_resolved_members(
                body,
                &mut loop_scope,
                fn_signatures,
                active_type_params,
            )?;
        }
        Stmt::ForIn {
            names,
            iterator,
            body,
            ..
        } => {
            annotate_expr_resolved_members(iterator, vars, fn_signatures, active_type_params)?;
            let mut loop_scope = vars.clone();
            if let Ok(Type::Array(element_ty)) =
                infer_expr(iterator, vars, fn_signatures, active_type_params, None)
            {
                if names.len() == 1 {
                    loop_scope.insert(
                        names[0].clone(),
                        binding_for(*element_ty, Rebindability::Const),
                    );
                } else if names.len() == 2 {
                    loop_scope.insert(
                        names[0].clone(),
                        binding_for(Type::Numeric(NumericType::I32), Rebindability::Const),
                    );
                    loop_scope.insert(
                        names[1].clone(),
                        binding_for(*element_ty, Rebindability::Const),
                    );
                }
            }
            annotate_stmts_resolved_members(
                body,
                &mut loop_scope,
                fn_signatures,
                active_type_params,
            )?;
        }
        Stmt::Break | Stmt::Continue => {}
        Stmt::ReturnMulti(values)
        | Stmt::LetMulti { values, .. }
        | Stmt::AssignMulti { values, .. } => {
            for value in values {
                annotate_expr_resolved_members(value, vars, fn_signatures, active_type_params)?;
            }
        }
    }
    Ok(())
}

fn annotate_expr_resolved_members(
    expr: &mut Expr,
    vars: &HashMap<String, Binding>,
    fn_signatures: &HashMap<String, FnSignature>,
    active_type_params: &HashSet<String>,
) -> Result<(), Diagnostic> {
    match expr {
        Expr::Unary {
            op,
            expr,
            resolved_name,
            ..
        } => {
            annotate_expr_resolved_members(expr, vars, fn_signatures, active_type_params)?;
            if matches!(op, waluau_ast::UnaryOp::Neg) {
                if let Ok(operand_ty) =
                    infer_expr(expr, vars, fn_signatures, active_type_params, None)
                {
                    *resolved_name = resolve_operator_overload(
                        "__neg",
                        std::slice::from_ref(&operand_ty),
                        fn_signatures,
                    )?
                    .map(|(name, _)| name);
                }
            }
        }
        Expr::Cast { expr, .. } | Expr::IsVariant { expr, .. } => {
            annotate_expr_resolved_members(expr, vars, fn_signatures, active_type_params)?
        }
        Expr::Binary {
            op,
            left,
            right,
            resolved_name,
            ..
        } => {
            annotate_expr_resolved_members(left, vars, fn_signatures, active_type_params)?;
            annotate_expr_resolved_members(right, vars, fn_signatures, active_type_params)?;
            let method = match op {
                waluau_ast::BinaryOp::Add => Some("__add"),
                waluau_ast::BinaryOp::Sub => Some("__sub"),
                waluau_ast::BinaryOp::Mul => Some("__mul"),
                waluau_ast::BinaryOp::Div => Some("__div"),
                _ => None,
            };
            if let Some(method) = method {
                if let (Ok(left_ty), Ok(right_ty)) = (
                    infer_expr(left, vars, fn_signatures, active_type_params, None),
                    infer_expr(right, vars, fn_signatures, active_type_params, None),
                ) {
                    *resolved_name =
                        resolve_operator_overload(method, &[left_ty, right_ty], fn_signatures)?
                            .map(|(name, _)| name);
                }
            }
        }
        Expr::If {
            condition,
            then_expr,
            else_expr,
            ..
        } => {
            annotate_expr_resolved_members(condition, vars, fn_signatures, active_type_params)?;
            annotate_expr_resolved_members(then_expr, vars, fn_signatures, active_type_params)?;
            annotate_expr_resolved_members(else_expr, vars, fn_signatures, active_type_params)?;
        }
        Expr::Call { callee, args, .. } => {
            let builtin_callee = is_builtin_callee(callee);
            if !builtin_callee {
                annotate_expr_resolved_members(callee, vars, fn_signatures, active_type_params)?;
            }
            for arg in args.iter_mut() {
                annotate_expr_resolved_members(arg, vars, fn_signatures, active_type_params)?;
            }
            // Direct calls to overloaded declared imports are rewritten to
            // the selected overload's unique internal name so IR lowering can
            // resolve the host import without re-running overload selection.
            if !builtin_callee {
                if let Expr::Name(name, _, _) = callee.as_mut() {
                    if vars.get(name).is_none() {
                        if let Some(FnSignature::Overloaded(variants)) =
                            fn_signatures.get(name.as_str())
                        {
                            let chosen = select_overload(
                                name,
                                variants,
                                None,
                                args,
                                vars,
                                fn_signatures,
                                active_type_params,
                            )?;
                            *name = chosen.name.clone();
                        }
                    }
                }
                // Namespaced declared imports called through a field
                // expression (`math.abs(x)`): rewrite the callee to a plain
                // name so symbol resolution binds it to the host import. For
                // overload sets the selected variant's unique internal name
                // is used; the mangle scheme composes with the dotted name
                // (`math.abs` -> `math.abs$overload0`).
                if let Expr::Field {
                    base,
                    name: field,
                    span,
                    ..
                } = callee.as_mut()
                {
                    if let Expr::Name(base_name, _, _) = base.as_ref() {
                        if vars.get(base_name).is_none() {
                            let dotted = format!("{base_name}.{field}");
                            let rewritten = match fn_signatures.get(&dotted) {
                                Some(FnSignature::Overloaded(variants)) => {
                                    let chosen = select_overload(
                                        &dotted,
                                        variants,
                                        None,
                                        args,
                                        vars,
                                        fn_signatures,
                                        active_type_params,
                                    )?;
                                    Some(chosen.name.clone())
                                }
                                Some(FnSignature::Mono { .. }) => Some(dotted),
                                _ => None,
                            };
                            if let Some(name) = rewritten {
                                let span = *span;
                                **callee = Expr::Name(name, None, span);
                            }
                        }
                    }
                }
            }
        }
        Expr::MethodCall {
            receiver,
            name,
            resolved_name,
            args,
            ..
        } => {
            annotate_expr_resolved_members(receiver, vars, fn_signatures, active_type_params)?;
            for arg in args.iter_mut() {
                annotate_expr_resolved_members(arg, vars, fn_signatures, active_type_params)?;
            }
            if let Ok(receiver_ty) =
                infer_expr(receiver, vars, fn_signatures, active_type_params, None)
            {
                *resolved_name = resolved_type_method_call_name(
                    &receiver_ty,
                    name,
                    args,
                    vars,
                    fn_signatures,
                    active_type_params,
                )?;
            }
        }
        Expr::Function(function) => {
            annotate_function_expr_resolved_members(
                function,
                vars,
                fn_signatures,
                active_type_params,
            )?;
        }
        Expr::ArrayLiteral { elements, .. } => {
            for element in elements {
                annotate_expr_resolved_members(element, vars, fn_signatures, active_type_params)?;
            }
        }
        Expr::TableLiteral { fields, .. } => {
            for field in fields {
                annotate_expr_resolved_members(
                    &mut field.value,
                    vars,
                    fn_signatures,
                    active_type_params,
                )?;
            }
        }
        Expr::Field {
            base,
            name,
            resolved_name,
            ..
        } => {
            annotate_expr_resolved_members(base, vars, fn_signatures, active_type_params)?;
            if let Ok(base_ty) = infer_expr(base, vars, fn_signatures, active_type_params, None) {
                *resolved_name = resolved_type_property_getter_name(&base_ty, name, fn_signatures);
            }
        }
        Expr::Index { base, index, .. } => {
            annotate_expr_resolved_members(base, vars, fn_signatures, active_type_params)?;
            annotate_expr_resolved_members(index, vars, fn_signatures, active_type_params)?;
        }
        Expr::Number(..)
        | Expr::Bool(..)
        | Expr::Nil(..)
        | Expr::String(..)
        | Expr::Bytes(..)
        | Expr::Vararg(..)
        | Expr::Name(..)
        | Expr::Require(..) => {}
    }
    Ok(())
}

pub fn type_check(program: &Program) -> Result<(), Diagnostic> {
    let _ = type_check_and_infer(program)?;
    Ok(())
}

/// Annotates unannotated `string.gsub` function replacements before type
/// inference runs: parameters take the pattern's capture types (whole match
/// when the pattern has no captures), and a missing return type becomes
/// `string` when the body returns a value or `unit` for purely procedural
/// replacers. Both HIR body checking and IR lowering then see fully typed
/// replacement lambdas.
fn fill_gsub_replacement_annotations(program: &mut Program) {
    for function in &mut program.functions {
        fill_gsub_annotations_in_stmts(&mut function.body);
    }
    fill_gsub_annotations_in_stmts(&mut program.top_level);
}

fn fill_gsub_annotations_in_stmts(stmts: &mut [Stmt]) {
    for stmt in stmts {
        match stmt {
            Stmt::Match { value, arms, .. } => {
                fill_gsub_annotations_in_expr(value);
                for arm in arms {
                    fill_gsub_annotations_in_stmts(&mut arm.body);
                }
            }
            Stmt::Let { value, .. }
            | Stmt::Assign { value, .. }
            | Stmt::Return(value)
            | Stmt::Expr(value) => fill_gsub_annotations_in_expr(value),
            Stmt::FieldAssign { base, value, .. } => {
                fill_gsub_annotations_in_expr(base);
                fill_gsub_annotations_in_expr(value);
            }
            Stmt::IndexAssign {
                base, index, value, ..
            } => {
                fill_gsub_annotations_in_expr(base);
                fill_gsub_annotations_in_expr(index);
                fill_gsub_annotations_in_expr(value);
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                fill_gsub_annotations_in_expr(condition);
                fill_gsub_annotations_in_stmts(then_body);
                fill_gsub_annotations_in_stmts(else_body);
            }
            Stmt::IfCast {
                value,
                then_body,
                else_body,
                ..
            } => {
                fill_gsub_annotations_in_expr(value);
                fill_gsub_annotations_in_stmts(then_body);
                fill_gsub_annotations_in_stmts(else_body);
            }
            Stmt::While { condition, body } => {
                fill_gsub_annotations_in_expr(condition);
                fill_gsub_annotations_in_stmts(body);
            }
            Stmt::Repeat { body, condition } => {
                fill_gsub_annotations_in_stmts(body);
                fill_gsub_annotations_in_expr(condition);
            }
            Stmt::NumericFor {
                start,
                stop,
                step,
                body,
                ..
            } => {
                fill_gsub_annotations_in_expr(start);
                fill_gsub_annotations_in_expr(stop);
                if let Some(step) = step {
                    fill_gsub_annotations_in_expr(step);
                }
                fill_gsub_annotations_in_stmts(body);
            }
            Stmt::ForIn { iterator, body, .. } => {
                fill_gsub_annotations_in_expr(iterator);
                fill_gsub_annotations_in_stmts(body);
            }
            Stmt::ReturnMulti(values)
            | Stmt::LetMulti { values, .. }
            | Stmt::AssignMulti { values, .. } => {
                for value in values {
                    fill_gsub_annotations_in_expr(value);
                }
            }
            Stmt::Break | Stmt::Continue => {}
        }
    }
}

fn fill_gsub_annotations_in_expr(expr: &mut Expr) {
    match expr {
        Expr::Call { callee, args, .. } => {
            fill_gsub_annotations_in_expr(callee);
            let is_gsub = matches!(
                callee.as_ref(),
                Expr::Field { base, name, .. }
                    if name == "gsub" && matches!(base.as_ref(), Expr::Name(ns, _, _) if ns == "string")
            );
            if is_gsub && args.len() >= 3 {
                let (head, tail) = args.split_at_mut(2);
                annotate_gsub_replacement(&head[1], &mut tail[0]);
            }
            for arg in args {
                fill_gsub_annotations_in_expr(arg);
            }
        }
        Expr::MethodCall {
            receiver,
            name,
            args,
            ..
        } => {
            fill_gsub_annotations_in_expr(receiver);
            if name == "gsub" && args.len() >= 2 {
                let (head, tail) = args.split_at_mut(1);
                annotate_gsub_replacement(&head[0], &mut tail[0]);
            }
            for arg in args {
                fill_gsub_annotations_in_expr(arg);
            }
        }
        Expr::Unary { expr, .. } | Expr::Cast { expr, .. } | Expr::IsVariant { expr, .. } => {
            fill_gsub_annotations_in_expr(expr);
        }
        Expr::Binary { left, right, .. } => {
            fill_gsub_annotations_in_expr(left);
            fill_gsub_annotations_in_expr(right);
        }
        Expr::If {
            condition,
            then_expr,
            else_expr,
            ..
        } => {
            fill_gsub_annotations_in_expr(condition);
            fill_gsub_annotations_in_expr(then_expr);
            fill_gsub_annotations_in_expr(else_expr);
        }
        Expr::Function(function) => {
            fill_gsub_annotations_in_stmts(&mut function.body);
        }
        Expr::ArrayLiteral { elements, .. } => {
            for element in elements {
                fill_gsub_annotations_in_expr(element);
            }
        }
        Expr::TableLiteral { fields, .. } => {
            for field in fields {
                fill_gsub_annotations_in_expr(&mut field.value);
            }
        }
        Expr::Field { base, .. } => fill_gsub_annotations_in_expr(base),
        Expr::Index { base, index, .. } => {
            fill_gsub_annotations_in_expr(base);
            fill_gsub_annotations_in_expr(index);
        }
        Expr::Number(..)
        | Expr::Bool(..)
        | Expr::Nil(..)
        | Expr::String(..)
        | Expr::Bytes(..)
        | Expr::Name(..)
        | Expr::Vararg(..)
        | Expr::Require(..) => {}
    }
}

fn annotate_gsub_replacement(pattern_arg: &Expr, repl: &mut Expr) {
    let Expr::Function(function) = repl else {
        return;
    };
    let captures = waluau_ast::expr_pattern_captures(pattern_arg);
    let param_tys: Vec<Type> = if captures.is_empty() {
        vec![Type::String]
    } else {
        captures.iter().map(|kind| kind.value_type()).collect()
    };
    for (param, ty) in function.params.iter_mut().zip(param_tys) {
        if param.ty == Type::Unknown {
            param.ty = ty;
        }
    }
    if function.return_type.is_none() {
        function.return_type = Some(if stmts_contain_value_return(&function.body) {
            Type::String
        } else {
            Type::Unit
        });
    }
}

/// Whether a statement list contains a value-producing `return` (without
/// descending into nested function expressions).
fn stmts_contain_value_return(stmts: &[Stmt]) -> bool {
    stmts.iter().any(|stmt| match stmt {
        Stmt::Return(_) | Stmt::ReturnMulti(_) => true,
        Stmt::If {
            then_body,
            else_body,
            ..
        }
        | Stmt::IfCast {
            then_body,
            else_body,
            ..
        } => stmts_contain_value_return(then_body) || stmts_contain_value_return(else_body),
        Stmt::While { body, .. }
        | Stmt::Repeat { body, .. }
        | Stmt::NumericFor { body, .. }
        | Stmt::ForIn { body, .. } => stmts_contain_value_return(body),
        _ => false,
    })
}

/// Group `declare function` entries that share a source-level name into
/// overload sets.
///
/// - Textually identical re-declarations (same parameter types, return type,
///   and host name — common when several modules declare the same extern) are
///   deduplicated, keeping the first occurrence.
/// - Declarations with identical parameter types but conflicting return
///   types or host names are rejected.
/// - Genuine overloads (differing arity or parameter types) are renamed to
///   unique internal names (`base$overloadN`, in declaration order) while
///   their host names stay untouched, so each overload becomes its own host
///   import under the shared external name.
///
/// Returns the overload sets keyed by base name; the variants reference the
/// unique internal names the imports were renamed to.
fn disambiguate_declared_import_overloads(
    program: &mut Program,
) -> Result<HashMap<String, Vec<OverloadVariant>>, Diagnostic> {
    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    for (index, declared) in program.declared_imports.iter().enumerate() {
        match groups.iter_mut().find(|(name, _)| *name == declared.name) {
            Some((_, indices)) => indices.push(index),
            None => groups.push((declared.name.clone(), vec![index])),
        }
    }

    let mut removed: HashSet<usize> = HashSet::new();
    let mut overload_sets = HashMap::new();
    for (name, indices) in groups {
        if indices.len() == 1 {
            continue;
        }
        let mut kept: Vec<usize> = Vec::new();
        for index in indices {
            let candidate = &program.declared_imports[index];
            let duplicate_of = kept.iter().copied().find(|&kept_index| {
                let existing = &program.declared_imports[kept_index];
                existing.params.len() == candidate.params.len()
                    && existing
                        .params
                        .iter()
                        .zip(candidate.params.iter())
                        .all(|(a, b)| a.ty == b.ty)
            });
            match duplicate_of {
                Some(kept_index) => {
                    let existing = &program.declared_imports[kept_index];
                    if existing.return_type == candidate.return_type
                        && existing.host_name == candidate.host_name
                    {
                        // Exact re-declaration (e.g. the same extern declared
                        // by several modules): keep the first occurrence.
                        removed.insert(index);
                    } else {
                        return Err(Diagnostic::new(format!(
                            "conflicting declarations of host function '{name}': overloads must \
                             differ in parameter types, but two declarations share the parameter \
                             list and disagree on the return type or host name"
                        )));
                    }
                }
                None => kept.push(index),
            }
        }
        if kept.len() < 2 {
            continue;
        }
        let mut variants = Vec::with_capacity(kept.len());
        for (position, index) in kept.into_iter().enumerate() {
            let declared = &mut program.declared_imports[index];
            declared.name = waluau_ast::overload_variant_name(&name, position);
            variants.push(OverloadVariant {
                name: declared.name.clone(),
                params: declared
                    .params
                    .iter()
                    .map(|param| param.ty.clone())
                    .collect(),
                return_type: declared.return_type.clone(),
            });
        }
        overload_sets.insert(name, variants);
    }

    if !removed.is_empty() {
        let mut index = 0;
        program.declared_imports.retain(|_| {
            let keep = !removed.contains(&index);
            index += 1;
            keep
        });
    }

    Ok(overload_sets)
}

/// Drops exact re-declarations of a constant (e.g. the same builtin declared
/// by several merged modules) and rejects conflicting ones.
fn dedupe_declared_constants(program: &mut Program) -> Result<(), Diagnostic> {
    let mut seen: HashMap<String, waluau_ast::DeclaredConstant> = HashMap::new();
    let mut conflict = None;
    program
        .declared_constants
        .retain(|constant| match seen.get(&constant.name) {
            Some(existing) => {
                if existing != constant && conflict.is_none() {
                    conflict = Some(constant.name.clone());
                }
                false
            }
            None => {
                seen.insert(constant.name.clone(), constant.clone());
                true
            }
        });
    match conflict {
        Some(name) => Err(Diagnostic::new(format!(
            "conflicting declarations of constant '{name}'"
        ))),
        None => Ok(()),
    }
}

pub fn type_check_and_infer(program: &Program) -> Result<Program, Diagnostic> {
    type_check_and_infer_collect(program).map_err(|mut errors| errors.remove(0))
}

#[derive(Default)]
pub struct TypeCheckCache {
    prepared: Option<Program>,
    typed: Option<Program>,
    reused_functions: usize,
    changed_functions: Vec<usize>,
}

impl TypeCheckCache {
    pub fn reused_function_count(&self) -> usize {
        self.reused_functions
    }

    pub fn changed_functions(&self) -> &[usize] {
        &self.changed_functions
    }
}

fn incremental_context_matches(current: &Program, previous: &Program) -> bool {
    current.declared_imports == previous.declared_imports
        && current.declared_constants == previous.declared_constants
        && current.type_declarations == previous.type_declarations
        && current.top_level == previous.top_level
        && current.export == previous.export
        && current.entry_file_path == previous.entry_file_path
        && current.functions.len() == previous.functions.len()
        && current
            .functions
            .iter()
            .zip(&previous.functions)
            .all(|(current, previous)| {
                current.name == previous.name
                    && current.type_params == previous.type_params
                    && current.params == previous.params
                    && current.vararg == previous.vararg
                    && current.return_type == previous.return_type
                    && current.file_path == previous.file_path
            })
}

/// Type check, collecting a diagnostic per independently-failing function or
/// statement instead of stopping at the first. Whole-program phases (type
/// resolution, desugaring, signature construction) remain fail-fast; the
/// per-function inference and checking passes collect and continue.
pub fn type_check_and_infer_collect(program: &Program) -> Result<Program, Vec<Diagnostic>> {
    type_check_and_infer_collect_inner(program, None).map(|(typed, _)| typed)
}

pub fn type_check_and_infer_collect_cached<'a>(
    program: &Program,
    cache: &'a mut TypeCheckCache,
) -> Result<(&'a Program, &'a [usize]), Vec<Diagnostic>> {
    let (typed, prepared) = type_check_and_infer_collect_inner(program, Some(cache))?;
    if let Some(prepared) = prepared {
        cache.prepared = Some(prepared);
    }
    cache.typed = Some(typed);
    Ok((
        cache.typed.as_ref().expect("cached typed program"),
        &cache.changed_functions,
    ))
}

fn type_check_and_infer_collect_inner(
    program: &Program,
    mut cache: Option<&mut TypeCheckCache>,
) -> Result<(Program, Option<Program>), Vec<Diagnostic>> {
    let started = CompilerTimer::start();
    let mut typed = desugar_method_declarations(program).map_err(|error| vec![error])?;
    desugar_implicit_top_level_declarations(&mut typed);
    let desugared_at = started.elapsed();
    resolve_program_types(&mut typed).map_err(|error| vec![error])?;
    let types_at = started.elapsed();
    let overload_sets =
        disambiguate_declared_import_overloads(&mut typed).map_err(|error| vec![error])?;
    fill_gsub_replacement_annotations(&mut typed);
    if !typed.top_level.is_empty() {
        typed.functions.push(Function {
            name: FunctionName::Simple("__waluau_top_level_init".to_string()),
            symbol_id: None,
            type_params: Vec::new(),
            params: Vec::new(),
            vararg: false,
            return_type: Some(Type::Numeric(NumericType::I32)),
            body: {
                let mut body = typed.top_level.clone();
                body.push(Stmt::Return(Expr::Number(
                    NumberLiteral { raw: "0".into() },
                    None,
                )));
                body
            },
            file_path: typed.entry_file_path.clone(),
        });
    }

    let mut prepared = None;
    let mut reusable = vec![false; typed.functions.len()];
    let reusable_from_cache = cache.as_deref().and_then(|cache| {
        let previous_prepared = cache.prepared.as_ref()?;
        let previous_typed = cache.typed.as_ref()?;
        if !incremental_context_matches(&typed, previous_prepared) {
            return None;
        }
        let reusable = typed
            .functions
            .iter()
            .zip(&previous_prepared.functions)
            .map(|(current, previous)| current == previous)
            .collect::<Vec<_>>();
        typed
            .functions
            .iter()
            .zip(&previous_prepared.functions)
            .all(|(current, previous)| current == previous || current.return_type.is_some())
            .then_some((reusable, previous_typed.functions.len()))
    });
    if let Some((cached_reusable, cached_function_count)) = reusable_from_cache
        && cached_function_count == typed.functions.len()
    {
        reusable = cached_reusable;
        let cache = cache.as_deref_mut().expect("HIR cache");
        let mut cached_typed = cache.typed.take().expect("cached typed program");
        let previous_prepared = cache.prepared.as_mut().expect("cached prepared program");
        for (index, is_reusable) in reusable.iter().copied().enumerate() {
            if !is_reusable {
                previous_prepared.functions[index] = typed.functions[index].clone();
                cached_typed.functions[index] = typed.functions[index].clone();
            }
        }
        previous_prepared.sources = typed.sources.clone();
        cached_typed.sources = typed.sources.clone();
        typed = cached_typed;
    } else if cache.is_some() {
        prepared = Some(typed.clone());
    }
    if let Some(cache) = cache {
        cache.reused_functions = reusable.iter().filter(|reused| **reused).count();
        cache.changed_functions = reusable
            .iter()
            .enumerate()
            .filter_map(|(index, reused)| (!reused).then_some(index))
            .collect();
    }
    let reused_at = started.elapsed();

    let mut fn_signatures: HashMap<String, FnSignature> = HashMap::new();
    for declared in &typed.declared_imports {
        fn_signatures.insert(
            declared.name.clone(),
            FnSignature::Mono {
                params: declared
                    .params
                    .iter()
                    .map(|param| param.ty.clone())
                    .collect(),
                vararg: false,
                return_type: declared.return_type.clone(),
            },
        );
    }
    // Overloaded declared imports keep an overload-set entry under their
    // source-level base name; calls select a variant from the argument types.
    for (base_name, variants) in overload_sets {
        fn_signatures.insert(base_name, FnSignature::Overloaded(variants));
    }
    dedupe_declared_constants(&mut typed).map_err(|error| vec![error])?;
    for constant in &typed.declared_constants {
        if fn_signatures.contains_key(&constant.name) {
            return Err(vec![Diagnostic::new(format!(
                "declared constant '{}' conflicts with a declared host function of the same name",
                constant.name
            ))]);
        }
        fn_signatures.insert(
            constant.name.clone(),
            FnSignature::Const {
                ty: constant.ty.clone(),
            },
        );
    }
    for function in &typed.functions {
        if function.type_params.is_empty() {
            if let Some(ret) = &function.return_type {
                fn_signatures.insert(
                    function.name.to_string(),
                    FnSignature::Mono {
                        params: function
                            .params
                            .iter()
                            .map(|param| param.ty.clone())
                            .collect(),
                        vararg: function.vararg,
                        return_type: ret.clone(),
                    },
                );
            }
        } else if let Some(ret) = &function.return_type {
            fn_signatures.insert(
                function.name.to_string(),
                FnSignature::Generic(GenericScheme {
                    type_params: function.type_params.clone(),
                    params: function
                        .params
                        .iter()
                        .map(|param| param.ty.clone())
                        .collect(),
                    return_type: ret.clone(),
                }),
            );
        }
    }

    let mut module_bindings = collect_module_bindings(&typed.top_level, &fn_signatures)
        .map_err(|error| vec![error.with_file_path_if_missing(typed.entry_file_path.clone())])?;

    let mut unresolved: Vec<usize> = typed
        .functions
        .iter()
        .enumerate()
        .filter_map(|(idx, function)| {
            (function.return_type.is_none() && function.type_params.is_empty()).then_some(idx)
        })
        .collect();

    let mut errors: Vec<Diagnostic> = Vec::new();
    // Functions whose return-type inference failed: they get a permissive
    // `unknown` return signature so callers and siblings keep checking
    // without cascades, and are skipped by the checking pass below.
    let mut errored_functions: HashSet<String> = HashSet::new();

    while !unresolved.is_empty() {
        let mut progressed = false;
        let mut next_unresolved = Vec::new();
        let unresolved_names: Vec<String> = unresolved
            .iter()
            .map(|idx| typed.functions[*idx].name.to_string())
            .collect();
        for idx in unresolved {
            let function = &typed.functions[idx];
            let function_name = function.name.to_string();
            let function_vararg = function.vararg;
            let function_file_path = function.file_path.clone();
            let function_params: Vec<Type> = function
                .params
                .iter()
                .map(|param| param.ty.clone())
                .collect();
            match infer_top_level_function_return_type(
                function,
                &fn_signatures,
                &unresolved_names,
                function_module_bindings(function, &module_bindings),
            ) {
                Ok(Some(ret)) => {
                    typed.functions[idx].return_type = Some(ret.clone());
                    fn_signatures.insert(
                        function_name,
                        FnSignature::Mono {
                            params: function_params,
                            vararg: function_vararg,
                            return_type: ret,
                        },
                    );
                    progressed = true;
                }
                Ok(None) => next_unresolved.push(idx),
                Err(error) => {
                    errors.push(error.with_file_path_if_missing(function_file_path));
                    errored_functions.insert(function_name.clone());
                    typed.functions[idx].return_type = Some(Type::Unknown);
                    fn_signatures.insert(
                        function_name,
                        FnSignature::Mono {
                            params: function_params,
                            vararg: function_vararg,
                            return_type: Type::Unknown,
                        },
                    );
                    progressed = true;
                }
            }
        }
        if !progressed {
            let name = &typed.functions[next_unresolved[0]].name;
            errors.push(inference_diagnostic(
                "inference/unsupported",
                DiagnosticCategory::Unsupported,
                format!("cannot infer return type for recursive or cyclic function '{name}'"),
                "add an explicit return type annotation to break the cycle",
            ));
            return Err(errors);
        }
        unresolved = next_unresolved;
    }

    module_bindings = collect_module_bindings(&typed.top_level, &fn_signatures)
        .map_err(|error| vec![error.with_file_path_if_missing(typed.entry_file_path.clone())])?;

    if let Some(top_level_init) = typed
        .functions
        .iter_mut()
        .find(|function| function.name.to_string() == "__waluau_top_level_init")
    {
        let file_path = top_level_init.file_path.clone();
        if let Err(error) =
            resolve_implicit_self_functions(&mut top_level_init.body, &mut fn_signatures)
        {
            errors.push(error.with_file_path_if_missing(file_path));
        }
    }

    let prepared_at = started.elapsed();

    #[cfg(target_family = "wasm")]
    let checked_errors = typed
        .functions
        .iter()
        .zip(&reusable)
        .flat_map(|(function, reusable)| {
            if *reusable || errored_functions.contains(&function.name.to_string()) {
                Vec::new()
            } else {
                statements::check_function_collect(
                    function,
                    &fn_signatures,
                    &HashSet::new(),
                    function_module_bindings(function, &module_bindings),
                )
                .into_iter()
                .map(|error| error.with_file_path_if_missing(function.file_path.clone()))
                .collect()
            }
        })
        .collect::<Vec<_>>();
    #[cfg(not(target_family = "wasm"))]
    let (checked_errors, chunk_size) = {
        let workers = std::thread::available_parallelism()
            .map_or(1, std::num::NonZeroUsize::get)
            .min(typed.functions.len().max(1));
        let chunk_size = typed.functions.len().max(1).div_ceil(workers);
        let checked_errors = std::thread::scope(|scope| {
            let handles = typed
                .functions
                .chunks(chunk_size)
                .zip(reusable.chunks(chunk_size))
                .map(|(chunk, reusable)| {
                    let errored_functions = &errored_functions;
                    let fn_signatures = &fn_signatures;
                    let module_bindings = &module_bindings;
                    scope.spawn(move || {
                        let mut diagnostics = Vec::new();
                        for (function, reusable) in chunk.iter().zip(reusable) {
                            if *reusable {
                                continue;
                            }
                            if errored_functions.contains(&function.name.to_string()) {
                                continue;
                            }
                            diagnostics.extend(
                                statements::check_function_collect(
                                    function,
                                    fn_signatures,
                                    &HashSet::new(),
                                    function_module_bindings(function, module_bindings),
                                )
                                .into_iter()
                                .map(|error| {
                                    error.with_file_path_if_missing(function.file_path.clone())
                                }),
                            );
                        }
                        diagnostics
                    })
                })
                .collect::<Vec<_>>();
            handles
                .into_iter()
                .flat_map(|handle| handle.join().expect("HIR checking worker panicked"))
                .collect::<Vec<_>>()
        });
        (checked_errors, chunk_size)
    };
    errors.extend(checked_errors);
    if !errors.is_empty() {
        return Err(errors);
    }

    let checked = started.elapsed();

    #[cfg(target_family = "wasm")]
    let inferred_results = typed
        .functions
        .iter_mut()
        .zip(&reusable)
        .map(|(function, reusable)| {
            if *reusable {
                return Ok(());
            }
            let mut vars = function_module_bindings(function, &module_bindings).clone();
            vars.extend(function.params.iter().map(|param| {
                (
                    param.name.clone(),
                    binding_for(param.ty.clone(), Rebindability::Const),
                )
            }));
            let active = active_type_param_set(&function.type_params);
            annotate_inferred_stmt_locals(&mut function.body, &mut vars, &fn_signatures, &active)
                .map_err(|error| error.with_file_path_if_missing(function.file_path.clone()))
        })
        .collect::<Vec<_>>();
    #[cfg(not(target_family = "wasm"))]
    let inferred_results = std::thread::scope(|scope| {
        let handles = typed
            .functions
            .chunks_mut(chunk_size)
            .zip(reusable.chunks(chunk_size))
            .map(|(chunk, reusable)| {
                let fn_signatures = &fn_signatures;
                let module_bindings = &module_bindings;
                scope.spawn(move || {
                    chunk
                        .iter_mut()
                        .zip(reusable)
                        .map(|(function, reusable)| {
                            if *reusable {
                                return Ok(());
                            }
                            let mut vars =
                                function_module_bindings(function, module_bindings).clone();
                            vars.extend(function.params.iter().map(|param| {
                                (
                                    param.name.clone(),
                                    binding_for(param.ty.clone(), Rebindability::Const),
                                )
                            }));
                            let active = active_type_param_set(&function.type_params);
                            annotate_inferred_stmt_locals(
                                &mut function.body,
                                &mut vars,
                                fn_signatures,
                                &active,
                            )
                            .map_err(|error| {
                                error.with_file_path_if_missing(function.file_path.clone())
                            })
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().expect("HIR inference worker panicked"))
            .collect::<Vec<_>>()
    });
    for result in inferred_results {
        result.map_err(|error| vec![error])?;
    }

    let inferred = started.elapsed();

    annotate_resolved_extern_members(&mut typed, &fn_signatures, &module_bindings, &reusable)
        .map_err(|error| vec![error])?;

    if CompilerTimer::enabled() {
        eprintln!(
            "waluau hir timings: prepare={:?} check={:?} inferred={:?} members={:?}",
            prepared_at,
            checked - prepared_at,
            inferred - checked,
            started.elapsed() - inferred,
        );
        eprintln!(
            "waluau hir prepare detail: desugar={:?} types={:?} clone+reuse={:?} signatures={:?}",
            desugared_at,
            types_at - desugared_at,
            reused_at - types_at,
            prepared_at - reused_at,
        );
    }

    Ok((typed, prepared))
}

#[cfg(test)]
mod tests;
