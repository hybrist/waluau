use std::collections::HashMap;

use waluau_ast::{BinaryOp, Expr, Function, NumericType, Program, Stmt, Type};
use waluau_diagnostics::Diagnostic;

pub fn type_check(program: &Program) -> Result<(), Diagnostic> {
    let signatures: HashMap<_, _> = program
        .functions
        .iter()
        .map(|function| {
            (
                function.name.clone(),
                (
                    function.params.iter().map(|param| param.ty).collect(),
                    function.return_type,
                ),
            )
        })
        .collect();

    for function in &program.functions {
        check_function(function, &signatures)?;
    }
    Ok(())
}

fn check_function(
    function: &Function,
    signatures: &HashMap<String, (Vec<Type>, Type)>,
) -> Result<(), Diagnostic> {
    let mut vars: HashMap<String, Type> = HashMap::new();
    for param in &function.params {
        vars.insert(param.name.clone(), param.ty);
    }

    let mut saw_return = false;
    for stmt in &function.body {
        if check_stmt(stmt, &mut vars, signatures, &function.return_type)? {
            saw_return = true;
        }
    }
    if !saw_return {
        return Err(Diagnostic::new(format!(
            "function '{}' is missing a return",
            function.name
        )));
    }
    Ok(())
}

fn check_stmt(
    stmt: &Stmt,
    vars: &mut HashMap<String, Type>,
    signatures: &HashMap<String, (Vec<Type>, Type)>,
    expected_return: &Type,
) -> Result<bool, Diagnostic> {
    match stmt {
        Stmt::Let { name, ty, value } => {
            let value_ty = infer_expr(value, vars, signatures, Some(*ty))?;
            if &value_ty != ty {
                return Err(Diagnostic::new(format!(
                    "let '{}' expects {}, got {}",
                    name, ty, value_ty
                )));
            }
            vars.insert(name.clone(), *ty);
            Ok(false)
        }
        Stmt::Assign { name, value } => {
            let existing = vars
                .get(name)
                .ok_or_else(|| Diagnostic::new(format!("unknown local '{name}'")))?;
            let value_ty = infer_expr(value, vars, signatures, Some(*existing))?;
            if existing != &value_ty {
                return Err(Diagnostic::new(format!(
                    "assignment to '{}' expects {}, got {}",
                    name, existing, value_ty
                )));
            }
            Ok(false)
        }
        Stmt::If {
            condition,
            then_body,
            else_body,
        } => {
            let condition_ty = infer_expr(condition, vars, signatures, Some(Type::Bool))?;
            if condition_ty != Type::Bool {
                return Err(Diagnostic::new("if condition must be bool"));
            }
            let mut then_scope = vars.clone();
            let mut else_scope = vars.clone();
            let mut then_returns = false;
            let mut else_returns = false;
            for stmt in then_body {
                then_returns |= check_stmt(stmt, &mut then_scope, signatures, expected_return)?;
            }
            for stmt in else_body {
                else_returns |= check_stmt(stmt, &mut else_scope, signatures, expected_return)?;
            }
            Ok(then_returns && else_returns)
        }
        Stmt::While { condition, body } => {
            let condition_ty = infer_expr(condition, vars, signatures, Some(Type::Bool))?;
            if condition_ty != Type::Bool {
                return Err(Diagnostic::new("while condition must be bool"));
            }
            let mut loop_scope = vars.clone();
            for stmt in body {
                let _ = check_stmt(stmt, &mut loop_scope, signatures, expected_return)?;
            }
            Ok(false)
        }
        Stmt::Return(expr) => {
            let ty = infer_expr(expr, vars, signatures, Some(*expected_return))?;
            if &ty != expected_return {
                return Err(Diagnostic::new(format!(
                    "return expects {}, got {}",
                    expected_return, ty
                )));
            }
            Ok(true)
        }
        Stmt::Expr(expr) => {
            let _ = infer_expr(expr, vars, signatures, None)?;
            Ok(false)
        }
    }
}

fn infer_expr(
    expr: &Expr,
    vars: &HashMap<String, Type>,
    signatures: &HashMap<String, (Vec<Type>, Type)>,
    expected: Option<Type>,
) -> Result<Type, Diagnostic> {
    match expr {
        Expr::Number(value) => resolve_number_literal(*value, expected),
        Expr::Bool(_) => Ok(Type::Bool),
        Expr::Name(name) => vars
            .get(name)
            .cloned()
            .ok_or_else(|| Diagnostic::new(format!("unknown name '{name}'"))),
        Expr::Call { name, args } => {
            let (params, ret) = signatures
                .get(name)
                .ok_or_else(|| Diagnostic::new(format!("unknown function '{name}'")))?;
            if params.len() != args.len() {
                return Err(Diagnostic::new(format!(
                    "function '{}' expects {} arguments, got {}",
                    name,
                    params.len(),
                    args.len()
                )));
            }
            for (expected, arg) in params.iter().zip(args) {
                let actual = infer_expr(arg, vars, signatures, Some(*expected))?;
                if expected != &actual {
                    return Err(Diagnostic::new(format!(
                        "call '{}' expected {}, got {}",
                        name, expected, actual
                    )));
                }
            }
            Ok(*ret)
        }
        Expr::Binary { op, left, right } => match op {
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div => {
                let operand_hint = expected.filter(|ty| ty.is_numeric());
                let (left_ty, right_ty) =
                    infer_numeric_pair(left, right, vars, signatures, operand_hint)?;
                require_same_numeric(left_ty, right_ty)?;
                Ok(left_ty)
            }
            BinaryOp::Less | BinaryOp::Greater => {
                let (left_ty, right_ty) = infer_numeric_pair(left, right, vars, signatures, None)?;
                require_same_numeric(left_ty, right_ty)?;
                Ok(Type::Bool)
            }
            BinaryOp::And | BinaryOp::Or => {
                let left_ty = infer_expr(left, vars, signatures, Some(Type::Bool))?;
                let right_ty = infer_expr(right, vars, signatures, Some(Type::Bool))?;
                require_bool_pair(left_ty, right_ty)?;
                Ok(Type::Bool)
            }
            BinaryOp::Eq => {
                let left_ty = infer_expr(left, vars, signatures, None)?;
                let right_ty = infer_expr(right, vars, signatures, Some(left_ty))?;
                if left_ty != right_ty {
                    return Err(Diagnostic::new("== requires both sides to have same type"));
                }
                Ok(Type::Bool)
            }
        },
    }
}

fn infer_numeric_pair(
    left: &Expr,
    right: &Expr,
    vars: &HashMap<String, Type>,
    signatures: &HashMap<String, (Vec<Type>, Type)>,
    expected: Option<Type>,
) -> Result<(Type, Type), Diagnostic> {
    match (
        matches!(left, Expr::Number(_)),
        matches!(right, Expr::Number(_)),
    ) {
        (true, false) => {
            let right_ty = infer_expr(right, vars, signatures, expected)?;
            let left_ty = infer_expr(left, vars, signatures, Some(right_ty))?;
            Ok((left_ty, right_ty))
        }
        (false, true) => {
            let left_ty = infer_expr(left, vars, signatures, expected)?;
            let right_ty = infer_expr(right, vars, signatures, Some(left_ty))?;
            Ok((left_ty, right_ty))
        }
        _ => {
            let left_ty = infer_expr(left, vars, signatures, expected)?;
            let right_ty = infer_expr(right, vars, signatures, Some(left_ty))?;
            Ok((left_ty, right_ty))
        }
    }
}

fn require_same_numeric(left: Type, right: Type) -> Result<(), Diagnostic> {
    if left.is_numeric() && right.is_numeric() && left == right {
        Ok(())
    } else {
        Err(Diagnostic::new(
            "operation requires matching numeric operands",
        ))
    }
}

fn require_bool_pair(left: Type, right: Type) -> Result<(), Diagnostic> {
    if left == Type::Bool && right == Type::Bool {
        Ok(())
    } else {
        Err(Diagnostic::new("operation requires bool operands"))
    }
}

fn resolve_number_literal(value: f64, expected: Option<Type>) -> Result<Type, Diagnostic> {
    match expected {
        Some(Type::Numeric(numeric)) => {
            validate_numeric_literal(value, numeric)?;
            Ok(Type::Numeric(numeric))
        }
        Some(Type::Bool) => Err(Diagnostic::new("numeric literal is not assignable to bool")),
        None => Ok(Type::number()),
    }
}

fn validate_numeric_literal(value: f64, expected: NumericType) -> Result<(), Diagnostic> {
    match expected {
        NumericType::F32 => {
            if (value as f32).is_finite() || value == f64::INFINITY || value == f64::NEG_INFINITY {
                Ok(())
            } else {
                Err(Diagnostic::new("numeric literal is out of range for f32"))
            }
        }
        NumericType::F64 => Ok(()),
        NumericType::I32 => {
            if value.fract() != 0.0 {
                return Err(Diagnostic::new(
                    "numeric literal must be an integer for i32",
                ));
            }
            if (i32::MIN as f64..=i32::MAX as f64).contains(&value) {
                Ok(())
            } else {
                Err(Diagnostic::new("numeric literal is out of range for i32"))
            }
        }
        NumericType::U32 => {
            if value.fract() != 0.0 {
                return Err(Diagnostic::new(
                    "numeric literal must be an integer for u32",
                ));
            }
            if (0.0..=u32::MAX as f64).contains(&value) {
                Ok(())
            } else {
                Err(Diagnostic::new("numeric literal is out of range for u32"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use waluau_parser::parse;

    #[test]
    fn type_checks_valid_program() {
        let source = r#"
            fn add(x: i32, y: i32) -> i32
                return x + y
            end

            fn entry(flag: bool, x: i32, y: i32) -> i32
                let z: i32 = add(x, y)
                if flag then
                    z = z + 1
                else
                    z = z + 2
                end
                return z
            end
        "#;
        let program = parse(source).expect("parse should succeed");
        super::type_check(&program).expect("type check should succeed");
    }

    #[test]
    fn rejects_non_bool_condition() {
        let source = r#"
            fn entry(x: i32) -> i32
                if x then
                    return x
                end
                return x
            end
        "#;
        let program = parse(source).expect("parse should succeed");
        let error = super::type_check(&program).expect_err("type check should fail");
        assert_eq!(error.to_string(), "if condition must be bool");
    }

    #[test]
    fn accepts_numeric_alias_and_scalar_types() {
        let source = r#"
            fn widen(x: number, y: f32, z: u32) -> f64
                let sum: f64 = x + 1
                if z > 0 then
                    return sum
                end
                return x + 2
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        super::type_check(&program).expect("type check should succeed");
    }

    #[test]
    fn rejects_mixed_numeric_operands() {
        let source = r#"
            fn entry(x: i32, y: f64) -> i32
                return x + y
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        let error = super::type_check(&program).expect_err("type check should fail");
        assert_eq!(
            error.to_string(),
            "operation requires matching numeric operands"
        );
    }
}
