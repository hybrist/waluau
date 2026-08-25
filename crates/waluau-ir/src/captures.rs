use std::collections::{BTreeSet, HashMap, HashSet};
use waluau_ast::{Expr, Stmt, SymbolId, Type};
use crate::ValueId;

pub(crate) fn collect_assigned_names(stmts: &[Stmt]) -> BTreeSet<SymbolId> {
    let mut assigned = BTreeSet::new();
    collect_assigned_into(stmts, &mut assigned);
    assigned
}

fn collect_assigned_into(stmts: &[Stmt], out: &mut BTreeSet<SymbolId>) {
    for stmt in stmts {
        match stmt {
            Stmt::Let { symbol_id, .. } | Stmt::Assign { symbol_id, .. } => {
                if let Some(id) = symbol_id {
                    out.insert(*id);
                }
            }
            Stmt::LetMulti { bindings, .. } => {
                for binding in bindings {
                    if let Some(id) = binding.symbol_id {
                        out.insert(id);
                    }
                }
            }
            Stmt::AssignMulti { symbol_ids, .. } => {
                if let Some(ids) = symbol_ids {
                    for id in ids {
                        out.insert(*id);
                    }
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
            Stmt::IfCast {
                then_body,
                else_body,
                ..
            } => {
                collect_assigned_into(then_body, out);
                collect_assigned_into(else_body, out);
            }
            Stmt::Match { arms, .. } => {
                for arm in arms {
                    collect_assigned_into(&arm.body, out);
                }
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

pub(crate) fn collect_captures(
    function: &waluau_ast::FunctionExpr,
    env: &HashMap<SymbolId, ValueId>,
    types: &HashMap<SymbolId, Type>,
    signatures: &HashMap<SymbolId, (Vec<Type>, Type)>,
) -> Vec<(SymbolId, Type)> {
    let mut bound: HashSet<SymbolId> = function
        .params
        .iter()
        .filter_map(|param| param.symbol_id)
        .collect();
    if let Some(symbol_id) = function.symbol_id {
        bound.insert(symbol_id);
    }
    let mut captures = BTreeSet::new();
    for stmt in &function.body {
        collect_expr_captures_from_stmt(stmt, &bound, env, signatures, &mut captures);
    }
    captures
        .into_iter()
        .filter_map(|symbol_id| {
            env.get(&symbol_id)?;
            let ty = types.get(&symbol_id)?.clone();
            Some((symbol_id, ty))
        })
        .collect()
}

fn collect_expr_captures_from_stmt(
    stmt: &Stmt,
    bound: &HashSet<SymbolId>,
    env: &HashMap<SymbolId, ValueId>,
    signatures: &HashMap<SymbolId, (Vec<Type>, Type)>,
    captures: &mut BTreeSet<SymbolId>,
) {
    match stmt {
        Stmt::Let { value, .. } => collect_expr_captures(value, bound, env, signatures, captures),
        Stmt::Assign { symbol_id, value, .. } => {
            if let Some(id) = symbol_id {
                if !bound.contains(id) && env.contains_key(id) && !signatures.contains_key(id) {
                    captures.insert(*id);
                }
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
            for s in then_body {
                collect_expr_captures_from_stmt(s, bound, env, signatures, captures);
            }
            for s in else_body {
                collect_expr_captures_from_stmt(s, bound, env, signatures, captures);
            }
        }
        Stmt::IfCast {
            binding_symbol_id,
            value,
            then_body,
            else_body,
            ..
        } => {
            collect_expr_captures(value, bound, env, signatures, captures);
            let mut then_bound = bound.clone();
            if let Some(id) = binding_symbol_id {
                then_bound.insert(*id);
            }
            for s in then_body {
                collect_expr_captures_from_stmt(s, &then_bound, env, signatures, captures);
            }
            for s in else_body {
                collect_expr_captures_from_stmt(s, bound, env, signatures, captures);
            }
        }
        Stmt::Match { value, arms, .. } => {
            collect_expr_captures(value, bound, env, signatures, captures);
            for arm in arms {
                for stmt in &arm.body {
                    collect_expr_captures_from_stmt(stmt, bound, env, signatures, captures);
                }
            }
        }
        Stmt::While { condition, body } => {
            collect_expr_captures(condition, bound, env, signatures, captures);
            for s in body {
                collect_expr_captures_from_stmt(s, bound, env, signatures, captures);
            }
        }
        Stmt::Repeat { body, condition } => {
            for s in body {
                collect_expr_captures_from_stmt(s, bound, env, signatures, captures);
            }
            collect_expr_captures(condition, bound, env, signatures, captures);
        }
        Stmt::NumericFor {
            symbol_id,
            start,
            stop,
            step,
            body,
            ..
        } => {
            collect_expr_captures(start, bound, env, signatures, captures);
            collect_expr_captures(stop, bound, env, signatures, captures);
            if let Some(step_expr) = step {
                collect_expr_captures(step_expr, bound, env, signatures, captures);
            }
            let mut nested_bound = bound.clone();
            if let Some(id) = symbol_id {
                nested_bound.insert(*id);
            }
            for s in body {
                collect_expr_captures_from_stmt(s, &nested_bound, env, signatures, captures);
            }
        }
        Stmt::ForIn {
            symbol_ids,
            iterators,
            body,
            ..
        } => {
            for iterator in iterators {
                collect_expr_captures(iterator, bound, env, signatures, captures);
            }
            let mut nested_bound = bound.clone();
            if let Some(ids) = symbol_ids {
                for id in ids {
                    nested_bound.insert(*id);
                }
            }
            for s in body {
                collect_expr_captures_from_stmt(s, &nested_bound, env, signatures, captures);
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
        Stmt::AssignMulti { symbol_ids, values, .. } => {
            if let Some(ids) = symbol_ids {
                for id in ids {
                    if !bound.contains(id) && env.contains_key(id) && !signatures.contains_key(id) {
                        captures.insert(*id);
                    }
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
    bound: &HashSet<SymbolId>,
    env: &HashMap<SymbolId, ValueId>,
    signatures: &HashMap<SymbolId, (Vec<Type>, Type)>,
    captures: &mut BTreeSet<SymbolId>,
) {
    match expr {
        Expr::Name(_, Some(symbol_id), _) => {
            if !bound.contains(symbol_id) && env.contains_key(symbol_id) && !signatures.contains_key(symbol_id) {
                captures.insert(*symbol_id);
            }
        }
        Expr::Unary { expr, .. } | Expr::Cast { expr, .. } => {
            collect_expr_captures(expr, bound, env, signatures, captures)
        }
        Expr::IsVariant { expr, .. } => {
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
            args,
            ..
        } => {
            collect_expr_captures(callee, bound, env, signatures, captures);
            for arg in args {
                collect_expr_captures(arg, bound, env, signatures, captures);
            }
        }
        Expr::MethodCall { receiver, args, .. } => {
            collect_expr_captures(receiver, bound, env, signatures, captures);
            for arg in args {
                collect_expr_captures(arg, bound, env, signatures, captures);
            }
        }
        Expr::Function(function) => {
            // Recurse into nested closures so transitively captured names
            // (used only by an inner closure) still become captures of this
            // function. Capture detection is symbol-id based, so inner
            // bindings that shadow outer names cannot be misdetected.
            for stmt in &function.body {
                collect_expr_captures_from_stmt(stmt, bound, env, signatures, captures);
            }
        }
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
        Expr::Name(_, None, _)
        | Expr::Number(..)
        | Expr::Bool(..)
        | Expr::Nil(..)
        | Expr::String(..)
        | Expr::Bytes(..)
        | Expr::Vararg(..)
        | Expr::Require(..) => {}
    }
}

pub(crate) fn collect_nested_function_capture_names(
    function: &waluau_ast::Function,
) -> HashSet<SymbolId> {
    let mut out = HashSet::new();
    for stmt in &function.body {
        collect_nested_from_stmt(stmt, &mut out);
    }
    out
}

fn collect_nested_from_stmt(stmt: &Stmt, out: &mut HashSet<SymbolId>) {
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
        Stmt::IfCast {
            value,
            then_body,
            else_body,
            ..
        } => {
            collect_nested_from_expr(value, out);
            for s in then_body {
                collect_nested_from_stmt(s, out);
            }
            for s in else_body {
                collect_nested_from_stmt(s, out);
            }
        }
        Stmt::Match { value, arms, .. } => {
            collect_nested_from_expr(value, out);
            for arm in arms {
                for stmt in &arm.body {
                    collect_nested_from_stmt(stmt, out);
                }
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
        Stmt::ForIn {
            iterators, body, ..
        } => {
            for iterator in iterators {
                collect_nested_from_expr(iterator, out);
            }
            for s in body {
                collect_nested_from_stmt(s, out);
            }
        }
        Stmt::Break | Stmt::Continue => {}
    }
}

fn collect_nested_from_expr(expr: &Expr, out: &mut HashSet<SymbolId>) {
    match expr {
        Expr::Function(function) => {
            // collect free symbol IDs within this function expression
            let mut bound: HashSet<SymbolId> =
                function.params.iter().filter_map(|p| p.symbol_id).collect();
            if let Some(symbol_id) = function.symbol_id {
                bound.insert(symbol_id);
            }
            collect_free_names_in_stmts(&function.body, &bound, out);
            // Recurse into nested function expressions
            for stmt in &function.body {
                collect_nested_from_stmt(stmt, out);
            }
        }
        Expr::Unary { expr, .. } | Expr::Cast { expr, .. } => collect_nested_from_expr(expr, out),
        Expr::IsVariant { expr, .. } => {
            collect_nested_from_expr(expr, out)
        }
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
            args,
            ..
        } => {
            collect_nested_from_expr(callee, out);
            for a in args {
                collect_nested_from_expr(a, out);
            }
        }
        Expr::MethodCall { receiver, args, .. } => {
            collect_nested_from_expr(receiver, out);
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
        | Expr::Nil(..)
        | Expr::String(..)
        | Expr::Bytes(..)
        | Expr::Vararg(..)
        | Expr::Require(..) => {}
    }
}

fn collect_free_names_in_stmts(stmts: &[Stmt], bound: &HashSet<SymbolId>, out: &mut HashSet<SymbolId>) {
    for stmt in stmts {
        match stmt {
            Stmt::Let {
                value,
                ..
            } => collect_free_names_in_expr(value, bound, out),
            Stmt::Assign { symbol_id, value, .. } => {
                if let Some(id) = symbol_id {
                    if !bound.contains(id) {
                        out.insert(*id);
                    }
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
            Stmt::IfCast {
                binding_symbol_id,
                value,
                then_body,
                else_body,
                ..
            } => {
                collect_free_names_in_expr(value, bound, out);
                let mut then_bound = bound.clone();
                if let Some(id) = binding_symbol_id {
                    then_bound.insert(*id);
                }
                for s in then_body {
                    collect_free_names_in_stmts(std::slice::from_ref(s), &then_bound, out);
                }
                for s in else_body {
                    collect_free_names_in_stmts(std::slice::from_ref(s), bound, out);
                }
            }
            Stmt::Match { value, arms, .. } => {
                collect_free_names_in_expr(value, bound, out);
                for arm in arms {
                    collect_free_names_in_stmts(&arm.body, bound, out);
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
                symbol_id,
                start,
                stop,
                step,
                body,
                ..
            } => {
                collect_free_names_in_expr(start, bound, out);
                collect_free_names_in_expr(stop, bound, out);
                if let Some(step_expr) = step {
                    collect_free_names_in_expr(step_expr, bound, out);
                }
                let mut nested_bound = bound.clone();
                if let Some(id) = symbol_id {
                    nested_bound.insert(*id);
                }
                for s in body {
                    collect_free_names_in_stmts(std::slice::from_ref(s), &nested_bound, out);
                }
            }
            Stmt::ForIn {
                symbol_ids,
                iterators,
                body,
                ..
            } => {
                for iterator in iterators {
                    collect_free_names_in_expr(iterator, bound, out);
                }
                let mut nested_bound = bound.clone();
                if let Some(ids) = symbol_ids {
                    for id in ids {
                        nested_bound.insert(*id);
                    }
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

fn collect_free_names_in_expr(expr: &Expr, bound: &HashSet<SymbolId>, out: &mut HashSet<SymbolId>) {
    match expr {
        Expr::Name(_, Some(symbol_id), _) => {
            if !bound.contains(symbol_id) {
                out.insert(*symbol_id);
            }
        }
        Expr::Unary { expr, .. } | Expr::Cast { expr, .. } => {
            collect_free_names_in_expr(expr, bound, out)
        }
        Expr::IsVariant { expr, .. } => {
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
            args,
            ..
        } => {
            collect_free_names_in_expr(callee, bound, out);
            for a in args {
                collect_free_names_in_expr(a, bound, out);
            }
        }
        Expr::MethodCall { receiver, args, .. } => {
            collect_free_names_in_expr(receiver, bound, out);
            for a in args {
                collect_free_names_in_expr(a, bound, out);
            }
        }
        Expr::Function(function) => {
            // nested function - skip its own bound names when collecting free in its body
            let mut nested_bound: HashSet<SymbolId> =
                function.params.iter().filter_map(|p| p.symbol_id).collect();
            if let Some(symbol_id) = function.symbol_id {
                nested_bound.insert(symbol_id);
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
        Expr::Name(_, None, _)
        | Expr::Number(..)
        | Expr::Bool(..)
        | Expr::Nil(..)
        | Expr::String(..)
        | Expr::Bytes(..)
        | Expr::Vararg(..)
        | Expr::Require(..) => {}
    }
}
