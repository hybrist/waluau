use std::collections::HashMap;

use waluau_ast::{
    BinaryOp, Expr, Function, NumberLiteral, NumericType, Program, Stmt, Type, UnaryOp,
};
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
            let condition_ty = infer_expr(condition, vars, signatures, None)?;
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
            let condition_ty = infer_expr(condition, vars, signatures, None)?;
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
            if !matches!(expr, Expr::Call { .. }) {
                return Err(Diagnostic::new("expression statements must be calls"));
            }
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
        Expr::Number(value) => resolve_number_literal(value, expected),
        Expr::Bool(_) => Ok(Type::Bool),
        Expr::Name(name) => {
            let actual = vars
                .get(name)
                .cloned()
                .ok_or_else(|| Diagnostic::new(format!("unknown name '{name}'")))?;
            coerce_type(actual, expected)
        }
        Expr::Unary { op, expr } => match op {
            UnaryOp::Neg => {
                let actual = infer_expr(expr, vars, signatures, expected)?;
                match actual {
                    Type::Numeric(_) => coerce_type(actual, expected),
                    Type::Bool => Err(Diagnostic::new("unary '-' requires a numeric operand")),
                }
            }
            UnaryOp::Not => {
                let actual = infer_expr(expr, vars, signatures, Some(Type::Bool))?;
                if actual != Type::Bool {
                    return Err(Diagnostic::new("unary 'not' requires a bool operand"));
                }
                coerce_type(Type::Bool, expected)
            }
        },
        Expr::Cast { expr, ty } => {
            let actual = infer_expr(expr, vars, signatures, None)?;
            require_numeric_cast(actual, *ty)?;
            coerce_type(*ty, expected)
        }
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
            coerce_type(*ret, expected)
        }
        Expr::Binary { op, left, right } => match op {
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div => {
                let operand_ty =
                    infer_numeric_common_type(left, right, vars, signatures, expected)?;
                coerce_type(operand_ty, expected)
            }
            BinaryOp::Less | BinaryOp::Greater => {
                let _ = infer_numeric_common_type(left, right, vars, signatures, None)?;
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
                if left_ty == Type::Bool {
                    let right_ty = infer_expr(right, vars, signatures, Some(Type::Bool))?;
                    if right_ty != Type::Bool {
                        return Err(Diagnostic::new("== requires both sides to have same type"));
                    }
                } else if left_ty.is_numeric() {
                    let _ = infer_numeric_common_type(left, right, vars, signatures, None)?;
                } else {
                    let right_ty = infer_expr(right, vars, signatures, Some(left_ty))?;
                    if left_ty != right_ty {
                        return Err(Diagnostic::new("== requires both sides to have same type"));
                    }
                }
                Ok(Type::Bool)
            }
        },
    }
}

fn infer_numeric_common_type(
    left: &Expr,
    right: &Expr,
    vars: &HashMap<String, Type>,
    signatures: &HashMap<String, (Vec<Type>, Type)>,
    expected: Option<Type>,
) -> Result<Type, Diagnostic> {
    let expected_numeric = match expected {
        Some(Type::Numeric(numeric)) => Some(numeric),
        _ => None,
    };

    match (
        matches!(left, Expr::Number(_)),
        matches!(right, Expr::Number(_)),
    ) {
        (true, true) => {
            let ty = expected_numeric.unwrap_or(NumericType::F64);
            let left_ty = infer_expr(left, vars, signatures, Some(Type::Numeric(ty)))?;
            let right_ty = infer_expr(right, vars, signatures, Some(Type::Numeric(ty)))?;
            require_same_numeric(left_ty, right_ty)?;
            Ok(left_ty)
        }
        (true, false) => {
            let right_ty = infer_expr(right, vars, signatures, None)?;
            let left_ty = infer_expr(left, vars, signatures, Some(right_ty))?;
            common_numeric_type(left_ty, right_ty)
        }
        (false, true) => {
            let left_ty = infer_expr(left, vars, signatures, None)?;
            let right_ty = infer_expr(right, vars, signatures, Some(left_ty))?;
            common_numeric_type(left_ty, right_ty)
        }
        _ => {
            let left_ty = infer_expr(left, vars, signatures, None)?;
            let right_ty = infer_expr(right, vars, signatures, None)?;
            common_numeric_type(left_ty, right_ty)
        }
    }
}

fn common_numeric_type(left: Type, right: Type) -> Result<Type, Diagnostic> {
    match (left, right) {
        (Type::Numeric(left), Type::Numeric(right)) => left
            .common(right)
            .map(Type::Numeric)
            .ok_or_else(|| Diagnostic::new("operation requires compatible numeric operands")),
        _ => Err(Diagnostic::new(
            "operation requires compatible numeric operands",
        )),
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

fn coerce_type(actual: Type, expected: Option<Type>) -> Result<Type, Diagnostic> {
    match expected {
        None => Ok(actual),
        Some(expected) if actual == expected => Ok(expected),
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
        },
        Some(Type::Bool) => Err(Diagnostic::new(format!(
            "cannot implicitly convert {actual} to bool",
        ))),
    }
}

fn require_numeric_cast(actual: Type, target: Type) -> Result<(), Diagnostic> {
    match (actual, target) {
        (Type::Numeric(_), Type::Numeric(_)) => Ok(()),
        _ => Err(Diagnostic::new(
            "casts require numeric source and destination types",
        )),
    }
}

fn require_bool_pair(left: Type, right: Type) -> Result<(), Diagnostic> {
    if left == Type::Bool && right == Type::Bool {
        Ok(())
    } else {
        Err(Diagnostic::new("operation requires bool operands"))
    }
}

fn resolve_number_literal(
    value: &NumberLiteral,
    expected: Option<Type>,
) -> Result<Type, Diagnostic> {
    match expected {
        Some(Type::Numeric(numeric)) => {
            validate_numeric_literal(value, numeric)?;
            Ok(Type::Numeric(numeric))
        }
        Some(Type::Bool) => Err(Diagnostic::new("numeric literal is not assignable to bool")),
        None => Ok(Type::number()),
    }
}

fn validate_numeric_literal(
    value: &NumberLiteral,
    expected: NumericType,
) -> Result<(), Diagnostic> {
    match expected {
        NumericType::F32 => {
            let value = parse_float_literal(value)?;
            if (value as f32).is_finite() || value == f64::INFINITY || value == f64::NEG_INFINITY {
                Ok(())
            } else {
                Err(Diagnostic::new("numeric literal is out of range for f32"))
            }
        }
        NumericType::F64 => parse_float_literal(value).map(|_| ()),
        NumericType::I32 => parse_integer_literal::<i32>(value, "i32").map(|_| ()),
        NumericType::I64 => parse_integer_literal::<i64>(value, "i64").map(|_| ()),
        NumericType::U32 => parse_integer_literal::<u32>(value, "u32").map(|_| ()),
        NumericType::U64 => parse_integer_literal::<u64>(value, "u64").map(|_| ()),
    }
}

fn parse_float_literal(value: &NumberLiteral) -> Result<f64, Diagnostic> {
    value
        .raw
        .parse::<f64>()
        .map_err(|_| Diagnostic::new("invalid number literal"))
}

fn parse_integer_literal<T>(value: &NumberLiteral, ty_name: &str) -> Result<T, Diagnostic>
where
    T: std::str::FromStr,
{
    if value.raw.contains('.') {
        return Err(Diagnostic::new(format!(
            "numeric literal must be an integer for {ty_name}",
        )));
    }

    value
        .raw
        .parse::<T>()
        .map_err(|_| Diagnostic::new(format!("numeric literal is out of range for {ty_name}",)))
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
            fn widen(x: number, y: f32, z: u64, w: i64) -> f64
                let sum: f64 = x + 1
                if z > 0 then
                    return sum
                end
                if w > 0 then
                    return x + 2
                end
                return x + 3
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        super::type_check(&program).expect("type check should succeed");
    }

    #[test]
    fn rejects_mixed_numeric_operands() {
        let source = r#"
            fn entry(x: i64, y: f64) -> i64
                return x + y
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        let error = super::type_check(&program).expect_err("type check should fail");
        assert_eq!(
            error.to_string(),
            "operation requires compatible numeric operands"
        );
    }

    #[test]
    fn accepts_full_range_i64_and_u64_literals() {
        let source = r#"
            fn entry(x: i64, y: u64) -> i64
                let a: i64 = x + 1
                let b: u64 = 18446744073709551615
                if y > 0 then
                    return a
                end
                if b > 0 then
                    return 9223372036854775807
                end
                return x + 2
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        super::type_check(&program).expect("type check should succeed");
    }

    #[test]
    fn rejects_out_of_range_u64_literals() {
        let source = r#"
            fn entry() -> u64
                return 18446744073709551616
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        let error = super::type_check(&program).expect_err("type check should fail");
        assert_eq!(error.to_string(), "numeric literal is out of range for u64");
    }

    #[test]
    fn accepts_implicit_numeric_widening() {
        let source = r#"
            fn widen(x: i32, y: f32, z: u32) -> f64
                let a: i64 = x
                let b: f64 = x + 1
                let c: f64 = y
                let d: i64 = z + 1
                if a < d then
                    return b
                end
                return c
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        super::type_check(&program).expect("type check should succeed");
    }

    #[test]
    fn requires_explicit_cast_for_narrowing() {
        let source = r#"
            fn narrow(x: i64) -> i32
                return x
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        let error = super::type_check(&program).expect_err("type check should fail");
        assert_eq!(error.to_string(), "cannot implicitly convert i64 to i32");
    }

    #[test]
    fn accepts_explicit_numeric_casts() {
        let source = r#"
            fn narrow(x: i64, y: f64) -> i32
                let a: i32 = x :: i32
                let b: i32 = y :: i32
                return a + b
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        super::type_check(&program).expect("type check should succeed");
    }

    #[test]
    fn accepts_unary_negation_not_and_elseif() {
        let source = r#"
            fn entry(flag: bool, x: i32) -> i32
                if not flag then
                    return -x
                elseif x > 0 then
                    return x
                else
                    return 0
                end
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        super::type_check(&program).expect("type check should succeed");
    }

    #[test]
    fn rejects_non_call_expression_statements() {
        let source = r#"
            fn entry(x: i32) -> i32
                x + 1
                return x
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        let error = super::type_check(&program).expect_err("type check should fail");
        assert_eq!(error.to_string(), "expression statements must be calls");
    }
}
