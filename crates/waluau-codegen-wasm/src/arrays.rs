use std::collections::{BTreeMap, BTreeSet, HashMap};

use waluau_ast::{NumericType, Type};
use waluau_diagnostics::Diagnostic;
use waluau_ir::{Instruction as IrInstruction, Module};
use wasm_encoder::{HeapType, RefType, StorageType, ValType};

use crate::coroutines::coroutine_state_ref_type;
use crate::wasm_types::externref_val_type;

pub(crate) struct ArrayTypeRegistry {
    indices: HashMap<String, u32>,
    record_indices: HashMap<String, u32>,
    record_field_indices: HashMap<String, BTreeMap<String, u32>>,
    pub(crate) coroutine_state_type: Option<u32>,
    /// Type index of `$anyref_array = (array (ref null any) mutable)`.
    pub(crate) anyref_array_type: u32,
    /// Type index of `$func_val = (struct { orig_idx: i32, env: ref null $anyref_array, wrapper_idx: i32 })`.
    pub(crate) func_val_struct_type: u32,
}

impl ArrayTypeRegistry {
    pub(crate) fn with_function_type_offset(
        array_types: &[Type],
        record_types: &[Type],
        function_type_count: u32,
        record_type_offset: u32,
        anyref_array_type: u32,
        func_val_struct_type: u32,
    ) -> Self {
        let indices = array_types
            .iter()
            .enumerate()
            .map(|(offset, array_ty)| (type_key(array_ty), function_type_count + offset as u32))
            .collect();
        let record_indices = record_types
            .iter()
            .enumerate()
            .map(|(offset, record_ty)| (type_key(record_ty), record_type_offset + offset as u32))
            .collect();
        let mut record_field_indices = HashMap::new();
        for record_ty in record_types {
            let Type::Record(fields) = record_ty else {
                continue;
            };
            let mut field_indices = BTreeMap::new();
            for (index, name) in fields.keys().enumerate() {
                field_indices.insert(name.clone(), index as u32);
            }
            record_field_indices.insert(type_key(record_ty), field_indices);
        }
        Self {
            indices,
            record_indices,
            record_field_indices,
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

    pub(crate) fn record_index(&self, record_ty: &Type) -> Result<u32, Diagnostic> {
        self.record_indices
            .get(&type_key(record_ty))
            .copied()
            .ok_or_else(|| Diagnostic::new(format!("missing wasm record type for {record_ty}")))
    }

    pub(crate) fn record_field_index(
        &self,
        record_ty: &Type,
        field: &str,
    ) -> Result<u32, Diagnostic> {
        let by_name = self
            .record_field_indices
            .get(&type_key(record_ty))
            .ok_or_else(|| Diagnostic::new(format!("missing field index map for {record_ty}")))?;
        by_name.get(field).copied().ok_or_else(|| {
            Diagnostic::new(format!(
                "missing field '{}' in wasm record type for {}",
                field, record_ty
            ))
        })
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

pub(crate) fn collect_record_types(module: &Module) -> Vec<Type> {
    let mut seen = BTreeSet::new();
    let mut types = Vec::new();
    for function in &module.functions {
        for (_, ty) in &function.params {
            insert_record_type(ty, &mut seen, &mut types);
        }
        insert_record_type(&function.return_type, &mut seen, &mut types);
        for block in function.blocks.values() {
            for (_, instruction) in &block.instructions {
                collect_record_types_from_instruction(instruction, &mut seen, &mut types);
            }
        }
    }
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

fn insert_record_type(ty: &Type, seen: &mut BTreeSet<String>, out: &mut Vec<Type>) {
    match ty {
        Type::Record(fields) => {
            for field_ty in fields.values() {
                insert_record_type(field_ty, seen, out);
            }
            if seen.insert(type_key(ty)) {
                out.push(ty.clone());
            }
        }
        Type::Array(element) => insert_record_type(element, seen, out),
        Type::Multi(types) => {
            for nested in types {
                insert_record_type(nested, seen, out);
            }
        }
        Type::Function {
            params,
            return_type,
        } => {
            for param in params {
                insert_record_type(param, seen, out);
            }
            insert_record_type(return_type, seen, out);
        }
        _ => {}
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

fn collect_record_types_from_instruction(
    instruction: &IrInstruction,
    seen: &mut BTreeSet<String>,
    out: &mut Vec<Type>,
) {
    match instruction {
        IrInstruction::StructNew { struct_ty, .. } => insert_record_type(struct_ty, seen, out),
        IrInstruction::StructGet { field_ty, .. } => insert_record_type(field_ty, seen, out),
        IrInstruction::ArrayNew { element_ty, .. }
        | IrInstruction::ArrayGet { element_ty, .. }
        | IrInstruction::ArraySet { element_ty, .. } => insert_record_type(element_ty, seen, out),
        IrInstruction::CallValue {
            params,
            return_type,
            ..
        } => {
            for param in params {
                insert_record_type(param, seen, out);
            }
            insert_record_type(return_type, seen, out);
        }
        IrInstruction::Closure {
            params,
            return_type,
            ..
        } => {
            for param in params {
                insert_record_type(param, seen, out);
            }
            insert_record_type(return_type, seen, out);
        }
        IrInstruction::PackMulti { types, .. } => {
            for ty in types {
                insert_record_type(ty, seen, out);
            }
        }
        IrInstruction::Cast { from, to, .. } => {
            insert_record_type(from, seen, out);
            insert_record_type(to, seen, out);
        }
        IrInstruction::Phi(_)
        | IrInstruction::Param(_)
        | IrInstruction::Unit
        | IrInstruction::Number { .. }
        | IrInstruction::Bool(_)
        | IrInstruction::String(_)
        | IrInstruction::Binary { .. }
        | IrInstruction::MathIntrinsic { .. }
        | IrInstruction::Print { .. }
        | IrInstruction::ToString { .. }
        | IrInstruction::Call { .. }
        | IrInstruction::CoroutineCreate { .. }
        | IrInstruction::CoroutineResume { .. }
        | IrInstruction::CoroutineClose { .. }
        | IrInstruction::ArrayLen { .. }
        | IrInstruction::StructSet { .. }
        | IrInstruction::MultiGet { .. } => {}
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

pub(crate) fn record_storage_type(
    field_ty: &Type,
    registry: &ArrayTypeRegistry,
) -> Result<StorageType, Diagnostic> {
    match field_ty {
        Type::Numeric(NumericType::I32 | NumericType::U32) => Ok(StorageType::Val(ValType::I32)),
        Type::Numeric(NumericType::I64 | NumericType::U64) => Ok(StorageType::Val(ValType::I64)),
        Type::Numeric(NumericType::F32) => Ok(StorageType::Val(ValType::F32)),
        Type::Numeric(NumericType::F64) => Ok(StorageType::Val(ValType::F64)),
        Type::Bool => Ok(StorageType::Val(ValType::I32)),
        Type::String => Ok(StorageType::Val(externref_val_type())),
        Type::Array(_) => {
            let index = registry.index(field_ty)?;
            Ok(StorageType::Val(ValType::Ref(RefType {
                nullable: true,
                heap_type: HeapType::Concrete(index),
            })))
        }
        Type::Record(_) => {
            let index = registry.record_index(field_ty)?;
            Ok(StorageType::Val(ValType::Ref(RefType {
                nullable: true,
                heap_type: HeapType::Concrete(index),
            })))
        }
        Type::Function { .. } => Ok(StorageType::Val(ValType::Ref(RefType {
            nullable: true,
            heap_type: HeapType::Concrete(registry.func_val_struct_type),
        }))),
        Type::Thread => Ok(StorageType::Val(coroutine_state_ref_type(
            registry.coroutine_state_type()?,
        ))),
        Type::Multi(_) => Err(Diagnostic::new(
            "multi-value types are not supported in record fields",
        )),
        Type::TypeParam(_) => unreachable!(),
        Type::Unit => unreachable!(),
    }
}
