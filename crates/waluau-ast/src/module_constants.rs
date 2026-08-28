use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;

use crate::{BinaryOp, Expr, Program, Rebindability, Span, Stmt, TableField, Type, UnaryOp};

/// A module constant initializer that cannot be safely duplicated at each use.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModuleConstantError {
    message: String,
    span: Option<Span>,
}

impl ModuleConstantError {
    pub fn span(&self) -> Option<Span> {
        self.span
    }
}

impl fmt::Display for ModuleConstantError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ModuleConstantError {}

/// Collect top-level const bindings as expressions that a module linker can
/// inline at each use. Numeric arithmetic may refer to earlier constants;
/// calls, indexing, and all other potentially effectful forms are rejected.
pub fn collect_module_constants(
    program: &Program,
) -> Result<HashMap<String, Expr>, ModuleConstantError> {
    if let Some(cycle) = module_constant_cycle(program) {
        let first = cycle.first().map(String::as_str);
        let span = program.top_level.iter().find_map(|stmt| match stmt {
            Stmt::Let { name, value, .. } if Some(name.as_str()) == first => value.span(),
            _ => None,
        });
        return Err(ModuleConstantError {
            message: format!(
                "top-level const cycle detected: {}",
                cycle
                    .iter()
                    .map(|name| format!("'{name}'"))
                    .collect::<Vec<_>>()
                    .join(" -> ")
            ),
            span,
        });
    }

    let mut constants = HashMap::new();
    let type_declarations = program
        .type_declarations
        .iter()
        .map(|declaration| (declaration.name.as_str(), &declaration.ty))
        .collect::<HashMap<_, _>>();
    for stmt in &program.top_level {
        let Stmt::Let {
            name,
            rebindability: Rebindability::Const,
            ty,
            value,
            ..
        } = stmt
        else {
            continue;
        };
        let resolved_ty = ty
            .as_ref()
            .map(|ty| resolve_constant_type(ty, &type_declarations, &mut HashSet::new()));
        let expression = constant_expression(resolved_ty.as_ref(), value, &constants).ok_or_else(
            || ModuleConstantError {
                message: format!(
                    "top-level const '{name}' initializer must be a side-effect-free expression over literals and earlier constants"
                ),
                span: value.span(),
            },
        )?;
        constants.insert(name.clone(), expression);
    }
    Ok(constants)
}

fn constant_expression(
    ty: Option<&Type>,
    value: &Expr,
    constants: &HashMap<String, Expr>,
) -> Option<Expr> {
    match value {
        Expr::Number(..) => typed_numeric_expression(ty, value),
        Expr::Unary {
            op: UnaryOp::Neg,
            expr,
            resolved_name,
            span,
        } => {
            let expr = constant_expression(ty, expr, constants)?;
            is_numeric_constant_expression(&expr).then(|| Expr::Unary {
                op: UnaryOp::Neg,
                expr: Box::new(expr),
                resolved_name: resolved_name.clone(),
                span: *span,
            })
        }
        Expr::Binary {
            op,
            left,
            right,
            resolved_name,
            span,
        } if is_constant_arithmetic(*op) => {
            let left = constant_expression(ty, left, constants)?;
            let right = constant_expression(ty, right, constants)?;
            (is_numeric_constant_expression(&left) && is_numeric_constant_expression(&right)).then(
                || Expr::Binary {
                    op: *op,
                    left: Box::new(left),
                    right: Box::new(right),
                    resolved_name: resolved_name.clone(),
                    span: *span,
                },
            )
        }
        Expr::Bool(..) | Expr::String(..) | Expr::Bytes(..) | Expr::Nil(..) => Some(value.clone()),
        Expr::Cast { expr, ty, span } => Some(Expr::Cast {
            expr: Box::new(constant_expression(Some(ty), expr, constants)?),
            ty: ty.clone(),
            span: *span,
        }),
        Expr::Name(name, _, _) => constants.get(name).cloned(),
        Expr::TableLiteral { fields, span } => {
            let expected_fields = match ty {
                Some(Type::Record(fields)) => Some(fields),
                _ => None,
            };
            let fields = fields
                .iter()
                .map(|field| {
                    Some(TableField {
                        name: field.name.clone(),
                        value: constant_expression(
                            expected_fields.and_then(|fields| fields.get(&field.name)),
                            &field.value,
                            constants,
                        )?,
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            typed_aggregate_expression(
                Expr::TableLiteral {
                    fields,
                    span: *span,
                },
                ty,
            )
        }
        Expr::ArrayLiteral { elements, span } => {
            let element_ty = match ty {
                Some(Type::Array(element_ty)) => Some(element_ty.as_ref()),
                _ => None,
            };
            let elements = elements
                .iter()
                .map(|element| constant_expression(element_ty, element, constants))
                .collect::<Option<Vec<_>>>()?;
            typed_aggregate_expression(
                Expr::ArrayLiteral {
                    elements,
                    span: *span,
                },
                ty,
            )
        }
        _ => None,
    }
}

fn is_constant_arithmetic(op: BinaryOp) -> bool {
    matches!(
        op,
        BinaryOp::Add
            | BinaryOp::Sub
            | BinaryOp::Mul
            | BinaryOp::Div
            | BinaryOp::FloorDiv
            | BinaryOp::Mod
            | BinaryOp::Pow
    )
}

fn is_numeric_constant_expression(expr: &Expr) -> bool {
    match expr {
        Expr::Number(..) => true,
        Expr::Cast {
            expr,
            ty: Type::Numeric(_),
            ..
        } => is_numeric_constant_expression(expr),
        Expr::Unary {
            op: UnaryOp::Neg,
            expr,
            ..
        } => is_numeric_constant_expression(expr),
        Expr::Binary {
            op, left, right, ..
        } if is_constant_arithmetic(*op) => {
            is_numeric_constant_expression(left) && is_numeric_constant_expression(right)
        }
        _ => false,
    }
}

/// Find cycles made entirely from supported constant-expression forms. Calls
/// and other effectful forms are deliberately excluded so they retain the
/// side-effect diagnostic rather than being mistaken for constant evaluation.
fn module_constant_cycle(program: &Program) -> Option<Vec<String>> {
    let definitions = program
        .top_level
        .iter()
        .filter_map(|stmt| match stmt {
            Stmt::Let {
                name,
                rebindability: Rebindability::Const,
                value,
                ..
            } => Some((name.clone(), value)),
            _ => None,
        })
        .collect::<Vec<_>>();
    let indices = definitions
        .iter()
        .enumerate()
        .map(|(index, (name, _))| (name.clone(), index))
        .collect::<HashMap<_, _>>();
    let dependencies = definitions
        .iter()
        .map(|(_, value)| {
            let mut found = Vec::new();
            if collect_constant_dependencies(value, &indices, &mut found) {
                found
            } else {
                Vec::new()
            }
        })
        .collect::<Vec<_>>();
    let mut state = vec![0_u8; definitions.len()];
    let mut stack = Vec::new();
    for index in 0..definitions.len() {
        if let Some(cycle) =
            visit_dependencies(index, &dependencies, &definitions, &mut state, &mut stack)
        {
            return Some(cycle);
        }
    }
    None
}

fn collect_constant_dependencies(
    expr: &Expr,
    indices: &HashMap<String, usize>,
    dependencies: &mut Vec<usize>,
) -> bool {
    match expr {
        Expr::Number(..) | Expr::Bool(..) | Expr::Nil(..) | Expr::String(..) | Expr::Bytes(..) => {
            true
        }
        Expr::Name(name, ..) => {
            if let Some(index) = indices.get(name) {
                dependencies.push(*index);
            }
            true
        }
        Expr::Unary {
            op: UnaryOp::Neg,
            expr,
            ..
        }
        | Expr::Cast { expr, .. } => collect_constant_dependencies(expr, indices, dependencies),
        Expr::Binary {
            op, left, right, ..
        } if is_constant_arithmetic(*op) => {
            collect_constant_dependencies(left, indices, dependencies)
                && collect_constant_dependencies(right, indices, dependencies)
        }
        Expr::ArrayLiteral { elements, .. } => elements
            .iter()
            .all(|element| collect_constant_dependencies(element, indices, dependencies)),
        Expr::TableLiteral { fields, .. } => fields
            .iter()
            .all(|field| collect_constant_dependencies(&field.value, indices, dependencies)),
        _ => false,
    }
}

fn visit_dependencies(
    index: usize,
    dependencies: &[Vec<usize>],
    definitions: &[(String, &Expr)],
    state: &mut [u8],
    stack: &mut Vec<usize>,
) -> Option<Vec<String>> {
    if state[index] == 2 {
        return None;
    }
    if state[index] == 1 {
        let start = stack.iter().position(|candidate| *candidate == index)?;
        return Some(
            stack[start..]
                .iter()
                .copied()
                .chain(std::iter::once(index))
                .map(|member| definitions[member].0.clone())
                .collect(),
        );
    }

    state[index] = 1;
    stack.push(index);
    for dependency in &dependencies[index] {
        if let Some(cycle) =
            visit_dependencies(*dependency, dependencies, definitions, state, stack)
        {
            return Some(cycle);
        }
    }
    stack.pop();
    state[index] = 2;
    None
}

fn typed_aggregate_expression(value: Expr, ty: Option<&Type>) -> Option<Expr> {
    match ty {
        Some(ty @ (Type::Record(_) | Type::Array(_))) => Some(Expr::Cast {
            expr: Box::new(value),
            ty: ty.clone(),
            span: None,
        }),
        None => Some(value),
        Some(_) => None,
    }
}

fn resolve_constant_type(
    ty: &Type,
    declarations: &HashMap<&str, &Type>,
    visiting: &mut HashSet<String>,
) -> Type {
    match ty {
        Type::Named { name, type_args } if type_args.is_empty() => {
            let Some(declared) = declarations.get(name.as_str()) else {
                return ty.clone();
            };
            if !visiting.insert(name.clone()) {
                return ty.clone();
            }
            let resolved = resolve_constant_type(declared, declarations, visiting);
            visiting.remove(name);
            resolved
        }
        Type::Array(element) => Type::Array(Arc::new(resolve_constant_type(
            element,
            declarations,
            visiting,
        ))),
        Type::Record(fields) => Type::record(
            fields
                .iter()
                .map(|(name, ty)| {
                    (
                        name.clone(),
                        resolve_constant_type(ty, declarations, visiting),
                    )
                })
                .collect(),
        ),
        _ => ty.clone(),
    }
}

fn typed_numeric_expression(ty: Option<&Type>, value: &Expr) -> Option<Expr> {
    match ty {
        // Keep the annotated numeric type: a bare literal would default to f64
        // in unconstrained positions.
        Some(ty @ Type::Numeric(_)) => Some(Expr::Cast {
            expr: Box::new(value.clone()),
            ty: ty.clone(),
            span: None,
        }),
        None => Some(value.clone()),
        Some(_) => None,
    }
}
