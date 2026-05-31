use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use waluau_ast::{
    AssignOp, BinaryOp, Expr, Function as AstFunction, NumberLiteral, NumericType, Program, Stmt,
    Type, UnaryOp,
};
use waluau_diagnostics::{Diagnostic, DiagnosticCategory};

const COROUTINE_CREATE: &str = "coroutine_create";
const COROUTINE_RESUME: &str = "coroutine_resume";
const COROUTINE_STATUS: &str = "coroutine_status";
const MATH_ABS: &str = "math_abs";
const MATH_MIN: &str = "math_min";
const MATH_MAX: &str = "math_max";
const MATH_SQRT: &str = "math_sqrt";
const MATH_FLOOR: &str = "math_floor";
const MATH_CEIL: &str = "math_ceil";
const MATH_TRUNC: &str = "math_trunc";
const MATH_NEAREST: &str = "math_nearest";
const MATH_COPYSIGN: &str = "math_copysign";
const TO_STRING: &str = "tostring";
const ASSERT: &str = "assert";
const PRINT: &str = "print";

fn inference_diagnostic(
    code: &'static str,
    category: DiagnosticCategory,
    message: impl Into<String>,
    action: impl Into<String>,
) -> Diagnostic {
    Diagnostic::new(message)
        .with_code(code)
        .with_category(category)
        .with_action(action)
}

fn generic_diagnostic(code: &'static str, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(message)
        .with_code(code)
        .with_category(DiagnosticCategory::Unsupported)
}

fn substitute_type(ty: &Type, subst: &HashMap<String, Type>) -> Type {
    match ty {
        Type::TypeParam(name) => subst
            .get(name)
            .cloned()
            .unwrap_or_else(|| Type::TypeParam(name.clone())),
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

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct SpecializationKey {
    generic_name: String,
    type_args: Vec<Type>,
}

#[derive(Clone, Debug)]
struct ActiveSpecialization {
    generic_name: String,
    type_args: Vec<Type>,
}

struct Monomorphizer<'a> {
    generic_functions: HashMap<String, &'a AstFunction>,
    specialized_names: HashMap<SpecializationKey, String>,
    pending: Vec<SpecializationKey>,
}

impl<'a> Monomorphizer<'a> {
    fn new(program: &'a Program) -> Self {
        let generic_functions = program
            .functions
            .iter()
            .filter(|function| !function.type_params.is_empty())
            .map(|function| (function.name.clone(), function))
            .collect();
        Self {
            generic_functions,
            specialized_names: HashMap::new(),
            pending: Vec::new(),
        }
    }

    fn run(&mut self, program: &Program) -> Result<Program, Diagnostic> {
        let mut functions = program
            .functions
            .iter()
            .filter(|function| function.type_params.is_empty())
            .map(|function| self.rewrite_function(function, &HashMap::new(), None))
            .collect::<Result<Vec<_>, _>>()?;

        while let Some(key) = self.pending.pop() {
            let template = self
                .generic_functions
                .get(&key.generic_name)
                .copied()
                .ok_or_else(|| {
                    Diagnostic::new(format!(
                        "missing generic function '{}' during monomorphization",
                        key.generic_name
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
                generic_name: template.name.clone(),
                type_args: key.type_args.clone(),
            };
            functions.push(self.rewrite_function_with_name(
                template,
                specialized_name,
                &subst,
                Some(&active),
            )?);
        }

        Ok(Program {
            functions,
            top_level: program.top_level.clone(),
            export: program.export.clone(),
        })
    }

    fn rewrite_function(
        &mut self,
        function: &AstFunction,
        subst: &HashMap<String, Type>,
        active: Option<&ActiveSpecialization>,
    ) -> Result<AstFunction, Diagnostic> {
        self.rewrite_function_with_name(function, function.name.clone(), subst, active)
    }

    fn rewrite_function_with_name(
        &mut self,
        function: &AstFunction,
        name: String,
        subst: &HashMap<String, Type>,
        active: Option<&ActiveSpecialization>,
    ) -> Result<AstFunction, Diagnostic> {
        Ok(AstFunction {
            name,
            type_params: Vec::new(),
            params: function
                .params
                .iter()
                .map(|param| waluau_ast::Param {
                    name: param.name.clone(),
                    ty: substitute_type(&param.ty, subst),
                })
                .collect(),
            return_type: function
                .return_type
                .as_ref()
                .map(|ty| substitute_type(ty, subst)),
            body: self.rewrite_stmts(&function.body, subst, active)?,
        })
    }

    fn rewrite_stmts(
        &mut self,
        stmts: &[Stmt],
        subst: &HashMap<String, Type>,
        active: Option<&ActiveSpecialization>,
    ) -> Result<Vec<Stmt>, Diagnostic> {
        stmts
            .iter()
            .map(|stmt| self.rewrite_stmt(stmt, subst, active))
            .collect()
    }

    fn rewrite_stmt(
        &mut self,
        stmt: &Stmt,
        subst: &HashMap<String, Type>,
        active: Option<&ActiveSpecialization>,
    ) -> Result<Stmt, Diagnostic> {
        Ok(match stmt {
            Stmt::Let {
                name,
                rebindability,
                ty,
                value,
            } => Stmt::Let {
                name: name.clone(),
                rebindability: *rebindability,
                ty: ty.as_ref().map(|ty| substitute_type(ty, subst)),
                value: self.rewrite_expr(value, subst, active)?,
            },
            Stmt::Assign { op, name, value } => Stmt::Assign {
                op: *op,
                name: name.clone(),
                value: self.rewrite_expr(value, subst, active)?,
            },
            Stmt::IndexAssign {
                op,
                base,
                index,
                value,
            } => Stmt::IndexAssign {
                op: *op,
                base: Box::new(self.rewrite_expr(base, subst, active)?),
                index: Box::new(self.rewrite_expr(index, subst, active)?),
                value: self.rewrite_expr(value, subst, active)?,
            },
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => Stmt::If {
                condition: self.rewrite_expr(condition, subst, active)?,
                then_body: self.rewrite_stmts(then_body, subst, active)?,
                else_body: self.rewrite_stmts(else_body, subst, active)?,
            },
            Stmt::While { condition, body } => Stmt::While {
                condition: self.rewrite_expr(condition, subst, active)?,
                body: self.rewrite_stmts(body, subst, active)?,
            },
            Stmt::Repeat { body, condition } => Stmt::Repeat {
                body: self.rewrite_stmts(body, subst, active)?,
                condition: self.rewrite_expr(condition, subst, active)?,
            },
            Stmt::Break => Stmt::Break,
            Stmt::Continue => Stmt::Continue,
            Stmt::Return(expr) => Stmt::Return(self.rewrite_expr(expr, subst, active)?),
            Stmt::ReturnMulti(values) => Stmt::ReturnMulti(
                values
                    .iter()
                    .map(|expr| self.rewrite_expr(expr, subst, active))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            Stmt::LetMulti { bindings, values } => Stmt::LetMulti {
                bindings: bindings
                    .iter()
                    .map(|binding| waluau_ast::Binding {
                        name: binding.name.clone(),
                        rebindability: binding.rebindability,
                        ty: binding.ty.as_ref().map(|ty| substitute_type(ty, subst)),
                    })
                    .collect(),
                values: values
                    .iter()
                    .map(|expr| self.rewrite_expr(expr, subst, active))
                    .collect::<Result<Vec<_>, _>>()?,
            },
            Stmt::AssignMulti { targets, values } => Stmt::AssignMulti {
                targets: targets.clone(),
                values: values
                    .iter()
                    .map(|expr| self.rewrite_expr(expr, subst, active))
                    .collect::<Result<Vec<_>, _>>()?,
            },
            Stmt::Expr(expr) => Stmt::Expr(self.rewrite_expr(expr, subst, active)?),
        })
    }

    fn rewrite_expr(
        &mut self,
        expr: &Expr,
        subst: &HashMap<String, Type>,
        active: Option<&ActiveSpecialization>,
    ) -> Result<Expr, Diagnostic> {
        Ok(match expr {
            Expr::Number(_)
            | Expr::Bool(_)
            | Expr::String(_)
            | Expr::Name(_)
            | Expr::Require(_) => expr.clone(),
            Expr::Unary { op, expr } => Expr::Unary {
                op: *op,
                expr: Box::new(self.rewrite_expr(expr, subst, active)?),
            },
            Expr::Cast { expr, ty } => Expr::Cast {
                expr: Box::new(self.rewrite_expr(expr, subst, active)?),
                ty: substitute_type(ty, subst),
            },
            Expr::Binary { op, left, right } => Expr::Binary {
                op: *op,
                left: Box::new(self.rewrite_expr(left, subst, active)?),
                right: Box::new(self.rewrite_expr(right, subst, active)?),
            },
            Expr::If {
                condition,
                then_expr,
                else_expr,
            } => Expr::If {
                condition: Box::new(self.rewrite_expr(condition, subst, active)?),
                then_expr: Box::new(self.rewrite_expr(then_expr, subst, active)?),
                else_expr: Box::new(self.rewrite_expr(else_expr, subst, active)?),
            },
            Expr::Call {
                callee,
                type_args,
                args,
            } => self.rewrite_call_expr(callee, type_args, args, subst, active)?,
            Expr::Function(function) => {
                Expr::Function(self.rewrite_function_expr(function, subst, active)?)
            }
            Expr::ArrayLiteral { elements } => Expr::ArrayLiteral {
                elements: elements
                    .iter()
                    .map(|expr| self.rewrite_expr(expr, subst, active))
                    .collect::<Result<Vec<_>, _>>()?,
            },
            Expr::TableLiteral { fields } => Expr::TableLiteral {
                fields: fields
                    .iter()
                    .map(|field| {
                        Ok(waluau_ast::TableField {
                            name: field.name.clone(),
                            value: self.rewrite_expr(&field.value, subst, active)?,
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            },
            Expr::Field { base, name } => Expr::Field {
                base: Box::new(self.rewrite_expr(base, subst, active)?),
                name: name.clone(),
            },
            Expr::Index { base, index } => Expr::Index {
                base: Box::new(self.rewrite_expr(base, subst, active)?),
                index: Box::new(self.rewrite_expr(index, subst, active)?),
            },
        })
    }

    fn rewrite_call_expr(
        &mut self,
        callee: &Expr,
        type_args: &[Type],
        args: &[Expr],
        subst: &HashMap<String, Type>,
        active: Option<&ActiveSpecialization>,
    ) -> Result<Expr, Diagnostic> {
        let args = args
            .iter()
            .map(|expr| self.rewrite_expr(expr, subst, active))
            .collect::<Result<Vec<_>, _>>()?;

        if let Expr::Name(name) = callee {
            if self.generic_functions.contains_key(name) {
                let concrete_type_args = type_args
                    .iter()
                    .map(|ty| substitute_type(ty, subst))
                    .collect::<Vec<_>>();
                self.check_recursive_specialization(name, &concrete_type_args, active)?;
                let specialized_name =
                    self.ensure_specialization(name, concrete_type_args.clone())?;
                return Ok(Expr::Call {
                    callee: Box::new(Expr::Name(specialized_name)),
                    type_args: Vec::new(),
                    args,
                });
            }
        }

        if let Expr::Function(function) = callee {
            if !function.type_params.is_empty() {
                let specialized =
                    self.specialize_function_expr(function, type_args, subst, active)?;
                return Ok(Expr::Call {
                    callee: Box::new(Expr::Function(specialized)),
                    type_args: Vec::new(),
                    args,
                });
            }
        }

        Ok(Expr::Call {
            callee: Box::new(self.rewrite_expr(callee, subst, active)?),
            type_args: type_args
                .iter()
                .map(|ty| substitute_type(ty, subst))
                .collect(),
            args,
        })
    }

    fn rewrite_function_expr(
        &mut self,
        function: &waluau_ast::FunctionExpr,
        subst: &HashMap<String, Type>,
        active: Option<&ActiveSpecialization>,
    ) -> Result<waluau_ast::FunctionExpr, Diagnostic> {
        if !function.type_params.is_empty() {
            return Err(generic_diagnostic(
                "generic/uninstantiated-value",
                "generic function expression must be instantiated before IR lowering",
            ));
        }
        Ok(waluau_ast::FunctionExpr {
            name: function.name.clone(),
            type_params: Vec::new(),
            params: function
                .params
                .iter()
                .map(|param| waluau_ast::Param {
                    name: param.name.clone(),
                    ty: substitute_type(&param.ty, subst),
                })
                .collect(),
            return_type: function
                .return_type
                .as_ref()
                .map(|ty| substitute_type(ty, subst)),
            body: self.rewrite_stmts(&function.body, subst, active)?,
        })
    }

    fn specialize_function_expr(
        &mut self,
        function: &waluau_ast::FunctionExpr,
        type_args: &[Type],
        subst: &HashMap<String, Type>,
        active: Option<&ActiveSpecialization>,
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
        Ok(waluau_ast::FunctionExpr {
            name: function.name.clone(),
            type_params: Vec::new(),
            params: function
                .params
                .iter()
                .map(|param| waluau_ast::Param {
                    name: param.name.clone(),
                    ty: substitute_type(&param.ty, &local_subst),
                })
                .collect(),
            return_type: function
                .return_type
                .as_ref()
                .map(|ty| substitute_type(ty, &local_subst)),
            body: self.rewrite_stmts(&function.body, &local_subst, active)?,
        })
    }

    fn ensure_specialization(
        &mut self,
        generic_name: &str,
        type_args: Vec<Type>,
    ) -> Result<String, Diagnostic> {
        let template = self
            .generic_functions
            .get(generic_name)
            .copied()
            .ok_or_else(|| {
                Diagnostic::new(format!(
                    "missing generic function '{}' during monomorphization",
                    generic_name
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
            generic_name: generic_name.to_string(),
            type_args,
        };
        if let Some(existing) = self.specialized_names.get(&key) {
            return Ok(existing.clone());
        }
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
        generic_name: &str,
        type_args: &[Type],
        active: Option<&ActiveSpecialization>,
    ) -> Result<(), Diagnostic> {
        let Some(active) = active else {
            return Ok(());
        };
        if active.generic_name == generic_name && active.type_args != type_args {
            return Err(generic_diagnostic(
                "generic/cross-specialization-recursion",
                format!(
                    "generic function '{generic_name}' cannot recursively instantiate different type arguments in this MVP"
                ),
            ));
        }
        Ok(())
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

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BlockId(pub usize);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ValueId(pub usize);

#[derive(Clone, Debug, PartialEq)]
pub struct Module {
    pub functions: Vec<Function>,
    pub start: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Function {
    pub name: String,
    pub params: Vec<(String, Type)>,
    pub return_type: Type,
    pub entry: BlockId,
    pub blocks: BTreeMap<BlockId, BasicBlock>,
    next_value: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BasicBlock {
    pub id: BlockId,
    pub instructions: Vec<(ValueId, Instruction)>,
    pub terminator: Terminator,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Instruction {
    Param(usize),
    Number {
        ty: NumericType,
        literal: NumberLiteral,
    },
    Bool(bool),
    String(String),
    Cast {
        value: ValueId,
        from: Type,
        to: Type,
    },
    Binary {
        op: BinaryOp,
        left: ValueId,
        right: ValueId,
        operand_ty: Type,
        result_ty: Type,
    },
    MathIntrinsic {
        intrinsic: MathIntrinsic,
        args: Vec<ValueId>,
        operand_ty: Type,
        result_ty: Type,
    },
    ToString {
        value: ValueId,
        from: Type,
    },
    Print {
        value: ValueId,
    },
    Call {
        name: String,
        args: Vec<ValueId>,
    },
    CallValue {
        callee: ValueId,
        args: Vec<ValueId>,
        params: Vec<Type>,
        return_type: Type,
    },
    Closure {
        name: String,
        captures: Vec<ValueId>,
        params: Vec<Type>,
        return_type: Type,
    },
    ArrayNew {
        element_ty: Type,
        elements: Vec<ValueId>,
    },
    ArrayGet {
        array: ValueId,
        index: ValueId,
        element_ty: Type,
    },
    ArraySet {
        array: ValueId,
        index: ValueId,
        value: ValueId,
        element_ty: Type,
    },
    ArrayLen {
        array: ValueId,
    },
    PackMulti {
        values: Vec<ValueId>,
        types: Vec<Type>,
    },
    MultiGet {
        value: ValueId,
        index: usize,
        ty: Type,
    },
    Phi(Vec<(BlockId, ValueId)>),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MathIntrinsic {
    Abs,
    Min,
    Max,
    Sqrt,
    Floor,
    Ceil,
    Trunc,
    Nearest,
    Copysign,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Terminator {
    Jump(BlockId),
    Branch {
        condition: ValueId,
        then_block: BlockId,
        else_block: BlockId,
    },
    Return(ValueId),
    Unreachable,
}

pub fn build(program: &Program) -> Result<Module, Diagnostic> {
    let monomorphic = Monomorphizer::new(program).run(program)?;
    let signatures: HashMap<_, _> = monomorphic
        .functions
        .iter()
        .map(|function| {
            let return_type = function.return_type.clone().ok_or_else(|| {
                Diagnostic::new(format!(
                    "function '{}' must have a concrete return type before IR lowering",
                    function.name
                ))
            })?;
            Ok((
                function.name.clone(),
                (
                    function
                        .params
                        .iter()
                        .map(|param| param.ty.clone())
                        .collect(),
                    return_type,
                ),
            ))
        })
        .collect::<Result<HashMap<_, _>, _>>()?;
    let mut functions = Vec::new();
    for function in &monomorphic.functions {
        let mut lowered = build_function(function, &signatures)?;
        functions.push(lowered.remove(0));
        functions.extend(lowered);
    }
    let start = functions
        .iter()
        .position(|function| function.name == "__waluau_top_level_init");
    let module = Module { functions, start };
    verify(&module)?;
    Ok(module)
}

pub fn verify(module: &Module) -> Result<(), Diagnostic> {
    let signatures: HashMap<_, _> = module
        .functions
        .iter()
        .map(|function| {
            (
                function.name.clone(),
                (
                    function
                        .params
                        .iter()
                        .map(|(_, ty)| ty.clone())
                        .collect::<Vec<_>>(),
                    function.return_type.clone(),
                ),
            )
        })
        .collect();
    for function in &module.functions {
        verify_function(function, &signatures)?;
    }
    Ok(())
}

fn verify_function(
    function: &Function,
    signatures: &HashMap<String, (Vec<Type>, Type)>,
) -> Result<(), Diagnostic> {
    let predecessors = predecessors(function);
    let dominators = compute_dominators(function, &predecessors)?;
    let mut definitions = HashMap::new();
    for block in function.blocks.values() {
        for (value, instruction) in &block.instructions {
            let ty = match instruction {
                Instruction::Phi(_) => None,
                _ => Some(infer_instruction_type(function, instruction, signatures)?),
            };
            if definitions
                .insert(
                    *value,
                    ValueDefinition {
                        block: block.id,
                        ty,
                    },
                )
                .is_some()
            {
                return Err(Diagnostic::new(format!("duplicate value id {:?}", value)));
            }
        }
    }
    resolve_phi_types(function, &mut definitions)?;

    for block in function.blocks.values() {
        let mut seen_in_block = HashSet::new();
        for (value, instruction) in &block.instructions {
            match instruction {
                Instruction::Binary {
                    left,
                    right,
                    operand_ty,
                    ..
                } => {
                    let left_ty = require_dominating_definition(
                        &definitions,
                        &dominators,
                        &seen_in_block,
                        block.id,
                        *left,
                    )?;
                    let right_ty = require_dominating_definition(
                        &definitions,
                        &dominators,
                        &seen_in_block,
                        block.id,
                        *right,
                    )?;
                    if left_ty != *operand_ty || right_ty != *operand_ty {
                        return Err(Diagnostic::new(format!(
                            "binary operands in block {:?} must both have type {}",
                            block.id, operand_ty
                        )));
                    }
                }
                Instruction::MathIntrinsic {
                    args,
                    operand_ty,
                    result_ty,
                    ..
                } => {
                    for arg in args {
                        let arg_ty = require_dominating_definition(
                            &definitions,
                            &dominators,
                            &seen_in_block,
                            block.id,
                            *arg,
                        )?;
                        if arg_ty != *operand_ty {
                            return Err(Diagnostic::new(format!(
                                "math intrinsic argument in block {:?} has type {}, expected {}",
                                block.id, arg_ty, operand_ty
                            )));
                        }
                    }
                    if !matches!(
                        (operand_ty, result_ty),
                        (Type::Numeric(_), Type::Numeric(_))
                    ) {
                        return Err(Diagnostic::new(format!(
                            "math intrinsic in block {:?} must have numeric operand/result types",
                            block.id
                        )));
                    }
                }
                Instruction::Print { value } => {
                    let value_ty = require_dominating_definition(
                        &definitions,
                        &dominators,
                        &seen_in_block,
                        block.id,
                        *value,
                    )?;
                    if value_ty != Type::String {
                        return Err(Diagnostic::new(format!(
                            "print argument in block {:?} has type {}, expected string",
                            block.id, value_ty
                        )));
                    }
                }
                Instruction::Cast { value, from, to } => {
                    let value_ty = require_dominating_definition(
                        &definitions,
                        &dominators,
                        &seen_in_block,
                        block.id,
                        *value,
                    )?;
                    if value_ty != *from {
                        return Err(Diagnostic::new(format!(
                            "cast source in block {:?} has type {}, expected {}",
                            block.id, value_ty, from
                        )));
                    }
                    require_numeric_cast(from.clone(), to.clone())?;
                }
                Instruction::Call { name, args } => {
                    let (param_types, _) = signatures
                        .get(name)
                        .ok_or_else(|| Diagnostic::new(format!("unknown function '{}'", name)))?;
                    if args.len() != param_types.len() {
                        return Err(Diagnostic::new(format!(
                            "call to '{}' has {} args but signature expects {}",
                            name,
                            args.len(),
                            param_types.len()
                        )));
                    }
                    for (arg, param_ty) in args.iter().zip(param_types.iter()) {
                        let arg_ty = require_dominating_definition(
                            &definitions,
                            &dominators,
                            &seen_in_block,
                            block.id,
                            *arg,
                        )?;
                        if arg_ty != *param_ty {
                            return Err(Diagnostic::new(format!(
                                "call argument in block {:?} has type {}, expected {}",
                                block.id, arg_ty, param_ty
                            )));
                        }
                    }
                }
                Instruction::CallValue {
                    callee,
                    args,
                    params,
                    return_type,
                } => {
                    let callee_ty = require_dominating_definition(
                        &definitions,
                        &dominators,
                        &seen_in_block,
                        block.id,
                        *callee,
                    )?;
                    let expected_callee_ty = Type::Function {
                        params: params.clone(),
                        return_type: Box::new(return_type.clone()),
                    };
                    if callee_ty != expected_callee_ty {
                        return Err(Diagnostic::new(format!(
                            "indirect call in block {:?} expects callee {}, got {}",
                            block.id, expected_callee_ty, callee_ty
                        )));
                    }
                    if args.len() != params.len() {
                        return Err(Diagnostic::new(format!(
                            "indirect call in block {:?} has {} args but expects {}",
                            block.id,
                            args.len(),
                            params.len()
                        )));
                    }
                    for (arg, param_ty) in args.iter().zip(params.iter()) {
                        let arg_ty = require_dominating_definition(
                            &definitions,
                            &dominators,
                            &seen_in_block,
                            block.id,
                            *arg,
                        )?;
                        if arg_ty != *param_ty {
                            return Err(Diagnostic::new(format!(
                                "indirect call argument in block {:?} has type {}, expected {}",
                                block.id, arg_ty, param_ty
                            )));
                        }
                    }
                }
                Instruction::Closure {
                    name,
                    captures,
                    params,
                    return_type,
                } => {
                    let (sig_params, sig_ret) = signatures
                        .get(name)
                        .ok_or_else(|| Diagnostic::new(format!("unknown function '{}'", name)))?;
                    if sig_params.len() < captures.len() {
                        return Err(Diagnostic::new(format!(
                            "closure in block {:?} has too many captures for '{}'",
                            block.id, name
                        )));
                    }
                    for (capture, capture_ty) in captures.iter().zip(sig_params.iter()) {
                        let actual = require_dominating_definition(
                            &definitions,
                            &dominators,
                            &seen_in_block,
                            block.id,
                            *capture,
                        )?;
                        // Allow the captured value to be either the raw value type or a
                        // 1-element array cell containing the value (for mutable capture).
                        let ok = actual == *capture_ty
                            || (actual.is_array()
                                && actual
                                    .element_type()
                                    .map(|e| e == *capture_ty)
                                    .unwrap_or(false));
                        if !ok {
                            return Err(Diagnostic::new(format!(
                                "closure capture in block {:?} has type {}, expected {}",
                                block.id, actual, capture_ty
                            )));
                        }
                    }
                    let expected_sig = Type::Function {
                        params: params.clone(),
                        return_type: Box::new(return_type.clone()),
                    };
                    let actual_sig = Type::Function {
                        params: sig_params[captures.len()..].to_vec(),
                        return_type: Box::new(sig_ret.clone()),
                    };
                    if expected_sig != actual_sig {
                        return Err(Diagnostic::new(format!(
                            "closure in block {:?} signature mismatch: expected {}, got {}",
                            block.id, expected_sig, actual_sig
                        )));
                    }
                }
                Instruction::ArrayNew {
                    element_ty,
                    elements,
                } => {
                    for element in elements {
                        let element_value_ty = require_dominating_definition(
                            &definitions,
                            &dominators,
                            &seen_in_block,
                            block.id,
                            *element,
                        )?;
                        if element_value_ty != *element_ty {
                            return Err(Diagnostic::new(format!(
                                "array literal element in block {:?} has type {}, expected {}",
                                block.id, element_value_ty, element_ty
                            )));
                        }
                    }
                }
                Instruction::ArrayGet {
                    array,
                    index,
                    element_ty,
                } => {
                    let array_ty = require_dominating_definition(
                        &definitions,
                        &dominators,
                        &seen_in_block,
                        block.id,
                        *array,
                    )?;
                    let expected_array_ty = Type::Array(Box::new(element_ty.clone()));
                    // Allow the array operand to be either an array of the element type
                    // or the raw element type itself (degenerate cell representation).
                    let ok_array = array_ty == expected_array_ty || array_ty == *element_ty;
                    if !ok_array {
                        return Err(Diagnostic::new(format!(
                            "array get in block {:?} expects {}, got {}",
                            block.id, expected_array_ty, array_ty
                        )));
                    }
                    let index_ty = require_dominating_definition(
                        &definitions,
                        &dominators,
                        &seen_in_block,
                        block.id,
                        *index,
                    )?;
                    if index_ty != Type::Numeric(NumericType::I32) {
                        return Err(Diagnostic::new(format!(
                            "array index in block {:?} must be i32",
                            block.id
                        )));
                    }
                }
                Instruction::ArraySet {
                    array,
                    index,
                    value,
                    element_ty,
                } => {
                    let array_ty = require_dominating_definition(
                        &definitions,
                        &dominators,
                        &seen_in_block,
                        block.id,
                        *array,
                    )?;
                    let expected_array_ty = Type::Array(Box::new(element_ty.clone()));
                    let ok_array = array_ty == expected_array_ty || array_ty == *element_ty;
                    if !ok_array {
                        return Err(Diagnostic::new(format!(
                            "array set in block {:?} expects {}, got {}",
                            block.id, expected_array_ty, array_ty
                        )));
                    }
                    let index_ty = require_dominating_definition(
                        &definitions,
                        &dominators,
                        &seen_in_block,
                        block.id,
                        *index,
                    )?;
                    if index_ty != Type::Numeric(NumericType::I32) {
                        return Err(Diagnostic::new(format!(
                            "array index in block {:?} must be i32",
                            block.id
                        )));
                    }
                    let value_ty = require_dominating_definition(
                        &definitions,
                        &dominators,
                        &seen_in_block,
                        block.id,
                        *value,
                    )?;
                    if value_ty != *element_ty {
                        return Err(Diagnostic::new(format!(
                            "array set value in block {:?} has type {}, expected {}",
                            block.id, value_ty, element_ty
                        )));
                    }
                }
                Instruction::ArrayLen { array } => {
                    let array_ty = require_dominating_definition(
                        &definitions,
                        &dominators,
                        &seen_in_block,
                        block.id,
                        *array,
                    )?;
                    if !array_ty.is_array() {
                        return Err(Diagnostic::new(format!(
                            "array.len operand in block {:?} must be an array",
                            block.id
                        )));
                    }
                }
                Instruction::PackMulti { values, types } => {
                    if values.len() != types.len() {
                        return Err(Diagnostic::new(format!(
                            "pack multi in block {:?} must have equal value/type arity",
                            block.id
                        )));
                    }
                    for (value, ty) in values.iter().zip(types.iter()) {
                        let actual = require_dominating_definition(
                            &definitions,
                            &dominators,
                            &seen_in_block,
                            block.id,
                            *value,
                        )?;
                        if actual != *ty {
                            return Err(Diagnostic::new(format!(
                                "pack multi in block {:?} value type {}, expected {}",
                                block.id, actual, ty
                            )));
                        }
                    }
                }
                Instruction::MultiGet { value, index, ty } => {
                    let actual = require_dominating_definition(
                        &definitions,
                        &dominators,
                        &seen_in_block,
                        block.id,
                        *value,
                    )?;
                    let Type::Multi(types) = actual else {
                        return Err(Diagnostic::new(format!(
                            "multi-get in block {:?} requires multi-value operand",
                            block.id
                        )));
                    };
                    let expected = types.get(*index).ok_or_else(|| {
                        Diagnostic::new(format!(
                            "multi-get in block {:?} index {} is out of range",
                            block.id, index
                        ))
                    })?;
                    if expected != ty {
                        return Err(Diagnostic::new(format!(
                            "multi-get in block {:?} expects {}, got {}",
                            block.id, expected, ty
                        )));
                    }
                }
                Instruction::Phi(incoming) => {
                    let expected_preds = predecessors.get(&block.id).cloned().unwrap_or_default();
                    if incoming.len() != expected_preds.len() {
                        return Err(Diagnostic::new(format!(
                            "phi in block {:?} must have exactly {} incoming values",
                            block.id,
                            expected_preds.len()
                        )));
                    }
                    for (index, (pred, value)) in incoming.iter().enumerate() {
                        if *pred != expected_preds[index] {
                            return Err(Diagnostic::new(format!(
                                "phi in block {:?} predecessor order mismatch at {}",
                                block.id, index
                            )));
                        }
                        let value_def = definitions.get(value).ok_or_else(|| {
                            Diagnostic::new(format!("use of undefined value {:?}", value))
                        })?;
                        if value_def.block != *pred
                            && !dominators
                                .get(pred)
                                .is_some_and(|doms| doms.contains(&value_def.block))
                        {
                            return Err(Diagnostic::new(format!(
                                "value {:?} does not dominate phi edge {:?} -> {:?}",
                                value, pred, block.id
                            )));
                        }
                    }
                }
                Instruction::Param(_)
                | Instruction::Number { .. }
                | Instruction::Bool(_)
                | Instruction::String(_) => {}
                Instruction::ToString { value, from } => {
                    let value_ty = require_dominating_definition(
                        &definitions,
                        &dominators,
                        &seen_in_block,
                        block.id,
                        *value,
                    )?;
                    if &value_ty != from {
                        return Err(Diagnostic::new(format!(
                            "tostring source type mismatch in block {:?}: expected {}, got {}",
                            block.id, from, value_ty
                        )));
                    }
                    if !(from.is_numeric() || *from == Type::Bool || *from == Type::String) {
                        return Err(Diagnostic::new(format!(
                            "tostring requires primitive source type, got {}",
                            from
                        )));
                    }
                }
            }
            seen_in_block.insert(*value);
        }

        match &block.terminator {
            Terminator::Jump(target) => require_block(function, *target)?,
            Terminator::Branch {
                condition,
                then_block,
                else_block,
            } => {
                let condition_ty = require_dominating_definition(
                    &definitions,
                    &dominators,
                    &seen_in_block,
                    block.id,
                    *condition,
                )?;
                if condition_ty != Type::Bool {
                    return Err(Diagnostic::new(format!(
                        "branch condition in block {:?} must have type bool",
                        block.id
                    )));
                }
                require_block(function, *then_block)?;
                require_block(function, *else_block)?;
            }
            Terminator::Return(value) => {
                let value_ty = require_dominating_definition(
                    &definitions,
                    &dominators,
                    &seen_in_block,
                    block.id,
                    *value,
                )?;
                if value_ty != function.return_type {
                    return Err(Diagnostic::new(format!(
                        "return in block {:?} has type {}, expected {}",
                        block.id, value_ty, function.return_type
                    )));
                }
            }
            Terminator::Unreachable => {}
        }
    }

    Ok(())
}

#[derive(Clone)]
struct ValueDefinition {
    block: BlockId,
    ty: Option<Type>,
}

fn require_dominating_definition(
    definitions: &HashMap<ValueId, ValueDefinition>,
    dominators: &HashMap<BlockId, HashSet<BlockId>>,
    seen_in_block: &HashSet<ValueId>,
    use_block: BlockId,
    value: ValueId,
) -> Result<Type, Diagnostic> {
    let definition = definitions
        .get(&value)
        .ok_or_else(|| Diagnostic::new(format!("use of undefined value {:?}", value)))?;
    if definition.block == use_block {
        if !seen_in_block.contains(&value) {
            return Err(Diagnostic::new(format!(
                "value {:?} does not dominate its use in block {:?}",
                value, use_block
            )));
        }
    } else if !dominators
        .get(&use_block)
        .is_some_and(|doms| doms.contains(&definition.block))
    {
        return Err(Diagnostic::new(format!(
            "value {:?} does not dominate its use in block {:?}",
            value, use_block
        )));
    }
    definition
        .ty
        .clone()
        .ok_or_else(|| Diagnostic::new(format!("could not infer type for value {:?}", value)))
}

fn require_block(function: &Function, block: BlockId) -> Result<(), Diagnostic> {
    if function.blocks.contains_key(&block) {
        Ok(())
    } else {
        Err(Diagnostic::new(format!("unknown block {:?}", block)))
    }
}

fn infer_instruction_type(
    function: &Function,
    instruction: &Instruction,
    signatures: &HashMap<String, (Vec<Type>, Type)>,
) -> Result<Type, Diagnostic> {
    match instruction {
        Instruction::Param(index) => function
            .params
            .get(*index)
            .map(|(_, ty)| ty.clone())
            .ok_or_else(|| Diagnostic::new(format!("param index {} out of bounds", index))),
        Instruction::Number { ty, .. } => Ok(Type::Numeric(*ty)),
        Instruction::Bool(_) => Ok(Type::Bool),
        Instruction::String(_) => Ok(Type::String),
        Instruction::Cast { to, .. } => Ok(to.clone()),
        Instruction::Binary { result_ty, .. } => Ok(result_ty.clone()),
        Instruction::MathIntrinsic { result_ty, .. } => Ok(result_ty.clone()),
        Instruction::ToString { .. } => Ok(Type::String),
        Instruction::Print { .. } => Ok(Type::Numeric(NumericType::I32)),
        Instruction::Call { name, .. } => signatures
            .get(name)
            .map(|(_, ret)| ret.clone())
            .ok_or_else(|| Diagnostic::new(format!("unknown function '{}'", name))),
        Instruction::CallValue { return_type, .. } => Ok(return_type.clone()),
        Instruction::Closure {
            params,
            return_type,
            ..
        } => Ok(Type::Function {
            params: params.clone(),
            return_type: Box::new(return_type.clone()),
        }),
        Instruction::ArrayNew { element_ty, .. } => Ok(Type::Array(Box::new(element_ty.clone()))),
        Instruction::ArrayGet { element_ty, .. } => Ok(element_ty.clone()),
        Instruction::ArraySet { .. } => Ok(Type::Numeric(NumericType::I32)),
        Instruction::ArrayLen { .. } => Ok(Type::Numeric(NumericType::I32)),
        Instruction::PackMulti { types, .. } => Ok(Type::Multi(types.clone())),
        Instruction::MultiGet { ty, .. } => Ok(ty.clone()),
        Instruction::Phi(_) => Err(Diagnostic::new(
            "phi type must be resolved before infer_instruction_type",
        )),
    }
}

fn compute_dominators(
    function: &Function,
    predecessors: &HashMap<BlockId, Vec<BlockId>>,
) -> Result<HashMap<BlockId, HashSet<BlockId>>, Diagnostic> {
    if !function.blocks.contains_key(&function.entry) {
        return Err(Diagnostic::new(format!(
            "entry block {:?} does not exist",
            function.entry
        )));
    }
    let all_blocks: HashSet<_> = function.blocks.keys().copied().collect();
    let mut dominators = HashMap::new();
    for id in function.blocks.keys().copied() {
        if id == function.entry {
            dominators.insert(id, HashSet::from([id]));
        } else {
            dominators.insert(id, all_blocks.clone());
        }
    }

    let mut changed = true;
    while changed {
        changed = false;
        for block in function.blocks.keys().copied() {
            if block == function.entry {
                continue;
            }
            let preds = predecessors.get(&block).cloned().unwrap_or_default();
            let mut next = if preds.is_empty() {
                HashSet::new()
            } else {
                let mut it = preds.iter();
                let first = it
                    .next()
                    .and_then(|pred| dominators.get(pred))
                    .cloned()
                    .unwrap_or_default();
                it.fold(first, |acc, pred| {
                    let pred_set = dominators.get(pred).cloned().unwrap_or_default();
                    acc.intersection(&pred_set).copied().collect()
                })
            };
            next.insert(block);
            if dominators
                .get(&block)
                .is_some_and(|current| *current != next)
            {
                dominators.insert(block, next);
                changed = true;
            }
        }
    }
    Ok(dominators)
}

fn resolve_phi_types(
    function: &Function,
    definitions: &mut HashMap<ValueId, ValueDefinition>,
) -> Result<(), Diagnostic> {
    let mut changed = true;
    while changed {
        changed = false;
        for block in function.blocks.values() {
            for (value, instruction) in &block.instructions {
                let Instruction::Phi(incoming) = instruction else {
                    continue;
                };
                if definitions
                    .get(value)
                    .and_then(|def| def.ty.as_ref())
                    .is_some()
                {
                    continue;
                }
                let mut incoming_ty: Option<Type> = None;
                for (_, incoming_value) in incoming {
                    if incoming_value == value {
                        continue;
                    }
                    let ty = match definitions
                        .get(incoming_value)
                        .and_then(|def| def.ty.as_ref())
                    {
                        Some(ty) => ty,
                        None => continue,
                    };
                    if let Some(ref expected) = incoming_ty {
                        if expected != ty {
                            return Err(Diagnostic::new(format!(
                                "phi in block {:?} has inconsistent incoming types",
                                block.id
                            )));
                        }
                    } else {
                        incoming_ty = Some(ty.clone());
                    }
                }
                if let Some(resolved) = incoming_ty {
                    definitions
                        .get_mut(value)
                        .expect("phi definition must exist")
                        .ty = Some(resolved);
                    changed = true;
                }
            }
        }
    }

    for block in function.blocks.values() {
        for (value, instruction) in &block.instructions {
            if matches!(instruction, Instruction::Phi(_))
                && definitions
                    .get(value)
                    .and_then(|def| def.ty.as_ref())
                    .is_none()
            {
                return Err(Diagnostic::new(format!(
                    "could not infer phi type for value {:?}",
                    value
                )));
            }
        }
    }
    Ok(())
}

impl Function {
    pub fn dump(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("fn {}:\n", self.name));
        for (id, block) in &self.blocks {
            out.push_str(&format!("  b{}:\n", id.0));
            for (value, instruction) in &block.instructions {
                out.push_str(&format!("    v{} = {:?}\n", value.0, instruction));
            }
            out.push_str(&format!("    {:?}\n", block.terminator));
        }
        out
    }

    fn next_value(&mut self) -> ValueId {
        let value = ValueId(self.next_value);
        self.next_value += 1;
        value
    }
}

fn build_function(
    function: &AstFunction,
    signatures: &HashMap<String, (Vec<Type>, Type)>,
) -> Result<Vec<Function>, Diagnostic> {
    let return_type = function.return_type.clone().ok_or_else(|| {
        inference_diagnostic(
            "inference/unsupported",
            DiagnosticCategory::Unsupported,
            format!(
                "function '{}' must have a concrete return type before IR lowering",
                function.name
            ),
            "ensure type inference runs before IR lowering or add an explicit return type",
        )
    })?;
    let mut out = Function {
        name: function.name.clone(),
        params: function
            .params
            .iter()
            .map(|param| (param.name.clone(), param.ty.clone()))
            .collect(),
        return_type,
        entry: BlockId(0),
        blocks: BTreeMap::new(),
        next_value: 0,
    };

    out.blocks.insert(
        out.entry,
        BasicBlock {
            id: out.entry,
            instructions: Vec::new(),
            terminator: Terminator::Unreachable,
        },
    );

    let mut env = HashMap::new();
    let mut type_env = HashMap::new();
    let entry = out.entry;
    // Precompute names that are referenced by any nested function so we can
    // represent them as cell-backed storage (1-element arrays) to support
    // mutable closure capture semantics.
    let captured_names: HashSet<String> = collect_nested_function_capture_names(function);

    for (index, (name, ty)) in out.params.clone().into_iter().enumerate() {
        let value = out.next_value();
        block_mut(&mut out, entry)
            .instructions
            .push((value, Instruction::Param(index)));
        // If this parameter is captured by a nested function, wrap it in a 1-element
        // array cell so closures share mutable storage. Otherwise keep the raw value.
        if captured_names.contains(&name) {
            let cell = out.next_value();
            block_mut(&mut out, entry).instructions.push((
                cell,
                Instruction::ArrayNew {
                    element_ty: ty.clone(),
                    elements: vec![value],
                },
            ));
            env.insert(name, cell);
        } else {
            env.insert(name, value);
        }
        type_env.insert(out.params[index].0.clone(), ty);
    }

    let mut builder = Builder {
        function: out,
        current_block: BlockId(0),
        next_block: 1,
        signatures,
        lifted_functions: Vec::new(),
        lambda_counter: 0,
        loop_stack: Vec::new(),
        cell_names: captured_names,
    };
    for stmt in &function.body {
        if builder.current_block == DEAD_BLOCK {
            break;
        }
        builder.lower_stmt(stmt, &mut env, &mut type_env)?;
    }
    let mut functions = vec![builder.function];
    functions.extend(builder.lifted_functions);
    Ok(functions)
}

const DEAD_BLOCK: BlockId = BlockId(usize::MAX);

struct Builder<'a> {
    function: Function,
    current_block: BlockId,
    next_block: usize,
    signatures: &'a HashMap<String, (Vec<Type>, Type)>,
    lifted_functions: Vec<Function>,
    lambda_counter: usize,
    loop_stack: Vec<LoopContext>,
    /// Names that are represented as 1-element array "cells" to support mutable capture.
    cell_names: HashSet<String>,
}

#[derive(Clone)]
struct LoopContext {
    header: BlockId,
    continue_target: BlockId,
    break_target: BlockId,
    phis: HashMap<String, ValueId>,
}

impl Builder<'_> {
    fn function_expr_return_type(function: &waluau_ast::FunctionExpr) -> Result<Type, Diagnostic> {
        function.return_type.clone().ok_or_else(|| {
            Diagnostic::new(
                "function return inference is only supported for named functions in this MVP",
            )
        })
    }

    fn new_block(&mut self) -> BlockId {
        let id = BlockId(self.next_block);
        self.next_block += 1;
        self.function.blocks.insert(
            id,
            BasicBlock {
                id,
                instructions: Vec::new(),
                terminator: Terminator::Unreachable,
            },
        );
        id
    }

    fn emit(&mut self, instruction: Instruction) -> ValueId {
        let value = self.function.next_value();
        block_mut(&mut self.function, self.current_block)
            .instructions
            .push((value, instruction));
        value
    }

    fn set_terminator(&mut self, block: BlockId, terminator: Terminator) {
        block_mut(&mut self.function, block).terminator = terminator;
    }

    fn lower_break(&mut self, _env: &HashMap<String, ValueId>) -> Result<(), Diagnostic> {
        let Some(loop_ctx) = self.loop_stack.last() else {
            return Err(Diagnostic::new("break is only allowed inside loops"));
        };
        if self.current_block == DEAD_BLOCK {
            return Ok(());
        }
        let current = self.current_block;
        self.set_terminator(current, Terminator::Jump(loop_ctx.break_target));
        self.current_block = DEAD_BLOCK;
        Ok(())
    }

    fn lower_continue(&mut self, env: &HashMap<String, ValueId>) -> Result<(), Diagnostic> {
        let Some(loop_ctx) = self.loop_stack.last() else {
            return Err(Diagnostic::new("continue is only allowed inside loops"));
        };
        if self.current_block == DEAD_BLOCK {
            return Ok(());
        }
        let current = self.current_block;
        for (name, phi) in &loop_ctx.phis {
            if let Some(value) = env.get(name).copied() {
                add_phi_incoming(&mut self.function, loop_ctx.header, *phi, (current, value));
            }
        }
        self.set_terminator(current, Terminator::Jump(loop_ctx.continue_target));
        self.current_block = DEAD_BLOCK;
        Ok(())
    }

    fn lower_stmt(
        &mut self,
        stmt: &Stmt,
        env: &mut HashMap<String, ValueId>,
        types: &mut HashMap<String, Type>,
    ) -> Result<(), Diagnostic> {
        match stmt {
            Stmt::Let {
                name,
                rebindability: _,
                ty,
                value,
            } => {
                let inferred_ty = if let Some(ty) = ty.clone() {
                    ty
                } else {
                    self.infer_expr_type(value, types, None)?
                };
                let value = self.lower_expr(value, env, types, Some(inferred_ty.clone()))?;
                // If this local is captured by any nested function, represent it as a 1-element
                // array cell so closures can observe and mutate the same storage location.
                if self.cell_names.contains(name) {
                    let cell = self.emit(Instruction::ArrayNew {
                        element_ty: inferred_ty.clone(),
                        elements: vec![value],
                    });
                    env.insert(name.clone(), cell);
                    // Keep the declared type as the inner element type for type checking.
                    types.insert(name.clone(), inferred_ty);
                } else {
                    env.insert(name.clone(), value);
                    types.insert(name.clone(), inferred_ty);
                }
            }
            Stmt::Assign { op, name, value } => {
                let ty = types.get(name).cloned().ok_or_else(|| {
                    Diagnostic::new(format!("unknown local '{name}' during IR lowering"))
                })?;
                if self.cell_names.contains(name) {
                    // Captured local: stored in a 1-element array (cell). Perform ArraySet
                    // rather than rebinding the env entry.
                    let cell = env.get(name).copied().ok_or_else(|| {
                        Diagnostic::new(format!("unknown local '{name}' during IR lowering"))
                    })?;
                    let index0 = self.emit(Instruction::Number {
                        ty: NumericType::I32,
                        literal: NumberLiteral { raw: "0".into() },
                    });
                    match op {
                        AssignOp::Set => {
                            let rhs = self.lower_expr(value, env, types, Some(ty.clone()))?;
                            self.emit(Instruction::ArraySet {
                                array: cell,
                                index: index0,
                                value: rhs,
                                element_ty: ty,
                            });
                        }
                        AssignOp::Add => {
                            if !ty.is_numeric() {
                                return Err(Diagnostic::new(format!(
                                    "compound assignment to '{}' requires a numeric target",
                                    name
                                )));
                            }
                            // load current, add, store back
                            let current = self.emit(Instruction::ArrayGet {
                                array: cell,
                                index: index0,
                                element_ty: ty.clone(),
                            });
                            let rhs = self.lower_expr(value, env, types, Some(ty.clone()))?;
                            let sum = self.emit(Instruction::Binary {
                                op: BinaryOp::Add,
                                left: current,
                                right: rhs,
                                operand_ty: ty.clone(),
                                result_ty: ty.clone(),
                            });
                            self.emit(Instruction::ArraySet {
                                array: cell,
                                index: index0,
                                value: sum,
                                element_ty: ty.clone(),
                            });
                        }
                    }
                    // Do not replace env entry -- it remains the cell.
                } else {
                    let value = match op {
                        AssignOp::Set => self.lower_expr(value, env, types, Some(ty))?,
                        AssignOp::Add => {
                            if !ty.is_numeric() {
                                return Err(Diagnostic::new(format!(
                                    "compound assignment to '{}' requires a numeric target",
                                    name
                                )));
                            }
                            let current = *env.get(name).ok_or_else(|| {
                                Diagnostic::new(format!(
                                    "unknown local '{name}' during IR lowering"
                                ))
                            })?;
                            let rhs = self.lower_expr(value, env, types, Some(ty.clone()))?;
                            self.emit(Instruction::Binary {
                                op: BinaryOp::Add,
                                left: current,
                                right: rhs,
                                operand_ty: ty.clone(),
                                result_ty: ty,
                            })
                        }
                    };
                    env.insert(name.clone(), value);
                }
            }
            Stmt::IndexAssign {
                op,
                base,
                index,
                value,
            } => {
                let base_ty = self.infer_expr_type(base, types, None)?;
                let element_ty = base_ty.element_type().ok_or_else(|| {
                    Diagnostic::new("array element assignment requires an array operand")
                })?;
                let array = self.lower_expr(base, env, types, Some(base_ty))?;
                let index =
                    self.lower_expr(index, env, types, Some(Type::Numeric(NumericType::I32)))?;
                let value = match op {
                    AssignOp::Set => {
                        self.lower_expr(value, env, types, Some(element_ty.clone()))?
                    }
                    AssignOp::Add => {
                        if !element_ty.is_numeric() {
                            return Err(Diagnostic::new(
                                "compound array assignment requires numeric elements",
                            ));
                        }
                        let current = self.emit(Instruction::ArrayGet {
                            array,
                            index,
                            element_ty: element_ty.clone(),
                        });
                        let rhs = self.lower_expr(value, env, types, Some(element_ty.clone()))?;
                        self.emit(Instruction::Binary {
                            op: BinaryOp::Add,
                            left: current,
                            right: rhs,
                            operand_ty: element_ty.clone(),
                            result_ty: element_ty.clone(),
                        })
                    }
                };
                self.emit(Instruction::ArraySet {
                    array,
                    index,
                    value,
                    element_ty,
                });
            }
            Stmt::Expr(expr) => {
                if let Expr::Call {
                    callee,
                    type_args: _,
                    args,
                } = expr
                {
                    if let Expr::Name(name) = callee.as_ref() {
                        if name == ASSERT {
                            self.lower_assert_call(args, env, types)?;
                            return Ok(());
                        }
                        if name == PRINT {
                            if args.len() != 1 {
                                return Err(Diagnostic::new(format!(
                                    "{PRINT} expects 1 argument, got {}",
                                    args.len()
                                )));
                            }
                            let value =
                                self.lower_expr(&args[0], env, types, Some(Type::String))?;
                            let _ = self.emit(Instruction::Print { value });
                            return Ok(());
                        }
                    }
                }
                let _ = self.lower_expr(expr, env, types, None)?;
            }
            Stmt::Break => {
                self.lower_break(env)?;
            }
            Stmt::Continue => {
                self.lower_continue(env)?;
            }
            Stmt::Return(expr) => {
                let value =
                    self.lower_expr(expr, env, types, Some(self.function.return_type.clone()))?;
                self.set_terminator(self.current_block, Terminator::Return(value));
                self.current_block = DEAD_BLOCK;
            }
            Stmt::ReturnMulti(values) => {
                let expected = match &self.function.return_type {
                    Type::Multi(types) => types.clone(),
                    other => vec![other.clone()],
                };
                let lowered = self.lower_expr_list(values, env, types, Some(&expected))?;
                if lowered.len() != expected.len() {
                    return Err(Diagnostic::new(format!(
                        "return expects {} values, got {}",
                        expected.len(),
                        lowered.len()
                    )));
                }
                let packed = self.emit(Instruction::PackMulti {
                    values: lowered,
                    types: expected,
                });
                self.set_terminator(self.current_block, Terminator::Return(packed));
                self.current_block = DEAD_BLOCK;
            }
            Stmt::LetMulti { bindings, values } => {
                let all_typed = bindings.iter().all(|binding| binding.ty.is_some());
                let any_typed = bindings.iter().any(|binding| binding.ty.is_some());
                if any_typed && !all_typed {
                    return Err(Diagnostic::new(
                        "multi-binding declaration must either annotate all bindings or none",
                    ));
                }
                if all_typed {
                    let expected: Vec<Type> = bindings
                        .iter()
                        .map(|binding| binding.ty.clone().expect("checked above"))
                        .collect();
                    let lowered = self.lower_expr_list(values, env, types, Some(&expected))?;
                    if lowered.len() != expected.len() {
                        return Err(Diagnostic::new(format!(
                            "multi-binding declaration expects {} values, got {}",
                            expected.len(),
                            lowered.len()
                        )));
                    }
                    for ((binding, value), expected_ty) in
                        bindings.iter().zip(lowered).zip(expected)
                    {
                        env.insert(binding.name.clone(), value);
                        types.insert(binding.name.clone(), expected_ty);
                    }
                } else {
                    // No explicit type annotations: infer types from the RHS expressions.
                    let mut inferred_types = Vec::new();
                    for expr in values {
                        let ty = self.infer_expr_type(expr, types, None)?;
                        match ty {
                            Type::Multi(types_for_expr) => {
                                inferred_types.extend(types_for_expr);
                            }
                            other => inferred_types.push(other),
                        }
                    }
                    if inferred_types.len() != bindings.len() {
                        return Err(Diagnostic::new(format!(
                            "multi-binding declaration expects {} values, got {}",
                            bindings.len(),
                            inferred_types.len()
                        )));
                    }
                    let lowered = self.lower_expr_list(values, env, types, None)?;
                    if lowered.len() != inferred_types.len() {
                        return Err(Diagnostic::new(format!(
                            "multi-binding declaration expects {} values, got {}",
                            inferred_types.len(),
                            lowered.len()
                        )));
                    }
                    for ((binding, value), ty) in bindings.iter().zip(lowered).zip(inferred_types) {
                        env.insert(binding.name.clone(), value);
                        types.insert(binding.name.clone(), ty);
                    }
                }
            }
            Stmt::AssignMulti { targets, values } => {
                let mut expected = Vec::new();
                for target in targets {
                    let ty = types.get(target).cloned().ok_or_else(|| {
                        Diagnostic::new(format!("unknown local '{target}' during IR lowering"))
                    })?;
                    expected.push(ty);
                }
                let lowered = self.lower_expr_list(values, env, types, Some(&expected))?;
                if lowered.len() != expected.len() {
                    return Err(Diagnostic::new(format!(
                        "multi-assignment expects {} values, got {}",
                        expected.len(),
                        lowered.len()
                    )));
                }
                for (target, value) in targets.iter().zip(lowered) {
                    env.insert(target.clone(), value);
                }
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                self.lower_if(condition, then_body, else_body, env, types)?;
            }
            Stmt::While { condition, body } => {
                self.lower_while(condition, body, env, types)?;
            }
            Stmt::Repeat { body, condition } => {
                self.lower_repeat(body, condition, env, types)?;
            }
        }
        Ok(())
    }

    fn lower_assert_call(
        &mut self,
        args: &[Expr],
        env: &mut HashMap<String, ValueId>,
        types: &mut HashMap<String, Type>,
    ) -> Result<(), Diagnostic> {
        if args.len() != 1 {
            return Err(Diagnostic::new(format!(
                "{ASSERT} expects 1 argument, got {}",
                args.len()
            )));
        }
        let condition = self.lower_expr(&args[0], env, types, Some(Type::Bool))?;
        let continue_block = self.new_block();
        let trap_block = self.new_block();
        self.set_terminator(
            self.current_block,
            Terminator::Branch {
                condition,
                then_block: continue_block,
                else_block: trap_block,
            },
        );
        self.set_terminator(trap_block, Terminator::Unreachable);
        self.current_block = continue_block;
        Ok(())
    }

    fn lower_if(
        &mut self,
        condition: &Expr,
        then_body: &[Stmt],
        else_body: &[Stmt],
        env: &mut HashMap<String, ValueId>,
        types: &mut HashMap<String, Type>,
    ) -> Result<(), Diagnostic> {
        let condition = self.lower_expr(condition, env, types, Some(Type::Bool))?;
        let then_block = self.new_block();
        let else_block = self.new_block();
        let merge_block = self.new_block();
        self.set_terminator(
            self.current_block,
            Terminator::Branch {
                condition,
                then_block,
                else_block,
            },
        );

        let mut then_env = env.clone();
        let mut then_types = types.clone();
        self.current_block = then_block;
        for stmt in then_body {
            if self.current_block == DEAD_BLOCK {
                break;
            }
            self.lower_stmt(stmt, &mut then_env, &mut then_types)?;
        }
        let then_exit = self.current_block;
        if then_exit != DEAD_BLOCK {
            self.set_terminator(then_exit, Terminator::Jump(merge_block));
        }

        let mut else_env = env.clone();
        let mut else_types = types.clone();
        self.current_block = else_block;
        for stmt in else_body {
            if self.current_block == DEAD_BLOCK {
                break;
            }
            self.lower_stmt(stmt, &mut else_env, &mut else_types)?;
        }
        let else_exit = self.current_block;
        if else_exit != DEAD_BLOCK {
            self.set_terminator(else_exit, Terminator::Jump(merge_block));
        }

        if then_exit == DEAD_BLOCK && else_exit == DEAD_BLOCK {
            self.current_block = DEAD_BLOCK;
            return Ok(());
        }

        self.current_block = merge_block;
        for name in env.keys().cloned().collect::<Vec<_>>() {
            let t = then_env
                .get(&name)
                .copied()
                .or_else(|| env.get(&name).copied());
            let e = else_env
                .get(&name)
                .copied()
                .or_else(|| env.get(&name).copied());
            if let (Some(tv), Some(ev)) = (t, e) {
                if tv != ev {
                    let mut incoming = Vec::new();
                    if then_exit != DEAD_BLOCK {
                        incoming.push((then_exit, tv));
                    }
                    if else_exit != DEAD_BLOCK {
                        incoming.push((else_exit, ev));
                    }
                    let phi = self.emit(Instruction::Phi(incoming));
                    env.insert(name, phi);
                }
            }
        }
        Ok(())
    }

    fn lower_while(
        &mut self,
        condition: &Expr,
        body: &[Stmt],
        env: &mut HashMap<String, ValueId>,
        types: &mut HashMap<String, Type>,
    ) -> Result<(), Diagnostic> {
        let preheader = self.current_block;
        let header = self.new_block();
        let loop_body = self.new_block();
        let exit = self.new_block();
        self.set_terminator(preheader, Terminator::Jump(header));

        let mutated = collect_assigned_names(body);
        self.current_block = header;
        let mut loop_env = env.clone();
        let loop_types = types.clone();
        let mut phis = HashMap::new();
        for name in &mutated {
            if let Some(initial) = env.get(name).copied() {
                let phi = self.emit(Instruction::Phi(vec![(preheader, initial)]));
                loop_env.insert(name.clone(), phi);
                phis.insert(name.clone(), phi);
            }
        }

        self.loop_stack.push(LoopContext {
            header,
            continue_target: header,
            break_target: exit,
            phis: phis.clone(),
        });

        let cond_value = self.lower_expr(condition, &loop_env, &loop_types, Some(Type::Bool))?;
        self.set_terminator(
            header,
            Terminator::Branch {
                condition: cond_value,
                then_block: loop_body,
                else_block: exit,
            },
        );

        self.current_block = loop_body;
        let mut body_env = loop_env.clone();
        let mut body_types = loop_types.clone();
        for stmt in body {
            if self.current_block == DEAD_BLOCK {
                break;
            }
            self.lower_stmt(stmt, &mut body_env, &mut body_types)?;
        }
        let loop_ctx = self
            .loop_stack
            .pop()
            .expect("loop stack must contain entry for while loop");
        let phis = loop_ctx.phis;
        let body_exit = self.current_block;
        if body_exit != DEAD_BLOCK {
            self.set_terminator(body_exit, Terminator::Jump(header));
            for (name, phi) in &phis {
                if let Some(next_value) = body_env.get(name).copied() {
                    add_phi_incoming(&mut self.function, header, *phi, (body_exit, next_value));
                }
            }
        }

        for (name, phi) in phis {
            env.insert(name, phi);
        }
        self.current_block = exit;
        Ok(())
    }

    fn lower_repeat(
        &mut self,
        body: &[Stmt],
        condition: &Expr,
        env: &mut HashMap<String, ValueId>,
        types: &mut HashMap<String, Type>,
    ) -> Result<(), Diagnostic> {
        let preheader = self.current_block;
        let loop_body = self.new_block();
        let check = self.new_block();
        let exit = self.new_block();
        self.set_terminator(preheader, Terminator::Jump(loop_body));

        let mutated = collect_assigned_names(body);
        self.current_block = loop_body;
        let mut loop_env = env.clone();
        let loop_types = types.clone();
        let mut phis = HashMap::new();
        for name in &mutated {
            if let Some(initial) = env.get(name).copied() {
                let phi = self.emit(Instruction::Phi(vec![(preheader, initial)]));
                loop_env.insert(name.clone(), phi);
                phis.insert(name.clone(), phi);
            }
        }

        self.loop_stack.push(LoopContext {
            header: loop_body,
            continue_target: check,
            break_target: exit,
            phis: phis.clone(),
        });

        let mut body_env = loop_env.clone();
        let mut body_types = loop_types.clone();
        for stmt in body {
            if self.current_block == DEAD_BLOCK {
                break;
            }
            self.lower_stmt(stmt, &mut body_env, &mut body_types)?;
        }
        let loop_ctx = self
            .loop_stack
            .pop()
            .expect("loop stack must contain entry for repeat-until loop");
        let phis = loop_ctx.phis;
        let body_exit = self.current_block;
        if body_exit == DEAD_BLOCK {
            for (name, phi) in phis {
                env.insert(name, phi);
            }
            self.current_block = exit;
            return Ok(());
        }
        self.set_terminator(body_exit, Terminator::Jump(check));

        self.current_block = check;
        let cond_value = self.lower_expr(condition, &body_env, &body_types, Some(Type::Bool))?;
        self.set_terminator(
            check,
            Terminator::Branch {
                condition: cond_value,
                then_block: exit,
                else_block: loop_body,
            },
        );

        for (name, phi) in &phis {
            if let Some(next_value) = body_env.get(name).copied() {
                add_phi_incoming(&mut self.function, loop_body, *phi, (check, next_value));
            }
        }

        for (name, phi) in phis {
            env.insert(name.clone(), body_env.get(&name).copied().unwrap_or(phi));
        }
        self.current_block = exit;
        Ok(())
    }

    fn lower_expr(
        &mut self,
        expr: &Expr,
        env: &HashMap<String, ValueId>,
        types: &HashMap<String, Type>,
        expected: Option<Type>,
    ) -> Result<ValueId, Diagnostic> {
        let value = match expr {
            Expr::Number(number) => {
                let ty = match self.infer_expr_type(expr, types, expected)? {
                    Type::Numeric(ty) => ty,
                    Type::Bool => unreachable!("number literal cannot lower as bool"),
                    Type::String => {
                        return Err(Diagnostic::new(
                            "numeric literal is not assignable to string",
                        ));
                    }
                    Type::Array(_) => {
                        return Err(Diagnostic::new(
                            "numeric literal is not assignable to array",
                        ));
                    }
                    Type::Function { .. } => {
                        return Err(Diagnostic::new(
                            "numeric literal is not assignable to function",
                        ));
                    }
                    Type::Multi(_) => {
                        return Err(Diagnostic::new(
                            "numeric literal is not assignable to multiple values",
                        ));
                    }
                    Type::Record(_) => {
                        return Err(Diagnostic::new(
                            "numeric literal is not assignable to namespace",
                        ));
                    }
                    Type::TypeParam(_) => {
                        return Err(Diagnostic::new(
                            "generic type parameters must be specialized before IR lowering",
                        ));
                    }
                };
                self.emit(Instruction::Number {
                    ty,
                    literal: number.clone(),
                })
            }
            Expr::Bool(value) => self.emit(Instruction::Bool(*value)),
            Expr::String(value) => self.emit(Instruction::String(value.clone())),
            Expr::Name(name) => {
                if let Some(value) = env.get(name).copied() {
                    let actual = types.get(name).cloned().ok_or_else(|| {
                        Diagnostic::new(format!("unknown local '{name}' during IR lowering"))
                    })?;
                    // If this name is represented as a cell (1-element array) then
                    // load the element before coercion so the value reflects mutations.
                    if self.cell_names.contains(name) {
                        let index0 = self.emit(Instruction::Number {
                            ty: NumericType::I32,
                            literal: NumberLiteral { raw: "0".into() },
                        });
                        let val = self.emit(Instruction::ArrayGet {
                            array: value,
                            index: index0,
                            element_ty: actual.clone(),
                        });
                        self.coerce_value(val, actual, expected)?
                    } else {
                        self.coerce_value(value, actual, expected)?
                    }
                } else if let Some((params, return_type)) = self.signatures.get(name).cloned() {
                    // A bare top-level function name used as a value becomes a
                    // capture-free function reference (funcref), enabling it to
                    // be stored, returned, and called indirectly.
                    let value = self.emit(Instruction::Closure {
                        name: name.clone(),
                        captures: Vec::new(),
                        params: params.clone(),
                        return_type: return_type.clone(),
                    });
                    let actual = Type::Function {
                        params,
                        return_type: Box::new(return_type),
                    };
                    self.coerce_value(value, actual, expected)?
                } else {
                    return Err(Diagnostic::new(format!(
                        "unknown local '{name}' during IR lowering"
                    )));
                }
            }
            Expr::Unary { op, expr } => {
                let actual = self.infer_expr_type(expr, types, None)?;
                match op {
                    UnaryOp::Neg => {
                        let operand_ty = match actual {
                            Type::Numeric(ty) => ty,
                            Type::Bool => {
                                return Err(Diagnostic::new(
                                    "unary '-' requires a numeric operand",
                                ));
                            }
                            Type::String => {
                                return Err(Diagnostic::new(
                                    "unary '-' requires a numeric operand",
                                ));
                            }
                            Type::Array(_) => {
                                return Err(Diagnostic::new(
                                    "unary '-' requires a numeric operand",
                                ));
                            }
                            Type::Function { .. } | Type::Record(_) => {
                                return Err(Diagnostic::new(
                                    "unary '-' requires a numeric operand",
                                ));
                            }
                            Type::Multi(_) => {
                                return Err(Diagnostic::new(
                                    "unary '-' requires a numeric operand",
                                ));
                            }
                            Type::TypeParam(_) => {
                                return Err(Diagnostic::new(
                                    "unary '-' requires a numeric operand",
                                ));
                            }
                        };
                        let zero = self.emit(Instruction::Number {
                            ty: operand_ty,
                            literal: NumberLiteral {
                                raw: "0".to_string(),
                            },
                        });
                        let operand =
                            self.lower_expr(expr, env, types, Some(Type::Numeric(operand_ty)))?;
                        let value = self.emit(Instruction::Binary {
                            op: BinaryOp::Sub,
                            left: zero,
                            right: operand,
                            operand_ty: Type::Numeric(operand_ty),
                            result_ty: Type::Numeric(operand_ty),
                        });
                        self.coerce_value(value, Type::Numeric(operand_ty), expected)?
                    }
                    UnaryOp::Not => {
                        let operand = self.lower_expr(expr, env, types, Some(Type::Bool))?;
                        let false_value = self.emit(Instruction::Bool(false));
                        let value = self.emit(Instruction::Binary {
                            op: BinaryOp::Eq,
                            left: operand,
                            right: false_value,
                            operand_ty: Type::Bool,
                            result_ty: Type::Bool,
                        });
                        self.coerce_value(value, Type::Bool, expected)?
                    }
                    UnaryOp::Len => {
                        let actual = self.infer_expr_type(expr, types, None)?;
                        if !actual.is_array() {
                            return Err(Diagnostic::new("# requires an array operand"));
                        }
                        let array = self.lower_expr(expr, env, types, Some(actual))?;
                        let len = self.emit(Instruction::ArrayLen { array });
                        self.coerce_value(len, Type::Numeric(NumericType::I32), expected)?
                    }
                }
            }
            Expr::Cast { expr, ty } => {
                let value = self.lower_expr(expr, env, types, None)?;
                let actual = self.infer_expr_type(expr, types, None)?;
                let cast = self.explicit_cast(value, actual, ty.clone())?;
                self.coerce_value(cast, ty.clone(), expected)?
            }
            Expr::If {
                condition,
                then_expr,
                else_expr,
            } => {
                let result_ty = self.infer_expr_type(expr, types, expected.clone())?;
                let condition = self.lower_expr(condition, env, types, Some(Type::Bool))?;
                let then_block = self.new_block();
                let else_block = self.new_block();
                let merge_block = self.new_block();
                self.set_terminator(
                    self.current_block,
                    Terminator::Branch {
                        condition,
                        then_block,
                        else_block,
                    },
                );

                self.current_block = then_block;
                let then_value = self.lower_expr(then_expr, env, types, Some(result_ty.clone()))?;
                let then_exit = self.current_block;
                self.set_terminator(then_exit, Terminator::Jump(merge_block));

                self.current_block = else_block;
                let else_value = self.lower_expr(else_expr, env, types, Some(result_ty.clone()))?;
                let else_exit = self.current_block;
                self.set_terminator(else_exit, Terminator::Jump(merge_block));

                self.current_block = merge_block;
                self.emit(Instruction::Phi(vec![
                    (then_exit, then_value),
                    (else_exit, else_value),
                ]))
            }
            Expr::Binary { op, left, right } => match op {
                BinaryOp::And | BinaryOp::Or => {
                    let result_ty = self.infer_expr_type(expr, types, expected.clone())?;
                    let left_value = self.lower_expr(left, env, types, Some(Type::Bool))?;
                    let rhs_block = self.new_block();
                    let short_block = self.new_block();
                    let merge_block = self.new_block();

                    match op {
                        BinaryOp::And => {
                            self.set_terminator(
                                self.current_block,
                                Terminator::Branch {
                                    condition: left_value,
                                    then_block: rhs_block,
                                    else_block: short_block,
                                },
                            );
                        }
                        BinaryOp::Or => {
                            self.set_terminator(
                                self.current_block,
                                Terminator::Branch {
                                    condition: left_value,
                                    then_block: short_block,
                                    else_block: rhs_block,
                                },
                            );
                        }
                        _ => unreachable!(),
                    }

                    self.current_block = short_block;
                    let short_circuit_value =
                        self.emit(Instruction::Bool(matches!(op, BinaryOp::Or)));
                    let short_exit = self.current_block;
                    self.set_terminator(short_exit, Terminator::Jump(merge_block));

                    self.current_block = rhs_block;
                    let rhs_value = self.lower_expr(right, env, types, Some(Type::Bool))?;
                    let rhs_exit = self.current_block;
                    self.set_terminator(rhs_exit, Terminator::Jump(merge_block));

                    self.current_block = merge_block;
                    let mut incoming =
                        vec![(short_exit, short_circuit_value), (rhs_exit, rhs_value)];
                    incoming.sort_by_key(|(pred, _)| *pred);
                    let value = self.emit(Instruction::Phi(incoming));
                    self.coerce_value(value, result_ty, expected)?
                }
                _ => {
                    let operand_ty =
                        self.infer_binary_operand_type(left, right, op, types, None)?;
                    let left = self.lower_expr(left, env, types, Some(operand_ty.clone()))?;
                    let right = self.lower_expr(right, env, types, Some(operand_ty.clone()))?;
                    let raw_result_ty = self.infer_expr_type(expr, types, None)?;
                    let value = self.emit(Instruction::Binary {
                        op: *op,
                        left,
                        right,
                        operand_ty,
                        result_ty: raw_result_ty.clone(),
                    });
                    self.coerce_value(value, raw_result_ty, expected)?
                }
            },
            Expr::Call {
                callee,
                type_args: _,
                args,
            } => {
                if let Expr::Name(name) = callee.as_ref() {
                    if let Some(result) =
                        self.lower_math_builtin_call(name, args, env, types, expected.clone())
                    {
                        return result;
                    }
                }
                if let Expr::Name(name) = callee.as_ref() {
                    if let Some(result) =
                        self.lower_coroutine_builtin_call(name, args, env, types, expected.clone())
                    {
                        return result;
                    }
                }
                if let Expr::Name(name) = callee.as_ref() {
                    if let Some(result) =
                        self.lower_tostring_builtin_call(name, args, env, types, expected.clone())
                    {
                        return result;
                    }
                }
                if let Expr::Name(name) = callee.as_ref() {
                    if let Some((param_types, _)) = self.signatures.get(name) {
                        let args = args
                            .iter()
                            .zip(param_types.iter())
                            .map(|(arg, param_ty)| {
                                self.lower_expr(arg, env, types, Some(param_ty.clone()))
                            })
                            .collect::<Result<Vec<_>, _>>()?;
                        let value = self.emit(Instruction::Call {
                            name: name.clone(),
                            args,
                        });
                        let actual = self.infer_expr_type(expr, types, None)?;
                        return self.coerce_value(value, actual, expected);
                    }
                }
                let callee_ty = self.infer_expr_type(callee, types, None)?;
                let Type::Function {
                    params: param_types,
                    return_type,
                } = callee_ty
                else {
                    return Err(Diagnostic::new("attempt to call non-function value"));
                };
                let callee_value = self.lower_expr(
                    callee,
                    env,
                    types,
                    Some(Type::Function {
                        params: param_types.clone(),
                        return_type: return_type.clone(),
                    }),
                )?;
                let args = args
                    .iter()
                    .zip(param_types.iter())
                    .map(|(arg, param_ty)| self.lower_expr(arg, env, types, Some(param_ty.clone())))
                    .collect::<Result<Vec<_>, _>>()?;
                let value = self.emit(Instruction::CallValue {
                    callee: callee_value,
                    args,
                    params: param_types.clone(),
                    return_type: *return_type,
                });
                let actual = self.infer_expr_type(expr, types, None)?;
                self.coerce_value(value, actual, expected)?
            }
            Expr::Function(function) => {
                let value = self.lower_function_expr(function, env, types)?;
                let actual = self.infer_expr_type(expr, types, None)?;
                self.coerce_value(value, actual, expected)?
            }
            Expr::Require(path) => {
                return Err(Diagnostic::new(format!(
                    "unresolved require(\"{path}\") reached IR lowering"
                )));
            }
            Expr::ArrayLiteral { elements } => {
                let array_ty = self.infer_array_literal_type(elements, types, expected.clone())?;
                let element_ty = array_ty
                    .element_type()
                    .expect("array literal must have element type");
                let lowered = elements
                    .iter()
                    .map(|element| self.lower_expr(element, env, types, Some(element_ty.clone())))
                    .collect::<Result<Vec<_>, _>>()?;
                let value = self.emit(Instruction::ArrayNew {
                    element_ty,
                    elements: lowered,
                });
                self.coerce_value(value, array_ty, expected)?
            }
            Expr::Index { base, index } => {
                let base_ty = self.infer_expr_type(base, types, None)?;
                let element_ty = base_ty
                    .element_type()
                    .ok_or_else(|| Diagnostic::new("indexing requires an array operand"))?;
                let array = self.lower_expr(base, env, types, Some(base_ty))?;
                let index =
                    self.lower_expr(index, env, types, Some(Type::Numeric(NumericType::I32)))?;
                let value = self.emit(Instruction::ArrayGet {
                    array,
                    index,
                    element_ty: element_ty.clone(),
                });
                self.coerce_value(value, element_ty, expected)?
            }
            Expr::TableLiteral { .. } => {
                return Err(Diagnostic::new(
                    "table literals are only supported in module export expressions",
                ));
            }
            Expr::Field { .. } => {
                return Err(Diagnostic::new(
                    "namespace member access must be resolved before IR lowering",
                ));
            }
        };
        Ok(value)
    }

    fn lower_expr_list(
        &mut self,
        exprs: &[Expr],
        env: &HashMap<String, ValueId>,
        types: &HashMap<String, Type>,
        expected: Option<&[Type]>,
    ) -> Result<Vec<ValueId>, Diagnostic> {
        let mut out = Vec::new();
        for expr in exprs {
            let slot_expected = expected.and_then(|types| types.get(out.len()).cloned());
            let ty = if matches!(expr, Expr::Call { .. }) {
                self.infer_expr_type(expr, types, None)?
            } else {
                self.infer_expr_type(expr, types, slot_expected.clone())?
            };
            match ty {
                Type::Multi(multi_types) => {
                    let tuple = self.lower_expr(expr, env, types, None)?;
                    for (index, part) in multi_types.into_iter().enumerate() {
                        let coerced =
                            if let Some(exp) = expected.and_then(|types| types.get(out.len())) {
                                coerce_type(part, Some(exp.clone()))?
                            } else {
                                part
                            };
                        let value = self.emit(Instruction::MultiGet {
                            value: tuple,
                            index,
                            ty: coerced,
                        });
                        out.push(value);
                    }
                }
                scalar => {
                    let coerced = if let Some(exp) = expected.and_then(|types| types.get(out.len()))
                    {
                        coerce_type(scalar, Some(exp.clone()))?
                    } else {
                        scalar
                    };
                    let value = self.lower_expr(expr, env, types, Some(coerced))?;
                    out.push(value);
                }
            }
        }
        Ok(out)
    }

    fn lower_function_expr(
        &mut self,
        function: &waluau_ast::FunctionExpr,
        env: &HashMap<String, ValueId>,
        types: &HashMap<String, Type>,
    ) -> Result<ValueId, Diagnostic> {
        let return_ty = Self::function_expr_return_type(function)?;
        let captures = collect_captures(function, env, types, self.signatures);
        let capture_values = captures
            .iter()
            .map(|(name, _)| {
                env.get(name).copied().ok_or_else(|| {
                    Diagnostic::new(format!("unknown local '{name}' during IR lowering"))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let lifted_name = format!("{}$lambda{}", self.function.name, self.lambda_counter);
        self.lambda_counter += 1;

        let mut lifted = Function {
            name: lifted_name.clone(),
            params: Vec::new(),
            return_type: return_ty.clone(),
            entry: BlockId(0),
            blocks: BTreeMap::new(),
            next_value: 0,
        };
        lifted.blocks.insert(
            lifted.entry,
            BasicBlock {
                id: lifted.entry,
                instructions: Vec::new(),
                terminator: Terminator::Unreachable,
            },
        );
        for (name, ty) in &captures {
            // Captured variables are passed as 1-element array "cells" to nested
            // (lifted) functions so they can observe/mutate shared storage.
            lifted
                .params
                .push((name.clone(), Type::Array(Box::new(ty.clone()))));
        }
        for param in &function.params {
            lifted.params.push((param.name.clone(), param.ty.clone()));
        }

        let mut nested_env = HashMap::new();
        let mut nested_types = HashMap::new();
        let lifted_entry = lifted.entry;
        for (index, (name, ty)) in lifted.params.clone().into_iter().enumerate() {
            let value = lifted.next_value();
            block_mut(&mut lifted, lifted_entry)
                .instructions
                .push((value, Instruction::Param(index)));
            nested_env.insert(name.clone(), value);
            // If the lifted param is an array cell for a captured variable, expose
            // the inner element type within the nested function's type map so that
            // expressions using the name are treated as the element type during lowering.
            if let Some(elem) = ty.element_type() {
                nested_types.insert(name, elem);
            } else {
                nested_types.insert(name, ty);
            }
        }

        // nested builder should treat the capture parameters as cell-backed names
        // so the nested function will access them via ArrayGet/ArraySet.
        let mut capture_param_names: HashSet<String> =
            captures.iter().map(|(n, _)| n.clone()).collect();
        // Also include any names that the nested function's inner nested functions capture.
        let nested_inner_captures = collect_nested_function_capture_names(&waluau_ast::Function {
            name: function.name.clone().unwrap_or_default(),
            type_params: function.type_params.clone(),
            params: function.params.clone(),
            return_type: Some(return_ty.clone()),
            body: function.body.clone(),
        });
        capture_param_names.extend(nested_inner_captures);

        let mut nested = Builder {
            function: lifted,
            current_block: BlockId(0),
            next_block: 1,
            signatures: self.signatures,
            lifted_functions: Vec::new(),
            lambda_counter: 0,
            loop_stack: Vec::new(),
            cell_names: capture_param_names,
        };
        if let Some(name) = &function.name {
            let capture_param_values = captures
                .iter()
                .map(|(capture_name, _)| {
                    nested_env.get(capture_name).copied().ok_or_else(|| {
                        Diagnostic::new(format!(
                            "missing capture '{}' in nested function lowering",
                            capture_name
                        ))
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            let self_callee = nested.emit(Instruction::Closure {
                name: lifted_name.clone(),
                captures: capture_param_values,
                params: function
                    .params
                    .iter()
                    .map(|param| param.ty.clone())
                    .collect(),
                return_type: return_ty.clone(),
            });
            nested_env.insert(name.clone(), self_callee);
            nested_types.insert(
                name.clone(),
                Type::Function {
                    params: function
                        .params
                        .iter()
                        .map(|param| param.ty.clone())
                        .collect(),
                    return_type: Box::new(return_ty.clone()),
                },
            );
        }
        for stmt in &function.body {
            if nested.current_block == DEAD_BLOCK {
                break;
            }
            nested.lower_stmt(stmt, &mut nested_env, &mut nested_types)?;
        }
        self.lifted_functions.push(nested.function);
        self.lifted_functions.extend(nested.lifted_functions);

        Ok(self.emit(Instruction::Closure {
            name: lifted_name,
            captures: capture_values,
            params: function
                .params
                .iter()
                .map(|param| param.ty.clone())
                .collect(),
            return_type: return_ty,
        }))
    }

    fn infer_expr_type(
        &self,
        expr: &Expr,
        types: &HashMap<String, Type>,
        expected: Option<Type>,
    ) -> Result<Type, Diagnostic> {
        match expr {
            Expr::Number(_) => match expected {
                Some(Type::Numeric(ty)) => Ok(Type::Numeric(ty)),
                Some(Type::Bool) => {
                    Err(Diagnostic::new("numeric literal is not assignable to bool"))
                }
                Some(Type::String) => Err(Diagnostic::new(
                    "numeric literal is not assignable to string",
                )),
                Some(Type::Array(_)) => Err(Diagnostic::new(
                    "numeric literal is not assignable to array",
                )),
                Some(Type::Function { .. }) => Err(Diagnostic::new(
                    "numeric literal is not assignable to function",
                )),
                Some(Type::Multi(_)) => Err(Diagnostic::new(
                    "numeric literal is not assignable to multiple values",
                )),
                Some(Type::Record(_)) => Err(Diagnostic::new(
                    "numeric literal is not assignable to namespace",
                )),
                Some(Type::TypeParam(_)) => Err(Diagnostic::new(
                    "numeric literal is not assignable to generic type parameter",
                )),
                None => Ok(Type::number()),
            },
            Expr::Bool(_) => Ok(Type::Bool),
            Expr::String(_) => Ok(Type::String),
            Expr::Require(path) => Err(Diagnostic::new(format!(
                "unresolved require(\"{path}\") reached IR lowering"
            ))),
            Expr::Name(name) => {
                if let Some(ty) = types.get(name) {
                    Ok(ty.clone())
                } else if let Some((params, ret)) = self.signatures.get(name) {
                    Ok(Type::Function {
                        params: params.clone(),
                        return_type: Box::new(ret.clone()),
                    })
                } else {
                    Err(Diagnostic::new(format!(
                        "unknown local '{name}' during IR lowering"
                    )))
                }
            }
            Expr::Unary { op, expr } => match op {
                UnaryOp::Neg => {
                    let actual = self.infer_expr_type(expr, types, expected.clone())?;
                    match actual {
                        Type::Numeric(_) => coerce_type(actual, expected),
                        Type::Bool => Err(Diagnostic::new("unary '-' requires a numeric operand")),
                        Type::String => {
                            Err(Diagnostic::new("unary '-' requires a numeric operand"))
                        }
                        Type::Array(_) => {
                            Err(Diagnostic::new("unary '-' requires a numeric operand"))
                        }
                        Type::Function { .. } | Type::Record(_) => {
                            Err(Diagnostic::new("unary '-' requires a numeric operand"))
                        }
                        Type::Multi(_) => {
                            Err(Diagnostic::new("unary '-' requires a numeric operand"))
                        }
                        Type::TypeParam(_) => {
                            Err(Diagnostic::new("unary '-' requires a numeric operand"))
                        }
                    }
                }
                UnaryOp::Not => {
                    let actual = self.infer_expr_type(expr, types, Some(Type::Bool))?;
                    if actual == Type::Bool {
                        Ok(Type::Bool)
                    } else {
                        Err(Diagnostic::new("unary 'not' requires a bool operand"))
                    }
                }
                UnaryOp::Len => {
                    let actual = self.infer_expr_type(expr, types, None)?;
                    if !actual.is_array() {
                        Err(Diagnostic::new("# requires an array operand"))
                    } else {
                        coerce_type(Type::Numeric(NumericType::I32), expected)
                    }
                }
            },
            Expr::Cast { expr, ty } => {
                let actual = self.infer_expr_type(expr, types, None)?;
                require_numeric_cast(actual, ty.clone())?;
                Ok(ty.clone())
            }
            Expr::If {
                condition,
                then_expr,
                else_expr,
            } => {
                let condition_ty = self.infer_expr_type(condition, types, Some(Type::Bool))?;
                if condition_ty != Type::Bool {
                    return Err(Diagnostic::new("if expression condition must be bool"));
                }
                let then_ty = self.infer_expr_type(then_expr, types, expected.clone())?;
                let else_ty = self.infer_expr_type(else_expr, types, expected.clone())?;
                if then_ty == else_ty {
                    Ok(then_ty)
                } else {
                    Err(Diagnostic::new(
                        "if expression branches must resolve to the same type",
                    ))
                }
            }
            Expr::Call { callee, .. } => {
                if let Expr::Name(name) = callee.as_ref() {
                    if let Some(result) = self.infer_math_builtin_call_type(name, expr, types) {
                        return result;
                    }
                }
                if let Expr::Name(name) = callee.as_ref() {
                    if let Some(result) = self.infer_coroutine_builtin_call_type(name, expr, types)
                    {
                        return result;
                    }
                }
                if let Expr::Name(name) = callee.as_ref() {
                    if let Some(result) = self.infer_tostring_builtin_call_type(name, expr, types) {
                        return result;
                    }
                }
                let callee_ty = self.infer_expr_type(callee, types, None)?;
                match callee_ty {
                    Type::Function { return_type, .. } => Ok(*return_type),
                    other => Err(Diagnostic::new(format!(
                        "attempt to call non-function value of type {other}",
                    ))),
                }
            }
            Expr::Function(function) => Ok(Type::Function {
                return_type: Box::new(Self::function_expr_return_type(function)?),
                params: function
                    .params
                    .iter()
                    .map(|param| param.ty.clone())
                    .collect(),
            }),
            Expr::ArrayLiteral { elements } => {
                self.infer_array_literal_type(elements, types, expected)
            }
            Expr::TableLiteral { .. } => Err(Diagnostic::new(
                "table literals are only supported in module export expressions",
            )),
            Expr::Field { .. } => Err(Diagnostic::new(
                "namespace member access must be resolved before IR lowering",
            )),
            Expr::Index { base, index } => {
                let base_ty = self.infer_expr_type(base, types, None)?;
                let element_ty = base_ty
                    .element_type()
                    .ok_or_else(|| Diagnostic::new("indexing requires an array operand"))?;
                let index_ty =
                    self.infer_expr_type(index, types, Some(Type::Numeric(NumericType::I32)))?;
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
                | BinaryOp::Mod
                | BinaryOp::Concat => {
                    let raw = self.infer_binary_operand_type(left, right, op, types, None)?;
                    coerce_type(raw, expected)
                }
                BinaryOp::Less
                | BinaryOp::Greater
                | BinaryOp::Eq
                | BinaryOp::And
                | BinaryOp::Or => Ok(Type::Bool),
            },
        }
    }

    fn infer_binary_operand_type(
        &self,
        left: &Expr,
        right: &Expr,
        op: &BinaryOp,
        types: &HashMap<String, Type>,
        expected: Option<Type>,
    ) -> Result<Type, Diagnostic> {
        let expected_numeric = match expected {
            Some(Type::Numeric(numeric)) => Some(numeric),
            _ => None,
        };

        match op {
            BinaryOp::And | BinaryOp::Or => Ok(Type::Bool),
            BinaryOp::Eq => {
                let left_ty = self.infer_expr_type(left, types, None)?;
                if left_ty == Type::Bool {
                    let right_ty = self.infer_expr_type(right, types, Some(Type::Bool))?;
                    if right_ty == Type::Bool {
                        Ok(Type::Bool)
                    } else {
                        Err(Diagnostic::new(
                            "could not resolve operand type during IR lowering",
                        ))
                    }
                } else if left_ty == Type::String {
                    let right_ty = self.infer_expr_type(right, types, Some(Type::String))?;
                    if right_ty == Type::String {
                        Ok(Type::String)
                    } else {
                        Err(Diagnostic::new(
                            "could not resolve operand type during IR lowering",
                        ))
                    }
                } else {
                    infer_numeric_common_type(
                        left,
                        right,
                        types,
                        expected_numeric,
                        |expr, expected| self.infer_expr_type(expr, types, expected),
                    )
                }
            }
            BinaryOp::Concat => {
                let left_ty = self.infer_expr_type(left, types, None)?;
                if left_ty == Type::String {
                    let right_ty = self.infer_expr_type(right, types, Some(Type::String))?;
                    if right_ty == Type::String {
                        Ok(Type::String)
                    } else {
                        Err(Diagnostic::new(
                            "could not resolve operand type during IR lowering",
                        ))
                    }
                } else {
                    Err(Diagnostic::new(
                        "could not resolve operand type during IR lowering",
                    ))
                }
            }
            BinaryOp::Add => {
                let left_ty = self.infer_expr_type(left, types, None)?;
                if left_ty == Type::String {
                    Err(Diagnostic::new(
                        "could not resolve operand type during IR lowering",
                    ))
                } else {
                    infer_numeric_common_type(
                        left,
                        right,
                        types,
                        expected_numeric,
                        |expr, expected| self.infer_expr_type(expr, types, expected),
                    )
                }
            }
            BinaryOp::Sub
            | BinaryOp::Mul
            | BinaryOp::Div
            | BinaryOp::FloorDiv
            | BinaryOp::Mod
            | BinaryOp::Less
            | BinaryOp::Greater => {
                infer_numeric_common_type(left, right, types, expected_numeric, |expr, expected| {
                    self.infer_expr_type(expr, types, expected)
                })
            }
        }
    }

    fn coerce_value(
        &mut self,
        value: ValueId,
        actual: Type,
        expected: Option<Type>,
    ) -> Result<ValueId, Diagnostic> {
        match expected {
            None => Ok(value),
            Some(expected) if actual == expected => Ok(value),
            Some(expected) => {
                let target = coerce_type(actual.clone(), Some(expected.clone()))?;
                if target == actual {
                    Ok(value)
                } else {
                    Ok(self.emit(Instruction::Cast {
                        value,
                        from: actual,
                        to: target,
                    }))
                }
            }
        }
    }

    fn explicit_cast(
        &mut self,
        value: ValueId,
        from: Type,
        to: Type,
    ) -> Result<ValueId, Diagnostic> {
        require_numeric_cast(from.clone(), to.clone())?;
        if from == to {
            Ok(value)
        } else {
            Ok(self.emit(Instruction::Cast { value, from, to }))
        }
    }

    fn infer_array_literal_type(
        &self,
        elements: &[Expr],
        types: &HashMap<String, Type>,
        expected: Option<Type>,
    ) -> Result<Type, Diagnostic> {
        if elements.is_empty() {
            return Err(inference_diagnostic(
                "inference/missing-context",
                DiagnosticCategory::MissingContext,
                "empty array literal requires explicit element type",
                "add an explicit element type annotation, e.g. local xs: {i32} = {}",
            ));
        }

        let expected_element = expected.as_ref().and_then(Type::element_type);
        let mut iter = elements.iter();
        let first = iter.next().expect("non-empty array literal");
        let mut element_ty = self.infer_expr_type(first, types, expected_element.clone())?;
        for element in iter {
            let actual = self.infer_expr_type(element, types, Some(element_ty.clone()))?;
            element_ty = common_element_type(element_ty, actual)?;
        }

        coerce_type(Type::Array(Box::new(element_ty)), expected)
    }

    fn lower_coroutine_builtin_call(
        &mut self,
        name: &str,
        args: &[Expr],
        env: &HashMap<String, ValueId>,
        types: &HashMap<String, Type>,
        expected: Option<Type>,
    ) -> Option<Result<ValueId, Diagnostic>> {
        match name {
            COROUTINE_CREATE => {
                if args.len() != 1 {
                    return Some(Err(Diagnostic::new(format!(
                        "{COROUTINE_CREATE} expects 1 argument, got {}",
                        args.len()
                    ))));
                }
                let coroutine_ty = match self.infer_expr_type(&args[0], types, None) {
                    Ok(ty) => ty,
                    Err(error) => return Some(Err(error)),
                };
                match &coroutine_ty {
                    Type::Function { params, .. } if params.is_empty() => {}
                    _ => {
                        return Some(Err(Diagnostic::new(
                            "coroutine_create expects a zero-argument function",
                        )));
                    }
                }
                let coroutine =
                    match self.lower_expr(&args[0], env, types, Some(coroutine_ty.clone())) {
                        Ok(value) => value,
                        Err(error) => return Some(Err(error)),
                    };
                Some(self.coerce_value(coroutine, coroutine_ty, expected))
            }
            COROUTINE_RESUME => {
                if args.len() != 1 {
                    return Some(Err(Diagnostic::new(format!(
                        "{COROUTINE_RESUME} expects 1 argument, got {}",
                        args.len()
                    ))));
                }
                let coroutine_ty = match self.infer_expr_type(&args[0], types, None) {
                    Ok(ty) => ty,
                    Err(error) => return Some(Err(error)),
                };
                match coroutine_ty {
                    Type::Function {
                        params,
                        return_type,
                    } if params.is_empty() => {
                        let coroutine = match self.lower_expr(
                            &args[0],
                            env,
                            types,
                            Some(Type::Function {
                                params: Vec::new(),
                                return_type: return_type.clone(),
                            }),
                        ) {
                            Ok(value) => value,
                            Err(error) => return Some(Err(error)),
                        };
                        let value = self.emit(Instruction::CallValue {
                            callee: coroutine,
                            args: Vec::new(),
                            params: Vec::new(),
                            return_type: (*return_type).clone(),
                        });
                        Some(self.coerce_value(value, *return_type, expected))
                    }
                    _ => Some(Err(Diagnostic::new(
                        "coroutine_resume expects a coroutine created from a zero-argument function",
                    ))),
                }
            }
            COROUTINE_STATUS => {
                if args.len() != 1 {
                    return Some(Err(Diagnostic::new(format!(
                        "{COROUTINE_STATUS} expects 1 argument, got {}",
                        args.len()
                    ))));
                }
                let coroutine_ty = match self.infer_expr_type(&args[0], types, None) {
                    Ok(ty) => ty,
                    Err(error) => return Some(Err(error)),
                };
                match coroutine_ty {
                    Type::Function { params, .. } if params.is_empty() => {
                        let value = self.emit(Instruction::Bool(true));
                        Some(self.coerce_value(value, Type::Bool, expected))
                    }
                    _ => Some(Err(Diagnostic::new(
                        "coroutine_status expects a coroutine created from a zero-argument function",
                    ))),
                }
            }
            _ => None,
        }
    }

    fn infer_coroutine_builtin_call_type(
        &self,
        name: &str,
        call: &Expr,
        types: &HashMap<String, Type>,
    ) -> Option<Result<Type, Diagnostic>> {
        let Expr::Call { args, .. } = call else {
            return None;
        };
        match name {
            COROUTINE_CREATE => {
                if args.len() != 1 {
                    return Some(Err(Diagnostic::new(format!(
                        "{COROUTINE_CREATE} expects 1 argument, got {}",
                        args.len()
                    ))));
                }
                let coroutine_ty = match self.infer_expr_type(&args[0], types, None) {
                    Ok(ty) => ty,
                    Err(error) => return Some(Err(error)),
                };
                match &coroutine_ty {
                    Type::Function { params, .. } if params.is_empty() => Some(Ok(coroutine_ty)),
                    _ => Some(Err(Diagnostic::new(
                        "coroutine_create expects a zero-argument function",
                    ))),
                }
            }
            COROUTINE_RESUME => {
                if args.len() != 1 {
                    return Some(Err(Diagnostic::new(format!(
                        "{COROUTINE_RESUME} expects 1 argument, got {}",
                        args.len()
                    ))));
                }
                let coroutine_ty = match self.infer_expr_type(&args[0], types, None) {
                    Ok(ty) => ty,
                    Err(error) => return Some(Err(error)),
                };
                match coroutine_ty {
                    Type::Function {
                        params,
                        return_type,
                    } if params.is_empty() => Some(Ok(*return_type)),
                    _ => Some(Err(Diagnostic::new(
                        "coroutine_resume expects a coroutine created from a zero-argument function",
                    ))),
                }
            }
            COROUTINE_STATUS => {
                if args.len() != 1 {
                    return Some(Err(Diagnostic::new(format!(
                        "{COROUTINE_STATUS} expects 1 argument, got {}",
                        args.len()
                    ))));
                }
                let coroutine_ty = match self.infer_expr_type(&args[0], types, None) {
                    Ok(ty) => ty,
                    Err(error) => return Some(Err(error)),
                };
                match coroutine_ty {
                    Type::Function { params, .. } if params.is_empty() => Some(Ok(Type::Bool)),
                    _ => Some(Err(Diagnostic::new(
                        "coroutine_status expects a coroutine created from a zero-argument function",
                    ))),
                }
            }
            _ => None,
        }
    }

    fn lower_math_builtin_call(
        &mut self,
        name: &str,
        args: &[Expr],
        env: &HashMap<String, ValueId>,
        types: &HashMap<String, Type>,
        expected: Option<Type>,
    ) -> Option<Result<ValueId, Diagnostic>> {
        let (intrinsic, arity) = match name {
            MATH_ABS => (MathIntrinsic::Abs, 1),
            MATH_MIN => (MathIntrinsic::Min, 2),
            MATH_MAX => (MathIntrinsic::Max, 2),
            MATH_SQRT => (MathIntrinsic::Sqrt, 1),
            MATH_FLOOR => (MathIntrinsic::Floor, 1),
            MATH_CEIL => (MathIntrinsic::Ceil, 1),
            MATH_TRUNC => (MathIntrinsic::Trunc, 1),
            MATH_NEAREST => (MathIntrinsic::Nearest, 1),
            MATH_COPYSIGN => (MathIntrinsic::Copysign, 2),
            _ => return None,
        };
        if args.len() != arity {
            return Some(Err(Diagnostic::new(format!(
                "{name} expects {arity} argument{}, got {}",
                if arity == 1 { "" } else { "s" },
                args.len()
            ))));
        }
        let operand_ty = match self.infer_math_builtin_call_type(
            name,
            &Expr::Call {
                callee: Box::new(Expr::Name(name.to_string())),
                type_args: Vec::new(),
                args: args.to_vec(),
            },
            types,
        ) {
            Some(Ok(Type::Numeric(ty))) => Type::Numeric(ty),
            Some(Ok(_)) => unreachable!(),
            Some(Err(error)) => return Some(Err(error)),
            None => return None,
        };
        let mut lowered = Vec::with_capacity(args.len());
        for arg in args {
            match self.lower_expr(arg, env, types, Some(operand_ty.clone())) {
                Ok(value) => lowered.push(value),
                Err(error) => return Some(Err(error)),
            }
        }
        let value = self.emit(Instruction::MathIntrinsic {
            intrinsic,
            args: lowered,
            operand_ty: operand_ty.clone(),
            result_ty: operand_ty.clone(),
        });
        Some(self.coerce_value(value, operand_ty, expected))
    }

    fn lower_tostring_builtin_call(
        &mut self,
        name: &str,
        args: &[Expr],
        env: &HashMap<String, ValueId>,
        types: &HashMap<String, Type>,
        expected: Option<Type>,
    ) -> Option<Result<ValueId, Diagnostic>> {
        if name != TO_STRING {
            return None;
        }
        if args.len() != 1 {
            return Some(Err(Diagnostic::new(format!(
                "{TO_STRING} expects 1 argument, got {}",
                args.len()
            ))));
        }
        let arg_ty = match self.infer_expr_type(&args[0], types, None) {
            Ok(ty) => ty,
            Err(error) => return Some(Err(error)),
        };
        if !(arg_ty.is_numeric() || arg_ty == Type::Bool || arg_ty == Type::String) {
            return Some(Err(Diagnostic::new(format!(
                "{TO_STRING} expects a primitive argument (numeric, bool, or string), got {arg_ty}",
            ))));
        }
        let lowered = match self.lower_expr(&args[0], env, types, Some(arg_ty.clone())) {
            Ok(value) => value,
            Err(error) => return Some(Err(error)),
        };
        let value = if arg_ty == Type::String {
            lowered
        } else {
            self.emit(Instruction::ToString {
                value: lowered,
                from: arg_ty,
            })
        };
        Some(self.coerce_value(value, Type::String, expected))
    }

    fn infer_math_builtin_call_type(
        &self,
        name: &str,
        call: &Expr,
        types: &HashMap<String, Type>,
    ) -> Option<Result<Type, Diagnostic>> {
        let Expr::Call { args, .. } = call else {
            return None;
        };
        let (intrinsic, arity) = match name {
            MATH_ABS => (MathIntrinsic::Abs, 1),
            MATH_MIN => (MathIntrinsic::Min, 2),
            MATH_MAX => (MathIntrinsic::Max, 2),
            MATH_SQRT => (MathIntrinsic::Sqrt, 1),
            MATH_FLOOR => (MathIntrinsic::Floor, 1),
            MATH_CEIL => (MathIntrinsic::Ceil, 1),
            MATH_TRUNC => (MathIntrinsic::Trunc, 1),
            MATH_NEAREST => (MathIntrinsic::Nearest, 1),
            MATH_COPYSIGN => (MathIntrinsic::Copysign, 2),
            _ => return None,
        };
        if args.len() != arity {
            return Some(Err(Diagnostic::new(format!(
                "{name} expects {arity} argument{}, got {}",
                if arity == 1 { "" } else { "s" },
                args.len()
            ))));
        }

        let first_ty = match self.infer_expr_type(&args[0], types, None) {
            Ok(ty) => ty,
            Err(error) => return Some(Err(error)),
        };
        let Type::Numeric(first_numeric) = first_ty else {
            return Some(Err(Diagnostic::new(format!(
                "{name} expects numeric arguments"
            ))));
        };
        if arity == 2 {
            let second =
                match self.infer_expr_type(&args[1], types, Some(Type::Numeric(first_numeric))) {
                    Ok(ty) => ty,
                    Err(error) => return Some(Err(error)),
                };
            if second != Type::Numeric(first_numeric) {
                return Some(Err(Diagnostic::new(format!(
                    "{name} requires both arguments to have the same numeric type"
                ))));
            }
        }

        let supports = match intrinsic {
            MathIntrinsic::Min | MathIntrinsic::Max => {
                matches!(first_numeric, NumericType::F32 | NumericType::F64)
            }
            MathIntrinsic::Abs
            | MathIntrinsic::Sqrt
            | MathIntrinsic::Floor
            | MathIntrinsic::Ceil
            | MathIntrinsic::Trunc
            | MathIntrinsic::Nearest
            | MathIntrinsic::Copysign => {
                matches!(first_numeric, NumericType::F32 | NumericType::F64)
            }
        };
        if !supports {
            return Some(Err(Diagnostic::new(format!(
                "{name} does not support {}",
                Type::Numeric(first_numeric)
            ))));
        }

        Some(Ok(Type::Numeric(first_numeric)))
    }

    fn infer_tostring_builtin_call_type(
        &self,
        name: &str,
        call: &Expr,
        types: &HashMap<String, Type>,
    ) -> Option<Result<Type, Diagnostic>> {
        if name != TO_STRING {
            return None;
        }
        let Expr::Call { args, .. } = call else {
            return None;
        };
        if args.len() != 1 {
            return Some(Err(Diagnostic::new(format!(
                "{TO_STRING} expects 1 argument, got {}",
                args.len()
            ))));
        }
        let arg_ty = match self.infer_expr_type(&args[0], types, None) {
            Ok(ty) => ty,
            Err(error) => return Some(Err(error)),
        };
        if arg_ty.is_numeric() || arg_ty == Type::Bool || arg_ty == Type::String {
            Some(Ok(Type::String))
        } else {
            Some(Err(Diagnostic::new(format!(
                "{TO_STRING} expects a primitive argument (numeric, bool, or string), got {arg_ty}",
            ))))
        }
    }
}

fn common_element_type(left: Type, right: Type) -> Result<Type, Diagnostic> {
    match (left, right) {
        (Type::Numeric(left), Type::Numeric(right)) => {
            left.common(right).map(Type::Numeric).ok_or_else(|| {
                inference_diagnostic(
                    "inference/conflict",
                    DiagnosticCategory::Conflict,
                    "array literal elements must share a common type",
                    "cast elements to a common numeric type or annotate the array type",
                )
            })
        }
        (left, right) if left == right => Ok(left),
        _ => Err(inference_diagnostic(
            "inference/conflict",
            DiagnosticCategory::Conflict,
            "array literal elements must share a common type",
            "cast elements to a common type or split values into separate arrays",
        )),
    }
}

fn infer_numeric_common_type(
    left: &Expr,
    right: &Expr,
    _types: &HashMap<String, Type>,
    expected: Option<NumericType>,
    infer: impl Fn(&Expr, Option<Type>) -> Result<Type, Diagnostic>,
) -> Result<Type, Diagnostic> {
    match (
        matches!(left, Expr::Number(_)),
        matches!(right, Expr::Number(_)),
    ) {
        (true, true) => {
            let ty = Type::Numeric(expected.unwrap_or(NumericType::F64));
            let left_ty = infer(left, Some(ty.clone()))?;
            let right_ty = infer(right, Some(ty))?;
            if left_ty == right_ty {
                Ok(left_ty)
            } else {
                Err(inference_diagnostic(
                    "inference/ambiguous",
                    DiagnosticCategory::Ambiguous,
                    "could not resolve operand type during IR lowering",
                    "add an explicit cast to disambiguate numeric operand types",
                ))
            }
        }
        (true, false) => {
            let right_ty = infer(right, None)?;
            let left_ty = infer(left, Some(right_ty.clone()))?;
            common_numeric_type(left_ty, right_ty)
        }
        (false, true) => {
            let left_ty = infer(left, None)?;
            let right_ty = infer(right, Some(left_ty.clone()))?;
            common_numeric_type(left_ty, right_ty)
        }
        (false, false) => {
            let left_ty = infer(left, None)?;
            let right_ty = infer(right, None)?;
            common_numeric_type(left_ty, right_ty)
        }
    }
}

fn common_numeric_type(left: Type, right: Type) -> Result<Type, Diagnostic> {
    match (left, right) {
        (Type::Numeric(left), Type::Numeric(right)) => {
            left.common(right).map(Type::Numeric).ok_or_else(|| {
                inference_diagnostic(
                    "inference/ambiguous",
                    DiagnosticCategory::Ambiguous,
                    "could not resolve operand type during IR lowering",
                    "add an explicit cast to disambiguate numeric operand types",
                )
            })
        }
        _ => Err(inference_diagnostic(
            "inference/conflict",
            DiagnosticCategory::Conflict,
            "could not resolve operand type during IR lowering",
            "change one operand type or cast explicitly",
        )),
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
            Type::String => Err(Diagnostic::new(format!(
                "cannot implicitly convert string to {expected_numeric}",
            ))),
            Type::Array(_) => Err(Diagnostic::new(format!(
                "cannot implicitly convert array to {expected_numeric}",
            ))),
            Type::Multi(_) => Err(Diagnostic::new(format!(
                "cannot implicitly convert multiple values to {expected_numeric}",
            ))),
            Type::Function { .. } => Err(Diagnostic::new(format!(
                "cannot implicitly convert function to {expected_numeric}",
            ))),
            Type::Record(_) => Err(Diagnostic::new(format!(
                "cannot implicitly convert namespace to {expected_numeric}",
            ))),
            Type::TypeParam(_) => Err(Diagnostic::new(format!(
                "cannot implicitly convert generic type parameter to {expected_numeric}",
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

fn block_mut(function: &mut Function, block: BlockId) -> &mut BasicBlock {
    function
        .blocks
        .get_mut(&block)
        .expect("block must exist when mutating")
}

fn collect_assigned_names(stmts: &[Stmt]) -> BTreeSet<String> {
    let mut assigned = BTreeSet::new();
    collect_assigned_into(stmts, &mut assigned);
    assigned
}

fn collect_captures(
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
        Expr::Name(name) => {
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
        } => {
            collect_expr_captures(condition, bound, env, signatures, captures);
            collect_expr_captures(then_expr, bound, env, signatures, captures);
            collect_expr_captures(else_expr, bound, env, signatures, captures);
        }
        Expr::Call {
            callee,
            type_args: _,
            args,
        } => {
            collect_expr_captures(callee, bound, env, signatures, captures);
            for arg in args {
                collect_expr_captures(arg, bound, env, signatures, captures);
            }
        }
        Expr::Function(_) => {}
        Expr::ArrayLiteral { elements } => {
            for element in elements {
                collect_expr_captures(element, bound, env, signatures, captures);
            }
        }
        Expr::TableLiteral { fields } => {
            for field in fields {
                collect_expr_captures(&field.value, bound, env, signatures, captures);
            }
        }
        Expr::Field { base, .. } => {
            collect_expr_captures(base, bound, env, signatures, captures);
        }
        Expr::Index { base, index } => {
            collect_expr_captures(base, bound, env, signatures, captures);
            collect_expr_captures(index, bound, env, signatures, captures);
        }
        Expr::Number(_) | Expr::Bool(_) | Expr::String(_) | Expr::Require(_) => {}
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
            Stmt::IndexAssign { .. } => {}
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
fn collect_nested_function_capture_names(function: &waluau_ast::Function) -> HashSet<String> {
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
        } => {
            collect_nested_from_expr(condition, out);
            collect_nested_from_expr(then_expr, out);
            collect_nested_from_expr(else_expr, out);
        }
        Expr::Call {
            callee,
            type_args: _,
            args,
        } => {
            collect_nested_from_expr(callee, out);
            for a in args {
                collect_nested_from_expr(a, out);
            }
        }
        Expr::ArrayLiteral { elements } => {
            for e in elements {
                collect_nested_from_expr(e, out);
            }
        }
        Expr::TableLiteral { fields } => {
            for field in fields {
                collect_nested_from_expr(&field.value, out);
            }
        }
        Expr::Field { base, .. } => collect_nested_from_expr(base, out),
        Expr::Index { base, index } => {
            collect_nested_from_expr(base, out);
            collect_nested_from_expr(index, out);
        }
        Expr::Name(_) | Expr::Number(_) | Expr::Bool(_) | Expr::String(_) | Expr::Require(_) => {}
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
        Expr::Name(name) => {
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
        } => {
            collect_free_names_in_expr(condition, bound, out);
            collect_free_names_in_expr(then_expr, bound, out);
            collect_free_names_in_expr(else_expr, bound, out);
        }
        Expr::Call {
            callee,
            type_args: _,
            args,
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
        Expr::ArrayLiteral { elements } => {
            for e in elements {
                collect_free_names_in_expr(e, bound, out);
            }
        }
        Expr::TableLiteral { fields } => {
            for field in fields {
                collect_free_names_in_expr(&field.value, bound, out);
            }
        }
        Expr::Field { base, .. } => collect_free_names_in_expr(base, bound, out),
        Expr::Index { base, index } => {
            collect_free_names_in_expr(base, bound, out);
            collect_free_names_in_expr(index, bound, out);
        }
        Expr::Number(_) | Expr::Bool(_) | Expr::String(_) | Expr::Require(_) => {}
    }
}

fn add_phi_incoming(
    function: &mut Function,
    block: BlockId,
    phi: ValueId,
    incoming: (BlockId, ValueId),
) {
    let bb = block_mut(function, block);
    let (_, instruction) = bb
        .instructions
        .iter_mut()
        .find(|(value, _)| *value == phi)
        .expect("phi value must exist in header block");
    if let Instruction::Phi(values) = instruction {
        values.push(incoming);
    }
}

fn predecessors(function: &Function) -> HashMap<BlockId, Vec<BlockId>> {
    let mut out: HashMap<BlockId, Vec<BlockId>> = HashMap::new();
    for (id, block) in &function.blocks {
        match &block.terminator {
            Terminator::Jump(target) => out.entry(*target).or_default().push(*id),
            Terminator::Branch {
                then_block,
                else_block,
                ..
            } => {
                out.entry(*then_block).or_default().push(*id);
                out.entry(*else_block).or_default().push(*id);
            }
            Terminator::Return(_) | Terminator::Unreachable => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        BasicBlock, BlockId, Function, Instruction, Module, Terminator, ValueId, build, verify,
    };
    use waluau_ast::{BinaryOp, NumberLiteral, NumericType, Type};
    use waluau_diagnostics::DiagnosticCategory;
    use waluau_parser::parse;

    #[test]
    fn inserts_phi_after_if_merge() {
        let source = r#"
            function entry(flag: bool, x: i32): i32
                local y: i32 = x
                if flag then
                    y = y + 1
                else
                    y = y + 2
                end
                return y
            end
        "#;
        let program = parse(source).expect("parse should succeed");
        let module = build(&program).expect("ir build should succeed");
        let function = &module.functions[0];
        let has_merge_phi = function.blocks.values().any(|block| {
            block
                .instructions
                .iter()
                .any(|(_, instruction)| matches!(instruction, Instruction::Phi(incoming) if incoming.len() == 2))
        });
        assert!(
            has_merge_phi,
            "expected merge phi in function:\n{}",
            function.dump()
        );
    }

    #[test]
    fn lowers_if_expression_with_phi_result() {
        let source = r#"
            function entry(flag: bool, x: i32, y: i32): i32
                return if flag then x + 1 else y + 2
            end
        "#;
        let program = parse(source).expect("parse should succeed");
        let module = build(&program).expect("ir build should succeed");
        let function = &module.functions[0];
        let has_branch_phi = function.blocks.values().any(|block| {
            block
                .instructions
                .iter()
                .any(|(_, instruction)| matches!(instruction, Instruction::Phi(incoming) if incoming.len() == 2))
        });
        assert!(
            has_branch_phi,
            "expected branch phi in function:\n{}",
            function.dump()
        );
    }

    #[test]
    fn inserts_phi_for_loop_carried_variable() {
        let source = r#"
            function entry(limit: i32): i32
                local i: i32 = 0
                while i < limit do
                    i = i + 1
                end
                return i
            end
        "#;
        let program = parse(source).expect("parse should succeed");
        let module = build(&program).expect("ir build should succeed");
        let function = &module.functions[0];
        let loop_phi = function.blocks.values().find_map(|block| {
            block.instructions.iter().find_map(|(_, instruction)| {
                if let Instruction::Phi(incoming) = instruction {
                    Some(incoming.len())
                } else {
                    None
                }
            })
        });
        assert_eq!(
            loop_phi,
            Some(2),
            "expected loop phi with two incoming edges"
        );
    }

    #[test]
    fn lowers_repeat_until_with_post_test_condition() {
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
        let module = build(&program).expect("ir build should succeed");
        let function = &module.functions[0];
        let repeat_branch = function.blocks.values().find(|block| {
            matches!(
                block.terminator,
                Terminator::Branch {
                    then_block,
                    else_block,
                    ..
                } if then_block != else_block
            )
        });
        assert!(
            repeat_branch.is_some(),
            "expected repeat-until branch terminator"
        );
        let loop_phi = function.blocks.values().find_map(|block| {
            block.instructions.iter().find_map(|(_, instruction)| {
                if let Instruction::Phi(incoming) = instruction {
                    Some(incoming.len())
                } else {
                    None
                }
            })
        });
        assert_eq!(
            loop_phi,
            Some(2),
            "expected repeat-until phi with two incoming edges"
        );
    }

    #[test]
    fn emits_branches_and_returns() {
        let source = r#"
            function entry(flag: bool, x: i32): i32
                if flag then
                    return x
                end
                return x + 1
            end
        "#;
        let program = parse(source).expect("parse should succeed");
        let module = build(&program).expect("ir build should succeed");
        let function = &module.functions[0];
        let branch_count = function
            .blocks
            .values()
            .filter(|block| matches!(block.terminator, Terminator::Branch { .. }))
            .count();
        let return_count = function
            .blocks
            .values()
            .filter(|block| matches!(block.terminator, Terminator::Return(_)))
            .count();
        assert_eq!(branch_count, 1);
        assert_eq!(return_count, 2);
    }

    #[test]
    fn records_numeric_scalar_kinds_in_instructions() {
        let source = r#"
            function entry(x: i64, y: u64, z: f64): f64
                local a: i64 = x + 1
                local b: u64 = y + 2
                local c: f64 = z + 3
                return c
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        let module = build(&program).expect("ir build should succeed");
        let function = &module.functions[0];
        assert!(function.blocks.values().any(|block| {
            block.instructions.iter().any(|(_, instruction)| {
                matches!(
                    instruction,
                    Instruction::Number {
                        ty: NumericType::I64,
                        literal,
                    } if literal.raw == "1"
                )
            })
        }));
        assert!(function.blocks.values().any(|block| {
            block.instructions.iter().any(|(_, instruction)| {
                matches!(
                    instruction,
                    Instruction::Number {
                        ty: NumericType::U64,
                        literal,
                    } if literal.raw == "2"
                )
            })
        }));
        assert!(function.blocks.values().any(|block| {
            block.instructions.iter().any(|(_, instruction)| {
                matches!(
                    instruction,
                    Instruction::Number {
                        ty: NumericType::I64,
                        ..
                    }
                )
            })
        }));
        assert!(function.blocks.values().any(|block| {
            block.instructions.iter().any(|(_, instruction)| {
                matches!(
                    instruction,
                    Instruction::Binary {
                        operand_ty: Type::Numeric(NumericType::F64),
                        result_ty: Type::Numeric(NumericType::F64),
                        ..
                    }
                )
            })
        }));
    }

    #[test]
    fn preserves_full_range_integer_literals_in_ir() {
        let source = r#"
            function entry(): u64
                return 18446744073709551615
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        let module = build(&program).expect("ir build should succeed");
        let function = &module.functions[0];
        assert!(function.blocks.values().any(|block| {
            block.instructions.iter().any(|(_, instruction)| {
                matches!(
                    instruction,
                    Instruction::Number {
                        ty: NumericType::U64,
                        literal,
                    } if literal.raw == "18446744073709551615"
                )
            })
        }));
    }

    #[test]
    fn lowers_compound_index_assignment_with_single_target_evaluation() {
        let source = r#"
            function idx(): i32
                return 0
            end

            function entry(xs: {i32}): i32
                xs[idx()] += 5
                return xs[0]
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        let module = build(&program).expect("ir build should succeed");
        let function = module
            .functions
            .iter()
            .find(|function| function.name == "entry")
            .expect("entry function should exist");
        let call_count = function
            .blocks
            .values()
            .flat_map(|block| {
                block
                    .instructions
                    .iter()
                    .map(|(_, instruction)| instruction)
            })
            .filter(|instruction| {
                matches!(
                    instruction,
                    Instruction::Call { name, .. } if name == "idx"
                )
            })
            .count();
        assert_eq!(call_count, 1, "expected idx() call to be evaluated once");
    }

    #[test]
    fn lowers_and_expression_with_short_circuit_cfg() {
        let source = r#"
            function entry(a: bool, b: bool): bool
                return a and b
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        let module = build(&program).expect("ir build should succeed");
        let function = &module.functions[0];

        assert!(
            !function.blocks.values().any(|block| {
                block.instructions.iter().any(|(_, instruction)| {
                    matches!(
                        instruction,
                        Instruction::Binary {
                            op: BinaryOp::And,
                            ..
                        }
                    )
                })
            }),
            "expected 'and' to lower to control-flow, not a binary instruction:\n{}",
            function.dump()
        );

        let branch_count = function
            .blocks
            .values()
            .filter(|block| matches!(block.terminator, Terminator::Branch { .. }))
            .count();
        assert!(
            branch_count >= 1,
            "expected at least one branch for short-circuit 'and', got {} in function:\n{}",
            branch_count,
            function.dump()
        );

        let phi_count = function
            .blocks
            .values()
            .flat_map(|block| block.instructions.iter())
            .filter(|(_, instruction)| matches!(instruction, Instruction::Phi(incoming) if incoming.len() == 2))
            .count();
        assert!(
            phi_count >= 1,
            "expected a phi node merging 'and' results, got {} in function:\n{}",
            phi_count,
            function.dump()
        );
    }

    #[test]
    fn lowers_or_expression_with_short_circuit_cfg() {
        let source = r#"
            function entry(a: bool, b: bool): bool
                return a or b
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        let module = build(&program).expect("ir build should succeed");
        let function = &module.functions[0];

        assert!(
            !function.blocks.values().any(|block| {
                block.instructions.iter().any(|(_, instruction)| {
                    matches!(
                        instruction,
                        Instruction::Binary {
                            op: BinaryOp::Or,
                            ..
                        }
                    )
                })
            }),
            "expected 'or' to lower to control-flow, not a binary instruction:\n{}",
            function.dump()
        );

        let branch_count = function
            .blocks
            .values()
            .filter(|block| matches!(block.terminator, Terminator::Branch { .. }))
            .count();
        assert!(
            branch_count >= 1,
            "expected at least one branch for short-circuit 'or', got {} in function:\n{}",
            branch_count,
            function.dump()
        );

        let phi_count = function
            .blocks
            .values()
            .flat_map(|block| block.instructions.iter())
            .filter(|(_, instruction)| matches!(instruction, Instruction::Phi(incoming) if incoming.len() == 2))
            .count();
        assert!(
            phi_count >= 1,
            "expected a phi node merging 'or' results, got {} in function:\n{}",
            phi_count,
            function.dump()
        );
    }

    #[test]
    fn inserts_casts_for_implicit_and_explicit_conversions() {
        let source = r#"
            function entry(x: i32, y: i64): i32
                local widened: i64 = x
                local sum: i64 = widened + y
                return sum :: i32
            end
        "#;

        let program = parse(source).expect("parse should succeed");
        let module = build(&program).expect("ir build should succeed");
        let function = &module.functions[0];
        let casts = function
            .blocks
            .values()
            .flat_map(|block| {
                block
                    .instructions
                    .iter()
                    .map(|(_, instruction)| instruction)
            })
            .filter(|instruction| matches!(instruction, Instruction::Cast { .. }))
            .count();
        assert_eq!(casts, 2, "expected implicit widen and explicit narrow cast");
    }

    #[test]
    fn rejects_non_bool_branch_condition() {
        let function = Function {
            name: "entry".into(),
            params: vec![],
            return_type: Type::Numeric(NumericType::I64),
            entry: BlockId(0),
            next_value: 2,
            blocks: BTreeMap::from([
                (
                    BlockId(0),
                    BasicBlock {
                        id: BlockId(0),
                        instructions: vec![(
                            ValueId(0),
                            Instruction::Number {
                                ty: NumericType::I64,
                                literal: NumberLiteral { raw: "1".into() },
                            },
                        )],
                        terminator: Terminator::Branch {
                            condition: ValueId(0),
                            then_block: BlockId(1),
                            else_block: BlockId(1),
                        },
                    },
                ),
                (
                    BlockId(1),
                    BasicBlock {
                        id: BlockId(1),
                        instructions: vec![(
                            ValueId(1),
                            Instruction::Number {
                                ty: NumericType::I64,
                                literal: NumberLiteral { raw: "0".into() },
                            },
                        )],
                        terminator: Terminator::Return(ValueId(1)),
                    },
                ),
            ]),
        };
        let err = verify(&Module {
            functions: vec![function],
            start: None,
        })
        .expect_err("expected verifier to reject non-bool branch");
        assert!(err.to_string().contains("branch condition"));
    }

    #[test]
    fn rejects_return_type_mismatch() {
        let function = Function {
            name: "entry".into(),
            params: vec![],
            return_type: Type::Bool,
            entry: BlockId(0),
            next_value: 1,
            blocks: BTreeMap::from([(
                BlockId(0),
                BasicBlock {
                    id: BlockId(0),
                    instructions: vec![(
                        ValueId(0),
                        Instruction::Number {
                            ty: NumericType::I64,
                            literal: NumberLiteral { raw: "1".into() },
                        },
                    )],
                    terminator: Terminator::Return(ValueId(0)),
                },
            )]),
        };
        let err = verify(&Module {
            functions: vec![function],
            start: None,
        })
        .expect_err("expected verifier to reject return type mismatch");
        assert!(err.to_string().contains("return in block"));
    }

    #[test]
    fn lowers_array_literals_indexing_length_and_mutation() {
        let source = r#"
            function score_count(): i32
                local scores: {number} = {100, 250, 300}
                local first: number = scores[0]
                scores[1] = first + 1
                return #scores
            end
        "#;
        let program = parse(source).expect("parse should succeed");
        let module = build(&program).expect("ir build should succeed");
        let function = &module.functions[0];
        assert!(function.blocks.values().any(|block| {
            block
                .instructions
                .iter()
                .any(|(_, instruction)| matches!(instruction, Instruction::ArrayNew { .. }))
        }));
        assert!(function.blocks.values().any(|block| {
            block
                .instructions
                .iter()
                .any(|(_, instruction)| matches!(instruction, Instruction::ArrayGet { .. }))
        }));
        assert!(function.blocks.values().any(|block| {
            block
                .instructions
                .iter()
                .any(|(_, instruction)| matches!(instruction, Instruction::ArraySet { .. }))
        }));
        assert!(function.blocks.values().any(|block| {
            block
                .instructions
                .iter()
                .any(|(_, instruction)| matches!(instruction, Instruction::ArrayLen { .. }))
        }));
    }

    #[test]
    fn rejects_phi_predecessor_order_mismatch() {
        let function = Function {
            name: "entry".into(),
            params: vec![],
            return_type: Type::Numeric(NumericType::I64),
            entry: BlockId(0),
            next_value: 5,
            blocks: BTreeMap::from([
                (
                    BlockId(0),
                    BasicBlock {
                        id: BlockId(0),
                        instructions: vec![(ValueId(0), Instruction::Bool(true))],
                        terminator: Terminator::Branch {
                            condition: ValueId(0),
                            then_block: BlockId(1),
                            else_block: BlockId(2),
                        },
                    },
                ),
                (
                    BlockId(1),
                    BasicBlock {
                        id: BlockId(1),
                        instructions: vec![(
                            ValueId(1),
                            Instruction::Number {
                                ty: NumericType::I64,
                                literal: NumberLiteral { raw: "1".into() },
                            },
                        )],
                        terminator: Terminator::Jump(BlockId(3)),
                    },
                ),
                (
                    BlockId(2),
                    BasicBlock {
                        id: BlockId(2),
                        instructions: vec![(
                            ValueId(2),
                            Instruction::Number {
                                ty: NumericType::I64,
                                literal: NumberLiteral { raw: "2".into() },
                            },
                        )],
                        terminator: Terminator::Jump(BlockId(3)),
                    },
                ),
                (
                    BlockId(3),
                    BasicBlock {
                        id: BlockId(3),
                        instructions: vec![(
                            ValueId(3),
                            Instruction::Phi(vec![
                                (BlockId(2), ValueId(2)),
                                (BlockId(1), ValueId(1)),
                            ]),
                        )],
                        terminator: Terminator::Return(ValueId(3)),
                    },
                ),
            ]),
        };
        let err = verify(&Module {
            functions: vec![function],
            start: None,
        })
        .expect_err("expected verifier to reject phi predecessor ordering");
        assert!(err.to_string().contains("predecessor order mismatch"));
    }

    #[test]
    fn lowers_function_expression_with_capture_and_indirect_call() {
        let source = r#"
            function entry(x: i32): i32
                local addx: (i32) -> i32 = function(y: i32): i32
                    return x + y
                end
                return addx(7)
            end
        "#;
        let program = parse(source).expect("parse should succeed");
        let module = build(&program).expect("ir build should succeed");
        assert!(
            module
                .functions
                .iter()
                .any(|function| function.name == "entry$lambda0"),
            "expected lifted lambda function in module"
        );
        let entry = module
            .functions
            .iter()
            .find(|function| function.name == "entry")
            .expect("entry function should exist");
        assert!(entry.blocks.values().any(|block| {
            block
                .instructions
                .iter()
                .any(|(_, instruction)| matches!(instruction, Instruction::Closure { .. }))
        }));
        assert!(entry.blocks.values().any(|block| {
            block
                .instructions
                .iter()
                .any(|(_, instruction)| matches!(instruction, Instruction::CallValue { .. }))
        }));
    }

    #[test]
    fn lowers_named_function_expression_recursion() {
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
        let module = build(&program).expect("ir build should succeed");
        let lifted = module
            .functions
            .iter()
            .find(|function| function.name == "entry$lambda0")
            .expect("expected lifted recursive function");
        assert!(lifted.blocks.values().any(|block| {
            block
                .instructions
                .iter()
                .any(|(_, instruction)| matches!(instruction, Instruction::CallValue { .. }))
        }));
    }

    #[test]
    fn verifies_loop_with_break_and_continue() {
        let source = r#"
            function entry(xs: {i32}, len: i32): i32
                local i: i32 = 0
                local acc: i32 = 0
                while i < len do
                    local x: i32 = xs[i]
                    if x < 0 then
                        i += 1
                        continue
                    end
                    acc += x
                    if acc > 1000 then
                        break
                    end
                    i += 1
                end
                return acc
            end
        "#;
        let program = parse(source).expect("parse should succeed");

        let signatures: std::collections::HashMap<_, (Vec<waluau_ast::Type>, waluau_ast::Type)> =
            program
                .functions
                .iter()
                .map(|function| {
                    let return_type = function.return_type.clone().ok_or_else(|| {
                        waluau_diagnostics::Diagnostic::new(format!(
                            "function '{}' must have a concrete return type before IR lowering",
                            function.name
                        ))
                    })?;
                    Ok((
                        function.name.clone(),
                        (
                            function
                                .params
                                .iter()
                                .map(|param| param.ty.clone())
                                .collect(),
                            return_type,
                        ),
                    ))
                })
                .collect::<Result<_, waluau_diagnostics::Diagnostic>>()
                .expect("signatures should build");

        let mut lowered = super::build_function(&program.functions[0], &signatures)
            .expect("ir lowering should succeed");
        let mut functions = Vec::new();
        functions.push(lowered.remove(0));
        functions.extend(lowered);
        let module = super::Module {
            functions,
            start: None,
        };

        let function = &module.functions[0];
        if let Err(err) = super::verify(&module) {
            panic!("verify failed: {err}\n{}", function.dump());
        }
    }

    #[test]
    fn lowers_string_value_to_ir() {
        let source = r#"
            function entry(): string
                return "hello"
            end
        "#;
        let program = parse(source).expect("parse should succeed");
        let module = build(&program).expect("ir build should succeed for string values");
        let function = &module.functions[0];
        assert_eq!(function.return_type, Type::String);
        let block = function.blocks.get(&function.entry).expect("entry block");
        let (_, instruction) = &block.instructions[0];
        assert!(
            matches!(instruction, Instruction::String(s) if s == "hello"),
            "expected String instruction with 'hello', got {:?}",
            instruction
        );
    }

    #[test]
    fn monomorphizes_generic_calls_once_per_type_arguments() {
        let source = r#"
            function identity<T>(value: T): T
                return value
            end

            function forward<T>(value: T): T
                return identity<T>(value)
            end

            function main(): i32
                local a: i32 = forward<i32>(41)
                local b: i32 = forward<i32>(1)
                return a + b
            end
        "#;
        let program = parse(source).expect("parse should succeed");
        let module = build(&program).expect("ir build should succeed");

        let forward_specialization_name = module
            .functions
            .iter()
            .find(|function| function.name.starts_with("__waluau_generic$forward"))
            .map(|function| function.name.clone())
            .expect("forward specialization should exist");
        let identity_specialization_name = module
            .functions
            .iter()
            .find(|function| function.name.starts_with("__waluau_generic$identity"))
            .map(|function| function.name.clone())
            .expect("identity specialization should exist");
        assert_eq!(
            module
                .functions
                .iter()
                .filter(|function| function.name == forward_specialization_name)
                .count(),
            1
        );
        assert_eq!(
            module
                .functions
                .iter()
                .filter(|function| function.name == identity_specialization_name)
                .count(),
            1
        );

        let main = module
            .functions
            .iter()
            .find(|function| function.name == "main")
            .expect("main function should exist");
        let forward_calls = main
            .blocks
            .values()
            .flat_map(|block| {
                block
                    .instructions
                    .iter()
                    .map(|(_, instruction)| instruction)
            })
            .filter(|instruction| {
                matches!(
                    instruction,
                    Instruction::Call { name, .. } if name == &forward_specialization_name
                )
            })
            .count();
        assert_eq!(
            forward_calls, 2,
            "expected both calls to reuse one specialization"
        );
    }

    #[test]
    fn rejects_cross_specialization_recursive_generics() {
        let source = r#"
            function loop<T>(value: T): {T}
                return loop<{T}>({value})
            end

            function main(): i32
                local xs: {i32} = loop<i32>(1)
                return xs[0]
            end
        "#;
        let program = parse(source).expect("parse should succeed");
        let error = build(&program).expect_err("cross-specialization recursion should fail");
        assert_eq!(error.code(), Some("generic/cross-specialization-recursion"));
    }

    #[test]
    fn tags_ir_inference_failures_with_structured_diagnostics() {
        let source = r#"
            function entry(): i32
                local xs = {}
                return 0
            end
        "#;
        let program = parse(source).expect("parse should succeed");
        let error = build(&program).expect_err("ir build should fail");
        assert_eq!(error.code(), Some("inference/missing-context"));
        assert_eq!(error.category(), Some(DiagnosticCategory::MissingContext));
        assert_eq!(
            error.action(),
            Some("add an explicit element type annotation, e.g. local xs: {i32} = {}")
        );
        assert_eq!(error.span(), None);
    }
}
