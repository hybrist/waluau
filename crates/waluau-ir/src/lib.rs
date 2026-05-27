use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use waluau_ast::{
    BinaryOp, Expr, Function as AstFunction, NumberLiteral, NumericType, Program, Stmt, Type,
    UnaryOp,
};
use waluau_diagnostics::Diagnostic;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BlockId(pub usize);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ValueId(pub usize);

#[derive(Clone, Debug, PartialEq)]
pub struct Module {
    pub functions: Vec<Function>,
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
    Call {
        name: String,
        args: Vec<ValueId>,
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
    Phi(Vec<(BlockId, ValueId)>),
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
    let functions = program
        .functions
        .iter()
        .map(|function| build_function(function, &signatures))
        .collect::<Result<Vec<_>, _>>()?;
    let module = Module { functions };
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
                    if array_ty != expected_array_ty {
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
                    if array_ty != expected_array_ty {
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
                Instruction::Param(_) | Instruction::Number { .. } | Instruction::Bool(_) => {}
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
        Instruction::Cast { to, .. } => Ok(to.clone()),
        Instruction::Binary { result_ty, .. } => Ok(result_ty.clone()),
        Instruction::Call { name, .. } => signatures
            .get(name)
            .map(|(_, ret)| ret.clone())
            .ok_or_else(|| Diagnostic::new(format!("unknown function '{}'", name))),
        Instruction::ArrayNew { element_ty, .. } => Ok(Type::Array(Box::new(element_ty.clone()))),
        Instruction::ArrayGet { element_ty, .. } => Ok(element_ty.clone()),
        Instruction::ArraySet { .. } => Ok(Type::Numeric(NumericType::I32)),
        Instruction::ArrayLen { .. } => Ok(Type::Numeric(NumericType::I32)),
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
                let mut incoming_ty = None;
                let mut complete = true;
                for (_, incoming_value) in incoming {
                    let ty = definitions
                        .get(incoming_value)
                        .and_then(|def| def.ty.as_ref());
                    let Some(ty) = ty else {
                        complete = false;
                        break;
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
                if complete {
                    let resolved = incoming_ty.ok_or_else(|| {
                        Diagnostic::new(format!(
                            "phi in block {:?} has no typed incoming values",
                            block.id
                        ))
                    })?;
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
) -> Result<Function, Diagnostic> {
    let mut out = Function {
        name: function.name.clone(),
        params: function
            .params
            .iter()
            .map(|param| (param.name.clone(), param.ty.clone()))
            .collect(),
        return_type: function.return_type.clone(),
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
    for (index, (name, ty)) in out.params.clone().into_iter().enumerate() {
        let value = out.next_value();
        block_mut(&mut out, entry)
            .instructions
            .push((value, Instruction::Param(index)));
        env.insert(name, value);
        type_env.insert(out.params[index].0.clone(), ty);
    }

    let mut builder = Builder {
        function: out,
        current_block: BlockId(0),
        next_block: 1,
        signatures,
    };
    for stmt in &function.body {
        if builder.current_block == DEAD_BLOCK {
            break;
        }
        builder.lower_stmt(stmt, &mut env, &mut type_env)?;
    }
    Ok(builder.function)
}

const DEAD_BLOCK: BlockId = BlockId(usize::MAX);

struct Builder<'a> {
    function: Function,
    current_block: BlockId,
    next_block: usize,
    signatures: &'a HashMap<String, (Vec<Type>, Type)>,
}

impl Builder<'_> {
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

    fn lower_stmt(
        &mut self,
        stmt: &Stmt,
        env: &mut HashMap<String, ValueId>,
        types: &mut HashMap<String, Type>,
    ) -> Result<(), Diagnostic> {
        match stmt {
            Stmt::Let { name, ty, value } => {
                let value = self.lower_expr(value, env, types, Some(ty.clone()))?;
                env.insert(name.clone(), value);
                types.insert(name.clone(), ty.clone());
            }
            Stmt::Assign { name, value } => {
                let ty = types.get(name).cloned().ok_or_else(|| {
                    Diagnostic::new(format!("unknown local '{name}' during IR lowering"))
                })?;
                let value = self.lower_expr(value, env, types, Some(ty))?;
                env.insert(name.clone(), value);
            }
            Stmt::IndexAssign { base, index, value } => {
                let base_ty = self.infer_expr_type(base, types, None)?;
                let element_ty = base_ty.element_type().ok_or_else(|| {
                    Diagnostic::new("array element assignment requires an array operand")
                })?;
                let array = self.lower_expr(base, env, types, Some(base_ty))?;
                let index =
                    self.lower_expr(index, env, types, Some(Type::Numeric(NumericType::I32)))?;
                let value = self.lower_expr(value, env, types, Some(element_ty.clone()))?;
                self.emit(Instruction::ArraySet {
                    array,
                    index,
                    value,
                    element_ty,
                });
            }
            Stmt::Expr(expr) => {
                let _ = self.lower_expr(expr, env, types, None)?;
            }
            Stmt::Return(expr) => {
                let value =
                    self.lower_expr(expr, env, types, Some(self.function.return_type.clone()))?;
                self.set_terminator(self.current_block, Terminator::Return(value));
                self.current_block = DEAD_BLOCK;
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

        let mut body_env = loop_env.clone();
        let mut body_types = loop_types.clone();
        for stmt in body {
            if self.current_block == DEAD_BLOCK {
                break;
            }
            self.lower_stmt(stmt, &mut body_env, &mut body_types)?;
        }
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
                    Type::Array(_) => {
                        return Err(Diagnostic::new(
                            "numeric literal is not assignable to array",
                        ));
                    }
                };
                self.emit(Instruction::Number {
                    ty,
                    literal: number.clone(),
                })
            }
            Expr::Bool(value) => self.emit(Instruction::Bool(*value)),
            Expr::Name(name) => {
                let value = *env.get(name).ok_or_else(|| {
                    Diagnostic::new(format!("unknown local '{name}' during IR lowering"))
                })?;
                let actual = types.get(name).cloned().ok_or_else(|| {
                    Diagnostic::new(format!("unknown local '{name}' during IR lowering"))
                })?;
                self.coerce_value(value, actual, expected)?
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
                            Type::Array(_) => {
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
            Expr::Binary { op, left, right } => {
                let operand_ty = self.infer_binary_operand_type(left, right, op, types, None)?;
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
            Expr::Call { name, args } => {
                let (param_types, _) = self.signatures.get(name).ok_or_else(|| {
                    Diagnostic::new(format!("unknown function '{name}' during IR lowering"))
                })?;
                let args = args
                    .iter()
                    .zip(param_types.iter())
                    .map(|(arg, param_ty)| self.lower_expr(arg, env, types, Some(param_ty.clone())))
                    .collect::<Result<Vec<_>, _>>()?;
                let value = self.emit(Instruction::Call {
                    name: name.clone(),
                    args,
                });
                let actual = self.infer_expr_type(expr, types, None)?;
                self.coerce_value(value, actual, expected)?
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
        };
        Ok(value)
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
                Some(Type::Array(_)) => Err(Diagnostic::new(
                    "numeric literal is not assignable to array",
                )),
                None => Ok(Type::number()),
            },
            Expr::Bool(_) => Ok(Type::Bool),
            Expr::Name(name) => types.get(name).cloned().ok_or_else(|| {
                Diagnostic::new(format!("unknown local '{name}' during IR lowering"))
            }),
            Expr::Unary { op, expr } => match op {
                UnaryOp::Neg => {
                    let actual = self.infer_expr_type(expr, types, expected.clone())?;
                    match actual {
                        Type::Numeric(_) => coerce_type(actual, expected),
                        Type::Bool => Err(Diagnostic::new("unary '-' requires a numeric operand")),
                        Type::Array(_) => {
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
            Expr::Call { name, .. } => self
                .signatures
                .get(name)
                .map(|(_, ret)| ret.clone())
                .ok_or_else(|| {
                    Diagnostic::new(format!("unknown function '{name}' during IR lowering"))
                }),
            Expr::ArrayLiteral { elements } => {
                self.infer_array_literal_type(elements, types, expected)
            }
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
                BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div => {
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
            BinaryOp::Add
            | BinaryOp::Sub
            | BinaryOp::Mul
            | BinaryOp::Div
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
            return Err(Diagnostic::new(
                "empty array literal requires explicit element type",
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
                Err(Diagnostic::new(
                    "could not resolve operand type during IR lowering",
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
        (Type::Numeric(left), Type::Numeric(right)) => left
            .common(right)
            .map(Type::Numeric)
            .ok_or_else(|| Diagnostic::new("could not resolve operand type during IR lowering")),
        _ => Err(Diagnostic::new(
            "could not resolve operand type during IR lowering",
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
            Type::Array(_) => Err(Diagnostic::new(format!(
                "cannot implicitly convert array to {expected_numeric}",
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

fn collect_assigned_into(stmts: &[Stmt], out: &mut BTreeSet<String>) {
    for stmt in stmts {
        match stmt {
            Stmt::Let { name, .. } | Stmt::Assign { name, .. } => {
                out.insert(name.clone());
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
            Stmt::Return(_) | Stmt::Expr(_) => {}
        }
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
    use waluau_ast::{NumberLiteral, NumericType, Type};
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
        })
        .expect_err("expected verifier to reject phi predecessor ordering");
        assert!(err.to_string().contains("predecessor order mismatch"));
    }
}
