pub fn verify(module: &Module) -> Result<(), Diagnostic> {
    let signatures: HashMap<_, _> = module
        .functions
        .iter()
        .map(|function| {
            (
                function.name.clone(),
                (
                    function
                        .params
                        .iter()
                        .map(|(_, ty)| ty.clone())
                        .collect::<Vec<_>>(),
                    function.return_type.clone(),
                ),
            )
        })
        .collect();
    let host_signatures: HashMap<_, _> = module
        .declared_imports
        .iter()
        .map(|declared| {
            (
                declared.symbol_id,
                (declared.params.clone(), declared.return_type.clone()),
            )
        })
        .collect();
    for function in &module.functions {
        verify_function(function, &signatures, &host_signatures)?;
    }
    Ok(())
}

fn verify_function(
    function: &Function,
    signatures: &HashMap<String, (Vec<Type>, Type)>,
    host_signatures: &HashMap<SymbolId, (Vec<Type>, Type)>,
) -> Result<(), Diagnostic> {
    let predecessors = predecessors(function);
    let dominators = compute_dominators(function, &predecessors)?;
    let mut definitions = HashMap::new();
    for block in function.blocks.values() {
        for (value, instruction) in &block.instructions {
            let ty = match instruction {
                Instruction::Phi(_) => None,
                _ => Some(infer_instruction_type(function, instruction, signatures)?),
            };
            if definitions
                .insert(
                    *value,
                    ValueDefinition {
                        block: block.id,
                        ty,
                    },
                )
                .is_some()
            {
                return Err(Diagnostic::new(format!("duplicate value id {:?}", value)));
            }
        }
    }
    resolve_phi_types(function, &mut definitions)?;

    for block in function.blocks.values() {
        let mut seen_in_block = HashSet::new();
        for (value, instruction) in &block.instructions {
            match instruction {
                Instruction::Binary {
                    left,
                    right,
                    operand_ty,
                    ..
                } => {
                    let left_ty = require_dominating_definition(
                        &definitions,
                        &dominators,
                        &seen_in_block,
                        block.id,
                        *left,
                    )?;
                    let right_ty = require_dominating_definition(
                        &definitions,
                        &dominators,
                        &seen_in_block,
                        block.id,
                        *right,
                    )?;
                    if left_ty != *operand_ty || right_ty != *operand_ty {
                        return Err(Diagnostic::new(format!(
                            "binary operands in block {:?} must both have type {}",
                            block.id, operand_ty
                        )));
                    }
                }
                Instruction::MathIntrinsic {
                    args,
                    operand_ty,
                    result_ty,
                    ..
                } => {
                    for arg in args {
                        let arg_ty = require_dominating_definition(
                            &definitions,
                            &dominators,
                            &seen_in_block,
                            block.id,
                            *arg,
                        )?;
                        if arg_ty != *operand_ty {
                            return Err(Diagnostic::new(format!(
                                "math intrinsic argument in block {:?} has type {}, expected {}",
                                block.id, arg_ty, operand_ty
                            )));
                        }
                    }
                    if !matches!(
                        (operand_ty, result_ty),
                        (Type::Numeric(_), Type::Numeric(_))
                    ) {
                        return Err(Diagnostic::new(format!(
                            "math intrinsic in block {:?} must have numeric operand/result types",
                            block.id
                        )));
                    }
                }
                Instruction::Print { value } => {
                    let value_ty = require_dominating_definition(
                        &definitions,
                        &dominators,
                        &seen_in_block,
                        block.id,
                        *value,
                    )?;
                    if value_ty != Type::String {
                        return Err(Diagnostic::new(format!(
                            "print argument in block {:?} has type {}, expected string",
                            block.id, value_ty
                        )));
                    }
                }
                Instruction::Cast { value, from, to } => {
                    let value_ty = require_dominating_definition(
                        &definitions,
                        &dominators,
                        &seen_in_block,
                        block.id,
                        *value,
                    )?;
                    if value_ty != *from {
                        return Err(Diagnostic::new(format!(
                            "cast source in block {:?} has type {}, expected {}",
                            block.id, value_ty, from
                        )));
                    }
                    crate::lower::require_numeric_cast(from.clone(), to.clone())?;
                }
                Instruction::Call { name, args, .. } => {
                    let (param_types, _) = signatures
                        .get(name)
                        .ok_or_else(|| Diagnostic::new(format!("unknown function '{}'", name)))?;
                    if args.len() != param_types.len() {
                        return Err(Diagnostic::new(format!(
                            "call to '{}' has {} args but signature expects {}",
                            name,
                            args.len(),
                            param_types.len()
                        )));
                    }
                    for (arg, param_ty) in args.iter().zip(param_types.iter()) {
                        let arg_ty = require_dominating_definition(
                            &definitions,
                            &dominators,
                            &seen_in_block,
                            block.id,
                            *arg,
                        )?;
                        if !types_match(&arg_ty, param_ty) {
                            return Err(Diagnostic::new(format!(
                                "call argument in block {:?} has type {}, expected {}",
                                block.id, arg_ty, param_ty
                            )));
                        }
                    }
                }
                Instruction::HostCall {
                    name,
                    symbol_id,
                    args,
                    ..
                } => {
                    let (param_types, _) = host_signatures.get(symbol_id).ok_or_else(|| {
                        Diagnostic::new(format!("unknown host function '{}'", name))
                    })?;
                    if args.len() != param_types.len() {
                        return Err(Diagnostic::new(format!(
                            "host call to '{}' has {} args but signature expects {}",
                            name,
                            args.len(),
                            param_types.len()
                        )));
                    }
                    for (arg, param_ty) in args.iter().zip(param_types.iter()) {
                        let arg_ty = require_dominating_definition(
                            &definitions,
                            &dominators,
                            &seen_in_block,
                            block.id,
                            *arg,
                        )?;
                        if !types_match(&arg_ty, param_ty) {
                            return Err(Diagnostic::new(format!(
                                "host call argument in block {:?} has type {}, expected {}",
                                block.id, arg_ty, param_ty
                            )));
                        }
                    }
                }
                Instruction::CallValue {
                    callee,
                    args,
                    params,
                    return_type,
                } => {
                    let callee_ty = require_dominating_definition(
                        &definitions,
                        &dominators,
                        &seen_in_block,
                        block.id,
                        *callee,
                    )?;
                    let expected_callee_ty = Type::Function {
                        params: params.clone(),
                        return_type: Box::new(return_type.clone()),
                    };
                    if callee_ty != expected_callee_ty {
                        return Err(Diagnostic::new(format!(
                            "indirect call in block {:?} expects callee {}, got {}",
                            block.id, expected_callee_ty, callee_ty
                        )));
                    }
                    if args.len() != params.len() {
                        return Err(Diagnostic::new(format!(
                            "indirect call in block {:?} has {} args but expects {}",
                            block.id,
                            args.len(),
                            params.len()
                        )));
                    }
                    for (arg, param_ty) in args.iter().zip(params.iter()) {
                        let arg_ty = require_dominating_definition(
                            &definitions,
                            &dominators,
                            &seen_in_block,
                            block.id,
                            *arg,
                        )?;
                        if !types_match(&arg_ty, param_ty) {
                            return Err(Diagnostic::new(format!(
                                "indirect call argument in block {:?} has type {}, expected {}",
                                block.id, arg_ty, param_ty
                            )));
                        }
                    }
                }
                Instruction::CoroutineCreate { callee } => {
                    let callee_ty = require_dominating_definition(
                        &definitions,
                        &dominators,
                        &seen_in_block,
                        block.id,
                        *callee,
                    )?;
                    let expected_callee_ty = Type::Function {
                        params: Vec::new(),
                        return_type: Box::new(Type::Numeric(NumericType::I32)),
                    };
                    if callee_ty != expected_callee_ty {
                        return Err(Diagnostic::new(format!(
                            "coroutine create in block {:?} expects callee {}, got {}",
                            block.id, expected_callee_ty, callee_ty
                        )));
                    }
                }
                Instruction::CoroutineResume { coroutine }
                | Instruction::CoroutineClose { coroutine } => {
                    let coroutine_ty = require_dominating_definition(
                        &definitions,
                        &dominators,
                        &seen_in_block,
                        block.id,
                        *coroutine,
                    )?;
                    if coroutine_ty != Type::Thread {
                        return Err(Diagnostic::new(format!(
                            "coroutine resume/close in block {:?} expects thread, got {}",
                            block.id, coroutine_ty
                        )));
                    }
                }
                Instruction::CoroutineResumeTagged { coroutine, .. } => {
                    let coroutine_ty = require_dominating_definition(
                        &definitions,
                        &dominators,
                        &seen_in_block,
                        block.id,
                        *coroutine,
                    )?;
                    if coroutine_ty != Type::Thread {
                        return Err(Diagnostic::new(format!(
                            "coroutine resume-tagged in block {:?} expects thread, got {}",
                            block.id, coroutine_ty
                        )));
                    }
                }
                Instruction::Closure {
                    name,
                    captures,
                    params,
                    return_type,
                } => {
                    let (sig_params, sig_ret) = signatures
                        .get(name)
                        .ok_or_else(|| Diagnostic::new(format!("unknown function '{}'", name)))?;
                    if sig_params.len() < captures.len() {
                        return Err(Diagnostic::new(format!(
                            "closure in block {:?} has too many captures for '{}'",
                            block.id, name
                        )));
                    }
                    for (capture, capture_ty) in captures.iter().zip(sig_params.iter()) {
                        let actual = require_dominating_definition(
                            &definitions,
                            &dominators,
                            &seen_in_block,
                            block.id,
                            *capture,
                        )?;
                        // Allow the captured value to be either the raw value type or a
                        // 1-element array cell containing the value (for mutable capture).
                        let ok = actual == *capture_ty
                            || (actual.is_array()
                                && actual
                                    .element_type()
                                    .map(|e| e == *capture_ty)
                                    .unwrap_or(false));
                        if !ok {
                            return Err(Diagnostic::new(format!(
                                "closure capture in block {:?} has type {}, expected {}",
                                block.id, actual, capture_ty
                            )));
                        }
                    }
                    let expected_sig = Type::Function {
                        params: params.clone(),
                        return_type: Box::new(return_type.clone()),
                    };
                    let actual_sig = Type::Function {
                        params: sig_params[captures.len()..].to_vec(),
                        return_type: Box::new(sig_ret.clone()),
                    };
                    if expected_sig != actual_sig {
                        return Err(Diagnostic::new(format!(
                            "closure in block {:?} signature mismatch: expected {}, got {}",
                            block.id, expected_sig, actual_sig
                        )));
                    }
                }
                Instruction::ArrayNew {
                    element_ty,
                    elements,
                } => {
                    for element in elements {
                        let element_value_ty = require_dominating_definition(
                            &definitions,
                            &dominators,
                            &seen_in_block,
                            block.id,
                            *element,
                        )?;
                        if element_value_ty != *element_ty {
                            return Err(Diagnostic::new(format!(
                                "array literal element in block {:?} has type {}, expected {}",
                                block.id, element_value_ty, element_ty
                            )));
                        }
                    }
                }
                Instruction::ArrayGet {
                    array,
                    index,
                    element_ty,
                } => {
                    let array_ty = require_dominating_definition(
                        &definitions,
                        &dominators,
                        &seen_in_block,
                        block.id,
                        *array,
                    )?;
                    let expected_array_ty = Type::Array(Box::new(element_ty.clone()));
                    // Allow the array operand to be either an array of the element type
                    // or the raw element type itself (degenerate cell representation).
                    let ok_array = array_ty == expected_array_ty || array_ty == *element_ty;
                    if !ok_array {
                        return Err(Diagnostic::new(format!(
                            "array get in block {:?} expects {}, got {}",
                            block.id, expected_array_ty, array_ty
                        )));
                    }
                    let index_ty = require_dominating_definition(
                        &definitions,
                        &dominators,
                        &seen_in_block,
                        block.id,
                        *index,
                    )?;
                    if index_ty != Type::Numeric(NumericType::I32) {
                        return Err(Diagnostic::new(format!(
                            "array index in block {:?} must be i32",
                            block.id
                        )));
                    }
                }
                Instruction::ArraySet {
                    array,
                    index,
                    value,
                    element_ty,
                } => {
                    let array_ty = require_dominating_definition(
                        &definitions,
                        &dominators,
                        &seen_in_block,
                        block.id,
                        *array,
                    )?;
                    let expected_array_ty = Type::Array(Box::new(element_ty.clone()));
                    let ok_array = array_ty == expected_array_ty || array_ty == *element_ty;
                    if !ok_array {
                        return Err(Diagnostic::new(format!(
                            "array set in block {:?} expects {}, got {}",
                            block.id, expected_array_ty, array_ty
                        )));
                    }
                    let index_ty = require_dominating_definition(
                        &definitions,
                        &dominators,
                        &seen_in_block,
                        block.id,
                        *index,
                    )?;
                    if index_ty != Type::Numeric(NumericType::I32) {
                        return Err(Diagnostic::new(format!(
                            "array index in block {:?} must be i32",
                            block.id
                        )));
                    }
                    let value_ty = require_dominating_definition(
                        &definitions,
                        &dominators,
                        &seen_in_block,
                        block.id,
                        *value,
                    )?;
                    if value_ty != *element_ty {
                        return Err(Diagnostic::new(format!(
                            "array set value in block {:?} has type {}, expected {}",
                            block.id, value_ty, element_ty
                        )));
                    }
                }
                Instruction::ArrayLen { array } => {
                    let array_ty = require_dominating_definition(
                        &definitions,
                        &dominators,
                        &seen_in_block,
                        block.id,
                        *array,
                    )?;
                    if !array_ty.is_array() {
                        return Err(Diagnostic::new(format!(
                            "array.len operand in block {:?} must be an array",
                            block.id
                        )));
                    }
                }
                Instruction::BytesGet { bytes, index } => {
                    let bytes_ty = require_dominating_definition(
                        &definitions,
                        &dominators,
                        &seen_in_block,
                        block.id,
                        *bytes,
                    )?;
                    if bytes_ty != Type::Bytes {
                        return Err(Diagnostic::new(format!(
                            "bytes get in block {:?} expects bytes, got {}",
                            block.id, bytes_ty
                        )));
                    }
                    let index_ty = require_dominating_definition(
                        &definitions,
                        &dominators,
                        &seen_in_block,
                        block.id,
                        *index,
                    )?;
                    if index_ty != Type::Numeric(NumericType::I32) {
                        return Err(Diagnostic::new(format!(
                            "bytes index in block {:?} must be i32",
                            block.id
                        )));
                    }
                }
                Instruction::BytesLen { bytes } => {
                    let bytes_ty = require_dominating_definition(
                        &definitions,
                        &dominators,
                        &seen_in_block,
                        block.id,
                        *bytes,
                    )?;
                    if bytes_ty != Type::Bytes {
                        return Err(Diagnostic::new(format!(
                            "bytes.len operand in block {:?} must be bytes",
                            block.id
                        )));
                    }
                }
                Instruction::StructNew { struct_ty, fields } => {
                    let Type::Record(record_fields) = struct_ty else {
                        return Err(Diagnostic::new(format!(
                            "struct new in block {:?} requires a record type",
                            block.id
                        )));
                    };
                    if fields.len() != record_fields.len() {
                        return Err(Diagnostic::new(format!(
                            "struct new in block {:?} has {} fields but type has {}",
                            block.id,
                            fields.len(),
                            record_fields.len()
                        )));
                    }
                    for (value, (_, field_ty)) in fields.iter().zip(record_fields.iter()) {
                        let actual = require_dominating_definition(
                            &definitions,
                            &dominators,
                            &seen_in_block,
                            block.id,
                            *value,
                        )?;
                        if actual != *field_ty {
                            return Err(Diagnostic::new(format!(
                                "struct new field in block {:?} has type {}, expected {}",
                                block.id, actual, field_ty
                            )));
                        }
                    }
                }
                Instruction::StructGet {
                    base,
                    field,
                    field_ty,
                } => {
                    let base_ty = require_dominating_definition(
                        &definitions,
                        &dominators,
                        &seen_in_block,
                        block.id,
                        *base,
                    )?;
                    let actual_field_ty = base_ty.record_field(field).ok_or_else(|| {
                        Diagnostic::new(format!(
                            "struct get in block {:?} missing field '{}'",
                            block.id, field
                        ))
                    })?;
                    if actual_field_ty != *field_ty {
                        return Err(Diagnostic::new(format!(
                            "struct get in block {:?} field '{}' has type {}, expected {}",
                            block.id, field, actual_field_ty, field_ty
                        )));
                    }
                }
                Instruction::StructSet { base, field, value } => {
                    let base_ty = require_dominating_definition(
                        &definitions,
                        &dominators,
                        &seen_in_block,
                        block.id,
                        *base,
                    )?;
                    let expected_field_ty = base_ty.record_field(field).ok_or_else(|| {
                        Diagnostic::new(format!(
                            "struct set in block {:?} missing field '{}'",
                            block.id, field
                        ))
                    })?;
                    let actual_value_ty = require_dominating_definition(
                        &definitions,
                        &dominators,
                        &seen_in_block,
                        block.id,
                        *value,
                    )?;
                    if actual_value_ty != expected_field_ty {
                        return Err(Diagnostic::new(format!(
                            "struct set in block {:?} field '{}' has type {}, expected {}",
                            block.id, field, actual_value_ty, expected_field_ty
                        )));
                    }
                }
                Instruction::PackMulti { values, types } => {
                    if values.len() != types.len() {
                        return Err(Diagnostic::new(format!(
                            "pack multi in block {:?} must have equal value/type arity",
                            block.id
                        )));
                    }
                    for (value, ty) in values.iter().zip(types.iter()) {
                        let actual = require_dominating_definition(
                            &definitions,
                            &dominators,
                            &seen_in_block,
                            block.id,
                            *value,
                        )?;
                        if actual != *ty {
                            return Err(Diagnostic::new(format!(
                                "pack multi in block {:?} value type {}, expected {}",
                                block.id, actual, ty
                            )));
                        }
                    }
                }
                Instruction::MultiGet { value, index, ty } => {
                    let actual = require_dominating_definition(
                        &definitions,
                        &dominators,
                        &seen_in_block,
                        block.id,
                        *value,
                    )?;
                    let Type::Multi(types) = actual else {
                        return Err(Diagnostic::new(format!(
                            "multi-get in block {:?} requires multi-value operand",
                            block.id
                        )));
                    };
                    let expected = types.get(*index).ok_or_else(|| {
                        Diagnostic::new(format!(
                            "multi-get in block {:?} index {} is out of range",
                            block.id, index
                        ))
                    })?;
                    if expected != ty {
                        return Err(Diagnostic::new(format!(
                            "multi-get in block {:?} expects {}, got {}",
                            block.id, expected, ty
                        )));
                    }
                }
                Instruction::Phi(incoming) => {
                    let expected_preds = predecessors.get(&block.id).cloned().unwrap_or_default();
                    if incoming.len() != expected_preds.len() {
                        return Err(Diagnostic::new(format!(
                            "phi in block {:?} must have exactly {} incoming values",
                            block.id,
                            expected_preds.len()
                        )));
                    }
                    for (index, (pred, value)) in incoming.iter().enumerate() {
                        if *pred != expected_preds[index] {
                            return Err(Diagnostic::new(format!(
                                "phi in block {:?} predecessor order mismatch at {}",
                                block.id, index
                            )));
                        }
                        let value_def = definitions.get(value).ok_or_else(|| {
                            Diagnostic::new(format!("use of undefined value {:?}", value))
                        })?;
                        if value_def.block != *pred
                            && !dominators
                                .get(pred)
                                .is_some_and(|doms| doms.contains(&value_def.block))
                        {
                            return Err(Diagnostic::new(format!(
                                "value {:?} does not dominate phi edge {:?} -> {:?}",
                                value, pred, block.id
                            )));
                        }
                    }
                }
                Instruction::Param(_)
                | Instruction::Number { .. }
                | Instruction::Unit
                | Instruction::Bool(_)
                | Instruction::Null { .. }
                | Instruction::String(_)
                | Instruction::Bytes(_) => {}
                Instruction::IsNull { value, ty } => {
                    let value_ty = require_dominating_definition(
                        &definitions,
                        &dominators,
                        &seen_in_block,
                        block.id,
                        *value,
                    )?;
                    if !types_match(&value_ty, ty) {
                        return Err(Diagnostic::new(format!(
                            "null test in block {:?} expects {}, got {}",
                            block.id, ty, value_ty
                        )));
                    }
                }
                Instruction::ToString { value, from } => {
                    let value_ty = require_dominating_definition(
                        &definitions,
                        &dominators,
                        &seen_in_block,
                        block.id,
                        *value,
                    )?;
                    if &value_ty != from {
                        return Err(Diagnostic::new(format!(
                            "tostring source type mismatch in block {:?}: expected {}, got {}",
                            block.id, from, value_ty
                        )));
                    }
                    if !(from.is_numeric() || *from == Type::Bool || *from == Type::String) {
                        return Err(Diagnostic::new(format!(
                            "tostring requires primitive source type, got {}",
                            from
                        )));
                    }
                }
            }
            seen_in_block.insert(*value);
        }

        match &block.terminator {
            Terminator::Jump(target) => require_block(function, *target)?,
            Terminator::Branch {
                condition,
                then_block,
                else_block,
            } => {
                let condition_ty = require_dominating_definition(
                    &definitions,
                    &dominators,
                    &seen_in_block,
                    block.id,
                    *condition,
                )?;
                if condition_ty != Type::Bool {
                    return Err(Diagnostic::new(format!(
                        "branch condition in block {:?} must have type bool",
                        block.id
                    )));
                }
                require_block(function, *then_block)?;
                require_block(function, *else_block)?;
            }
            Terminator::CoroutineYield {
                value,
                resume_block,
            } => {
                // The yield value must be `i32`. Whether a coroutine is on the call stack is
                // a runtime check (see design 0007), so there is no static context rule here.
                let value_ty = require_dominating_definition(
                    &definitions,
                    &dominators,
                    &seen_in_block,
                    block.id,
                    *value,
                )?;
                if value_ty != Type::Numeric(NumericType::I32) {
                    return Err(Diagnostic::new(format!(
                        "coroutine yield in block {:?} expects an i32 value, got {}",
                        block.id, value_ty
                    )));
                }
                require_block(function, *resume_block)?;
            }
            Terminator::Return(value) => {
                let value_ty = require_dominating_definition(
                    &definitions,
                    &dominators,
                    &seen_in_block,
                    block.id,
                    *value,
                )?;
                if value_ty != function.return_type {
                    return Err(Diagnostic::new(format!(
                        "return in block {:?} has type {}, expected {}",
                        block.id, value_ty, function.return_type
                    )));
                }
            }
            Terminator::Unreachable { .. } => {}
        }
    }

    Ok(())
}

#[derive(Clone)]
struct ValueDefinition {
    block: BlockId,
    ty: Option<Type>,
}

fn require_dominating_definition(
    definitions: &HashMap<ValueId, ValueDefinition>,
    dominators: &HashMap<BlockId, HashSet<BlockId>>,
    seen_in_block: &HashSet<ValueId>,
    use_block: BlockId,
    value: ValueId,
) -> Result<Type, Diagnostic> {
    let definition = definitions
        .get(&value)
        .ok_or_else(|| Diagnostic::new(format!("use of undefined value {:?}", value)))?;
    if definition.block == use_block {
        if !seen_in_block.contains(&value) {
            return Err(Diagnostic::new(format!(
                "value {:?} does not dominate its use in block {:?}",
                value, use_block
            )));
        }
    } else if !dominators
        .get(&use_block)
        .is_some_and(|doms| doms.contains(&definition.block))
    {
        return Err(Diagnostic::new(format!(
            "value {:?} does not dominate its use in block {:?}",
            value, use_block
        )));
    }
    definition
        .ty
        .clone()
        .ok_or_else(|| Diagnostic::new(format!("could not infer type for value {:?}", value)))
}

fn require_block(function: &Function, block: BlockId) -> Result<(), Diagnostic> {
    if function.blocks.contains_key(&block) {
        Ok(())
    } else {
        Err(Diagnostic::new(format!("unknown block {:?}", block)))
    }
}

/// At the Wasm/IR level, `TaggedUnion`, `TaggedVariant`, and the canonical record
/// `{ tag: i32, value: unknown }` all share the same runtime representation.
/// Return true when two types are compatible for call-argument and return-type checks.
fn types_match(a: &Type, b: &Type) -> bool {
    if a == b {
        return true;
    }
    let is_tagged = |t: &Type| {
        matches!(t, Type::TaggedUnion(_) | Type::TaggedVariant(_))
            || t == &Type::canonical_tagged_union_record()
    };
    if is_tagged(a) && is_tagged(b) {
        return true;
    }
    match (a, b) {
        (Type::Nullable(inner), other) | (other, Type::Nullable(inner)) => inner.as_ref() == other,
        _ => false,
    }
}

fn infer_instruction_type(
    function: &Function,
    instruction: &Instruction,
    signatures: &HashMap<String, (Vec<Type>, Type)>,
) -> Result<Type, Diagnostic> {
    match instruction {
        Instruction::Param(index) => function
            .params
            .get(*index)
            .map(|(_, ty)| {
                // TaggedUnion/TaggedVariant values are canonical records at the IR level.
                if matches!(ty, Type::TaggedUnion(_) | Type::TaggedVariant(_)) {
                    Type::canonical_tagged_union_record()
                } else {
                    ty.clone()
                }
            })
            .ok_or_else(|| Diagnostic::new(format!("param index {} out of bounds", index))),
        Instruction::Number { ty, .. } => Ok(Type::Numeric(*ty)),
        Instruction::Unit => Ok(Type::Unit),
        Instruction::Bool(_) => Ok(Type::Bool),
        Instruction::Null { ty } => Ok(ty.clone()),
        Instruction::String(_) => Ok(Type::String),
        Instruction::Bytes(_) => Ok(Type::Bytes),
        Instruction::Cast { to, .. } => Ok(to.clone()),
        Instruction::Binary { result_ty, .. } => Ok(result_ty.clone()),
        Instruction::MathIntrinsic { result_ty, .. } => Ok(result_ty.clone()),
        Instruction::ToString { .. } => Ok(Type::String),
        Instruction::IsNull { .. } => Ok(Type::Bool),
        Instruction::Print { .. } => Ok(Type::Unit),
        Instruction::Call { name, .. } => signatures
            .get(name)
            .map(|(_, ret)| ret.clone())
            .ok_or_else(|| Diagnostic::new(format!("unknown function '{}'", name))),
        Instruction::HostCall { return_type, .. } => Ok(return_type.clone()),
        Instruction::CallValue { return_type, .. } => Ok(return_type.clone()),
        Instruction::CoroutineCreate { .. } => Ok(Type::Thread),
        Instruction::CoroutineResume { .. } => Ok(Type::Multi(vec![
            Type::Bool,
            Type::Numeric(NumericType::I32),
        ])),
        Instruction::CoroutineResumeTagged { .. } => Ok(Type::canonical_tagged_union_record()),
        Instruction::CoroutineClose { .. } => Ok(Type::Bool),
        Instruction::Closure {
            params,
            return_type,
            ..
        } => Ok(Type::Function {
            params: params.clone(),
            return_type: Box::new(return_type.clone()),
        }),
        Instruction::ArrayNew { element_ty, .. } => Ok(Type::Array(Box::new(element_ty.clone()))),
        Instruction::ArrayGet { element_ty, .. } => Ok(element_ty.clone()),
        Instruction::ArraySet { .. } => Ok(Type::Numeric(NumericType::I32)),
        Instruction::ArrayLen { .. } => Ok(Type::Numeric(NumericType::I32)),
        Instruction::BytesGet { .. } => Ok(Type::Numeric(NumericType::I32)),
        Instruction::BytesLen { .. } => Ok(Type::Numeric(NumericType::I32)),
        Instruction::StructNew { struct_ty, .. } => Ok(struct_ty.clone()),
        Instruction::StructGet { field_ty, .. } => Ok(field_ty.clone()),
        Instruction::StructSet { .. } => Ok(Type::Unit),
        Instruction::PackMulti { types, .. } => Ok(Type::Multi(types.clone())),
        Instruction::MultiGet { ty, .. } => Ok(ty.clone()),
        Instruction::Phi(_) => Err(Diagnostic::new(
            "phi type must be resolved before infer_instruction_type",
        )),
    }
}

fn compute_dominators(
    function: &Function,
    predecessors: &HashMap<BlockId, Vec<BlockId>>,
) -> Result<HashMap<BlockId, HashSet<BlockId>>, Diagnostic> {
    if !function.blocks.contains_key(&function.entry) {
        return Err(Diagnostic::new(format!(
            "entry block {:?} does not exist",
            function.entry
        )));
    }
    let all_blocks: HashSet<_> = function.blocks.keys().copied().collect();
    let mut dominators = HashMap::new();
    for id in function.blocks.keys().copied() {
        if id == function.entry {
            dominators.insert(id, HashSet::from([id]));
        } else {
            dominators.insert(id, all_blocks.clone());
        }
    }

    let mut changed = true;
    while changed {
        changed = false;
        for block in function.blocks.keys().copied() {
            if block == function.entry {
                continue;
            }
            let preds = predecessors.get(&block).cloned().unwrap_or_default();
            let mut next = if preds.is_empty() {
                HashSet::new()
            } else {
                let mut it = preds.iter();
                let first = it
                    .next()
                    .and_then(|pred| dominators.get(pred))
                    .cloned()
                    .unwrap_or_default();
                it.fold(first, |acc, pred| {
                    let pred_set = dominators.get(pred).cloned().unwrap_or_default();
                    acc.intersection(&pred_set).copied().collect()
                })
            };
            next.insert(block);
            if dominators
                .get(&block)
                .is_some_and(|current| *current != next)
            {
                dominators.insert(block, next);
                changed = true;
            }
        }
    }
    Ok(dominators)
}

fn resolve_phi_types(
    function: &Function,
    definitions: &mut HashMap<ValueId, ValueDefinition>,
) -> Result<(), Diagnostic> {
    let mut changed = true;
    while changed {
        changed = false;
        for block in function.blocks.values() {
            for (value, instruction) in &block.instructions {
                let Instruction::Phi(incoming) = instruction else {
                    continue;
                };
                if definitions
                    .get(value)
                    .and_then(|def| def.ty.as_ref())
                    .is_some()
                {
                    continue;
                }
                let mut incoming_ty: Option<Type> = None;
                for (_, incoming_value) in incoming {
                    if incoming_value == value {
                        continue;
                    }
                    let ty = match definitions
                        .get(incoming_value)
                        .and_then(|def| def.ty.as_ref())
                    {
                        Some(ty) => ty,
                        None => continue,
                    };
                    if let Some(ref expected) = incoming_ty {
                        if expected != ty {
                            return Err(Diagnostic::new(format!(
                                "phi in block {:?} has inconsistent incoming types",
                                block.id
                            )));
                        }
                    } else {
                        incoming_ty = Some(ty.clone());
                    }
                }
                if let Some(resolved) = incoming_ty {
                    definitions
                        .get_mut(value)
                        .expect("phi definition must exist")
                        .ty = Some(resolved);
                    changed = true;
                }
            }
        }
    }

    for block in function.blocks.values() {
        for (value, instruction) in &block.instructions {
            if matches!(instruction, Instruction::Phi(_))
                && definitions
                    .get(value)
                    .and_then(|def| def.ty.as_ref())
                    .is_none()
            {
                return Err(Diagnostic::new(format!(
                    "could not infer phi type for value {:?}",
                    value
                )));
            }
        }
    }
    Ok(())
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
            Terminator::CoroutineYield { resume_block, .. } => {
                out.entry(*resume_block).or_default().push(*id);
            }
            Terminator::Return(_) | Terminator::Unreachable { .. } => {}
        }
    }
    out
}
