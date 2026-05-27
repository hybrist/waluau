use std::collections::{BTreeMap, HashMap};

use waluau_ast::{BinaryOp, NumberLiteral, NumericType, Type};
use waluau_diagnostics::Diagnostic;
use waluau_ir::{
    BasicBlock, Function as IrFunction, Instruction as IrInstruction, Module, Terminator, ValueId,
};
use wasm_encoder::{
    BlockType, CodeSection, ExportKind, ExportSection, Function, FunctionSection, Instruction,
    Module as WasmModule, TypeSection, ValType,
};
use wasmparser::Validator;

pub fn emit(module: &Module) -> Result<Vec<u8>, Diagnostic> {
    let signatures = module
        .functions
        .iter()
        .enumerate()
        .map(|(index, function)| {
            (
                function.name.clone(),
                FunctionSignature {
                    index: index as u32,
                    result: function.return_type.clone(),
                },
            )
        })
        .collect::<HashMap<_, _>>();

    let mut wasm = WasmModule::new();
    let mut types = TypeSection::new();
    for function in &module.functions {
        let params = function
            .params
            .iter()
            .map(|(_, ty)| wasm_type(ty.clone()))
            .collect::<Result<Vec<_>, _>>()?;
        let results = [wasm_type(function.return_type.clone())?];
        types.ty().function(params, results);
    }

    let mut functions = FunctionSection::new();
    let mut exports = ExportSection::new();
    let mut codes = CodeSection::new();
    for (index, function) in module.functions.iter().enumerate() {
        functions.function(index as u32);
        exports.export(&function.name, ExportKind::Func, index as u32);
        codes.function(&emit_function(function, &signatures)?);
    }

    wasm.section(&types);
    wasm.section(&functions);
    wasm.section(&exports);
    wasm.section(&codes);

    let bytes = wasm.finish();
    Validator::new()
        .validate_all(&bytes)
        .map_err(|err| Diagnostic::new(format!("emitted invalid wasm: {err}")))?;
    Ok(bytes)
}

#[derive(Clone)]
struct FunctionSignature {
    index: u32,
    result: Type,
}

fn emit_function(
    function: &IrFunction,
    signatures: &HashMap<String, FunctionSignature>,
) -> Result<Function, Diagnostic> {
    let value_types = infer_value_types(function, signatures)?;
    let local_plan = build_local_plan(function, &value_types)?;
    let locals = compress_locals(
        local_plan
            .extra_locals
            .iter()
            .map(|(_, ty)| wasm_type(ty.clone()))
            .collect::<Result<Vec<_>, _>>()?,
    );
    let mut out = Function::new(locals);

    let pc_local = local_plan.pc_local;
    out.instruction(&Instruction::I32Const(function.entry.0 as i32));
    out.instruction(&Instruction::LocalSet(pc_local));
    out.instruction(&Instruction::Loop(BlockType::Empty));

    for block in function.blocks.values() {
        out.instruction(&Instruction::LocalGet(pc_local));
        out.instruction(&Instruction::I32Const(block.id.0 as i32));
        out.instruction(&Instruction::I32Eq);
        out.instruction(&Instruction::If(BlockType::Empty));
        emit_block(
            &mut out,
            function,
            block,
            signatures,
            &value_types,
            &local_plan,
        )?;
        out.instruction(&Instruction::End);
    }

    out.instruction(&Instruction::Unreachable);
    out.instruction(&Instruction::End);
    out.instruction(&Instruction::Unreachable);
    out.instruction(&Instruction::End);
    Ok(out)
}

struct LocalPlan {
    slots: BTreeMap<ValueId, u32>,
    extra_locals: Vec<(ValueId, Type)>,
    pc_local: u32,
}

fn build_local_plan(
    function: &IrFunction,
    value_types: &BTreeMap<ValueId, Type>,
) -> Result<LocalPlan, Diagnostic> {
    let mut slots = BTreeMap::new();
    let mut extra_locals = Vec::new();
    let mut next_local = function.params.len() as u32;

    for block in function.blocks.values() {
        for (value, instruction) in &block.instructions {
            if let IrInstruction::Param(index) = instruction {
                slots.insert(*value, *index as u32);
                continue;
            }

            let ty = value_types
                .get(value)
                .cloned()
                .ok_or_else(|| Diagnostic::new(format!("missing type for value {:?}", value)))?;
            slots.insert(*value, next_local);
            extra_locals.push((*value, ty));
            next_local += 1;
        }
    }

    let pc_local = next_local;
    extra_locals.push((ValueId(usize::MAX), Type::Numeric(NumericType::I32)));
    Ok(LocalPlan {
        slots,
        extra_locals,
        pc_local,
    })
}

fn infer_value_types(
    function: &IrFunction,
    signatures: &HashMap<String, FunctionSignature>,
) -> Result<BTreeMap<ValueId, Type>, Diagnostic> {
    let mut types = BTreeMap::new();

    for block in function.blocks.values() {
        for (value, instruction) in &block.instructions {
            let ty = match instruction {
                IrInstruction::Param(index) => function.params[*index].1.clone(),
                IrInstruction::Number { ty, .. } => Type::Numeric(*ty),
                IrInstruction::Bool(_) => Type::Bool,
                IrInstruction::Cast { to, .. } => to.clone(),
                IrInstruction::Binary { result_ty, .. } => result_ty.clone(),
                IrInstruction::Call { name, .. } => signatures
                    .get(name)
                    .ok_or_else(|| {
                        Diagnostic::new(format!("unknown function '{name}' during wasm emission"))
                    })?
                    .result
                    .clone(),
                IrInstruction::Phi(_) => continue,
            };
            types.insert(*value, ty);
        }
    }

    loop {
        let mut progress = false;
        for block in function.blocks.values() {
            for (value, instruction) in &block.instructions {
                if types.contains_key(value) {
                    continue;
                }

                let IrInstruction::Phi(incoming) = instruction else {
                    continue;
                };
                let Some((_, first)) = incoming.first() else {
                    return Err(Diagnostic::new(format!(
                        "phi {:?} has no incoming values",
                        value
                    )));
                };
                let Some(first_ty) = types.get(first).cloned() else {
                    continue;
                };
                if incoming
                    .iter()
                    .all(|(_, incoming)| types.get(incoming).cloned() == Some(first_ty.clone()))
                {
                    types.insert(*value, first_ty);
                    progress = true;
                }
            }
        }

        if !progress {
            break;
        }
    }

    for block in function.blocks.values() {
        for (value, instruction) in &block.instructions {
            if matches!(instruction, IrInstruction::Phi(_)) && !types.contains_key(value) {
                return Err(Diagnostic::new(format!(
                    "could not infer phi type for value {:?}",
                    value
                )));
            }
        }
    }

    Ok(types)
}

fn emit_block(
    out: &mut Function,
    function: &IrFunction,
    block: &BasicBlock,
    signatures: &HashMap<String, FunctionSignature>,
    value_types: &BTreeMap<ValueId, Type>,
    local_plan: &LocalPlan,
) -> Result<(), Diagnostic> {
    for (value, instruction) in &block.instructions {
        match instruction {
            IrInstruction::Param(_) | IrInstruction::Phi(_) => {}
            IrInstruction::Number { ty, literal } => {
                emit_numeric_const(out, *ty, literal)?;
                out.instruction(&Instruction::LocalSet(local(local_plan, *value)?));
            }
            IrInstruction::Bool(flag) => {
                out.instruction(&Instruction::I32Const(i32::from(*flag)));
                out.instruction(&Instruction::LocalSet(local(local_plan, *value)?));
            }
            IrInstruction::Cast {
                value: source,
                from,
                to,
            } => {
                out.instruction(&Instruction::LocalGet(local(local_plan, *source)?));
                emit_cast(out, from.clone(), to.clone())?;
                out.instruction(&Instruction::LocalSet(local(local_plan, *value)?));
            }
            IrInstruction::Binary {
                op,
                left,
                right,
                operand_ty,
                result_ty,
            } => {
                out.instruction(&Instruction::LocalGet(local(local_plan, *left)?));
                out.instruction(&Instruction::LocalGet(local(local_plan, *right)?));
                emit_binary(out, *op, operand_ty.clone(), result_ty.clone())?;
                out.instruction(&Instruction::LocalSet(local(local_plan, *value)?));
            }
            IrInstruction::Call { name, args } => {
                for arg in args {
                    out.instruction(&Instruction::LocalGet(local(local_plan, *arg)?));
                }
                let callee = signatures.get(name).ok_or_else(|| {
                    Diagnostic::new(format!("unknown function '{name}' during wasm emission"))
                })?;
                out.instruction(&Instruction::Call(callee.index));
                out.instruction(&Instruction::LocalSet(local(local_plan, *value)?));
            }
        }
    }

    match &block.terminator {
        Terminator::Jump(target) => {
            emit_phi_copies(out, function, block.id, *target, local_plan)?;
            out.instruction(&Instruction::I32Const(target.0 as i32));
            out.instruction(&Instruction::LocalSet(local_plan.pc_local));
            out.instruction(&Instruction::Br(1));
        }
        Terminator::Branch {
            condition,
            then_block,
            else_block,
        } => {
            out.instruction(&Instruction::LocalGet(local(local_plan, *condition)?));
            out.instruction(&Instruction::If(BlockType::Empty));
            emit_phi_copies(out, function, block.id, *then_block, local_plan)?;
            out.instruction(&Instruction::I32Const(then_block.0 as i32));
            out.instruction(&Instruction::LocalSet(local_plan.pc_local));
            out.instruction(&Instruction::Else);
            emit_phi_copies(out, function, block.id, *else_block, local_plan)?;
            out.instruction(&Instruction::I32Const(else_block.0 as i32));
            out.instruction(&Instruction::LocalSet(local_plan.pc_local));
            out.instruction(&Instruction::End);
            out.instruction(&Instruction::Br(1));
        }
        Terminator::Return(value) => {
            let _ = value_types.get(value).ok_or_else(|| {
                Diagnostic::new(format!("missing type for return value {:?}", value))
            })?;
            out.instruction(&Instruction::LocalGet(local(local_plan, *value)?));
            out.instruction(&Instruction::Return);
        }
        Terminator::Unreachable => {
            out.instruction(&Instruction::Unreachable);
        }
    }

    Ok(())
}

fn emit_phi_copies(
    out: &mut Function,
    function: &IrFunction,
    source: waluau_ir::BlockId,
    target: waluau_ir::BlockId,
    local_plan: &LocalPlan,
) -> Result<(), Diagnostic> {
    let target_block = function
        .blocks
        .get(&target)
        .ok_or_else(|| Diagnostic::new(format!("unknown target block {:?}", target)))?;

    let mut copies = Vec::new();
    for (value, instruction) in &target_block.instructions {
        let IrInstruction::Phi(incoming) = instruction else {
            continue;
        };
        let incoming_value = incoming
            .iter()
            .find_map(|(pred, value)| (*pred == source).then_some(*value))
            .ok_or_else(|| {
                Diagnostic::new(format!(
                    "missing phi input from {:?} to {:?}",
                    source, target
                ))
            })?;
        copies.push((
            local(local_plan, *value)?,
            local(local_plan, incoming_value)?,
        ));
    }

    for (_, src) in &copies {
        out.instruction(&Instruction::LocalGet(*src));
    }
    for (dst, _) in copies.iter().rev() {
        out.instruction(&Instruction::LocalSet(*dst));
    }
    Ok(())
}

fn emit_numeric_const(
    out: &mut Function,
    ty: NumericType,
    literal: &NumberLiteral,
) -> Result<(), Diagnostic> {
    match ty {
        NumericType::U32 => {
            out.instruction(&Instruction::I32Const(
                parse_numeric_literal::<u32>(literal, "u32")? as i32,
            ));
        }
        NumericType::I32 => {
            out.instruction(&Instruction::I32Const(parse_numeric_literal::<i32>(
                literal, "i32",
            )?));
        }
        NumericType::U64 => {
            out.instruction(&Instruction::I64Const(
                parse_numeric_literal::<u64>(literal, "u64")? as i64,
            ));
        }
        NumericType::I64 => {
            out.instruction(&Instruction::I64Const(parse_numeric_literal::<i64>(
                literal, "i64",
            )?));
        }
        NumericType::F32 => {
            out.instruction(&Instruction::F32Const(parse_numeric_literal::<f32>(
                literal, "f32",
            )?));
        }
        NumericType::F64 => {
            out.instruction(&Instruction::F64Const(parse_numeric_literal::<f64>(
                literal, "f64",
            )?));
        }
    }

    Ok(())
}

fn parse_numeric_literal<T>(literal: &NumberLiteral, ty_name: &str) -> Result<T, Diagnostic>
where
    T: std::str::FromStr,
{
    literal.raw.parse::<T>().map_err(|_| {
        Diagnostic::new(format!(
            "invalid {ty_name} numeric literal during wasm emission"
        ))
    })
}

fn emit_cast(out: &mut Function, from: Type, to: Type) -> Result<(), Diagnostic> {
    if from == to {
        return Ok(());
    }

    let (Type::Numeric(from), Type::Numeric(to)) = (from, to) else {
        return Err(Diagnostic::new(
            "only numeric casts are supported during wasm emission",
        ));
    };

    match (from, to) {
        (NumericType::U32 | NumericType::I32, NumericType::U32 | NumericType::I32)
        | (NumericType::F32, NumericType::F32)
        | (NumericType::F64, NumericType::F64)
        | (NumericType::U64 | NumericType::I64, NumericType::U64 | NumericType::I64) => {}
        (NumericType::U32, NumericType::U64) => {
            out.instruction(&Instruction::I64ExtendI32U);
        }
        (NumericType::I32, NumericType::I64) => {
            out.instruction(&Instruction::I64ExtendI32S);
        }
        (NumericType::U32, NumericType::I64) => {
            out.instruction(&Instruction::I64ExtendI32U);
        }
        (NumericType::I32, NumericType::U64) => {
            out.instruction(&Instruction::I64ExtendI32S);
        }
        (NumericType::U64 | NumericType::I64, NumericType::U32 | NumericType::I32) => {
            out.instruction(&Instruction::I32WrapI64);
        }
        (NumericType::F32, NumericType::F64) => {
            out.instruction(&Instruction::F64PromoteF32);
        }
        (NumericType::F64, NumericType::F32) => {
            out.instruction(&Instruction::F32DemoteF64);
        }
        (NumericType::U32, NumericType::F32) => {
            out.instruction(&Instruction::F32ConvertI32U);
        }
        (NumericType::I32, NumericType::F32) => {
            out.instruction(&Instruction::F32ConvertI32S);
        }
        (NumericType::U64, NumericType::F32) => {
            out.instruction(&Instruction::F32ConvertI64U);
        }
        (NumericType::I64, NumericType::F32) => {
            out.instruction(&Instruction::F32ConvertI64S);
        }
        (NumericType::U32, NumericType::F64) => {
            out.instruction(&Instruction::F64ConvertI32U);
        }
        (NumericType::I32, NumericType::F64) => {
            out.instruction(&Instruction::F64ConvertI32S);
        }
        (NumericType::U64, NumericType::F64) => {
            out.instruction(&Instruction::F64ConvertI64U);
        }
        (NumericType::I64, NumericType::F64) => {
            out.instruction(&Instruction::F64ConvertI64S);
        }
        (NumericType::F32, NumericType::U32) => {
            out.instruction(&Instruction::I32TruncF32U);
        }
        (NumericType::F32, NumericType::I32) => {
            out.instruction(&Instruction::I32TruncF32S);
        }
        (NumericType::F64, NumericType::U32) => {
            out.instruction(&Instruction::I32TruncF64U);
        }
        (NumericType::F64, NumericType::I32) => {
            out.instruction(&Instruction::I32TruncF64S);
        }
        (NumericType::F32, NumericType::U64) => {
            out.instruction(&Instruction::I64TruncF32U);
        }
        (NumericType::F32, NumericType::I64) => {
            out.instruction(&Instruction::I64TruncF32S);
        }
        (NumericType::F64, NumericType::U64) => {
            out.instruction(&Instruction::I64TruncF64U);
        }
        (NumericType::F64, NumericType::I64) => {
            out.instruction(&Instruction::I64TruncF64S);
        }
    }

    Ok(())
}

fn emit_binary(
    out: &mut Function,
    op: BinaryOp,
    operand_ty: Type,
    _result_ty: Type,
) -> Result<(), Diagnostic> {
    if matches!(operand_ty, Type::Array(_)) {
        return Err(Diagnostic::new(
            "arrays are not yet supported in code generation",
        ));
    }

    match op {
        BinaryOp::Add => match operand_ty {
            Type::Numeric(NumericType::U32 | NumericType::I32) => {
                out.instruction(&Instruction::I32Add);
            }
            Type::Numeric(NumericType::U64 | NumericType::I64) => {
                out.instruction(&Instruction::I64Add);
            }
            Type::Numeric(NumericType::F32) => {
                out.instruction(&Instruction::F32Add);
            }
            Type::Numeric(NumericType::F64) => {
                out.instruction(&Instruction::F64Add);
            }
            Type::Bool => {
                return Err(Diagnostic::new(
                    "bool add is not supported during wasm emission",
                ));
            }
            Type::Array(_) => unreachable!(),
        },
        BinaryOp::Sub => match operand_ty {
            Type::Numeric(NumericType::U32 | NumericType::I32) => {
                out.instruction(&Instruction::I32Sub);
            }
            Type::Numeric(NumericType::U64 | NumericType::I64) => {
                out.instruction(&Instruction::I64Sub);
            }
            Type::Numeric(NumericType::F32) => {
                out.instruction(&Instruction::F32Sub);
            }
            Type::Numeric(NumericType::F64) => {
                out.instruction(&Instruction::F64Sub);
            }
            Type::Bool => {
                return Err(Diagnostic::new(
                    "bool sub is not supported during wasm emission",
                ));
            }
            Type::Array(_) => unreachable!(),
        },
        BinaryOp::Mul => match operand_ty {
            Type::Numeric(NumericType::U32 | NumericType::I32) => {
                out.instruction(&Instruction::I32Mul);
            }
            Type::Numeric(NumericType::U64 | NumericType::I64) => {
                out.instruction(&Instruction::I64Mul);
            }
            Type::Numeric(NumericType::F32) => {
                out.instruction(&Instruction::F32Mul);
            }
            Type::Numeric(NumericType::F64) => {
                out.instruction(&Instruction::F64Mul);
            }
            Type::Bool => {
                return Err(Diagnostic::new(
                    "bool mul is not supported during wasm emission",
                ));
            }
            Type::Array(_) => unreachable!(),
        },
        BinaryOp::Div => match operand_ty {
            Type::Numeric(NumericType::U32) => {
                out.instruction(&Instruction::I32DivU);
            }
            Type::Numeric(NumericType::I32) => {
                out.instruction(&Instruction::I32DivS);
            }
            Type::Numeric(NumericType::U64) => {
                out.instruction(&Instruction::I64DivU);
            }
            Type::Numeric(NumericType::I64) => {
                out.instruction(&Instruction::I64DivS);
            }
            Type::Numeric(NumericType::F32) => {
                out.instruction(&Instruction::F32Div);
            }
            Type::Numeric(NumericType::F64) => {
                out.instruction(&Instruction::F64Div);
            }
            Type::Bool => {
                return Err(Diagnostic::new(
                    "bool div is not supported during wasm emission",
                ));
            }
            Type::Array(_) => unreachable!(),
        },
        BinaryOp::Eq => match operand_ty {
            Type::Numeric(NumericType::U32 | NumericType::I32) | Type::Bool => {
                out.instruction(&Instruction::I32Eq);
            }
            Type::Numeric(NumericType::U64 | NumericType::I64) => {
                out.instruction(&Instruction::I64Eq);
            }
            Type::Numeric(NumericType::F32) => {
                out.instruction(&Instruction::F32Eq);
            }
            Type::Numeric(NumericType::F64) => {
                out.instruction(&Instruction::F64Eq);
            }
            Type::Array(_) => unreachable!(),
        },
        BinaryOp::Less => match operand_ty {
            Type::Numeric(NumericType::U32) => {
                out.instruction(&Instruction::I32LtU);
            }
            Type::Numeric(NumericType::I32) => {
                out.instruction(&Instruction::I32LtS);
            }
            Type::Numeric(NumericType::U64) => {
                out.instruction(&Instruction::I64LtU);
            }
            Type::Numeric(NumericType::I64) => {
                out.instruction(&Instruction::I64LtS);
            }
            Type::Numeric(NumericType::F32) => {
                out.instruction(&Instruction::F32Lt);
            }
            Type::Numeric(NumericType::F64) => {
                out.instruction(&Instruction::F64Lt);
            }
            Type::Bool => {
                return Err(Diagnostic::new(
                    "bool comparison is not supported during wasm emission",
                ));
            }
            Type::Array(_) => unreachable!(),
        },
        BinaryOp::Greater => match operand_ty {
            Type::Numeric(NumericType::U32) => {
                out.instruction(&Instruction::I32GtU);
            }
            Type::Numeric(NumericType::I32) => {
                out.instruction(&Instruction::I32GtS);
            }
            Type::Numeric(NumericType::U64) => {
                out.instruction(&Instruction::I64GtU);
            }
            Type::Numeric(NumericType::I64) => {
                out.instruction(&Instruction::I64GtS);
            }
            Type::Numeric(NumericType::F32) => {
                out.instruction(&Instruction::F32Gt);
            }
            Type::Numeric(NumericType::F64) => {
                out.instruction(&Instruction::F64Gt);
            }
            Type::Bool => {
                return Err(Diagnostic::new(
                    "bool comparison is not supported during wasm emission",
                ));
            }
            Type::Array(_) => unreachable!(),
        },
        BinaryOp::And => {
            out.instruction(&Instruction::I32And);
        }
        BinaryOp::Or => {
            out.instruction(&Instruction::I32Or);
        }
    }
    Ok(())
}

fn local(local_plan: &LocalPlan, value: ValueId) -> Result<u32, Diagnostic> {
    local_plan
        .slots
        .get(&value)
        .copied()
        .ok_or_else(|| Diagnostic::new(format!("missing local slot for value {:?}", value)))
}

fn wasm_type(ty: Type) -> Result<ValType, Diagnostic> {
    match ty {
        Type::Bool | Type::Numeric(NumericType::U32 | NumericType::I32) => Ok(ValType::I32),
        Type::Numeric(NumericType::U64 | NumericType::I64) => Ok(ValType::I64),
        Type::Numeric(NumericType::F32) => Ok(ValType::F32),
        Type::Numeric(NumericType::F64) => Ok(ValType::F64),
        Type::Array(_) => Err(Diagnostic::new(
            "arrays are not yet supported in code generation",
        )),
    }
}

fn compress_locals(locals: Vec<ValType>) -> Vec<(u32, ValType)> {
    let mut compressed = Vec::new();
    for ty in locals {
        if let Some((count, last_ty)) = compressed.last_mut() {
            if *last_ty == ty {
                *count += 1;
                continue;
            }
        }

        compressed.push((1, ty));
    }
    compressed
}

#[cfg(test)]
mod tests {
    use wasmparser::Validator;

    #[test]
    fn emits_valid_wasm_for_scalar_program() {
        let source = r#"
            function entry(x: i32): i32
                return x + 1
            end
        "#;
        let program = waluau_parser::parse(source).expect("parse should succeed");
        let ir = waluau_ir::build(&program).expect("ir should succeed");
        let wasm = super::emit(&ir).expect("emit should succeed");
        Validator::new()
            .validate_all(&wasm)
            .expect("emitted module should validate");
    }
}
