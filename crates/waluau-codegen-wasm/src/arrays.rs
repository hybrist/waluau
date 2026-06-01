use std::collections::{BTreeSet, HashMap};

use waluau_ast::{NumericType, Type};
use waluau_diagnostics::Diagnostic;
use waluau_ir::{Instruction as IrInstruction, Module};
use wasm_encoder::{HeapType, RefType, StorageType, ValType};

use crate::wasm_types::externref_val_type;

pub(crate) struct ArrayTypeRegistry {
    indices: HashMap<String, u32>,
    pub(crate) coroutine_state_type: Option<u32>,
    /// Type index of `$anyref_array = (array (ref null any) mutable)`.
    pub(crate) anyref_array_type: u32,
    /// Type index of `$func_val = (struct { orig_idx: i32, env: ref null $anyref_array, wrapper_idx: i32 })`.
    pub(crate) func_val_struct_type: u32,
}

impl ArrayTypeRegistry {
    pub(crate) fn with_function_type_offset(
        array_types: &[Type],
        function_type_count: u32,
        anyref_array_type: u32,
        func_val_struct_type: u32,
    ) -> Self {
        let indices = array_types
            .iter()
            .enumerate()
            .map(|(offset, array_ty)| (type_key(array_ty), function_type_count + offset as u32))
            .collect();
        Self {
            indices,
            coroutine_state_type: None,
            anyref_array_type,
            func_val_struct_type,
        }
    }

    pub(crate) fn index(&self, array_ty: &Type) -> Result<u32, Diagnostic> {
        self.indices
            .get(&type_key(array_ty))
            .copied()
            .ok_or_else(|| Diagnostic::new(format!("missing wasm array type for {array_ty}")))
    }

    pub(crate) fn coroutine_state_type(&self) -> Result<u32, Diagnostic> {
        self.coroutine_state_type
            .ok_or_else(|| Diagnostic::new("missing coroutine state struct type"))
    }
}

fn type_key(ty: &Type) -> String {
    ty.to_string()
}

pub(crate) fn collect_array_types(module: &Module) -> Vec<Type> {
    let mut seen = BTreeSet::new();
    let mut types = Vec::new();
    for function in &module.functions {
        for (_, ty) in &function.params {
            insert_array_type(ty, &mut seen, &mut types);
        }
        insert_array_type(&function.return_type, &mut seen, &mut types);
        for block in function.blocks.values() {
            for (_, instruction) in &block.instructions {
                collect_array_types_from_instruction(instruction, &mut seen, &mut types);
            }
        }
    }
    types.sort_by_key(array_type_depth);
    types
}

fn array_type_depth(ty: &Type) -> usize {
    match ty {
        Type::Array(element) => 1 + array_type_depth(element),
        _ => 0,
    }
}

fn insert_array_type(ty: &Type, seen: &mut BTreeSet<String>, out: &mut Vec<Type>) {
    if let Type::Array(element) = ty {
        insert_array_type(element, seen, out);
        if seen.insert(type_key(ty)) {
            out.push(ty.clone());
        }
    }
}

fn collect_array_types_from_instruction(
    instruction: &IrInstruction,
    seen: &mut BTreeSet<String>,
    out: &mut Vec<Type>,
) {
    match instruction {
        IrInstruction::ArrayNew { element_ty, .. } => {
            insert_array_type(&Type::Array(Box::new(element_ty.clone())), seen, out);
        }
        IrInstruction::ArrayGet { element_ty, .. } | IrInstruction::ArraySet { element_ty, .. } => {
            insert_array_type(&Type::Array(Box::new(element_ty.clone())), seen, out);
        }
        IrInstruction::ArrayLen { .. } => {}
        _ => {}
    }
}

pub(crate) fn array_storage_type(
    element_ty: &Type,
    registry: &ArrayTypeRegistry,
) -> Result<StorageType, Diagnostic> {
    match element_ty {
        Type::Numeric(NumericType::I32 | NumericType::U32) => Ok(StorageType::Val(ValType::I32)),
        Type::Numeric(NumericType::I64 | NumericType::U64) => Ok(StorageType::Val(ValType::I64)),
        Type::Numeric(NumericType::F32) => Ok(StorageType::Val(ValType::F32)),
        Type::Numeric(NumericType::F64) => Ok(StorageType::Val(ValType::F64)),
        Type::Bool => Ok(StorageType::Val(ValType::I32)),
        Type::String => Ok(StorageType::Val(externref_val_type())),
        Type::Array(_) => {
            let index = registry.index(element_ty)?;
            Ok(StorageType::Val(ValType::Ref(RefType {
                nullable: true,
                heap_type: HeapType::Concrete(index),
            })))
        }
        Type::Multi(_) => Err(Diagnostic::new(
            "multi-value types are not supported in array storage yet",
        )),
        Type::Function { .. } | Type::Record(_) | Type::TypeParam(_) | Type::Thread => {
            unreachable!()
        }
        Type::Unit => unreachable!(),
    }
}
