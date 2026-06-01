use std::collections::{HashMap, HashSet};

use waluau_ast::{Expr, Function, NumberLiteral, Program, Rebindability, Stmt, Type};
use waluau_diagnostics::{Diagnostic, DiagnosticCategory};

mod builtins;
mod expressions;
mod numeric;
mod signatures;
mod statements;

use signatures::{
    FnSignature, GenericScheme, infer_top_level_function_return_type, inference_diagnostic,
};
use statements::check_function;

#[derive(Clone)]
struct Binding {
    ty: Type,
    rebindability: Rebindability,
    record_open: bool,
}

fn binding_for(ty: Type, rebindability: Rebindability) -> Binding {
    let record_open = matches!(ty, Type::Record(_));
    Binding {
        ty,
        rebindability,
        record_open,
    }
}

pub fn type_check(program: &Program) -> Result<(), Diagnostic> {
    let _ = type_check_and_infer(program)?;
    Ok(())
}

pub fn type_check_and_infer(program: &Program) -> Result<Program, Diagnostic> {
    let mut typed = program.clone();
    if !typed.top_level.is_empty() {
        typed.functions.push(Function {
            name: "__waluau_top_level_init".to_string(),
            type_params: Vec::new(),
            params: Vec::new(),
            return_type: Some(Type::number()),
            body: {
                let mut body = typed.top_level.clone();
                body.push(Stmt::Return(Expr::Number(
                    NumberLiteral { raw: "0".into() },
                    None,
                )));
                body
            },
            file_path: typed.entry_file_path.clone(),
        });
    }

    let mut fn_signatures: HashMap<String, FnSignature> = HashMap::new();
    for function in &typed.functions {
        if function.type_params.is_empty() {
            if let Some(ret) = &function.return_type {
                fn_signatures.insert(
                    function.name.clone(),
                    FnSignature::Mono {
                        params: function
                            .params
                            .iter()
                            .map(|param| param.ty.clone())
                            .collect(),
                        return_type: ret.clone(),
                    },
                );
            }
        } else if let Some(ret) = &function.return_type {
            fn_signatures.insert(
                function.name.clone(),
                FnSignature::Generic(GenericScheme {
                    type_params: function.type_params.clone(),
                    params: function
                        .params
                        .iter()
                        .map(|param| param.ty.clone())
                        .collect(),
                    return_type: ret.clone(),
                }),
            );
        }
    }

    let mut unresolved: Vec<usize> = typed
        .functions
        .iter()
        .enumerate()
        .filter_map(|(idx, function)| {
            (function.return_type.is_none() && function.type_params.is_empty()).then_some(idx)
        })
        .collect();

    while !unresolved.is_empty() {
        let mut progressed = false;
        let mut next_unresolved = Vec::new();
        let unresolved_names: Vec<String> = unresolved
            .iter()
            .map(|idx| typed.functions[*idx].name.clone())
            .collect();
        for idx in unresolved {
            let function = &typed.functions[idx];
            let function_name = function.name.clone();
            let function_params: Vec<Type> = function
                .params
                .iter()
                .map(|param| param.ty.clone())
                .collect();
            match infer_top_level_function_return_type(function, &fn_signatures, &unresolved_names)?
            {
                Some(ret) => {
                    typed.functions[idx].return_type = Some(ret.clone());
                    fn_signatures.insert(
                        function_name,
                        FnSignature::Mono {
                            params: function_params,
                            return_type: ret,
                        },
                    );
                    progressed = true;
                }
                None => next_unresolved.push(idx),
            }
        }
        if !progressed {
            let name = &typed.functions[next_unresolved[0]].name;
            return Err(inference_diagnostic(
                "inference/unsupported",
                DiagnosticCategory::Unsupported,
                format!("cannot infer return type for recursive or cyclic function '{name}'"),
                "add an explicit return type annotation to break the cycle",
            ));
        }
        unresolved = next_unresolved;
    }

    for function in &typed.functions {
        check_function(function, &fn_signatures, &HashSet::new())?;
    }

    Ok(typed)
}

#[cfg(test)]
mod tests;
