pub(crate) fn collect_assigned_names(stmts: &[Stmt]) -> BTreeSet<String> {
    let mut assigned = BTreeSet::new();
    collect_assigned_into(stmts, &mut assigned);
    assigned
}

pub(crate) fn collect_captures(
    function: &waluau_ast::FunctionExpr,
    env: &HashMap<String, ValueId>,
    types: &HashMap<String, Type>,
    signatures: &HashMap<String, (Vec<Type>, Type)>,
) -> Vec<(String, Type)> {
    let mut bound: HashSet<String> = function
        .params
        .iter()
        .map(|param| param.name.clone())
        .collect();
    if let Some(name) = &function.name {
        bound.insert(name.clone());
    }
    let mut captures = BTreeSet::new();
    for stmt in &function.body {
        collect_expr_captures_from_stmt(stmt, &bound, env, signatures, &mut captures);
    }
    captures
        .into_iter()
        .filter_map(|name| {
            env.get(&name)?;
            let ty = types.get(&name)?.clone();
            Some((name, ty))
        })
        .collect()
}

fn collect_expr_captures_from_stmt(
    stmt: &Stmt,
    bound: &HashSet<String>,
    env: &HashMap<String, ValueId>,
    signatures: &HashMap<String, (Vec<Type>, Type)>,
    captures: &mut BTreeSet<String>,
) {
    match stmt {
        Stmt::Let { value, .. } => collect_expr_captures(value, bound, env, signatures, captures),
        Stmt::Assign { name, value, .. } => {
            if !bound.contains(name) && env.contains_key(name) && !signatures.contains_key(name) {
                captures.insert(name.clone());
            }
            collect_expr_captures(value, bound, env, signatures, captures)
        }
        Stmt::IndexAssign {
            base, index, value, ..
        } => {
            collect_expr_captures(base, bound, env, signatures, captures);
            collect_expr_captures(index, bound, env, signatures, captures);
            collect_expr_captures(value, bound, env, signatures, captures);
        }
        Stmt::FieldAssign { base, value, .. } => {
            collect_expr_captures(base, bound, env, signatures, captures);
            collect_expr_captures(value, bound, env, signatures, captures);
        }
        Stmt::If {
            condition,
            then_body,
            else_body,
        } => {
            collect_expr_captures(condition, bound, env, signatures, captures);
            for stmt in then_body {
                collect_expr_captures_from_stmt(stmt, bound, env, signatures, captures);
            }
            for stmt in else_body {
                collect_expr_captures_from_stmt(stmt, bound, env, signatures, captures);
            }
        }
        Stmt::While { condition, body } => {
            collect_expr_captures(condition, bound, env, signatures, captures);
            for stmt in body {
                collect_expr_captures_from_stmt(stmt, bound, env, signatures, captures);
            }
        }
        Stmt::Repeat { body, condition } => {
            for stmt in body {
                collect_expr_captures_from_stmt(stmt, bound, env, signatures, captures);
            }
            collect_expr_captures(condition, bound, env, signatures, captures);
        }
        Stmt::NumericFor {
            name,
            start,
            stop,
            step,
            body,
        } => {
            collect_expr_captures(start, bound, env, signatures, captures);
            collect_expr_captures(stop, bound, env, signatures, captures);
            if let Some(step_expr) = step {
                collect_expr_captures(step_expr, bound, env, signatures, captures);
            }
            let mut nested_bound = bound.clone();
            nested_bound.insert(name.clone());
            for stmt in body {
                collect_expr_captures_from_stmt(stmt, &nested_bound, env, signatures, captures);
            }
        }
        Stmt::ForIn {
            names,
            iterator,
            body,
        } => {
            collect_expr_captures(iterator, bound, env, signatures, captures);
            let mut nested_bound = bound.clone();
            for name in names {
                nested_bound.insert(name.clone());
            }
            for stmt in body {
                collect_expr_captures_from_stmt(stmt, &nested_bound, env, signatures, captures);
            }
        }
        Stmt::Return(expr) | Stmt::Expr(expr) => {
            collect_expr_captures(expr, bound, env, signatures, captures)
        }
        Stmt::ReturnMulti(values) => {
            for value in values {
                collect_expr_captures(value, bound, env, signatures, captures);
            }
        }
        Stmt::LetMulti { values, .. } => {
            for value in values {
                collect_expr_captures(value, bound, env, signatures, captures);
            }
        }
        Stmt::AssignMulti { targets, values } => {
            for target in targets {
                if !bound.contains(target)
                    && env.contains_key(target)
                    && !signatures.contains_key(target)
                {
                    captures.insert(target.clone());
                }
            }
            for value in values {
                collect_expr_captures(value, bound, env, signatures, captures);
            }
        }
        Stmt::Break | Stmt::Continue => {}
    }
}

fn collect_expr_captures(
    expr: &Expr,
    bound: &HashSet<String>,
    env: &HashMap<String, ValueId>,
    signatures: &HashMap<String, (Vec<Type>, Type)>,
    captures: &mut BTreeSet<String>,
) {
    match expr {
        Expr::Name(name, _) => {
            if !bound.contains(name) && env.contains_key(name) && !signatures.contains_key(name) {
                captures.insert(name.clone());
            }
        }
        Expr::Unary { expr, .. } | Expr::Cast { expr, .. } => {
            collect_expr_captures(expr, bound, env, signatures, captures)
        }
        Expr::Binary { left, right, .. } => {
            collect_expr_captures(left, bound, env, signatures, captures);
            collect_expr_captures(right, bound, env, signatures, captures);
        }
        Expr::If {
            condition,
            then_expr,
            else_expr,
            ..
        } => {
            collect_expr_captures(condition, bound, env, signatures, captures);
            collect_expr_captures(then_expr, bound, env, signatures, captures);
            collect_expr_captures(else_expr, bound, env, signatures, captures);
        }
        Expr::Call {
            callee,
            type_args: _,
            args,
            ..
        } => {
            collect_expr_captures(callee, bound, env, signatures, captures);
            for arg in args {
                collect_expr_captures(arg, bound, env, signatures, captures);
            }
        }
        Expr::Function(_) => {}
        Expr::ArrayLiteral { elements, .. } => {
            for element in elements {
                collect_expr_captures(element, bound, env, signatures, captures);
            }
        }
        Expr::TableLiteral { fields, .. } => {
            for field in fields {
                collect_expr_captures(&field.value, bound, env, signatures, captures);
            }
        }
        Expr::Field { base, .. } => {
            collect_expr_captures(base, bound, env, signatures, captures);
        }
        Expr::Index { base, index, .. } => {
            collect_expr_captures(base, bound, env, signatures, captures);
            collect_expr_captures(index, bound, env, signatures, captures);
        }
        Expr::Number(..) | Expr::Bool(..) | Expr::String(..) | Expr::Require(..) => {}
    }
}

fn collect_assigned_into(stmts: &[Stmt], out: &mut BTreeSet<String>) {
    for stmt in stmts {
        match stmt {
            Stmt::Let { name, .. } | Stmt::Assign { name, .. } => {
                out.insert(name.clone());
            }
            Stmt::LetMulti { bindings, .. } => {
                for binding in bindings {
                    out.insert(binding.name.clone());
                }
            }
            Stmt::AssignMulti { targets, .. } => {
                for target in targets {
                    out.insert(target.clone());
                }
            }
            Stmt::IndexAssign { .. } | Stmt::FieldAssign { .. } => {}
            Stmt::If {
                then_body,
                else_body,
                ..
            } => {
                collect_assigned_into(then_body, out);
                collect_assigned_into(else_body, out);
            }
            Stmt::While { body, .. } => collect_assigned_into(body, out),
            Stmt::Repeat { body, .. } => collect_assigned_into(body, out),
            Stmt::NumericFor { body, .. } => collect_assigned_into(body, out),
            Stmt::ForIn { body, .. } => collect_assigned_into(body, out),
            Stmt::Return(_)
            | Stmt::ReturnMulti(_)
            | Stmt::Expr(_)
            | Stmt::Break
            | Stmt::Continue => {}
        }
    }
}

/// Collect free variable names referenced by any nested FunctionExpr within `function`.
/// This returns a set of identifier names that are referenced inside nested functions
/// and are not bound by those nested functions' parameter lists or self name.
pub(crate) fn collect_nested_function_capture_names(
    function: &waluau_ast::Function,
) -> HashSet<String> {
    let mut out = HashSet::new();
    for stmt in &function.body {
        collect_nested_from_stmt(stmt, &mut out);
    }
    out
}

fn collect_nested_from_stmt(stmt: &Stmt, out: &mut HashSet<String>) {
    match stmt {
        Stmt::Let { value, .. }
        | Stmt::Assign { value, .. }
        | Stmt::Expr(value)
        | Stmt::Return(value) => collect_nested_from_expr(value, out),
        Stmt::ReturnMulti(values)
        | Stmt::LetMulti { values, .. }
        | Stmt::AssignMulti { values, .. } => {
            for v in values {
                collect_nested_from_expr(v, out);
            }
        }
        Stmt::IndexAssign {
            base, index, value, ..
        } => {
            collect_nested_from_expr(base, out);
            collect_nested_from_expr(index, out);
            collect_nested_from_expr(value, out);
        }
        Stmt::FieldAssign { base, value, .. } => {
            collect_nested_from_expr(base, out);
            collect_nested_from_expr(value, out);
        }
        Stmt::If {
            condition,
            then_body,
            else_body,
        } => {
            collect_nested_from_expr(condition, out);
            for s in then_body {
                collect_nested_from_stmt(s, out);
            }
            for s in else_body {
                collect_nested_from_stmt(s, out);
            }
        }
        Stmt::While { condition, body } => {
            collect_nested_from_expr(condition, out);
            for s in body {
                collect_nested_from_stmt(s, out);
            }
        }
        Stmt::Repeat { body, condition } => {
            for s in body {
                collect_nested_from_stmt(s, out);
            }
            collect_nested_from_expr(condition, out);
        }
        Stmt::NumericFor {
            start,
            stop,
            step,
            body,
            ..
        } => {
            collect_nested_from_expr(start, out);
            collect_nested_from_expr(stop, out);
            if let Some(step_expr) = step {
                collect_nested_from_expr(step_expr, out);
            }
            for s in body {
                collect_nested_from_stmt(s, out);
            }
        }
        Stmt::ForIn { iterator, body, .. } => {
            collect_nested_from_expr(iterator, out);
            for s in body {
                collect_nested_from_stmt(s, out);
            }
        }
        Stmt::Break | Stmt::Continue => {}
    }
}

fn collect_nested_from_expr(expr: &Expr, out: &mut HashSet<String>) {
    match expr {
        Expr::Function(function) => {
            // collect free names within this function expression
            let mut bound: HashSet<String> =
                function.params.iter().map(|p| p.name.clone()).collect();
            if let Some(name) = &function.name {
                bound.insert(name.clone());
            }
            collect_free_names_in_stmts(&function.body, &bound, out);
            // Recurse into nested function expressions
            for stmt in &function.body {
                collect_nested_from_stmt(stmt, out);
            }
        }
        Expr::Unary { expr, .. } | Expr::Cast { expr, .. } => collect_nested_from_expr(expr, out),
        Expr::Binary { left, right, .. } => {
            collect_nested_from_expr(left, out);
            collect_nested_from_expr(right, out);
        }
        Expr::If {
            condition,
            then_expr,
            else_expr,
            ..
        } => {
            collect_nested_from_expr(condition, out);
            collect_nested_from_expr(then_expr, out);
            collect_nested_from_expr(else_expr, out);
        }
        Expr::Call {
            callee,
            type_args: _,
            args,
            ..
        } => {
            collect_nested_from_expr(callee, out);
            for a in args {
                collect_nested_from_expr(a, out);
            }
        }
        Expr::ArrayLiteral { elements, .. } => {
            for e in elements {
                collect_nested_from_expr(e, out);
            }
        }
        Expr::TableLiteral { fields, .. } => {
            for field in fields {
                collect_nested_from_expr(&field.value, out);
            }
        }
        Expr::Field { base, .. } => collect_nested_from_expr(base, out),
        Expr::Index { base, index, .. } => {
            collect_nested_from_expr(base, out);
            collect_nested_from_expr(index, out);
        }
        Expr::Name(..)
        | Expr::Number(..)
        | Expr::Bool(..)
        | Expr::String(..)
        | Expr::Require(..) => {}
    }
}

fn collect_free_names_in_stmts(stmts: &[Stmt], bound: &HashSet<String>, out: &mut HashSet<String>) {
    for stmt in stmts {
        match stmt {
            Stmt::Let {
                name: _,
                rebindability: _,
                ty: _,
                value,
            } => collect_free_names_in_expr(value, bound, out),
            Stmt::Assign { name, value, .. } => {
                if !bound.contains(name) {
                    out.insert(name.clone());
                }
                collect_free_names_in_expr(value, bound, out)
            }
            Stmt::IndexAssign {
                base, index, value, ..
            } => {
                collect_free_names_in_expr(base, bound, out);
                collect_free_names_in_expr(index, bound, out);
                collect_free_names_in_expr(value, bound, out);
            }
            Stmt::FieldAssign { base, value, .. } => {
                collect_free_names_in_expr(base, bound, out);
                collect_free_names_in_expr(value, bound, out);
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                collect_free_names_in_expr(condition, bound, out);
                for s in then_body {
                    collect_free_names_in_stmts(std::slice::from_ref(s), bound, out);
                }
                for s in else_body {
                    collect_free_names_in_stmts(std::slice::from_ref(s), bound, out);
                }
            }
            Stmt::While { condition, body } => {
                collect_free_names_in_expr(condition, bound, out);
                for s in body {
                    collect_free_names_in_stmts(std::slice::from_ref(s), bound, out);
                }
            }
            Stmt::Repeat { body, condition } => {
                for s in body {
                    collect_free_names_in_stmts(std::slice::from_ref(s), bound, out);
                }
                collect_free_names_in_expr(condition, bound, out);
            }
            Stmt::NumericFor {
                name,
                start,
                stop,
                step,
                body,
            } => {
                collect_free_names_in_expr(start, bound, out);
                collect_free_names_in_expr(stop, bound, out);
                if let Some(step_expr) = step {
                    collect_free_names_in_expr(step_expr, bound, out);
                }
                let mut nested_bound = bound.clone();
                nested_bound.insert(name.clone());
                for s in body {
                    collect_free_names_in_stmts(std::slice::from_ref(s), &nested_bound, out);
                }
            }
            Stmt::ForIn {
                names,
                iterator,
                body,
            } => {
                collect_free_names_in_expr(iterator, bound, out);
                let mut nested_bound = bound.clone();
                for name in names {
                    nested_bound.insert(name.clone());
                }
                for s in body {
                    collect_free_names_in_stmts(std::slice::from_ref(s), &nested_bound, out);
                }
            }
            Stmt::Return(expr) | Stmt::Expr(expr) => {
                collect_free_names_in_expr(expr, bound, out);
            }
            Stmt::ReturnMulti(values)
            | Stmt::LetMulti { values, .. }
            | Stmt::AssignMulti { values, .. } => {
                for v in values {
                    collect_free_names_in_expr(v, bound, out);
                }
            }
            Stmt::Break | Stmt::Continue => {}
        }
    }
}

fn collect_free_names_in_expr(expr: &Expr, bound: &HashSet<String>, out: &mut HashSet<String>) {
    match expr {
        Expr::Name(name, _) => {
            if !bound.contains(name) {
                out.insert(name.clone());
            }
        }
        Expr::Unary { expr, .. } | Expr::Cast { expr, .. } => {
            collect_free_names_in_expr(expr, bound, out)
        }
        Expr::Binary { left, right, .. } => {
            collect_free_names_in_expr(left, bound, out);
            collect_free_names_in_expr(right, bound, out);
        }
        Expr::If {
            condition,
            then_expr,
            else_expr,
            ..
        } => {
            collect_free_names_in_expr(condition, bound, out);
            collect_free_names_in_expr(then_expr, bound, out);
            collect_free_names_in_expr(else_expr, bound, out);
        }
        Expr::Call {
            callee,
            type_args: _,
            args,
            ..
        } => {
            collect_free_names_in_expr(callee, bound, out);
            for a in args {
                collect_free_names_in_expr(a, bound, out);
            }
        }
        Expr::Function(function) => {
            // nested function - skip its own bound names when collecting free in its body
            let mut nested_bound: HashSet<String> =
                function.params.iter().map(|p| p.name.clone()).collect();
            if let Some(name) = &function.name {
                nested_bound.insert(name.clone());
            }
            collect_free_names_in_stmts(&function.body, &nested_bound, out);
        }
        Expr::ArrayLiteral { elements, .. } => {
            for e in elements {
                collect_free_names_in_expr(e, bound, out);
            }
        }
        Expr::TableLiteral { fields, .. } => {
            for field in fields {
                collect_free_names_in_expr(&field.value, bound, out);
            }
        }
        Expr::Field { base, .. } => collect_free_names_in_expr(base, bound, out),
        Expr::Index { base, index, .. } => {
            collect_free_names_in_expr(base, bound, out);
            collect_free_names_in_expr(index, bound, out);
        }
        Expr::Number(..) | Expr::Bool(..) | Expr::String(..) | Expr::Require(..) => {}
    }
}
