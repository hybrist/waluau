use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use waluau_ast::{NumericType, Type};
use waluau_diagnostics::Diagnostic;
use waluau_ir::{Instruction as IrInstruction, Module};
use wasm_encoder::{HeapType, RefType, StorageType, ValType};

use crate::coroutines::coroutine_state_ref_type;
use crate::wasm_types::externref_val_type;

/// Payload storage class of a typed nullable box (`i32?`, `f64?`, ...).
///
/// Nullable primitives are represented as `(ref null $nullable_box_K)` where
/// `$nullable_box_K = (struct (field mut K))`: null stands for nil and a
/// one-field box holds the payload. `i32`, `u32`, and `bool` share the i32
/// box; `i64`/`u64` share the i64 box. The payload field is mutable purely to
/// keep these struct types structurally distinct from the immutable
/// `$boxed_f64`/`$boxed_bool` unknown-value boxes under wasm GC's structural
/// (iso-recursive) type canonicalization.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) enum NullableBoxKind {
    I32,
    I64,
    F32,
    F64,
}

impl NullableBoxKind {
    /// The box kind for a nullable type's *inner* (payload) type.
    pub(crate) fn for_inner(inner: &Type) -> Option<Self> {
        match inner {
            Type::Numeric(NumericType::I32 | NumericType::U32)
            | Type::Bool
            | Type::TypedArray(_) => Some(Self::I32),
            Type::Numeric(NumericType::I64 | NumericType::U64) => Some(Self::I64),
            Type::Numeric(NumericType::F32) => Some(Self::F32),
            Type::Numeric(NumericType::F64) => Some(Self::F64),
            _ => None,
        }
    }

    /// The box kind for a nullable type (`Some` exactly for boxed nullables).
    pub(crate) fn of(ty: &Type) -> Option<Self> {
        match ty {
            Type::Nullable(inner) => Self::for_inner(inner),
            _ => None,
        }
    }

    pub(crate) fn payload_val_type(self) -> ValType {
        match self {
            Self::I32 => ValType::I32,
            Self::I64 => ValType::I64,
            Self::F32 => ValType::F32,
            Self::F64 => ValType::F64,
        }
    }

    /// Suffix used in the exported JS interop helper names
    /// (`__waluau_box_nullable_<suffix>` / `__waluau_unbox_nullable_<suffix>`).
    pub(crate) fn export_suffix(self) -> &'static str {
        match self {
            Self::I32 => "i32",
            Self::I64 => "i64",
            Self::F32 => "f32",
            Self::F64 => "f64",
        }
    }
}

#[derive(Clone)]
pub(crate) struct ArrayTypeRegistry {
    pub(crate) indices: HashMap<String, u32>,
    pub(crate) record_indices: HashMap<String, u32>,
    pub(crate) record_field_indices: HashMap<String, BTreeMap<String, u32>>,
    pub(crate) coroutine_state_type: Option<u32>,
    /// Type indices of the `$nullable_box_K` structs backing nullable
    /// primitives (`i32?` etc.). Only the kinds the module actually uses are
    /// emitted; see [`NullableBoxKind`].
    pub(crate) nullable_box_indices: BTreeMap<NullableBoxKind, u32>,
    /// Type index of `$anyref_array = (array (ref null any) mutable)`.
    pub(crate) anyref_array_type: u32,
    /// Type index of `$func_val = (struct { orig_idx: i32, env: ref null $anyref_array, wrapper_idx: i32 })`.
    pub(crate) func_val_struct_type: u32,
    /// Type index of `$boxed_f64 = (struct (field f64))`, used to box `f64` values
    /// into `anyref` (`unknown`). `i32` uses `i31ref`; bool has a distinct box so
    /// runtime `type(unknown)` can distinguish booleans from small integers.
    pub(crate) boxed_f64_struct_type: u32,
    /// Type index of `$boxed_bool = (struct (field i32))`, used to box `bool`
    /// values into `anyref` (`unknown`).
    pub(crate) boxed_bool_struct_type: u32,
    /// Whether the closure GC types (including `$boxed_f64`) were emitted for
    /// this module. When absent, `boxed_f64_struct_type` is a dummy index and
    /// no boxed f64 can exist at runtime.
    pub(crate) closure_gc_present: bool,
    /// Type indices for growable array wrapper structs, keyed by element type.
    /// Each growable array value is
    /// `(struct (field storage: ref null array) (field len: i32) (field kind: i32))`;
    /// `kind` distinguishes element types whose storage arrays are structurally
    /// identical after Wasm GC canonicalization.
    /// the struct is emitted immediately after its backing array type so nested
    /// arrays can reference the inner struct without a forward reference.
    pub(crate) growable_array_indices: HashMap<String, u32>,
    /// The element types behind `growable_array_indices`, in the deterministic
    /// depth-sorted emission order, for dynamic (unknown-operand) array ops
    /// that dispatch over every array type in the module.
    pub(crate) growable_array_element_types: Vec<(Type, u32)>,
}

pub(crate) struct RuntimeGcTypes {
    pub(crate) anyref_array_type: u32,
    pub(crate) func_val_struct_type: u32,
    pub(crate) boxed_f64_struct_type: u32,
    pub(crate) boxed_bool_struct_type: u32,
}

impl ArrayTypeRegistry {
    pub(crate) fn with_function_type_offset(
        array_types: &[Type],
        record_types: &[Type],
        function_type_count: u32,
        record_type_offset: u32,
        runtime_gc_types: RuntimeGcTypes,
    ) -> Self {
        // Array-related types are emitted as interleaved pairs: the raw storage
        // array at `base + 2*i` and its growable wrapper struct at `base + 2*i + 1`.
        // Because `array_types` is depth-sorted, an outer array's storage can
        // reference the inner element's growable struct as a backward reference.
        let indices = array_types
            .iter()
            .enumerate()
            .map(|(offset, array_ty)| (type_key(array_ty), function_type_count + 2 * offset as u32))
            .collect();
        let growable_array_indices = array_types
            .iter()
            .enumerate()
            .filter_map(|(offset, array_ty)| {
                let Type::Array(element) = array_ty else {
                    return None;
                };
                Some((
                    type_key(element),
                    function_type_count + 2 * offset as u32 + 1,
                ))
            })
            .collect();
        let growable_array_element_types = array_types
            .iter()
            .enumerate()
            .filter_map(|(offset, array_ty)| {
                let Type::Array(element) = array_ty else {
                    return None;
                };
                Some((
                    element.as_ref().clone(),
                    function_type_count + 2 * offset as u32 + 1,
                ))
            })
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
            nullable_box_indices: BTreeMap::new(),
            anyref_array_type: runtime_gc_types.anyref_array_type,
            func_val_struct_type: runtime_gc_types.func_val_struct_type,
            boxed_f64_struct_type: runtime_gc_types.boxed_f64_struct_type,
            boxed_bool_struct_type: runtime_gc_types.boxed_bool_struct_type,
            closure_gc_present: false,
            growable_array_indices,
            growable_array_element_types,
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

    pub(crate) fn growable_array_index(&self, element_ty: &Type) -> Result<u32, Diagnostic> {
        self.growable_array_indices
            .get(&type_key(element_ty))
            .copied()
            .ok_or_else(|| Diagnostic::new(format!("missing growable array type for {element_ty}")))
    }

    /// The `$nullable_box_K` type index for a nullable primitive's inner type.
    pub(crate) fn nullable_box_index_for_inner(&self, inner: &Type) -> Result<u32, Diagnostic> {
        let kind = NullableBoxKind::for_inner(inner).ok_or_else(|| {
            Diagnostic::new(format!("no nullable box representation for {inner}?"))
        })?;
        self.nullable_box_indices
            .get(&kind)
            .copied()
            .ok_or_else(|| Diagnostic::new(format!("missing nullable box type for {inner}?")))
    }

    /// The `$nullable_box_K` type index for a boxed nullable type (`i32?` etc.).
    pub(crate) fn nullable_box_index(&self, nullable_ty: &Type) -> Result<u32, Diagnostic> {
        let Type::Nullable(inner) = nullable_ty else {
            return Err(Diagnostic::new(format!(
                "expected a nullable type for its box index, got {nullable_ty}"
            )));
        };
        self.nullable_box_index_for_inner(inner)
    }

    /// The `(ref null $nullable_box_K)` value type for a boxed nullable type.
    pub(crate) fn nullable_box_val_type(&self, nullable_ty: &Type) -> Result<ValType, Diagnostic> {
        Ok(ValType::Ref(RefType {
            nullable: true,
            heap_type: HeapType::Concrete(self.nullable_box_index(nullable_ty)?),
        }))
    }
}

fn type_key(ty: &Type) -> String {
    ty.to_string()
}

pub(crate) fn collect_array_types(module: &Module) -> Vec<Type> {
    let mut seen = BTreeSet::new();
    let mut types = Vec::new();
    for global in &module.globals {
        insert_array_type(&global.ty, &mut seen, &mut types);
    }
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
    for global in &module.globals {
        insert_record_type(&global.ty, &mut seen, &mut types);
    }
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

/// Collect the nullable-box kinds (`i32?` etc.) a module needs, scanning every
/// type that reaches codegen: function/import signatures and each
/// type-carrying IR instruction. Missing a use site fails compilation with a
/// "missing nullable box type" diagnostic rather than miscompiling.
pub(crate) fn collect_nullable_box_kinds(
    module: &Module,
    declared_imports: &[&waluau_ir::DeclaredImport],
) -> Vec<NullableBoxKind> {
    let mut kinds = BTreeSet::new();
    for global in &module.globals {
        insert_nullable_box_kinds(&global.ty, &mut kinds);
    }
    for import in declared_imports {
        for param in &import.params {
            insert_nullable_box_kinds(param, &mut kinds);
        }
        insert_nullable_box_kinds(&import.return_type, &mut kinds);
    }
    for function in &module.functions {
        for (_, ty) in &function.params {
            insert_nullable_box_kinds(ty, &mut kinds);
        }
        insert_nullable_box_kinds(&function.return_type, &mut kinds);
        for block in function.blocks.values() {
            for (_, instruction) in &block.instructions {
                collect_nullable_box_kinds_from_instruction(instruction, &mut kinds);
            }
        }
    }
    kinds.into_iter().collect()
}

fn insert_nullable_box_kinds(ty: &Type, out: &mut BTreeSet<NullableBoxKind>) {
    match ty {
        Type::Nullable(inner) => {
            if let Some(kind) = NullableBoxKind::for_inner(inner) {
                out.insert(kind);
            }
            insert_nullable_box_kinds(inner, out);
        }
        Type::Array(element) => insert_nullable_box_kinds(element, out),
        // A variadic pack carries an element type like an array does; a
        // `T?` element still needs its box kind registered.
        Type::Variadic(element) => insert_nullable_box_kinds(element, out),
        Type::Record(fields) => {
            for field_ty in fields.values() {
                insert_nullable_box_kinds(field_ty, out);
            }
        }
        Type::Function {
            params,
            return_type,
            ..
        } => {
            for param in params {
                insert_nullable_box_kinds(param, out);
            }
            insert_nullable_box_kinds(return_type, out);
        }
        Type::Multi(types) => {
            for ty in types {
                insert_nullable_box_kinds(ty, out);
            }
        }
        Type::Opaque { ty, .. } | Type::ExternSubtype(ty) => insert_nullable_box_kinds(ty, out),
        Type::TaggedVariant(variant) => insert_nullable_box_kinds(&variant.payload, out),
        Type::TaggedUnion(variants) => {
            for variant in variants {
                insert_nullable_box_kinds(&variant.payload, out);
            }
        }
        Type::Numeric(_)
        | Type::Unit
        | Type::Bool
        | Type::String
        | Type::Bytes
        | Type::Extern
        | Type::Nil
        | Type::Named { .. }
        | Type::TypedArray(_)
        | Type::TypeParam(_)
        | Type::Thread
        | Type::Unknown
        | Type::StringLiteralUnion(_)
        | Type::NumberLiteralUnion(_) => {}
    }
}

fn collect_nullable_box_kinds_from_instruction(
    instruction: &IrInstruction,
    out: &mut BTreeSet<NullableBoxKind>,
) {
    let mut add = |ty: &Type| insert_nullable_box_kinds(ty, out);
    match instruction {
        IrInstruction::GlobalGet { ty, .. } | IrInstruction::GlobalSet { ty, .. } => add(ty),
        IrInstruction::Null { ty } | IrInstruction::IsNull { ty, .. } => add(ty),
        IrInstruction::Cast { from, to, .. } => {
            add(from);
            add(to);
        }
        IrInstruction::Binary {
            operand_ty,
            result_ty,
            ..
        } => {
            add(operand_ty);
            add(result_ty);
        }
        IrInstruction::ToString { from, .. }
        | IrInstruction::TypeName { from, .. }
        | IrInstruction::ToNumber { from, .. } => add(from),
        IrInstruction::HostCall { return_type, .. } => add(return_type),
        IrInstruction::CallValue {
            params,
            return_type,
            ..
        }
        | IrInstruction::ProtectedCall {
            params,
            return_type,
            ..
        }
        | IrInstruction::Closure {
            params,
            return_type,
            ..
        } => {
            for param in params {
                add(param);
            }
            add(return_type);
        }
        IrInstruction::ArrayNew { element_ty, .. }
        | IrInstruction::ArrayGet { element_ty, .. }
        | IrInstruction::ArraySet { element_ty, .. }
        | IrInstruction::ArrayPop { element_ty, .. }
        | IrInstruction::ArraySlice { element_ty, .. } => add(element_ty),
        IrInstruction::StructNew { struct_ty, .. } => add(struct_ty),
        IrInstruction::StructGet { field_ty, .. } => add(field_ty),
        IrInstruction::PackMulti { types, .. } => {
            for ty in types {
                add(ty);
            }
        }
        IrInstruction::MultiGet { ty, .. } => add(ty),
        _ => {}
    }
}

fn array_type_depth(ty: &Type) -> usize {
    match ty {
        Type::Array(element) | Type::Variadic(element) => 1 + array_type_depth(element),
        _ => 0,
    }
}

fn insert_array_type(ty: &Type, seen: &mut BTreeSet<String>, out: &mut Vec<Type>) {
    match ty {
        Type::Array(element) | Type::Variadic(element) => {
            insert_array_type(element, seen, out);
            let array_ty = Type::Array(element.clone());
            if seen.insert(type_key(&array_ty)) {
                out.push(array_ty);
            }
        }
        Type::Record(fields) => {
            for field_ty in fields.values() {
                insert_array_type(field_ty, seen, out);
            }
        }
        Type::Nullable(inner) => insert_array_type(inner, seen, out),
        Type::Multi(types) => {
            for nested in types {
                insert_array_type(nested, seen, out);
            }
        }
        Type::Function {
            params,
            return_type,
            ..
        } => {
            for param in params {
                insert_array_type(param, seen, out);
            }
            insert_array_type(return_type, seen, out);
        }
        _ => {}
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
        Type::Array(element) | Type::Variadic(element) => insert_record_type(element, seen, out),
        Type::Nullable(inner) => insert_record_type(inner, seen, out),
        Type::Multi(types) => {
            for nested in types {
                insert_record_type(nested, seen, out);
            }
        }
        Type::Function {
            params,
            return_type,
            ..
        } => {
            for param in params {
                insert_record_type(param, seen, out);
            }
            insert_record_type(return_type, seen, out);
        }
        Type::TaggedVariant(_) | Type::TaggedUnion(_) => {
            insert_record_type(&Type::canonical_tagged_union_record(), seen, out);
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
            insert_array_type(&Type::Array(Arc::new(element_ty.clone())), seen, out);
        }
        IrInstruction::ArrayGet { element_ty, .. }
        | IrInstruction::ArraySet { element_ty, .. }
        | IrInstruction::ArraySlice { element_ty, .. }
        | IrInstruction::ArrayPop { element_ty, .. } => {
            insert_array_type(&Type::Array(Arc::new(element_ty.clone())), seen, out);
        }
        IrInstruction::ArrayLen { .. }
        | IrInstruction::Bytes(_)
        | IrInstruction::BytesGet { .. }
        | IrInstruction::BytesLen { .. } => {}
        _ => {}
    }
}

fn collect_record_types_from_instruction(
    instruction: &IrInstruction,
    seen: &mut BTreeSet<String>,
    out: &mut Vec<Type>,
) {
    match instruction {
        IrInstruction::GlobalGet { ty, .. } | IrInstruction::GlobalSet { ty, .. } => {
            insert_record_type(ty, seen, out)
        }
        IrInstruction::StructNew { struct_ty, .. } => insert_record_type(struct_ty, seen, out),
        IrInstruction::StructGet { field_ty, .. } => insert_record_type(field_ty, seen, out),
        IrInstruction::ArrayNew { element_ty, .. }
        | IrInstruction::ArrayGet { element_ty, .. }
        | IrInstruction::ArraySet { element_ty, .. }
        | IrInstruction::ArraySlice { element_ty, .. }
        | IrInstruction::ArrayPop { element_ty, .. } => insert_record_type(element_ty, seen, out),
        IrInstruction::CallValue {
            params,
            return_type,
            ..
        }
        | IrInstruction::ProtectedCall {
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
        IrInstruction::CoroutineResumeTagged { .. } => {
            insert_record_type(&Type::canonical_tagged_union_record(), seen, out);
        }
        IrInstruction::Phi(_)
        | IrInstruction::Param(_)
        | IrInstruction::Unit
        | IrInstruction::Number { .. }
        | IrInstruction::Bool(_)
        | IrInstruction::String(_)
        | IrInstruction::Bytes(_)
        | IrInstruction::Binary { .. }
        | IrInstruction::MathIntrinsic { .. }
        | IrInstruction::BitwiseIntrinsic { .. }
        | IrInstruction::Print { .. }
        | IrInstruction::Throw { .. }
        | IrInstruction::ToString { .. }
        | IrInstruction::TypeName { .. }
        | IrInstruction::ToNumber { .. }
        | IrInstruction::Call { .. }
        | IrInstruction::HostCall { .. }
        | IrInstruction::CoroutineCreate { .. }
        | IrInstruction::CoroutineResume { .. }
        | IrInstruction::CoroutineAwaitResult
        | IrInstruction::CoroutineClose { .. }
        | IrInstruction::ArrayLen { .. }
        | IrInstruction::DynLen { .. }
        | IrInstruction::DynIndex { .. }
        | IrInstruction::BytesGet { .. }
        | IrInstruction::BytesLen { .. }
        | IrInstruction::StructSet { .. }
        | IrInstruction::Null { .. }
        | IrInstruction::IsNull { .. }
        | IrInstruction::ExternCastTest { .. }
        | IrInstruction::BufferNew { .. }
        | IrInstruction::BufferConst { .. }
        | IrInstruction::BufferNewSized { .. }
        | IrInstruction::BufferGet { .. }
        | IrInstruction::BufferSet { .. }
        | IrInstruction::BufferLen { .. }
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
        Type::String | Type::Bytes | Type::Extern | Type::ExternSubtype(_) | Type::Nil => {
            Ok(StorageType::Val(externref_val_type()))
        }
        // Nullable primitives are stored as their typed nullable box refs
        // (`ref null $nullable_box_K`); the box types are emitted before the
        // array types, so this is always a backward reference. Reference-typed
        // nullables reuse their inner (already nullable) representation.
        Type::Nullable(inner) if NullableBoxKind::for_inner(inner).is_some() => Ok(
            StorageType::Val(registry.nullable_box_val_type(element_ty)?),
        ),
        Type::Nullable(inner) => array_storage_type(inner, registry),
        // Typed arrays are i32 pointers into linear memory.
        Type::TypedArray(_) => Ok(StorageType::Val(ValType::I32)),
        Type::Unknown => Ok(StorageType::Val(crate::wasm_types::anyref_val_type())),
        // Array values are growable wrapper structs. `array_types` is
        // depth-sorted and each wrapper struct is emitted right after its raw
        // array, so the inner element's struct is always a backward reference.
        Type::Array(inner) | Type::Variadic(inner) => {
            let index = registry.growable_array_index(inner)?;
            Ok(StorageType::Val(ValType::Ref(RefType {
                nullable: true,
                heap_type: HeapType::Concrete(index),
            })))
        }
        // Function values must not point at the `$func_val` struct type
        // directly: array types are emitted before the closure GC types in the
        // Wasm type section, so `(array (ref null $func_val))` would create an
        // invalid forward reference. Store them as `anyref` and cast on
        // `array.get` instead (same treatment as `thread`).
        Type::Function { .. } => Ok(StorageType::Val(crate::wasm_types::anyref_val_type())),
        // Records (and tagged unions, which lower to the canonical record) are
        // emitted after the array types, so referencing them here would be an
        // invalid forward reference; store as `anyref` and cast on `array.get`.
        Type::Record(_) => Ok(StorageType::Val(crate::wasm_types::anyref_val_type())),
        // Thread capture cells must not point at the coroutine-state struct type directly:
        // array types are emitted before the coroutine state type exists in the Wasm type
        // section, so `(array (ref null $coroutine_state))` would create an invalid forward
        // reference. Store thread handles as `anyref` and cast on `array.get` instead.
        Type::Thread => Ok(StorageType::Val(crate::wasm_types::anyref_val_type())),
        Type::Named { .. } | Type::Opaque { .. } => {
            unreachable!("source aliases must be resolved before wasm lowering")
        }
        Type::Multi(_) => Err(Diagnostic::new(
            "multi-value types are not supported in array storage yet",
        )),
        Type::TaggedVariant(_) | Type::TaggedUnion(_) => {
            Ok(StorageType::Val(crate::wasm_types::anyref_val_type()))
        }
        Type::TypeParam(_) => unreachable!(),
        Type::Unit => unreachable!(),
        // Literal unions are erased to string/numeric before wasm lowering.
        Type::StringLiteralUnion(_) | Type::NumberLiteralUnion(_) => {
            unreachable!("literal unions must be erased before wasm lowering")
        }
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
        Type::String | Type::Bytes | Type::Extern | Type::ExternSubtype(_) | Type::Nil => {
            Ok(StorageType::Val(externref_val_type()))
        }
        // Nullable primitives are stored as their typed nullable box refs;
        // the box types precede the record types in the type section.
        Type::Nullable(inner) if NullableBoxKind::for_inner(inner).is_some() => {
            Ok(StorageType::Val(registry.nullable_box_val_type(field_ty)?))
        }
        Type::Nullable(inner) => record_storage_type(inner, registry),
        // Typed arrays are i32 pointers into linear memory.
        Type::TypedArray(_) => Ok(StorageType::Val(ValType::I32)),
        Type::Array(inner) | Type::Variadic(inner) => {
            let index = registry.growable_array_index(inner)?;
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
        Type::Unknown => Ok(StorageType::Val(crate::wasm_types::anyref_val_type())),
        Type::Named { .. } | Type::Opaque { .. } => {
            unreachable!("source aliases must be resolved before wasm lowering")
        }
        Type::Multi(_) => Err(Diagnostic::new(
            "multi-value types are not supported in record fields",
        )),
        Type::TaggedVariant(_) | Type::TaggedUnion(_) => {
            let canonical = Type::canonical_tagged_union_record();
            let index = registry.record_index(&canonical)?;
            Ok(StorageType::Val(ValType::Ref(RefType {
                nullable: true,
                heap_type: HeapType::Concrete(index),
            })))
        }
        Type::TypeParam(_) => unreachable!(),
        Type::Unit => unreachable!(),
        // Literal unions are erased to string/numeric before wasm lowering.
        Type::StringLiteralUnion(_) | Type::NumberLiteralUnion(_) => {
            unreachable!("literal unions must be erased before wasm lowering")
        }
    }
}
