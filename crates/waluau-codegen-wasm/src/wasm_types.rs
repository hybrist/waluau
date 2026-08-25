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

/// `anyref` (`ref null any`): the wasm representation of the `unknown` type. Boxed
/// primitives and any other heap reference are subtypes of `any`.
pub(crate) fn anyref_val_type() -> ValType {
    ValType::Ref(RefType {
        nullable: true,
        heap_type: HeapType::Abstract {
            shared: false,
            ty: AbstractHeapType::Any,
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
        Type::Named { .. } | Type::Opaque { .. } | Type::Readonly(_) => {
            unreachable!("source-only types must be erased before wasm lowering")
        }
        Type::StringLiteralUnion(_) | Type::NumberLiteralUnion(_) => {
            unreachable!("literal unions must be erased before wasm lowering")
        }
        // Array values are the growable wrapper struct `{storage, len}`, not the
        // raw wasm array (which only backs the struct's storage field).
        Type::Array(element) | Type::Variadic(element) => {
            let index = array_registry.growable_array_index(element)?;
            Ok(ValType::Ref(RefType {
                nullable: true,
                heap_type: HeapType::Concrete(index),
            }))
        }
        Type::String | Type::Bytes | Type::Extern | Type::ExternSubtype(_) => {
            Ok(externref_val_type())
        }
        // Typed arrays are i32 pointers into linear memory (the element count
        // lives in the allocation header, not the value).
        Type::TypedArray(_) => Ok(ValType::I32),
        // Nullable numerics, bools, and typed-array pointers have no null representation in their raw
        // value type, so they are typed nullable box refs
        // (`ref null $nullable_box_K`): null stands for nil and a one-field
        // struct holds the payload. Nullable reference types reuse the inner
        // (already nullable) reference representation.
        Type::Nullable(_) if ty.is_boxed_nullable() => array_registry.nullable_box_val_type(ty),
        Type::Nullable(inner) => wasm_type(inner, array_registry),
        Type::Nil => Ok(externref_val_type()),
        Type::Unknown => Ok(anyref_val_type()),
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
        Type::Record(_) => {
            let index = array_registry.record_index(ty)?;
            Ok(ValType::Ref(RefType {
                nullable: true,
                heap_type: HeapType::Concrete(index),
            }))
        }
        Type::TypeParam(_) => {
            unreachable!("generic type parameters must be specialized before codegen")
        }
        Type::TaggedVariant(_) | Type::TaggedUnion(_) => {
            let canonical = Type::canonical_tagged_union_record();
            let index = array_registry.record_index(&canonical)?;
            Ok(ValType::Ref(RefType {
                nullable: true,
                heap_type: HeapType::Concrete(index),
            }))
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
