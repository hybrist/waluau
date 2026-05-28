use std::collections::HashMap;

use waluau_ast::{
    AssignOp, BinaryOp, Expr, Function, FunctionExpr, NumberLiteral, NumericType, Program, Stmt,
    Type, UnaryOp,
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
            let value_ty = infer_expr(value, vars, signatures, Some(ty.clone()))?;
            if &value_ty != ty {
                return Err(Diagnostic::new(format!(
                    "let '{}' expects {}, got {}",
                    name, ty, value_ty
                )));
            }
            vars.insert(name.clone(), ty.clone());
            Ok(false)
        }
        Stmt::Assign { op, name, value } => {
            let existing = vars
                .get(name)
                .ok_or_else(|| Diagnostic::new(format!("unknown local '{name}'")))?;
            if *op == AssignOp::Add && !existing.is_numeric() {
                return Err(Diagnostic::new(format!(
                    "compound assignment to '{}' requires a numeric target",
                    name
                )));
            }
            let value_ty = infer_expr(value, vars, signatures, Some(existing.clone()))?;
            if existing != &value_ty {
                return Err(Diagnostic::new(format!(
                    "assignment to '{}' expects {}, got {}",
                    name, existing, value_ty
                )));
            }
            Ok(false)
        }
        Stmt::IndexAssign {
            op,
            base,
            index,
            value,
        } => {
            let base_ty = infer_expr(base, vars, signatures, None)?;
            let element_ty = base_ty.element_type().ok_or_else(|| {
                Diagnostic::new("array element assignment requires an array operand")
            })?;
            if *op == AssignOp::Add && !element_ty.is_numeric() {
                return Err(Diagnostic::new(
                    "compound array assignment requires numeric elements",
                ));
            }
            let index_ty = infer_expr(
                index,
                vars,
                signatures,
                Some(Type::Numeric(NumericType::I32)),
            )?;
            if index_ty != Type::Numeric(NumericType::I32) {
                return Err(Diagnostic::new("array index must be i32"));
            }
            let value_ty = infer_expr(value, vars, signatures, Some(element_ty.clone()))?;
            if value_ty != element_ty {
                return Err(Diagnostic::new(format!(
                    "array element assignment expects {}, got {}",
                    element_ty, value_ty
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
        Stmt::Repeat { body, condition } => {
            let mut loop_scope = vars.clone();
            for stmt in body {
                let _ = check_stmt(stmt, &mut loop_scope, signatures, expected_return)?;
            }
            let condition_ty = infer_expr(condition, &loop_scope, signatures, None)?;
            if condition_ty != Type::Bool {
                return Err(Diagnostic::new("repeat-until condition must be bool"));
            }
            Ok(false)
        }
        Stmt::Return(expr) => {
            let ty = infer_expr(expr, vars, signatures, Some(expected_return.clone()))?;
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
            let actual = if let Some(local) = vars.get(name) {
                local.clone()
            } else if let Some((params, return_type)) = signatures.get(name) {
                Type::Function {
                    params: params.clone(),
                    return_type: Box::new(return_type.clone()),
                }
            } else {
                return Err(Diagnostic::new(format!("unknown name '{name}'")));
            };
            coerce_type(actual, expected)
        }
        Expr::Unary { op, expr } => match op {
            UnaryOp::Neg => {
                let actual = infer_expr(expr, vars, signatures, expected.clone())?;
                match actual {
                    Type::Numeric(_) => coerce_type(actual, expected),
                    Type::Bool => Err(Diagnostic::new("unary '-' requires a numeric operand")),
                    Type::Array(_) => Err(Diagnostic::new("unary '-' requires a numeric operand")),
                    Type::Function { .. } => {
                        Err(Diagnostic::new("unary '-' requires a numeric operand"))
                    }
                }
            }
            UnaryOp::Not => {
                let actual = infer_expr(expr, vars, signatures, Some(Type::Bool))?;
                if actual != Type::Bool {
                    return Err(Diagnostic::new("unary 'not' requires a bool operand"));
                }
                coerce_type(Type::Bool, expected)
            }
            UnaryOp::Len => {
                let actual = infer_expr(expr, vars, signatures, None)?;
                if !actual.is_array() {
                    return Err(Diagnostic::new("# requires an array operand"));
                }
                coerce_type(Type::Numeric(NumericType::I32), expected)
            }
        },
        Expr::Cast { expr, ty } => {
            let actual = infer_expr(expr, vars, signatures, None)?;
            require_numeric_cast(actual, ty.clone())?;
            coerce_type(ty.clone(), expected)
        }
        Expr::Call { callee, args } => {
            let callee_ty = infer_expr(callee, vars, signatures, None)?;
            let (params, ret) = match callee_ty {
                Type::Function {
                    params,
                    return_type,
                } => (params, *return_type),
                other => {
                    return Err(Diagnostic::new(format!(
                        "attempt to call non-function value of type {other}",
                    )));
                }
            };
            if params.len() != args.len() {
                return Err(Diagnostic::new(format!(
                    "function expects {} arguments, got {}",
                    params.len(),
                    args.len()
                )));
            }
            for (expected, arg) in params.iter().zip(args) {
                let actual = infer_expr(arg, vars, signatures, Some(expected.clone()))?;
                if expected != &actual {
                    return Err(Diagnostic::new(format!(
                        "call expected {}, got {}",
                        expected, actual
                    )));
                }
            }
            coerce_type(ret, expected)
        }
        Expr::Function(function) => infer_function_expr(function, vars, signatures, expected),
        Expr::ArrayLiteral { elements } => {
            infer_array_literal(elements, vars, signatures, expected)
        }
        Expr::Index { base, index } => {
            let base_ty = infer_expr(base, vars, signatures, None)?;
            let element_ty = base_ty
                .element_type()
                .ok_or_else(|| Diagnostic::new("indexing requires an array operand"))?;
            let index_ty = infer_expr(
                index,
                vars,
                signatures,
                Some(Type::Numeric(NumericType::I32)),
            )?;
            if index_ty != Type::Numeric(NumericType::I32) {
                return Err(Diagnostic::new("array index must be i32"));
            }
            coerce_type(element_ty, expected)
        }
        Expr::Binary { op, left, right } => match op {
            BinaryOp::Add
            | BinaryOp::Sub
            | BinaryOp::Mul
            | BinaryOp::Div
            | BinaryOp::FloorDiv
            | BinaryOp::Mod => {
                let operand_ty =
                    infer_numeric_common_type(left, right, vars, signatures, expected.clone())?;
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
                    let right_ty = infer_expr(right, vars, signatures, Some(left_ty.clone()))?;
                    if left_ty != right_ty {
                        return Err(Diagnostic::new("== requires both sides to have same type"));
                    }
                }
                Ok(Type::Bool)
            }
        },
    }
}

fn infer_array_literal(
    elements: &[Expr],
    vars: &HashMap<String, Type>,
    signatures: &HashMap<String, (Vec<Type>, Type)>,
    expected: Option<Type>,
) -> Result<Type, Diagnostic> {
    if elements.is_empty() {
        return Err(Diagnostic::new(
            "empty array literal requires explicit element type",
        ));
    }

    let expected_element = expected.as_ref().and_then(Type::element_type);
    let mut iter = elements.iter();
    let first = iter.next().expect("non-empty array literal");
    let mut element_ty = infer_expr(first, vars, signatures, expected_element.clone())?;
    for element in iter {
        let actual = infer_expr(element, vars, signatures, Some(element_ty.clone()))?;
        element_ty = common_element_type(element_ty, actual)?;
    }

    let array_ty = Type::Array(Box::new(element_ty));
    coerce_type(array_ty, expected)
}

fn common_element_type(left: Type, right: Type) -> Result<Type, Diagnostic> {
    match (left, right) {
        (Type::Numeric(left), Type::Numeric(right)) => left
            .common(right)
            .map(Type::Numeric)
            .ok_or_else(|| Diagnostic::new("array literal elements must share a common type")),
        (left, right) if left == right => Ok(left),
        _ => Err(Diagnostic::new(
            "array literal elements must share a common type",
        )),
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
            require_same_numeric(left_ty.clone(), right_ty)?;
            Ok(left_ty)
        }
        (true, false) => {
            let right_ty = infer_expr(right, vars, signatures, None)?;
            let left_ty = infer_expr(left, vars, signatures, Some(right_ty.clone()))?;
            common_numeric_type(left_ty, right_ty)
        }
        (false, true) => {
            let left_ty = infer_expr(left, vars, signatures, None)?;
            let right_ty = infer_expr(right, vars, signatures, Some(left_ty.clone()))?;
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
            Type::Array(_) => Err(Diagnostic::new(format!(
                "cannot implicitly convert array to {expected_numeric}",
            ))),
            Type::Function { .. } => Err(Diagnostic::new(format!(
                "cannot implicitly convert function to {expected_numeric}",
            ))),
        },
        Some(Type::Bool) => Err(Diagnostic::new(format!(
            "cannot implicitly convert {actual} to bool",
        ))),
        Some(expected) => Err(Diagnostic::new(format!(
            "cannot implicitly convert {actual} to {expected}",
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
        Some(Type::Array(_)) => Err(Diagnostic::new(
            "numeric literal is not assignable to array",
        )),
        Some(Type::Function { .. }) => Err(Diagnostic::new(
            "numeric literal is not assignable to function",
        )),
        None => Ok(Type::number()),
    }
}

fn infer_function_expr(
    function: &FunctionExpr,
    vars: &HashMap<String, Type>,
    signatures: &HashMap<String, (Vec<Type>, Type)>,
    expected: Option<Type>,
) -> Result<Type, Diagnostic> {
    let function_ty = Type::Function {
        params: function.params.iter().map(|param| param.ty.clone()).collect(),
        return_type: Box::new(function.return_type.clone()),
    };
    let mut local_scope = vars.clone();
    for param in &function.params {
        local_scope.insert(param.name.clone(), param.ty.clone());
    }
    if let Some(name) = &function.name {
        local_scope.insert(name.clone(), function_ty.clone());
    }
    let mut saw_return = false;
    for stmt in &function.body {
        if check_stmt(stmt, &mut local_scope, signatures, &function.return_type)? {
            saw_return = true;
        }
    }
    if !saw_return {
        return Err(Diagnostic::new("function expression is missing a return"));
    }
    coerce_type(function_ty, expected)
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
            function add(x: i32, y: i32): i32
                return x + y
            end

            function entry(flag: bool, x: i32, y: i32): i32
                local z: i32 = add(x, y)
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
            function entry(x: i32): i32
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
            function widen(x: number, y: f32, z: u64, w: i64): f64
                local sum: f64 = x + 1
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
            function entry(x: i64, y: f64): i64
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
            function entry(x: i64, y: u64): i64
                local a: i64 = x + 1
                local b: u64 = 18446744073709551615
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
            function entry(): u64
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
            function widen(x: i32, y: f32, z: u32): f64
                local a: i64 = x
                local b: f64 = x + 1
                local c: f64 = y
                local d: i64 = z + 1
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
            function narrow(x: i64): i32
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
            function narrow(x: i64, y: f64): i32
                local a: i32 = x :: i32
                local b: i32 = y :: i32
                return a + b
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        super::type_check(&program).expect("type check should succeed");
    }

    #[test]
    fn accepts_unary_negation_not_and_elseif() {
        let source = r#"
            function entry(flag: bool, x: i32): i32
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
            function entry(x: i32): i32
                x + 1
                return x
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        let error = super::type_check(&program).expect_err("type check should fail");
        assert_eq!(error.to_string(), "expression statements must be calls");
    }

    #[test]
    fn type_checks_array_literals_indexing_and_length() {
        let source = r#"
            function score_count(): i32
                local scores: {number} = {100, 250, 300}
                local first: number = scores[0]
                scores[1] = first + 1
                return #scores
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        super::type_check(&program).expect("type check should succeed");
    }

    #[test]
    fn rejects_heterogeneous_array_literals() {
        let source = r#"
            function entry(): i32
                local xs: {i32} = {1, true}
                return #xs
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        let error = super::type_check(&program).expect_err("type check should fail");
        assert_eq!(
            error.to_string(),
            "array literal elements must share a common type"
        );
    }

    #[test]
    fn rejects_empty_array_literals() {
        let source = r#"
            function entry(): i32
                local xs: {i32} = {}
                return #xs
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        let error = super::type_check(&program).expect_err("type check should fail");
        assert_eq!(
            error.to_string(),
            "empty array literal requires explicit element type"
        );
    }

    #[test]
    fn rejects_length_on_non_array() {
        let source = r#"
            function entry(x: i32): i32
                return #x
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        let error = super::type_check(&program).expect_err("type check should fail");
        assert_eq!(error.to_string(), "# requires an array operand");
    }

    #[test]
    fn rejects_incompatible_array_assignment() {
        let source = r#"
            function entry(): i32
                local xs: {i32} = {1, 2, 3}
                local ys: {i64} = {1, 2, 3}
                xs = ys
                return #xs
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        let error = super::type_check(&program).expect_err("type check should fail");
        assert_eq!(
            error.to_string(),
            "cannot implicitly convert {i64} to {i32}"
        );
    }

    #[test]
    fn rejects_repeat_until_non_bool_condition() {
        let source = r#"
            function entry(x: i32): i32
                repeat
                    x = x + 1
                until x
                return x
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        let error = super::type_check(&program).expect_err("type check should fail");
        assert_eq!(error.to_string(), "repeat-until condition must be bool");
    }

    #[test]
    fn type_checks_repeat_until_loop() {
        let source = r#"
            function entry(limit: i32): i32
                local i: i32 = 0
                repeat
                    i = i + 1
                until i > limit
                return i
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        super::type_check(&program).expect("type check should succeed");
    }

    fn type_checks_closure_capture() {
        let source = r#"
            function entry(x: i32): i32
                local make: (i32) -> (i32) -> i32 = function(offset: i32): (i32) -> i32
                    return function(value: i32): i32
                        return x + offset + value
                    end
                end
                local add5: (i32) -> i32 = make(5)
                return add5(7)
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        super::type_check(&program).expect("type check should succeed");
    }

    #[test]
    fn type_checks_named_function_expression_recursion() {
        let source = r#"
            function entry(): i32
                local fact: (i32) -> i32 = function self(n: i32): i32
                    if n == 0 then
                        return 1
                    end
                    return n * self(n - 1)
                end
                return fact(5)
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        super::type_check(&program).expect("type check should succeed");
    }

    #[test]
    fn rejects_compound_assignment_on_non_numeric_targets() {
        let source = r#"
            function entry(flag: bool, xs: {bool}): i32
                flag += true
                xs[0] += false
                return 0
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        let error = super::type_check(&program).expect_err("type check should fail");
        assert_eq!(
            error.to_string(),
            "compound assignment to 'flag' requires a numeric target"
        );
    }
}
