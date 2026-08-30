use std::collections::HashMap;

use waluau_ast::Type;
use waluau_ir::{DeclaredImport, Instruction as IrInstruction, Module};

#[derive(Clone)]
pub(crate) struct SignatureRegistry {
    pub(crate) unique_signatures: Vec<(Vec<Type>, Type)>,
    signature_indices: HashMap<(Vec<Type>, Type), u32>,
    /// Wrapper signatures for `call_indirect` on closure values.
    /// Each entry corresponds to a closure logical type `(params, return_type)`.
    /// The Wasm type prepends an env param: `(ref null $anyref_array, params...) -> return`.
    pub(crate) wrapper_sigs: Vec<(Vec<Type>, Type)>,
    wrapper_sig_indices: HashMap<(Vec<Type>, Type), u32>,
}

impl SignatureRegistry {
    pub(crate) fn new() -> Self {
        Self {
            unique_signatures: Vec::new(),
            signature_indices: HashMap::new(),
            wrapper_sigs: Vec::new(),
            wrapper_sig_indices: HashMap::new(),
        }
    }

    pub(crate) fn add(&mut self, params: Vec<Type>, result: Type) {
        let key = (params, result);
        if !self.signature_indices.contains_key(&key) {
            let index = self.unique_signatures.len() as u32;
            self.signature_indices.insert(key.clone(), index);
            self.unique_signatures.push(key);
        }
    }

    pub(crate) fn get(&self, params: &[Type], result: &Type) -> Option<u32> {
        let key = (params.to_vec(), result.clone());
        self.signature_indices.get(&key).copied()
    }

    pub(crate) fn add_wrapper(&mut self, params: Vec<Type>, result: Type) {
        let key = (params, result);
        if !self.wrapper_sig_indices.contains_key(&key) {
            let index = self.wrapper_sigs.len() as u32;
            self.wrapper_sig_indices.insert(key.clone(), index);
            self.wrapper_sigs.push(key);
        }
    }

    /// Return the type section index of the wrapper sig for the given logical closure type.
    /// Wrapper types sit after all logical signature types in the type section.
    pub(crate) fn get_wrapper_type_index(
        &self,
        user_type_base: u32,
        params: &[Type],
        result: &Type,
    ) -> Option<u32> {
        let key = (params.to_vec(), result.clone());
        let wrapper_idx = self.wrapper_sig_indices.get(&key).copied()?;
        Some(user_type_base + self.unique_signatures.len() as u32 + wrapper_idx)
    }
}

/// Which exported host-callback/coroutine trampolines a module needs, so the
/// signature registry can pre-register their closure wrapper signatures.
#[derive(Clone, Copy, Default)]
pub(crate) struct TrampolineNeeds {
    pub(crate) callback_event_unit: bool,
    pub(crate) callback_f64_unit: bool,
    pub(crate) callback_unit_extern: bool,
    pub(crate) callback_unit: bool,
    pub(crate) promise_resume: bool,
}

pub(crate) fn collect_user_signatures(
    module: &Module,
    declared_imports: &[&DeclaredImport],
    start_thunk: bool,
    trampolines: TrampolineNeeds,
) -> SignatureRegistry {
    let mut registry = SignatureRegistry::new();
    for function in &module.functions {
        let params = function.params.iter().map(|(_, ty)| ty.clone()).collect();
        registry.add(params, function.return_type.clone());
    }
    for import in declared_imports {
        registry.add(import.params.clone(), import.return_type.clone());
    }
    for function in &module.functions {
        for block in function.blocks.values() {
            for (_, instruction) in &block.instructions {
                match instruction {
                    IrInstruction::Closure {
                        params,
                        return_type,
                        ..
                    } => {
                        registry.add(params.clone(), return_type.clone());
                        registry.add_wrapper(params.clone(), return_type.clone());
                    }
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
                        registry.add(params.clone(), return_type.clone());
                        registry.add_wrapper(params.clone(), return_type.clone());
                    }
                    IrInstruction::ProtectedCallUnknown { .. } => {
                        registry.add_wrapper(
                            vec![Type::Array(std::sync::Arc::new(Type::Unknown))],
                            Type::Unknown,
                        );
                    }
                    _ => {}
                }
            }
        }
    }
    if start_thunk {
        registry.add(Vec::new(), Type::Unit);
    }
    if trampolines.callback_event_unit {
        registry.add_wrapper(vec![Type::Extern], Type::Unit);
    }
    if trampolines.callback_f64_unit {
        registry.add_wrapper(
            vec![Type::Numeric(waluau_ast::NumericType::F64)],
            Type::Unit,
        );
    }
    if trampolines.callback_unit_extern {
        registry.add_wrapper(Vec::new(), Type::Extern);
    }
    if trampolines.callback_unit {
        registry.add_wrapper(Vec::new(), Type::Unit);
    }
    if trampolines.promise_resume {
        registry.add(Vec::new(), Type::Unit);
        registry.add(
            vec![
                Type::Thread,
                Type::Extern,
                Type::Numeric(waluau_ast::NumericType::I32),
            ],
            Type::Unit,
        );
    }
    registry
}
