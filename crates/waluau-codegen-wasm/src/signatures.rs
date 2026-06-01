use std::collections::HashMap;

use waluau_ast::Type;
use waluau_diagnostics::Diagnostic;
use waluau_ir::{Instruction as IrInstruction, Module};

#[derive(Clone)]
pub(crate) struct SignatureRegistry {
    pub(crate) unique_signatures: Vec<(Vec<Type>, Type)>,
    signature_indices: HashMap<(Vec<Type>, Type), u32>,
}

impl SignatureRegistry {
    pub(crate) fn new() -> Self {
        Self {
            unique_signatures: Vec::new(),
            signature_indices: HashMap::new(),
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
}

pub(crate) fn collect_user_signatures(module: &Module, start_thunk: bool) -> SignatureRegistry {
    let mut registry = SignatureRegistry::new();
    for function in &module.functions {
        let params = function.params.iter().map(|(_, ty)| ty.clone()).collect();
        registry.add(params, function.return_type.clone());
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
                    }
                    IrInstruction::CallValue {
                        params,
                        return_type,
                        ..
                    } => {
                        registry.add(params.clone(), return_type.clone());
                    }
                    _ => {}
                }
            }
        }
    }
    if start_thunk {
        registry.add(Vec::new(), Type::Unit);
    }
    registry
}

pub(crate) fn find_function_type_index(
    registry: &SignatureRegistry,
    user_type_base: u32,
    params: &[Type],
    return_type: &Type,
) -> Result<u32, Diagnostic> {
    registry
        .get(params, return_type)
        .map(|index| user_type_base + index)
        .ok_or_else(|| {
            Diagnostic::new(format!(
                "no wasm function type found for indirect call signature ({}) -> {}",
                params
                    .iter()
                    .map(|ty| ty.to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
                return_type
            ))
        })
}
