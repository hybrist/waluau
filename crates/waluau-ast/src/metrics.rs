//! Workload metrics over a linked [`Program`], used by build reports and the
//! performance benchmarks. A "node" is one statement or one expression; types,
//! names, and spans are attributes of a node, not nodes themselves.

use crate::{Expr, Program, Stmt};

/// Number of statement and expression nodes in `program`, including function
/// bodies, declared top-level statements, and the module export expression.
pub fn node_count(program: &Program) -> usize {
    let mut count = 0;
    for function in &program.functions {
        count += stmts(&function.body);
    }
    count += stmts(&program.top_level);
    if let Some(export) = &program.export {
        count += expr(export);
    }
    count
}

fn stmts(body: &[Stmt]) -> usize {
    body.iter().map(stmt).sum()
}

fn stmt(statement: &Stmt) -> usize {
    1 + match statement {
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => expr(value),
        Stmt::IndexAssign {
            base, index, value, ..
        } => expr(base) + expr(index) + expr(value),
        Stmt::FieldAssign { base, value, .. } => expr(base) + expr(value),
        Stmt::If {
            condition,
            then_body,
            else_body,
        } => expr(condition) + stmts(then_body) + stmts(else_body),
        Stmt::IfCast {
            value,
            then_body,
            else_body,
            ..
        } => expr(value) + stmts(then_body) + stmts(else_body),
        Stmt::Match { value, arms, .. } => {
            expr(value) + arms.iter().map(|arm| stmts(&arm.body)).sum::<usize>()
        }
        Stmt::While { condition, body } | Stmt::Repeat { body, condition } => {
            expr(condition) + stmts(body)
        }
        Stmt::NumericFor {
            start,
            stop,
            step,
            body,
            ..
        } => expr(start) + expr(stop) + step.as_ref().map_or(0, expr) + stmts(body),
        Stmt::ForIn {
            iterators, body, ..
        } => exprs(iterators) + stmts(body),
        Stmt::Break | Stmt::Continue => 0,
        Stmt::Return(value) | Stmt::Expr(value) => expr(value),
        Stmt::ReturnMulti(values) | Stmt::LetMulti { values, .. } => exprs(values),
        Stmt::AssignMulti { values, .. } => exprs(values),
    }
}

fn exprs(list: &[Expr]) -> usize {
    list.iter().map(expr).sum()
}

fn expr(expression: &Expr) -> usize {
    1 + match expression {
        Expr::Number(..)
        | Expr::Bool(..)
        | Expr::Nil(..)
        | Expr::String(..)
        | Expr::Bytes(..)
        | Expr::Name(..)
        | Expr::Vararg(..)
        | Expr::Require(..) => 0,
        Expr::Unary { expr: inner, .. }
        | Expr::Cast { expr: inner, .. }
        | Expr::IsVariant { expr: inner, .. } => expr(inner),
        Expr::Binary { left, right, .. } => expr(left) + expr(right),
        Expr::If {
            condition,
            then_expr,
            else_expr,
            ..
        } => expr(condition) + expr(then_expr) + expr(else_expr),
        Expr::Call { callee, args, .. } => expr(callee) + exprs(args),
        Expr::MethodCall { receiver, args, .. } => expr(receiver) + exprs(args),
        Expr::Function(function) => stmts(&function.body),
        Expr::ArrayLiteral { elements, .. } => exprs(elements),
        Expr::TableLiteral { fields, .. } => {
            fields.iter().map(|field| expr(&field.value)).sum::<usize>()
        }
        Expr::Field { base, .. } => expr(base),
        Expr::Index { base, index, .. } => expr(base) + expr(index),
    }
}
