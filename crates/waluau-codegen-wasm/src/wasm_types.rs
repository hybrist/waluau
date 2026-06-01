use waluau_ast::{NumericType, Type};
use waluau_diagnostics::Diagnostic;
use wasm_encoder::{AbstractHeapType, HeapType, RefType, ValType};

use crate::arrays::ArrayTypeRegistry;
use crate::coroutine_state_ref_type;

pub(crate) fn externref_val_type() -> ValType {
    ValType::Ref(RefType {
        nullable: true,
        heap_type: HeapType::Abstract {
            shared: false,
            ty: AbstractHeapType::Extern,
        },
    })
}

pub(crate) fn externref_nonnull_val_type() -> ValType {
    ValType::Ref(RefType {
        nullable: false,
        heap_type: HeapType::Abstract {
            shared: false,
            ty: AbstractHeapType::Extern,
        },
    })
}

pub(crate) fn wasm_type(
    ty: &Type,
    array_registry: &ArrayTypeRegistry,
) -> Result<ValType, Diagnostic> {
    match ty {
        Type::Bool | Type::Numeric(NumericType::U32 | NumericType::I32) => Ok(ValType::I32),
        Type::Numeric(NumericType::U64 | NumericType::I64) => Ok(ValType::I64),
        Type::Numeric(NumericType::F32) => Ok(ValType::F32),
        Type::Numeric(NumericType::F64) => Ok(ValType::F64),
        Type::Array(_) => {
            let index = array_registry.index(ty)?;
            Ok(ValType::Ref(RefType {
                nullable: true,
                heap_type: HeapType::Concrete(index),
            }))
        }
        Type::String => Ok(externref_val_type()),
        Type::Unit => Err(Diagnostic::new(
            "unit type has no wasm value representation",
        )),
        Type::Multi(_) => Err(Diagnostic::new(
            "multi-value types are not supported in Wasm signatures yet",
        )),
        Type::Function { .. } => Ok(ValType::Ref(RefType {
            nullable: true,
            heap_type: HeapType::Concrete(array_registry.func_val_struct_type),
        })),
        Type::Thread => Ok(coroutine_state_ref_type(
            array_registry.coroutine_state_type()?,
        )),
        Type::Record(_) => unreachable!("namespace types are not stored in wasm locals"),
        Type::TypeParam(_) => {
            unreachable!("generic type parameters must be specialized before codegen")
        }
    }
}

pub(crate) fn compress_locals(locals: Vec<ValType>) -> Vec<(u32, ValType)> {
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
