use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use waluau_ast::{BinaryOp, Expr, Function as AstFunction, Program, Stmt, Type};
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
    Number(f64),
    Bool(bool),
    Binary {
        op: BinaryOp,
        left: ValueId,
        right: ValueId,
    },
    Call {
        name: String,
        args: Vec<ValueId>,
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
    let functions = program
        .functions
        .iter()
        .map(build_function)
        .collect::<Result<Vec<_>, _>>()?;
    let module = Module { functions };
    verify(&module)?;
    Ok(module)
}

pub fn verify(module: &Module) -> Result<(), Diagnostic> {
    for function in &module.functions {
        verify_function(function)?;
    }
    Ok(())
}

fn verify_function(function: &Function) -> Result<(), Diagnostic> {
    let predecessors = predecessors(function);
    let mut defined = HashSet::new();
    for block in function.blocks.values() {
        for (value, _) in &block.instructions {
            if !defined.insert(*value) {
                return Err(Diagnostic::new(format!("duplicate value id {:?}", value)));
            }
        }
    }

    for block in function.blocks.values() {
        for (_value, instruction) in &block.instructions {
            match instruction {
                Instruction::Binary { left, right, .. } => {
                    require_defined(defined.contains(left), left)?;
                    require_defined(defined.contains(right), right)?;
                }
                Instruction::Call { args, .. } => {
                    for arg in args {
                        require_defined(defined.contains(arg), arg)?;
                    }
                }
                Instruction::Phi(incoming) => {
                    let pred_count = predecessors.get(&block.id).map_or(0, Vec::len);
                    if incoming.len() > pred_count {
                        return Err(Diagnostic::new(format!(
                            "phi in block {:?} has too many incoming values",
                            block.id
                        )));
                    }
                    for (pred, value) in incoming {
                        if !predecessors
                            .get(&block.id)
                            .is_some_and(|preds| preds.contains(pred))
                        {
                            return Err(Diagnostic::new(format!(
                                "phi in block {:?} references non-predecessor {:?}",
                                block.id, pred
                            )));
                        }
                        require_defined(defined.contains(value), value)?;
                    }
                }
                Instruction::Param(_) | Instruction::Number(_) | Instruction::Bool(_) => {}
            }
        }

        match &block.terminator {
            Terminator::Jump(target) => require_block(function, *target)?,
            Terminator::Branch {
                condition,
                then_block,
                else_block,
            } => {
                require_defined(defined.contains(condition), condition)?;
                require_block(function, *then_block)?;
                require_block(function, *else_block)?;
            }
            Terminator::Return(value) => require_defined(defined.contains(value), value)?,
            Terminator::Unreachable => {}
        }
    }

    Ok(())
}

fn require_defined(ok: bool, value: &ValueId) -> Result<(), Diagnostic> {
    if ok {
        Ok(())
    } else {
        Err(Diagnostic::new(format!(
            "use of undefined value {:?}",
            value
        )))
    }
}

fn require_block(function: &Function, block: BlockId) -> Result<(), Diagnostic> {
    if function.blocks.contains_key(&block) {
        Ok(())
    } else {
        Err(Diagnostic::new(format!("unknown block {:?}", block)))
    }
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

fn build_function(function: &AstFunction) -> Result<Function, Diagnostic> {
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
    let entry = out.entry;
    for (index, (name, _)) in out.params.clone().into_iter().enumerate() {
        let value = out.next_value();
        block_mut(&mut out, entry)
            .instructions
            .push((value, Instruction::Param(index)));
        env.insert(name, value);
    }

    let mut builder = Builder {
        function: out,
        current_block: BlockId(0),
        next_block: 1,
    };
    for stmt in &function.body {
        if builder.current_block == DEAD_BLOCK {
            break;
        }
        builder.lower_stmt(stmt, &mut env)?;
    }
    Ok(builder.function)
}

const DEAD_BLOCK: BlockId = BlockId(usize::MAX);

struct Builder {
    function: Function,
    current_block: BlockId,
    next_block: usize,
}

impl Builder {
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
    ) -> Result<(), Diagnostic> {
        match stmt {
            Stmt::Let { name, value, .. } | Stmt::Assign { name, value } => {
                let value = self.lower_expr(value, env)?;
                env.insert(name.clone(), value);
            }
            Stmt::Expr(expr) => {
                let _ = self.lower_expr(expr, env)?;
            }
            Stmt::Return(expr) => {
                let value = self.lower_expr(expr, env)?;
                self.set_terminator(self.current_block, Terminator::Return(value));
                self.current_block = DEAD_BLOCK;
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                self.lower_if(condition, then_body, else_body, env)?;
            }
            Stmt::While { condition, body } => {
                self.lower_while(condition, body, env)?;
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
    ) -> Result<(), Diagnostic> {
        let condition = self.lower_expr(condition, env)?;
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
        self.current_block = then_block;
        for stmt in then_body {
            if self.current_block == DEAD_BLOCK {
                break;
            }
            self.lower_stmt(stmt, &mut then_env)?;
        }
        let then_exit = self.current_block;
        if then_exit != DEAD_BLOCK {
            self.set_terminator(then_exit, Terminator::Jump(merge_block));
        }

        let mut else_env = env.clone();
        self.current_block = else_block;
        for stmt in else_body {
            if self.current_block == DEAD_BLOCK {
                break;
            }
            self.lower_stmt(stmt, &mut else_env)?;
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
    ) -> Result<(), Diagnostic> {
        let preheader = self.current_block;
        let header = self.new_block();
        let loop_body = self.new_block();
        let exit = self.new_block();
        self.set_terminator(preheader, Terminator::Jump(header));

        let mutated = collect_assigned_names(body);
        self.current_block = header;
        let mut loop_env = env.clone();
        let mut phis = HashMap::new();
        for name in &mutated {
            if let Some(initial) = env.get(name).copied() {
                let phi = self.emit(Instruction::Phi(vec![(preheader, initial)]));
                loop_env.insert(name.clone(), phi);
                phis.insert(name.clone(), phi);
            }
        }

        let cond_value = self.lower_expr(condition, &loop_env)?;
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
        for stmt in body {
            if self.current_block == DEAD_BLOCK {
                break;
            }
            self.lower_stmt(stmt, &mut body_env)?;
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

    fn lower_expr(
        &mut self,
        expr: &Expr,
        env: &HashMap<String, ValueId>,
    ) -> Result<ValueId, Diagnostic> {
        let value = match expr {
            Expr::Number(number) => self.emit(Instruction::Number(*number)),
            Expr::Bool(value) => self.emit(Instruction::Bool(*value)),
            Expr::Name(name) => *env.get(name).ok_or_else(|| {
                Diagnostic::new(format!("unknown local '{name}' during IR lowering"))
            })?,
            Expr::Binary { op, left, right } => {
                let left = self.lower_expr(left, env)?;
                let right = self.lower_expr(right, env)?;
                self.emit(Instruction::Binary {
                    op: *op,
                    left,
                    right,
                })
            }
            Expr::Call { name, args } => {
                let args = args
                    .iter()
                    .map(|arg| self.lower_expr(arg, env))
                    .collect::<Result<Vec<_>, _>>()?;
                self.emit(Instruction::Call {
                    name: name.clone(),
                    args,
                })
            }
        };
        Ok(value)
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
            Stmt::If {
                then_body,
                else_body,
                ..
            } => {
                collect_assigned_into(then_body, out);
                collect_assigned_into(else_body, out);
            }
            Stmt::While { body, .. } => collect_assigned_into(body, out),
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
    use super::{Instruction, Terminator, build};
    use waluau_parser::parse;

    #[test]
    fn inserts_phi_after_if_merge() {
        let source = r#"
            fn entry(flag: bool, x: number) -> number
                let y: number = x
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
            fn entry(limit: number) -> number
                let i: number = 0
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
    fn emits_branches_and_returns() {
        let source = r#"
            fn entry(flag: bool, x: number) -> number
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
}
