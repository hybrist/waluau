use std::collections::HashMap;

use waluau_ast::{BinaryOp, Expr, Function, Program, Stmt, Type};
use waluau_diagnostics::Diagnostic;

pub fn type_check(program: &Program) -> Result<(), Diagnostic> {
    let signatures: HashMap<_, _> = program
        .functions
        .iter()
        .map(|function| {
            (
                function.name.clone(),
                (
                    function
                        .params
                        .iter()
                        .map(|param| param.ty.clone())
                        .collect(),
                    function.return_type.clone(),
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
        vars.insert(param.name.clone(), param.ty.clone());
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
            let value_ty = infer_expr(value, vars, signatures)?;
            if &value_ty != ty {
                return Err(Diagnostic::new(format!(
                    "let '{}' expects {:?}, got {:?}",
                    name, ty, value_ty
                )));
            }
            vars.insert(name.clone(), ty.clone());
            Ok(false)
        }
        Stmt::Assign { name, value } => {
            let existing = vars
                .get(name)
                .ok_or_else(|| Diagnostic::new(format!("unknown local '{name}'")))?;
            let value_ty = infer_expr(value, vars, signatures)?;
            if existing != &value_ty {
                return Err(Diagnostic::new(format!(
                    "assignment to '{}' expects {:?}, got {:?}",
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
            let condition_ty = infer_expr(condition, vars, signatures)?;
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
            let condition_ty = infer_expr(condition, vars, signatures)?;
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
            let ty = infer_expr(expr, vars, signatures)?;
            if &ty != expected_return {
                return Err(Diagnostic::new(format!(
                    "return expects {:?}, got {:?}",
                    expected_return, ty
                )));
            }
            Ok(true)
        }
        Stmt::Expr(expr) => {
            let _ = infer_expr(expr, vars, signatures)?;
            Ok(false)
        }
    }
}

fn infer_expr(
    expr: &Expr,
    vars: &HashMap<String, Type>,
    signatures: &HashMap<String, (Vec<Type>, Type)>,
) -> Result<Type, Diagnostic> {
    match expr {
        Expr::Number(_) => Ok(Type::Number),
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
                let actual = infer_expr(arg, vars, signatures)?;
                if expected != &actual {
                    return Err(Diagnostic::new(format!(
                        "call '{}' expected {:?}, got {:?}",
                        name, expected, actual
                    )));
                }
            }
            Ok(ret.clone())
        }
        Expr::Binary { op, left, right } => {
            let left_ty = infer_expr(left, vars, signatures)?;
            let right_ty = infer_expr(right, vars, signatures)?;
            match op {
                BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div => {
                    require_number_pair(&left_ty, &right_ty)?;
                    Ok(Type::Number)
                }
                BinaryOp::Less | BinaryOp::Greater => {
                    require_number_pair(&left_ty, &right_ty)?;
                    Ok(Type::Bool)
                }
                BinaryOp::And | BinaryOp::Or => {
                    require_bool_pair(&left_ty, &right_ty)?;
                    Ok(Type::Bool)
                }
                BinaryOp::Eq => {
                    if left_ty != right_ty {
                        return Err(Diagnostic::new("== requires both sides to have same type"));
                    }
                    Ok(Type::Bool)
                }
            }
        }
    }
}

fn require_number_pair(left: &Type, right: &Type) -> Result<(), Diagnostic> {
    if left == &Type::Number && right == &Type::Number {
        Ok(())
    } else {
        Err(Diagnostic::new("operation requires number operands"))
    }
}

fn require_bool_pair(left: &Type, right: &Type) -> Result<(), Diagnostic> {
    if left == &Type::Bool && right == &Type::Bool {
        Ok(())
    } else {
        Err(Diagnostic::new("operation requires bool operands"))
    }
}

#[cfg(test)]
mod tests {
    use waluau_parser::parse;

    #[test]
    fn type_checks_valid_program() {
        let source = r#"
            fn add(x: number, y: number) -> number
                return x + y
            end

            fn entry(flag: bool, x: number, y: number) -> number
                let z: number = add(x, y)
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
            fn entry(x: number) -> number
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
}
